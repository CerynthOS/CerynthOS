use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

use cerynth_ipc::{Frame, RequestEnvelope, ResponseEnvelope};

const SOCKET_PATH: &str = "/tmp/cerynthd.sock";

pub fn send_request(
    request: RequestEnvelope,
) -> std::io::Result<ResponseEnvelope> {
    let mut stream = UnixStream::connect(SOCKET_PATH)?;

    let bytes = request.to_line().unwrap();

    stream.write_all(&bytes)?;

    let mut reader = BufReader::new(stream);

    let mut response = Vec::new();

    reader.read_until(b'\n', &mut response)?;

    Ok(ResponseEnvelope::from_line(&response).unwrap())
}
