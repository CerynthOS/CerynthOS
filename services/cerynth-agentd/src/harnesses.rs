fn normalized_event(
    kind: &str,
    id: Option<&str>,
    usage: Option<&Value>,
) -> cerynth_agent_api::NormalizedHarnessEvent {
    cerynth_agent_api::NormalizedHarnessEvent {
        sequence: 0,
        kind: kind.into(),
        tool_call_id: id.filter(|s| s.len() <= 256).map(str::to_owned),
        input_tokens: usage
            .and_then(|u| u["input_tokens"].as_u64().or_else(|| u["input"].as_u64())),
        output_tokens: usage
            .and_then(|u| u["output_tokens"].as_u64().or_else(|| u["output"].as_u64())),
        exit_code: None,
        cached_input_tokens: usage.and_then(|u| {
            u["cache"]["read"]
                .as_u64()
                .or_else(|| u["cached_input_tokens"].as_u64())
                .or_else(|| u["input_tokens_details"]["cached_tokens"].as_u64())
                .or_else(|| u["cache_read_input_tokens"].as_u64())
        }),
        cache_write_tokens: usage.and_then(|u| {
            u["cache"]["write"]
                .as_u64()
                .or_else(|| u["cache_creation_input_tokens"].as_u64())
        }),
    }
}

use crate::AgentService;

fn normalize_acp(value: &Value) -> Vec<cerynth_agent_api::NormalizedHarnessEvent> {
    let update = &value["params"]["update"];
    let kind = match update["sessionUpdate"].as_str() {
        Some("tool_call") => "harness_tool_started",
        Some("tool_call_update") if update["status"] == "failed" => "harness_tool_failed",
        Some("tool_call_update") if update["status"] == "completed" => "harness_tool_completed",
        _ => return Vec::new(),
    };
    vec![normalized_event(kind, update["toolCallId"].as_str(), None)]
}

pub fn normalize(line: &[u8]) -> Vec<cerynth_agent_api::NormalizedHarnessEvent> {
    let Ok(value) = serde_json::from_slice::<Value>(line) else {
        return Vec::new();
    };
    if value["type"] == "acp_update" {
        return normalize_acp(&value);
    }
    let mut events = Vec::new();
    let event_type = value["type"].as_str().unwrap_or_default();
    let mut push = |kind: &str, id: Option<&str>, usage: Option<&Value>| {
        let mut event = normalized_event(kind, id, usage);
        event.exit_code = value["item"]["exit_code"]
            .as_i64()
            .or_else(|| value["part"]["state"]["metadata"]["exit"].as_i64())
            .and_then(|code| i32::try_from(code).ok());
        if event.kind == "harness_tool_completed" && event.exit_code.is_some_and(|code| code != 0) {
            event.kind = "harness_tool_failed".into();
        }
        events.push(event);
    };
    match event_type {
        "item.started" | "item.completed"
            if matches!(
                value["item"]["type"].as_str(),
                Some("command_execution" | "file_change" | "mcp_tool_call")
            ) =>
        {
            push(
                if event_type == "item.started" {
                    "harness_tool_started"
                } else {
                    "harness_tool_completed"
                },
                value["item"]["id"].as_str(),
                None,
            );
        }
        "assistant" => {
            if let Some(content) = value["message"]["content"].as_array() {
                for item in content {
                    if item["type"] == "tool_use" {
                        push("harness_tool_started", item["id"].as_str(), None);
                    }
                }
            }
        }
        "user" => {
            if let Some(content) = value["message"]["content"].as_array() {
                for item in content {
                    if item["type"] == "tool_result" {
                        push(
                            if item["is_error"] == true {
                                "harness_tool_failed"
                            } else {
                                "harness_tool_completed"
                            },
                            item["tool_use_id"].as_str(),
                            None,
                        );
                    }
                }
            }
        }
        "tool_call" => push("harness_tool_started", value["id"].as_str(), None),
        "tool_use" => push(
            if value["part"]["state"]["status"] == "error" {
                "harness_tool_failed"
            } else if value["part"]["state"]["status"] == "completed" {
                "harness_tool_completed"
            } else {
                "harness_tool_started"
            },
            value["part"]["callID"]
                .as_str()
                .or_else(|| value["tool_call_id"].as_str())
                .or_else(|| value["id"].as_str()),
            None,
        ),
        "tool_result" => push(
            if value["is_error"] == true {
                "harness_tool_failed"
            } else {
                "harness_tool_completed"
            },
            value["tool_call_id"].as_str(),
            None,
        ),
        "turn.completed" | "result" | "usage" => {
            push(
                "harness_usage_reported",
                None,
                value.get("usage").or_else(|| value.get("tokens")),
            );
        }
        "step_finish" => push("harness_usage_reported", None, value["part"].get("tokens")),
        "error" | "turn.failed" => push("harness_error_reported", None, None),
        _ => {}
    }
    events
}
use anyhow::{Result, ensure};
use cerynth_agent_api::{HarnessAdapter, HarnessKind, LaunchRequest, Session};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};

