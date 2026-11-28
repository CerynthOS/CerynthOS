use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};

use cerynth_config::RuntimeState;

use cerynth_ipc::{MAX_MESSAGE_SIZE, Request, ResponseEnvelope, SocketResponse};

use crate::{backend::SharedBackend, handlers::handle_request, paths::state_path};

use super::codec::{decode_request, encode_response};

pub async fn handle_connection(stream: UnixStream, backend: SharedBackend) -> std::io::Result<()> {
    // Unix stream reads may split a JSON frame anywhere. Bound the complete
    // frame and its total read time, including clients sending one byte slowly.
    let mut reader = BufReader::new(stream).take((MAX_MESSAGE_SIZE + 1) as u64);
    let mut buffer = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        reader.read_until(b'\n', &mut buffer),
    )
    .await
    .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))??;
    if buffer.is_empty() {
        return Ok(());
    }
    if buffer.len() > MAX_MESSAGE_SIZE || buffer.last() != Some(&b'\n') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "oversized or incomplete request",
        ));
    }
    let mut stream = reader.into_inner().into_inner();
    let envelope = match decode_request(&buffer) {
        Ok(req) => req,
        Err(err) => {
            eprintln!("Failed to parse request: {err}");
            return Ok(());
        }
    };

    if let Err(err) = envelope.validate_version() {
        eprintln!("{err}");
        return Ok(());
    }

    let request: Request = envelope.request.clone().into();

    let response = {
        let mut backend = backend.lock().await;

        let response = handle_request(&mut **backend, &request);

        // Persist runtime state derived from the backend's own status so
        // profile/adaptation changes survive a daemon restart.
        if let Ok(status) = backend.status() {
            let runtime_state = RuntimeState {
                profile: status.profile,
                adaptation_enabled: status.adaptation_enabled,
                adaptation_mode: status.adaptation_mode,
                scheduler_backend: status.backend,
            };

            runtime_state.save(state_path());
        }

        response
    };

    let socket_response: SocketResponse = response.into();

    let response = ResponseEnvelope::for_request(&envelope, socket_response);

    let bytes = encode_response(&response).map_err(std::io::Error::other)?;
    tokio::time::timeout(std::time::Duration::from_secs(5), stream.write_all(&bytes))
        .await
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))??;

    Ok(())
}
