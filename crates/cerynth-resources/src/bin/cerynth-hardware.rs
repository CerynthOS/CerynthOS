#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&cerynth_resources::inventory().await?)?
    );
    Ok(())
}
