mod arxiv;
mod config;
mod db;
mod fetch;
mod llm;
mod mcp;
mod pubmed;
mod search;
mod slug;
mod worker;
mod workflow;

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use config::Config;

#[derive(Parser)]
#[command(name = "research_mcp", version, about = "Deep research tool — Feynman port with OpenCode Go LLM + multi-source search")]
struct Cli {
    /// Research topic (1-2 sentences)
    topic: Option<String>,

    /// Run as an MCP server (stdio)
    #[arg(long)]
    mcp: bool,

    /// OpenCode Go model ID (overrides config.toml)
    #[arg(long)]
    model: Option<String>,

    /// Max source pages to fetch (overrides config.toml)
    #[arg(long)]
    max_sources: Option<usize>,

    /// Output directory (overrides config.toml)
    #[arg(long)]
    out_dir: Option<PathBuf>,

    /// SQLite database path (overrides config.toml)
    #[arg(long)]
    db_path: Option<PathBuf>,

    /// OpenCode Go API key (overrides config.toml and env)
    #[arg(long)]
    api_key: Option<String>,

    /// Path to config file
    #[arg(long)]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Load config.
    let config_path = cli
        .config
        .clone()
        .or_else(|| Config::candidate_paths().into_iter().find(|p| p.exists()));
    let mut cfg = Config::load(config_path.as_deref())?;

    if let Some(m) = &cli.model {
        cfg.model = m.clone();
    }
    if let Some(n) = cli.max_sources {
        cfg.max_sources = n;
    }
    if let Some(d) = &cli.out_dir {
        cfg.out_dir = d.clone();
    }
    if let Some(d) = &cli.db_path {
        cfg.db_path = d.clone();
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

    let db = Arc::new(db::Db::open(&cfg.db_path)?);

    if cli.mcp {
        // ── MCP server mode ─────────────────────────────────────────
        let worker = worker::Worker::new(wf_cfg, db.clone());
        worker.spawn_mcp()?;
        return Ok(());
    }

    // ── CLI mode ───────────────────────────────────────────────────
    let topic = cli
        .topic
        .clone()
        .ok_or_else(|| anyhow::anyhow!("No topic given. Usage: research_mcp \"<topic>\" or research_mcp --mcp"))?;

    // Enqueue + run (synchronous, single-shot: enqueue and wait for completion).
    let slug = workflow::make_slug(&topic);
    let run = db.create_run(&topic, &slug, None, 0)?;
    println!("Run ID: {}", run.id);
    println!("Slug:   {slug}");

    let engine = workflow::Engine { cfg: wf_cfg, db: (*db).clone() };
    match engine.execute_run(&run).await {
        Ok(true) => {
            let r = db.get_run(&run.id)?.unwrap();
            println!("\n✔ Research complete.");
            println!("  slug: {}", r.slug);
            println!("  report:  {}", r.report_path.unwrap_or_default());
            println!("  provenance: {}", r.provenance_path.unwrap_or_default());
            Ok(())
        }
        Ok(false) => {
            let r = db.get_run(&run.id)?.unwrap();
            eprintln!("✖ Run ended without completion. Status: {}", r.status);
            if let Some(e) = r.error {
                eprintln!("  error: {e}");
            }
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("✖ Run failed: {e}");
            std::process::exit(1);
        }
    }
}
