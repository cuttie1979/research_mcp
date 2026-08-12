mod config;
mod fetch;
mod llm;
mod search;
mod slug;
mod workflow;

use clap::Parser;
use config::Config;

#[derive(Parser)]
#[command(name = "research_mcp", version, about = "Deep research tool — Feynman port with OpenCode Go LLM + DuckDuckGo search")]
struct Cli {
    /// Research topic (1-2 sentences)
    topic: String,

    /// OpenCode Go model ID (overrides config.toml)
    #[arg(long)]
    model: Option<String>,

    /// Max source pages to fetch (overrides config.toml)
    #[arg(long)]
    max_sources: Option<usize>,

    /// Output directory (overrides config.toml)
    #[arg(long)]
    out_dir: Option<std::path::PathBuf>,

    /// OpenCode Go API key (overrides config.toml and env)
    #[arg(long)]
    api_key: Option<String>,

    /// Path to config file
    #[arg(long)]
    config: Option<std::path::PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Load config from explicit path, or auto-discover config.toml.
    let config_path = cli
        .config
        .clone()
        .or_else(|| Config::candidate_paths().into_iter().find(|p| p.exists()));
    let mut cfg = Config::load(config_path.as_deref())?;

    // CLI overrides.
    if let Some(m) = &cli.model {
        cfg.model = m.clone();
    }
    if let Some(n) = cli.max_sources {
        cfg.max_sources = n;
    }
    if let Some(d) = &cli.out_dir {
        cfg.out_dir = d.clone();
    }
    if let Some(k) = &cli.api_key {
        cfg.api_key = Some(k.clone());
    }

    let api_key = cfg.require_api_key()?;

    let wf_cfg = workflow::Config {
        model: cfg.model.clone(),
        api_key,
        llm_base_url: cfg.llm_base_url.clone(),
        out_dir: cfg.out_dir.clone(),
        max_sources: cfg.max_sources,
        temperature: cfg.temperature,
    };

    let report = workflow::run(&wf_cfg, &cli.topic).await?;
    println!();
    println!("✔ Research complete.");
    println!("  slug: {}", report.slug);
    println!("  report:  {}", report.report_path.display());
    println!("  provenance: {}", report.provenance_path.display());
    println!("  sources accepted: {}", report.sources_accepted);

    Ok(())
}