pub fn executable(name: &str) -> Option<PathBuf> {
    let override_name = match name {
        "opencode" => Some("CERYNTH_OPENCODE_BINARY"),
        "hermes" => Some("CERYNTH_HERMES_BINARY"),
        "codex" => Some("CERYNTH_CODEX_BINARY"),
        "claude" => Some("CERYNTH_CLAUDE_BINARY"),
        _ => None,
    };
    if let Some(path) = override_name.and_then(std::env::var_os).map(PathBuf::from) {
        return path
            .is_absolute()
            .then(|| path.canonicalize().ok())
            .flatten()
            .filter(|p| p.is_file());
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|path| path.join(name))
        .find(|path| path.is_file())
        .and_then(|p| p.canonicalize().ok())
}

pub async fn launch(session: &Session, input: String) -> Result<LaunchRequest> {
    if session.spec.runtime.transport != cerynth_agent_api::HarnessTransport::CliJsonl {
        return Ok(
            cerynth_adapter_cli_harnesses::protocol::OpenCodeProtocolAdapter {
                native_program: executable("opencode")
                    .ok_or_else(|| anyhow::anyhow!("OpenCode is not installed"))?,
                bridge_program: protocol_bridge()?,
                transport: session.spec.runtime.transport,
            }
            .prepare_launch(session, cerynth_agent_api::AdapterInput::Prompt(input))
            .await?,
        );
    }
    let name = match session.spec.harness {
        HarnessKind::OpenCode => "opencode",
        HarnessKind::Hermes => "hermes",
        HarnessKind::Codex => "codex",
        HarnessKind::ClaudeCode => "claude",
        _ => anyhow::bail!("unsupported native harness; use start for generic_pty"),
    };
    let program = executable(name).ok_or_else(|| anyhow::anyhow!("{name} is not installed"))?;
    Ok(cerynth_adapter_cli_harnesses::CliHarnessAdapter {
        kind: session.spec.harness,
        program,
    }
    .prepare_launch(session, cerynth_agent_api::AdapterInput::Prompt(input))
    .await?)
}

pub(crate) fn protocol_bridge() -> Result<PathBuf> {
    std::env::var_os("CERYNTH_HARNESS_BRIDGE")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_exe()?.with_file_name("cerynth-harness-bridge"))
        .canonicalize()
        .map_err(Into::into)
}

impl AgentService {
    pub(crate) async fn discover_harness(&self, harness: HarnessKind) -> Result<Value> {
        ensure!(
            harness == HarnessKind::OpenCode,
            "native session discovery is unsupported for this harness"
        );
        let client = self
            .native_opencode
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("owner has not configured CERYNTH_OPENCODE_URL"))?;
        client.health().await?;
        let native = client.sessions().await?;
        let sessions = native
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("invalid native session list"))?;
        ensure!(
            sessions.len() <= 128,
            "native session discovery exceeds 128 entries"
        );
        let discovered:Vec<_>=sessions.iter().filter(|session|session["id"].as_str().is_some_and(|id|
            !id.is_empty() && id.len()<=256 && id.chars().all(|c|c.is_ascii_alphanumeric() || c=='_' || c=='-')))
            .map(|session|json!({"native_id":session["id"],"title":session["title"].as_str().map(|s|s.chars().take(256).collect::<String>()),
                "directory":session["directory"].as_str().filter(|s|s.len()<=4096),"parent_id":session["parentID"].as_str().filter(|s|s.len()<=256),
                "source":"harness_report","boundary":"unenforced_prototype","imported":false})).collect();
        Ok(json!({"harness":"open_code","sessions":discovered,"control_acquired":false}))
    }

    pub(crate) async fn adapter_status(&self) -> Result<Value> {
        let mut descriptors = vec![json!(
            cerynth_adapter_generic_pty::GenericPtyAdapter.descriptor()
        )];
        for name in ["opencode", "hermes", "codex", "claude"] {
            let path = executable(name);
            let authentication = if let Some(path) = &path {
                authentication_state(name, path).await
            } else {
                "not_installed"
            };
            let version = if let Some(path) = &path {
                installation_version(path).await
            } else {
                "not_installed".into()
            };
            let kind = match name {
                "opencode" => HarnessKind::OpenCode,
                "hermes" => HarnessKind::Hermes,
                "codex" => HarnessKind::Codex,
                _ => HarnessKind::ClaudeCode,
            };
            let mut capabilities = path.as_ref().map_or_else(Vec::new, |program| {
                cerynth_adapter_cli_harnesses::CliHarnessAdapter {
                    kind,
                    program: program.clone(),
                }
                .descriptor()
                .capabilities
            });
            if name == "opencode" && self.native_opencode.is_some() {
                capabilities.push(cerynth_agent_api::AdapterCapability::Discovery);
            }
            let protocol_transports =
                if name == "opencode" && path.is_some() && protocol_bridge().is_ok() {
                    vec!["open_code_http", "open_code_acp"]
                } else {
                    Vec::new()
                };
            descriptors.push(
                json!({"name":name,"path":path,"installed":path.is_some(),"version":version,
                "transport":"supervised_cli_jsonl","capabilities":capabilities,
                "optional_transports":protocol_transports,
                "authentication":authentication,"native_pause_resume":false,
                "cloud_credentials_inherited":false}),
            );
        }
        Ok(json!(descriptors))
    }
}

