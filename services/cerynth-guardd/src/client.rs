//! Typed integration client for the runtime and desktop. Agent callers must
//! receive only their session socket; the control socket is a trusted endpoint.
use crate::protocol::{Request, Response};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[derive(Clone, Debug)]
pub struct Client {
    socket: PathBuf,
}
impl Client {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
        }
    }
    pub fn socket(&self) -> &Path {
        &self.socket
    }
    /// One bounded request with a deadline. Approval waiting needs a longer
    /// deadline supplied by the caller; timeouts never imply authorization.
    pub async fn request(
        &self,
        request: &Request,
        deadline: std::time::Duration,
    ) -> anyhow::Result<Response> {
        tokio::time::timeout(deadline, async {
            let mut stream = UnixStream::connect(&self.socket).await?;
            let mut bytes = serde_json::to_vec(request)?;
            anyhow::ensure!(bytes.len() < 65536, "request exceeds frame limit");
            bytes.push(b'\n');
            stream.write_all(&bytes).await?;
            let mut reader = BufReader::new(stream).take(1024 * 1024 + 1);
            let mut response = Vec::new();
            reader.read_until(b'\n', &mut response).await?;
            anyhow::ensure!(
                response.len() <= 1024 * 1024 && response.last() == Some(&b'\n'),
                "invalid or oversized response"
            );
            Ok(serde_json::from_slice(&response)?)
        })
        .await
        .map_err(|_| anyhow::anyhow!("Guard request timed out; authorization unknown"))?
    }
}
