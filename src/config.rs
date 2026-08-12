//! Configuration loading — config.toml with env/CLI fallbacks.
//!
//! Precedence: CLI flag > config.toml > env var > built-in default.
//! Config path: ./config.toml, then $HOME/.config/research_mcp/config.toml.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// OpenCode Go API key. Secret — keep out of git.
    pub api_key: Option<String>,
    /// OpenCode Go model ID.
    pub model: String,
    /// Base URL for the LLM API.
    pub llm_base_url: String,
    /// Max source pages to fetch.
    pub max_sources: usize,
    /// Output directory for .md + .provenance.md.
    pub out_dir: PathBuf,
    /// SQLite database path.
    pub db_path: PathBuf,
    /// Temperature for drafting.
    pub temperature: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_key: None,
            model: "deepseek-v4-flash".to_string(),
            llm_base_url: "https://opencode.ai/zen/go/v1".to_string(),
            max_sources: 8,
            out_dir: PathBuf::from("/home/user/AIDocumentStore/raw/research"),
            db_path: PathBuf::from("/home/user/AIHome/research_mcp/research.db"),
            temperature: 0.3,
        }
    }
}

impl Config {
    /// Load config from a path, if it exists. Returns default config otherwise.
    pub fn load(path: Option<&std::path::Path>) -> Result<Self> {
        let mut cfg = Config::default();

        if let Some(p) = path {
            if p.exists() {
                let raw = std::fs::read_to_string(p)
                    .with_context(|| format!("read config {}", p.display()))?;
                let file_cfg: Config = toml::from_str(&raw)
                    .with_context(|| format!("parse config {}", p.display()))?;
                cfg.apply(file_cfg);
            }
        }

        // Env fallback for the API key (never stored in git).
        if cfg.api_key.is_none() {
            cfg.api_key = std::env::var("OPENCODE_GO_API_KEY").ok().filter(|k| !k.is_empty());
        }

        Ok(cfg)
    }

    fn apply(&mut self, other: Config) {
        if other.api_key.is_some() {
            self.api_key = other.api_key;
        }
        if !other.model.is_empty() {
            self.model = other.model;
        }
        if !other.llm_base_url.is_empty() {
            self.llm_base_url = other.llm_base_url;
        }
        if other.max_sources != 0 {
            self.max_sources = other.max_sources;
        }
        if !other.out_dir.as_os_str().is_empty() {
            self.out_dir = other.out_dir;
        }
        if !other.db_path.as_os_str().is_empty() {
            self.db_path = other.db_path;
        }
        if other.temperature != 0.0 {
            self.temperature = other.temperature;
        }
    }

    /// Resolve effective api_key or error out with a clear message.
    pub fn require_api_key(&self) -> Result<String> {
        self.api_key
            .clone()
            .filter(|k| !k.is_empty())
            .context("API key missing. Set it in config.toml (api_key) or env OPENCODE_GO_API_KEY.")
    }

    pub fn candidate_paths() -> Vec<PathBuf> {
        let mut paths = vec![PathBuf::from("config.toml")];
        if let Ok(home) = std::env::var("HOME") {
            paths.push(PathBuf::from(home).join(".config/research_mcp/config.toml"));
        }
        paths
    }
}
