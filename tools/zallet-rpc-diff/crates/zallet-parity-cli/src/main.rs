use color_eyre::eyre::Result;
use clap::Parser;
use tracing::{info, debug};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the method-suite manifest
    #[arg(short, long, default_value = "manifest.toml")]
    manifest: String,

    /// Path to the endpoint configuration
    #[arg(short, long, default_value = "config.toml")]
    config: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Install color-eyre for better error reporting
    color_eyre::install()?;

    // Initialize tracing
    tracing_subscriber::fmt::init();

    info!("Starting Zallet Parity Harness");

    let args = Args::parse();
    debug!("Arguments parsed: {:?}", args);

    // TODO: Implement execution runner

    info!("Done");
    Ok(())
}
