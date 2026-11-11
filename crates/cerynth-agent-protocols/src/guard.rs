//! Guard v1alpha1 client, matching Person 2 commit e592fa47797179ad24a99fc76143be48191a7649.
//! The socket belongs to the trusted owner plane and must never be agent-mounted.
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};

#[derive(Clone)]
pub struct GuardClient {
    socket: PathBuf,
}

impl GuardClient {
    /// # Errors
    /// Rejects relative socket paths; actual credentials are verified on every connection.
    pub fn new(socket: impl Into<PathBuf>) -> Result<Self> {
        let socket = socket.into();
        ensure!(socket.is_absolute(), "Guard socket must be absolute");
        Ok(Self { socket })
    }
    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// # Errors
    /// Timeouts, malformed responses and peer mismatch never grant permission.
    pub async fn request(&self, request: Value) -> Result<Value> {
        tokio::time::timeout(Duration::from_secs(3), async {
            let metadata = std::fs::symlink_metadata(&self.socket).context("Guard unavailable")?;
            let uid = nix::unistd::getuid().as_raw();
            ensure!(
                metadata.file_type().is_socket()
                    && (metadata.uid() == uid || metadata.uid() == 0)
                    && metadata.mode() & nix::libc::S_IRWXO == 0,
                "Guard control socket is not private or trusted"
            );
            let mut stream = UnixStream::connect(&self.socket)
                .await
                .context("Guard unavailable")?;
            ensure!(
                stream.peer_cred()?.uid() == metadata.uid(),
                "Guard peer UID mismatch"
            );
            let mut bytes = serde_json::to_vec(&request)?;
            ensure!(bytes.len() < 65536, "Guard request exceeds limit");
            bytes.push(b'\n');
            stream.write_all(&bytes).await?;
            let mut response = Vec::new();
            BufReader::new(stream)
                .take(1024 * 1024 + 1)
                .read_until(b'\n', &mut response)
                .await?;
            ensure!(
                response.len() <= 1024 * 1024 && response.pop() == Some(b'\n'),
                "invalid Guard frame"
            );
            serde_json::from_slice(&response).context("invalid Guard response")
        })
        .await
        .context("Guard deadline exceeded; authorization unknown")?
    }
}
