use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use cerynth_ipc::{
    Frame, MAX_MESSAGE_SIZE, MIN_PROTOCOL_VERSION, PROTOCOL_VERSION, RequestEnvelope,
    ResponseEnvelope, socket_path,
};

pub fn send_request(request: RequestEnvelope) -> std::io::Result<ResponseEnvelope> {
    let timeout = match request.request {
        cerynth_ipc::SocketRequest::Start
        | cerynth_ipc::SocketRequest::Restart
        | cerynth_ipc::SocketRequest::Stop
        | cerynth_ipc::SocketRequest::SetProfile { .. } => crate::timeout::LIFECYCLE_TIMEOUT,
        _ => crate::timeout::REQUEST_TIMEOUT,
    };
    exchange(UnixStream::connect(socket_path())?, request, timeout)
}

fn exchange(
    mut stream: UnixStream,
    request: RequestEnvelope,
    timeout: Duration,
) -> io::Result<ResponseEnvelope> {
    let deadline = Instant::now() + timeout;
    let bytes = request.to_line().map_err(io::Error::other)?;
    if bytes.len() > MAX_MESSAGE_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "request exceeds frame limit",
        ));
    }
    let mut remaining = bytes.as_slice();
    while !remaining.is_empty() {
        stream.set_write_timeout(Some(time_left(deadline)?))?;
        match stream.write(remaining) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => remaining = &remaining[n..],
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let mut response = Vec::new();
    let mut chunk = [0u8; 4096];
    while response.len() <= MAX_MESSAGE_SIZE {
        stream.set_read_timeout(Some(time_left(deadline)?))?;
        let available = chunk.len().min(MAX_MESSAGE_SIZE + 1 - response.len());
        match stream.read(&mut chunk[..available]) {
            Ok(0) => break,
            Ok(n) => {
                if let Some(end) = chunk[..n].iter().position(|byte| *byte == b'\n') {
                    response.extend_from_slice(&chunk[..=end]);
                    break;
                }
                response.extend_from_slice(&chunk[..n]);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    if response.len() > MAX_MESSAGE_SIZE || response.last() != Some(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "response is oversized or incomplete",
        ));
    }
    let response = ResponseEnvelope::from_line(&response).map_err(io::Error::other)?;
    if !(MIN_PROTOCOL_VERSION..=PROTOCOL_VERSION).contains(&response.version) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported response version",
        ));
    }
    if response.correlation_id != request.correlation_id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "response correlation does not match request",
        ));
    }
    Ok(response)
}

fn time_left(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
        .ok_or_else(|| io::ErrorKind::TimedOut.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cerynth_ipc::SocketRequest;

    #[test]
    fn silent_daemon_cannot_block_client_indefinitely() {
        let (client, _server) = UnixStream::pair().unwrap();
        let started = std::time::Instant::now();
        let error = exchange(
            client,
            RequestEnvelope::new(SocketRequest::Status),
            Duration::from_millis(30),
        )
        .unwrap_err();
        assert!(matches!(
            error.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
    #[test]
    fn slow_response_cannot_extend_total_deadline() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let worker = std::thread::spawn(move || {
            for _ in 0..20 {
                if server.write_all(b" ").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let started = Instant::now();
        assert!(
            exchange(
                client,
                RequestEnvelope::new(SocketRequest::Status),
                Duration::from_millis(40)
            )
            .is_err()
        );
        assert!(started.elapsed() < Duration::from_millis(150));
        worker.join().unwrap();
    }
    #[test]
    fn malformed_and_oversized_responses_return_errors_without_panicking() {
        for bytes in [b"not-json\n".to_vec(), vec![b'x'; MAX_MESSAGE_SIZE + 1]] {
            let (client, mut server) = UnixStream::pair().unwrap();
            let worker = std::thread::spawn(move || {
                let _ = server.write_all(&bytes);
            });
            assert!(
                exchange(
                    client,
                    RequestEnvelope::new(SocketRequest::Status),
                    Duration::from_secs(1)
                )
                .is_err()
            );
            worker.join().unwrap();
        }
    }
}
