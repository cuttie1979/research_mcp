mod fetch;
mod llm;
mod search;
mod slug;
mod workflow;

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(name = "research_mcp", version, about = "Deep research tool — Feynman port with OpenCode Go LLM + DuckDuckGo search")]
struct Cli {
    /// Research topic (1-2 sentences)
    topic: String,

    /// OpenCode Go model ID
    #[arg(long, default_value = "deepseek-v4-flash")]
    model: String,

    /// Max source pages to fetch
    #[arg(long, default_value = "8")]
    max_sources: usize,

    /// Output directory for .md + .provenance.md
    #[arg(long, default_value = "/home/user/AIDocumentStore/raw/research")]
    out_dir: PathBuf,

    /// OpenCode Go API key (falls back to env OPENCODE_GO_API_KEY)
    #[arg(long, env = "OPENCODE_GO_API_KEY")]
    api_key: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let api_key = cli
        .api_key
        .clone()
        .or_else(|| std::env::var("OPENCODE_GO_API_KEY").ok())
        .filter(|k| !k.is_empty());
    if api_key.is_none() {
        eprintln!("⚠  OPENCODE_GO_API_KEY not set. Set it via env or --api-key before running.");
        std::process::exit(2);
    }

    let cfg = workflow::Config {
        model: cli.model,
        api_key: api_key.unwrap(),
        out_dir: cli.out_dir,
        max_sources: cli.max_sources,
    };

    let report = workflow::run(&cfg, &cli.topic).await?;
    println!();
    println!("✔ Research complete.");
    println!("  slug: {}", report.slug);
    println!("  report:  {}", report.report_path.display());
    println!("  provenance: {}", report.provenance_path.display());
    println!("  sources accepted: {}", report.sources_accepted);

    Ok(())
}
