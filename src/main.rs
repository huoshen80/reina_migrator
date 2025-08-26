use anyhow::Result;
use reina_migrator::migrator;

#[tokio::main]
async fn main() -> Result<()> {
    migrator::run_migration().await
}