async fn installation_version(path: &std::path::Path) -> String {
    let Ok(home) = tempfile::tempdir() else {
        return "unavailable".into();
    };
    let output = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::process::Command::new(path)
            .arg("--version")
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", home.path())
            .env("HERMES_HOME", home.path().join(".hermes"))
            .env("DO_NOT_TRACK", "1")
            .env("OPENCODE_DISABLE_AUTOUPDATE", "true")
            .env("OPENCODE_DISABLE_MODELS_FETCH", "true")
            .kill_on_drop(true)
            .output(),
    )
    .await;
    match output {
        Ok(Ok(output)) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take(128)
            .collect(),
        _ => "unavailable".into(),
    }
}

async fn authentication_state(name: &str, path: &std::path::Path) -> &'static str {
    let arguments: &[&str] = match name {
        "codex" => &["login", "status"],
        "claude" => &["auth", "status", "--json"],
        _ => return "unsupported",
    };
    let output = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::process::Command::new(path)
            .args(arguments)
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let Ok(Ok(output)) = output else {
        return "unknown";
    };
    if name == "claude" {
        let value: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
        return match value["loggedIn"].as_bool() {
            Some(true) => "authenticated",
            Some(false) => "unauthenticated",
            None => "unknown",
        };
    }
    if output.status.success() {
        "authenticated"
    } else {
        "unknown"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_tool_failures_ids_and_usage_remain_distinct_without_raw_content() {
        let acp = normalize(br#"{"type":"acp_update","params":{"update":{"sessionUpdate":"tool_call_update","toolCallId":"acp-tool","status":"failed","rawOutput":{"token":"secret"}}}}"#);
        assert_eq!(acp[0].kind, "harness_tool_failed");
        assert_eq!(acp[0].tool_call_id.as_deref(), Some("acp-tool"));
        assert!(!serde_json::to_string(&acp).unwrap().contains("secret"));
        let failed=normalize(br#"{"type":"tool_use","part":{"callID":"call_native","state":{"status":"error","input":{"api_key":"secret"},"error":"private command"}}}"#);
        assert_eq!(failed[0].kind, "harness_tool_failed");
        assert_eq!(failed[0].tool_call_id.as_deref(), Some("call_native"));
        assert!(!serde_json::to_string(&failed).unwrap().contains("secret"));
        let hermes =
            normalize(br#"{"type":"tool_use","tool_call_id":"call_hermes","name":"tool_call"}"#);
        assert_eq!(hermes[0].tool_call_id.as_deref(), Some("call_hermes"));
        let result =
            normalize(br#"{"type":"tool_result","tool_call_id":"call_hermes","is_error":true}"#);
        assert_eq!(result[0].kind, "harness_tool_failed");
        let usage=normalize(br#"{"type":"step_finish","part":{"tokens":{"input":11,"output":7,"cache":{"read":13}}}}"#);
        assert_eq!(usage[0].input_tokens, Some(11));
        assert_eq!(usage[0].output_tokens, Some(7));
        assert_eq!(usage[0].cached_input_tokens, Some(13));
        let command=normalize(br#"{"type":"item.completed","item":{"id":"cmd","type":"command_execution","aggregated_output":"private"}}"#);
        assert_eq!(command[0].tool_call_id.as_deref(), Some("cmd"));
        assert!(normalize(b"unstructured terminal output").is_empty());
        let command=normalize(br#"{"type":"item.completed","item":{"id":"failed-command","type":"command_execution","exit_code":127}}"#);
        assert_eq!(command[0].kind, "harness_tool_failed");
        assert_eq!(command[0].exit_code, Some(127));
    }
}
