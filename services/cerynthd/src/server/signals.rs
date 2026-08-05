use tokio::signal;

pub async fn wait_for_shutdown() -> std::io::Result<()> {
    signal::ctrl_c().await?;

    println!("\nShutdown signal received.");

    Ok(())
}
