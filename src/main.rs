#[tokio::main]
pub async fn main() -> anyhow::Result<()> {
    autoscaler_genie::run().await?;
    Ok(())
}
