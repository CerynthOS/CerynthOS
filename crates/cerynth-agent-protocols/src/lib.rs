//! ACP v1 client and protocol capability negotiation. No prompt grants host authority.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const ACP_VERSION: u32 = 1;
pub const MCP_VERSION: &str = "2025-11-25";
pub const A2A_VERSION: &str = "0.3.0";

pub mod guard;
pub mod opencode;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
// These are independent peer-negotiated protocol flags, not lifecycle state.
#[allow(clippy::struct_excessive_bools)]
pub struct AcpCapabilities {
    pub load_session: bool,
    pub list_sessions: bool,
    pub mcp_http: bool,
    pub mcp_sse: bool,
}

/// Caller supplies pipes from an already authorized and isolated process.
pub struct AcpClient<R, W> {
    reader: R,
    writer: W,
    next_id: u64,
    pub capabilities: AcpCapabilities,
    pub session_id: Option<String>,
    initialized: bool,
}

impl<R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin> AcpClient<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader,
            writer,
            next_id: 0,
            capabilities: AcpCapabilities::default(),
            session_id: None,
            initialized: false,
        }
    }

    async fn send(&mut self, message: Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(&message)?;
        ensure!(bytes.len() <= 65536, "ACP outbound message exceeds limit");
        bytes.push(b'\n');
        self.writer.write_all(&bytes).await?;
        self.writer.flush().await?;
        Ok(())
    }

    async fn read(&mut self) -> Result<Value> {
        let mut bytes = Vec::new();
        (&mut self.reader)
            .take(65537)
            .read_until(b'\n', &mut bytes)
            .await?;
        ensure!(
            bytes.len() <= 65536 && bytes.pop() == Some(b'\n'),
            "invalid ACP frame"
        );
        serde_json::from_slice(&bytes).context("invalid ACP JSON")
    }

    async fn call(
        &mut self,
        method: &str,
        params: Value,
        updates: &mut Vec<Value>,
    ) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await?;
        tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                let message = self.read().await?;
                ensure!(message["jsonrpc"] == "2.0", "invalid ACP JSON-RPC version");
                if message["id"] == id && message.get("method").is_none() {
                    ensure!(message.get("error").is_none(), "ACP request rejected by agent");
                    return message.get("result").cloned().context("ACP response has no result");
                }
                if let Some(method) = message.get("method").and_then(Value::as_str) {
                    if let Some(request_id) = message.get("id") {
                        if method == "session/request_permission" {
                            // No approval broker was passed to this client. Deny explicitly.
                            self.send(json!({"jsonrpc":"2.0","id":request_id,
                                "result":{"outcome":{"outcome":"cancelled"}}})).await?;
                        } else {
                            self.send(json!({"jsonrpc":"2.0","id":request_id,
                                "error":{"code":-32601,"message":"capability not granted by Cerynth"}})).await?;
                        }
                    } else if method == "session/update" {
                        ensure!(updates.len() < 256, "ACP update buffer exceeds limit");
                        if let Some(session) = &self.session_id {
                            ensure!(message["params"]["sessionId"] == *session, "ACP update session mismatch");
                        }
                        updates.push(message["params"].clone());
                    }
                }
            }
        }).await.context("ACP request deadline exceeded")?
    }

    /// Advertise no filesystem/terminal/tool authority before a real broker exists.
    ///
    /// # Errors
    /// Returns an error if the peer does not negotiate ACP v1.
    pub async fn initialize(&mut self) -> Result<Value> {
        let result = self
            .call(
                "initialize",
                json!({"protocolVersion":ACP_VERSION,"clientCapabilities":{},
            "clientInfo":{"name":"cerynth","version":"0.1.0"}}),
                &mut Vec::new(),
            )
            .await?;
        ensure!(
            result["protocolVersion"] == ACP_VERSION,
            "unsupported ACP protocol version"
        );
        self.capabilities = AcpCapabilities {
            load_session: result["agentCapabilities"]["loadSession"]
                .as_bool()
                .unwrap_or(false),
            list_sessions: result["agentCapabilities"]["sessionCapabilities"]["list"].is_object(),
            mcp_http: result["agentCapabilities"]["mcpCapabilities"]["http"]
                .as_bool()
                .unwrap_or(false),
            mcp_sse: result["agentCapabilities"]["mcpCapabilities"]["sse"]
                .as_bool()
                .unwrap_or(false),
        };
        self.initialized = true;
        Ok(result)
    }

    /// # Errors
    /// Returns an error before initialization or for an invalid session response.
    pub async fn create_session(&mut self, cwd: &str) -> Result<String> {
        ensure!(
            self.initialized && std::path::Path::new(cwd).is_absolute(),
            "ACP must initialize before session creation"
        );
        let result = self
            .call(
                "session/new",
                json!({"cwd":cwd,"mcpServers":[]}),
                &mut Vec::new(),
            )
            .await?;
        let id = result["sessionId"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 256)
            .context("invalid ACP session identity")?
            .to_owned();
        self.session_id = Some(id.clone());
        Ok(id)
    }

    /// # Errors
    /// Returns an error if resumption was not negotiated or session/load fails.
    pub async fn load_session(&mut self, id: &str, cwd: &str) -> Result<Value> {
        ensure!(
            self.initialized && self.capabilities.load_session,
            "ACP session resumption is unsupported"
        );
        self.session_id = Some(id.into());
        self.call(
            "session/load",
            json!({"sessionId":id,"cwd":cwd,"mcpServers":[]}),
            &mut Vec::new(),
        )
        .await
    }

    /// # Errors
    /// Returns an error if no session exists, the input is invalid, or the peer fails.
    pub async fn prompt(&mut self, text: &str) -> Result<(Value, Vec<Value>)> {
        let id = self
            .session_id
            .clone()
            .context("ACP session is not initialized")?;
        ensure!(
            !text.is_empty() && text.len() <= 32768,
            "invalid ACP prompt"
        );
        let mut updates = Vec::new();
        let result = self
            .call(
                "session/prompt",
                json!({"sessionId":id,"prompt":[{"type":"text","text":text}]}),
                &mut updates,
            )
            .await?;
        Ok((result, updates))
    }

    /// # Errors
    /// Returns an error if there is no session or the notification cannot be sent.
    pub async fn cancel(&mut self) -> Result<()> {
        let id = self
            .session_id
            .clone()
            .context("ACP session is not initialized")?;
        self.send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":id}}))
            .await
    }
}

