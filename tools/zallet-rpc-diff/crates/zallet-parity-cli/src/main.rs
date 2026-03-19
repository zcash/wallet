use clap::{Parser, Subcommand};
use color_eyre::Result;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::path::PathBuf;
use zallet_parity_core::client::RpcClient;
use zallet_parity_core::engine::ParityEngine;
use zallet_parity_core::manifest::Manifest;
use zallet_parity_core::report::FinalReport;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Runs a parity check between two RPC endpoints.
    Run {
        /// URL of the upstream (source of truth) endpoint.
        #[arg(short, long, env = "UPSTREAM_URL")]
        upstream_url: String,

        /// URL of the target (to be tested) endpoint.
        #[arg(short, long, env = "TARGET_URL")]
        target_url: String,

        /// Path to the manifest file defining the RPC methods.
        #[arg(short, long, default_value = "manifest.toml")]
        manifest: PathBuf,

        /// Path where the report will be saved.
        #[arg(short, long, default_value = "report.json")]
        output: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            upstream_url,
            target_url,
            manifest,
            output,
        } => {
            run_parity_check(upstream_url, target_url, manifest, output).await?;
        }
    }

    Ok(())
}

async fn run_parity_check(
    upstream_url: String,
    target_url: String,
    manifest_path: PathBuf,
    output_path: PathBuf,
) -> Result<()> {
    println!("🚀 Starting Zallet Parity Check");
    println!("   Upstream: {}", upstream_url);
    println!("   Target:   {}", target_url);
    println!();

    let manifest = Manifest::load(&manifest_path)?;
    let upstream = RpcClient::new(&upstream_url)?;
    let target = RpcClient::new(&target_url)?;
    let engine = ParityEngine::new(upstream, target);

    let multi = MultiProgress::new();
    let pb = multi.add(ProgressBar::new(manifest.methods.len() as u64));
    
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}",
        )?
        .progress_chars("#>-"),
    );

    pb.set_message("Executing RPC calls...");

    let results = engine.run_all(manifest.methods).await;
    
    pb.finish_with_message("Done!");

    let report = FinalReport::new(results);
    
    // Save report as JSON
    let json_output = serde_json::to_string_pretty(&report)?;
    std::fs::write(&output_path, json_output)?;
    
    // Save report as Markdown
    let md_path = output_path.with_extension("md");
    std::fs::write(md_path, report.to_markdown())?;

    println!("\n✅ Parity check complete!");
    println!("   Summary: {} total, {} matches, {} diffs, {} errors", 
             report.summary.total, report.summary.matches, report.summary.diffs, report.summary.errors);
    println!("   Report saved to: {}", output_path.display());

    Ok(())
}
