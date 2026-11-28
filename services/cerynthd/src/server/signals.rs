use tokio::signal;

pub fn wait_for_shutdown() -> std::io::Result<impl std::future::Future<Output = std::io::Result<()>>>
{
    let mut terminate = signal::unix::signal(signal::unix::SignalKind::terminate())?;
    Ok(async move {
        tokio::select! {
            result = signal::ctrl_c() => result?,
            _ = terminate.recv() => {},
        }

        println!("\nShutdown signal received.");

        Ok(())
    })
}
