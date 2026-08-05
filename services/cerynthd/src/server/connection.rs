use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};

use cerynth_config::RuntimeState;

use cerynth_ipc::{
    MAX_MESSAGE_SIZE,
    Request,
    ResponseEnvelope,
    SocketResponse,
};

use crate::{
    backend::SharedBackend,
    handlers::handle_request,
};

use super::codec::{decode_request, encode_response};

pub async fn handle_connection(
    mut stream: UnixStream,
    backend: SharedBackend,
) -> std::io::Result<()> {
    let mut buffer = vec![0u8; MAX_MESSAGE_SIZE];

    let n = stream.read(&mut buffer).await?;

    if n == 0 {
        return Ok(());
    }

    let envelope = match decode_request(&buffer[..n]) {
        Ok(req) => req,
        Err(err) => {
            eprintln!("Failed to parse request: {}", err);
            return Ok(());
        }
    };

    if let Err(err) = envelope.validate_version() {
        eprintln!("{}", err);
        return Ok(());
    }

    let request: Request = envelope.request.clone().into();

    let response = {
        let mut backend = backend.lock().await;

        let response = handle_request(&mut *backend, request);

        let runtime_state = RuntimeState::from(backend.state());

        runtime_state.save("runtime_state.json");

        response
    };

    let socket_response: SocketResponse = response.into();

    let response =
        ResponseEnvelope::for_request(&envelope, socket_response);

    let bytes = encode_response(&response).unwrap();

    stream.write_all(&bytes).await?;

    Ok(())
}
