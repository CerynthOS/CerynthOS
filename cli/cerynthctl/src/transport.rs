use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

use cerynth_ipc::{Frame, RequestEnvelope, ResponseEnvelope};

const SOCKET_PATH: &str = "/tmp/cerynthd.sock";

/// Resolves the daemon's socket path to connect to.
///
/// Overridable via `CERYNTH_SOCKET_PATH` so tests can address a specific
/// daemon instance. Falls back to the default path when unset.
fn socket_path() -> String {
    std::env::var_os("CERYNTH_SOCKET_PATH")
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| SOCKET_PATH.to_string())
}

pub fn send_request(request: RequestEnvelope) -> std::io::Result<ResponseEnvelope> {
    let mut stream = UnixStream::connect(socket_path())?;

    let bytes = request.to_line().unwrap();

    stream.write_all(&bytes)?;

    let mut reader = BufReader::new(stream);

    let mut response = Vec::new();

    reader.read_until(b'\n', &mut response)?;

    Ok(ResponseEnvelope::from_line(&response).unwrap())
}
