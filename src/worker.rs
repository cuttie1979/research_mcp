//! Background worker — processes the SQLite queue.
//! - Max 1 concurrent run
//! - Crash recovery on startup (stale 'running' → 'queued')
//! - Retry policy: unlimited retries per phase, but auth errors (401/403) → failed immediately
//! - Exponential backoff between retries (1s → 2s → 4s → ... → max 60s)

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

use crate::db::Db;
use crate::workflow::{Config as WorkflowConfig, Engine};

const MAX_BACKOFF_SECS: u64 = 60;
const POLL_INTERVAL: Duration = Duration::from_secs(2);

pub struct Worker {
    engine: Arc<Engine>,
    db: Arc<Db>,
}

impl Worker {
    pub fn new(cfg: WorkflowConfig, db: Arc<Db>) -> Self {
        let engine = Arc::new(Engine { cfg, db: (*db).clone() });
        Self { engine, db }
    }

    /// Spawn the worker loop + serve the MCP server over stdio.
    pub async fn spawn_mcp(&self) -> anyhow::Result<()> {
        crate::mcp::ResearchMcp::serve_async(self.db.clone(), Worker {
            engine: self.engine.clone(),
            db: self.db.clone(),
        })
        .await?;
        Ok(())
    }

    /// Run the worker loop forever: pick the next queued run, execute it.
    pub async fn run_loop(&self) -> Result<()> {
        let recovered = self.db.recover_stale_runs()?;
        if recovered > 0 {
            log_warn!("worker: recovered {recovered} stale run(s) (running → queued)");
        }

        loop {
            let next = self.db.next_queued()?;
            match next {
                Some(run) => {
                    self.db.set_run_started(&run.id)?;
                    self.db.log_phase(&run.id, &run.phase, "started", "run picked up by worker")?;
                    log_info!("\n════════════════════════════════════════");
                    log_info!("worker: executing run {} ({})", run.id, run.topic);
                    log_info!("════════════════════════════════════════\n");

                    match self.execute_with_retry(&run).await {
                        Ok(completed) => {
                            if completed {
                                log_info!("worker: run {} COMPLETE", run.id);
                            } else {
                                // Run did not complete but returned Ok — treat as blocked.
                                log_warn!("worker: run {} did not complete", run.id);
                            }
                        }
                        Err(e) => {
                            // Retry policy: auth errors → failed; others → keep retrying
                            // inside execute_with_retry until success or auth failure.
                            log_warn!("worker: run {} failed: {e}", run.id);
                        }
                    }
                }
                None => {
                    tokio::time::sleep(POLL_INTERVAL).await;
                }
            }
        }
    }

    /// Execute a run with the retry policy:
    /// - auth errors (401/403) → failed immediately
    /// - other errors → unlimited retries with exponential backoff
    async fn execute_with_retry(&self, run: &crate::db::ResearchRun) -> Result<bool> {
        let mut backoff = 1u64;
        loop {
            match self.engine.execute_run(run).await {
                Ok(completed) => return Ok(completed),
                Err(e) => {
                    if is_auth_error(&e) {
                        self.db.update_status(&run.id, "failed", "planning", 0, Some(&e.to_string()))?;
                        self.db.log_phase(&run.id, "planning", "failed", &format!("auth error: {e}"))?;
                        return Ok(false);
                    }
                    let delay = Duration::from_secs(backoff.min(MAX_BACKOFF_SECS));
                    log_warn!("worker: run {} error (retrying in {}s): {e}", run.id, delay.as_secs());
                    tokio::time::sleep(delay).await;
                    backoff *= 2;
                    // Reset to queued phase bookkeeping so a crash between retries
                    // still resumes correctly (session data is already persisted).
                    self.db.update_status(&run.id, "running", "planning", 0, None)?;
                }
            }
        }
    }
}

fn is_auth_error(e: &anyhow::Error) -> bool {
    let msg = e.to_string();
    msg.contains("401") || msg.contains("403") || msg.contains("Invalid API key") || msg.contains("Unauthorized")
}