/// Build the pinned MCP initialization request for a future authorized tool gateway.
#[must_use]
pub fn mcp_initialize(id: u64) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":{"protocolVersion":MCP_VERSION,
        "capabilities":{},"clientInfo":{"name":"cerynth","version":"0.1.0"}}})
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{BufReader, split};

    #[tokio::test]
    async fn acp_negotiates_and_denies_ungranted_permissions() -> Result<()> {
        let (client, server) = tokio::io::duplex(65536);
        let peer = tokio::spawn(async move {
            let (read, mut write) = split(server);
            let mut read = BufReader::new(read);
            let mut line = String::new();
            read.read_line(&mut line).await.unwrap();
            let initialize: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(initialize["params"]["clientCapabilities"], json!({}));
            write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":1,\"agentCapabilities\":{}}}\n").await.unwrap();
            line.clear();
            read.read_line(&mut line).await.unwrap();
            write
                .write_all(
                    b"{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"sessionId\":\"fixture\"}}\n",
                )
                .await
                .unwrap();
            line.clear();
            read.read_line(&mut line).await.unwrap();
            write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":99,\"method\":\"session/request_permission\",\"params\":{}}\n").await.unwrap();
            line.clear();
            read.read_line(&mut line).await.unwrap();
            let decision: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(decision["result"]["outcome"]["outcome"], "cancelled");
            write.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"sessionId\":\"fixture\",\"update\":{\"sessionUpdate\":\"agent_message_chunk\"}}}\n{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{\"stopReason\":\"end_turn\"}}\n").await.unwrap();
        });
        let (read, write) = split(client);
        let mut client = AcpClient::new(BufReader::new(read), write);
        assert!(client.create_session("/tmp").await.is_err());
        client.initialize().await?;
        client.create_session("/tmp").await?;
        assert!(client.load_session("fixture", "/tmp").await.is_err());
        let (result, updates) = client.prompt("public fixture").await?;
        assert_eq!(result["stopReason"], "end_turn");
        assert_eq!(updates.len(), 1);
        peer.await?;
        Ok(())
    }
}
