use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use cerynth_ipc::{Frame, RequestEnvelope, ResponseEnvelope, socket_path};

use crate::timeout::REQUEST_TIMEOUT;

pub fn send_request(request: RequestEnvelope) -> std::io::Result<ResponseEnvelope> {
    // Shared with the daemon via cerynth-ipc so the two can never drift.
    let mut stream = UnixStream::connect(socket_path())?;

    // Apply the request timeout so a dead or unresponsive daemon cannot
    // block the CLI indefinitely.
    stream.set_read_timeout(Some(REQUEST_TIMEOUT))?;
    stream.set_write_timeout(Some(REQUEST_TIMEOUT))?;

    let bytes = request.to_line().unwrap();

    stream.write_all(&bytes)?;

    let mut reader = BufReader::new(stream);

    let mut response = Vec::new();

    reader.read_until(b'\n', &mut response)?;

    Ok(ResponseEnvelope::from_line(&response).unwrap())
}
