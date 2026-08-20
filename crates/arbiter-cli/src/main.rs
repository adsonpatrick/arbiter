mod commands;

use anyhow::Context;
use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = commands::Cli::parse();
    commands::run(cli).await.context("Arbiter command failed")
}
