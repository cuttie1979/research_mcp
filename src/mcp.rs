//! MCP server — exposes research queue tools to OpenCode over stdio.
//!
//! Tools:
//! - research_submit(topic, priority?) → run_id
//! - research_batch_submit(topics[]) → batch_id + run_ids
//! - research_list(status?, limit?, offset?) → runs
//! - research_status(run_id) → full state incl. phase log
//! - research_cancel(run_id | batch_id) → cancelled runs
//! - research_resume(run_id | batch_id) → re-queued runs
//! - research_output(run_id) → report + provenance content

use std::sync::Arc;

use anyhow::Result;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::IntoContents;
use rmcp::service::serve_server;
use rmcp::transport::stdio;
use rmcp::{ErrorData, RmcpError, tool, tool_router};
use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::db::{Db, ResearchRun};
use crate::worker::Worker;

pub struct ResearchMcp {
    db: Arc<Db>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SubmitParams {
    /// Research topic (1-2 sentences)
    pub topic: String,
    /// Queue priority (higher = sooner). Default 0.
    #[serde(default)]
    pub priority: Option<i64>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, JsonSchema)]
pub struct BatchSubmitParams {
    /// List of research topics
    pub topics: Vec<String>,
    /// Queue priority for all runs. Default 0.
    #[serde(default)]
    pub priority: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListParams {
    /// Filter by status: queued|running|complete|failed|blocked|cancelled
    pub status: Option<String>,
    /// Max results. Default 20.
    pub limit: Option<i64>,
    /// Offset. Default 0.
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunIdParams {
    /// Research run ID
    pub run_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CancelParams {
    /// Research run ID (optional if batch_id given)
    pub run_id: Option<String>,
    /// Batch ID (optional if run_id given)
    pub batch_id: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SubmitResult {
    pub run_id: String,
    pub status: String,
    pub slug: String,
}

impl IntoContents for SubmitResult {
    fn into_contents(self) -> Vec<rmcp::model::ContentBlock> {
        vec![rmcp::model::ContentBlock::text(serde_json::to_string(&self).unwrap_or_default())]
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct BatchSubmitResult {
    pub batch_id: String,
    pub run_ids: Vec<String>,
}

impl IntoContents for BatchSubmitResult {
    fn into_contents(self) -> Vec<rmcp::model::ContentBlock> {
        vec![rmcp::model::ContentBlock::text(serde_json::to_string(&self).unwrap_or_default())]
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RunView {
    pub id: String,
    pub topic: String,
    pub slug: String,
    pub status: String,
    pub phase: String,
    pub progress: i64,
    pub error: Option<String>,
    pub report_path: Option<String>,
    pub provenance_path: Option<String>,
    pub batch_id: Option<String>,
    pub priority: i64,
    pub attempt: i64,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RunListResult {
    pub runs: Vec<RunView>,
    pub total: usize,
}

impl IntoContents for RunListResult {
    fn into_contents(self) -> Vec<rmcp::model::ContentBlock> {
        vec![rmcp::model::ContentBlock::text(serde_json::to_string(&self).unwrap_or_default())]
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct StatusResult {
    pub run: RunView,
    pub phase_log: Vec<PhaseLogEntry>,
}

impl IntoContents for StatusResult {
    fn into_contents(self) -> Vec<rmcp::model::ContentBlock> {
        vec![rmcp::model::ContentBlock::text(serde_json::to_string(&self).unwrap_or_default())]
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PhaseLogEntry {
    pub phase: String,
    pub status: String,
    pub message: String,
    pub created_at: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct OutputResult {
    pub run_id: String,
    pub report_path: Option<String>,
    pub provenance_path: Option<String>,
    pub report: Option<String>,
    pub provenance: Option<String>,
}

impl IntoContents for OutputResult {
    fn into_contents(self) -> Vec<rmcp::model::ContentBlock> {
        vec![rmcp::model::ContentBlock::text(serde_json::to_string(&self).unwrap_or_default())]
    }
}

impl From<ResearchRun> for RunView {
    fn from(r: ResearchRun) -> Self {
        RunView {
            id: r.id,
            topic: r.topic,
            slug: r.slug,
            status: r.status,
            phase: r.phase,
            progress: r.progress,
            error: r.error,
            report_path: r.report_path,
            provenance_path: r.provenance_path,
            batch_id: r.batch_id,
            priority: r.priority,
            attempt: r.attempt,
            created_at: r.created_at,
            updated_at: r.updated_at,
            started_at: r.started_at,
            completed_at: r.completed_at,
        }
    }
}

impl ResearchMcp {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }

    #[allow(dead_code)]
    pub fn spawn(db: Arc<Db>, worker: Worker) -> Result<()> {
        // Start the worker in the background.
        tokio::spawn(async move {
            if let Err(e) = worker.run_loop().await {
                eprintln!("worker crashed: {e}");
            }
        });

        let server = Self::new(db);
        let transport = stdio();
        tokio::runtime::Handle::current().block_on(async {
            let _running = serve_server(server, transport).await?;
            Ok::<(), RmcpError>(())
        })?;
        Ok(())
    }

    /// Serve the MCP server forever (awaits the stdio connection). Runs the
    /// worker in the background. Must be called from a tokio runtime.
    pub async fn serve_async(db: Arc<Db>, worker: Worker) -> Result<(), RmcpError> {
        tokio::spawn(async move {
            if let Err(e) = worker.run_loop().await {
                eprintln!("worker crashed: {e}");
            }
        });
        let server = Self::new(db);
        let transport = stdio();
        let running = serve_server(server, transport).await?;
        // Keep the service alive: waiting() blocks until the connection ends.
        let _ = running.waiting().await;
        Ok(())
    }
}

#[tool_router(server_handler)]
impl ResearchMcp {
    /// Submit a research topic. Returns a run_id immediately; the worker
    /// processes the queue sequentially (max 1 concurrent run).
    #[tool(description = "Submit a deep research topic. Returns a run_id immediately. The worker runs one research at a time and updates status at every phase.")]
    async fn research_submit(
        &self,
        Parameters(params): Parameters<SubmitParams>,
    ) -> Result<SubmitResult, ErrorData> {
        let priority = params.priority.unwrap_or(0);
        let slug = crate::workflow::make_slug(&params.topic);
        let run = self
            .db
            .create_run(&params.topic, &slug, None, priority)
            .map_err(err)?;
        Ok(SubmitResult { run_id: run.id.clone(), status: run.status, slug: run.slug })
    }

    /// Re-run a job: creates a NEW run with the same topic and fresh session
    /// state. Unlike resume, this starts from scratch — for testing fixes.
    #[tool(description = "Re-run a previous research job: creates a new run with the same topic and fresh session state (starts from scratch, unlike resume). Returns the new run_id.")]
    async fn research_rerun(
        &self,
        Parameters(params): Parameters<RunIdParams>,
    ) -> Result<SubmitResult, ErrorData> {
        let original = self
            .db
            .get_run(&params.run_id)
            .map_err(err)?
            .ok_or_else(|| invalid_request(format!("run {} not found", params.run_id)))?;
        let slug = crate::workflow::make_slug(&original.topic);
        let run = self
            .db
            .create_run(&original.topic, &slug, None, original.priority)
            .map_err(err)?;
        Ok(SubmitResult { run_id: run.id.clone(), status: run.status, slug: run.slug })
    }

    /// Submit multiple research topics as one batch. Returns a batch_id plus
    /// the run_ids. The batch can be cancelled/resumed as a unit.
    #[tool(description = "Submit multiple research topics as a batch. Returns batch_id and run_ids. Batch can be cancelled/resumed as a unit.")]
    async fn research_batch_submit(
        &self,
        Parameters(params): Parameters<BatchSubmitParams>,
    ) -> Result<BatchSubmitResult, ErrorData> {
        if params.topics.is_empty() {
            return Err(invalid_request("topics must not be empty"));
        }
        let batch_id = uuid::Uuid::new_v4().to_string();
        let priority = params.priority.unwrap_or(0);
        let mut run_ids = Vec::new();
        for t in &params.topics {
            let slug = crate::workflow::make_slug(t);
            let run = self
                .db
                .create_run(t, &slug, Some(&batch_id), priority)
                .map_err(err)?;
            run_ids.push(run.id);
        }
        Ok(BatchSubmitResult { batch_id, run_ids })
    }

    /// List research runs, optionally filtered by status.
    #[tool(description = "List research runs, optionally filtered by status (queued|running|complete|failed|blocked|cancelled).")]
    async fn research_list(
        &self,
        Parameters(params): Parameters<ListParams>,
    ) -> Result<RunListResult, ErrorData> {
        let limit = params.limit.unwrap_or(20).clamp(1, 200);
        let offset = params.offset.unwrap_or(0).max(0);
        let runs = self
            .db
            .list_runs(params.status.as_deref(), limit, offset)
            .map_err(err)?;
        let views: Vec<RunView> = runs.into_iter().map(RunView::from).collect();
        let total = views.len();
        Ok(RunListResult { runs: views, total })
    }

    /// Get the full status of a research run, including its phase log.
    #[tool(description = "Get the full status of a research run by ID, including progress, phase, error, output paths, and the phase log.")]
    async fn research_status(
        &self,
        Parameters(params): Parameters<RunIdParams>,
    ) -> Result<StatusResult, ErrorData> {
        let run = self
            .db
            .get_run(&params.run_id)
            .map_err(err)?
            .ok_or_else(|| invalid_request(format!("run {} not found", params.run_id)))?;
        let events = self.db.phase_log(&params.run_id).map_err(err)?;
        let phase_log = events
            .into_iter()
            .map(|e| PhaseLogEntry { phase: e.phase, status: e.status, message: e.message, created_at: e.created_at })
            .collect();
        Ok(StatusResult { run: run.into(), phase_log })
    }

    /// Cancel a run or an entire batch. Running runs are marked cancelled
    /// (the worker checks the status before starting the next phase).
    #[tool(description = "Cancel a research run by run_id, or an entire batch by batch_id. Running runs are marked cancelled between phases.")]
    async fn research_cancel(
        &self,
        Parameters(params): Parameters<CancelParams>,
    ) -> Result<RunListResult, ErrorData> {
        let runs = self.resolve_target(&params.run_id, &params.batch_id)?;
        let mut out = Vec::new();
        for run in runs {
            if run.status == "queued" || run.status == "running" {
                self.db
                    .update_status(&run.id, "cancelled", &run.phase, run.progress, Some("cancelled by user"))
                    .map_err(err)?;
                self.db.log_phase(&run.id, &run.phase, "cancelled", "cancelled by user").map_err(err)?;
            }
            out.push(self.db.get_run(&run.id).map_err(err)?.unwrap().into());
        }
        let views = out;
        let total = views.len();
        Ok(RunListResult { runs: views, total })
    }

    /// Re-queue a failed/blocked/cancelled run or an entire batch.
    #[tool(description = "Re-queue a failed/blocked/cancelled run by run_id, or an entire batch by batch_id. Attempt counter resets.")]
    async fn research_resume(
        &self,
        Parameters(params): Parameters<CancelParams>,
    ) -> Result<RunListResult, ErrorData> {
        let runs = self.resolve_target(&params.run_id, &params.batch_id)?;
        let mut out = Vec::new();
        for run in runs {
            if run.status == "failed" || run.status == "blocked" || run.status == "cancelled" {
                self.db
                    .update_status(&run.id, "queued", "planning", 0, None)
                    .map_err(err)?;
                self.db.log_phase(&run.id, "planning", "resumed", "re-queued by user").map_err(err)?;
            }
            out.push(self.db.get_run(&run.id).map_err(err)?.unwrap().into());
        }
        let views = out;
        let total = views.len();
        Ok(RunListResult { runs: views, total })
    }

    /// Read the generated report and provenance files of a completed run.
    #[tool(description = "Read the generated markdown report and provenance file of a run. Returns file contents.")]
    async fn research_output(
        &self,
        Parameters(params): Parameters<RunIdParams>,
    ) -> Result<OutputResult, ErrorData> {
        let run = self
            .db
            .get_run(&params.run_id)
            .map_err(err)?
            .ok_or_else(|| invalid_request(format!("run {} not found", params.run_id)))?;
        let report = run.report_path.as_ref().and_then(|p| std::fs::read_to_string(p).ok());
        let provenance = run.provenance_path.as_ref().and_then(|p| std::fs::read_to_string(p).ok());
        Ok(OutputResult {
            run_id: run.id,
            report_path: run.report_path,
            provenance_path: run.provenance_path,
            report,
            provenance,
        })
    }

    fn resolve_target(&self, run_id: &Option<String>, batch_id: &Option<String>) -> Result<Vec<ResearchRun>, ErrorData> {
        match (run_id, batch_id) {
            (Some(id), _) => Ok(self
                .db
                .get_run(id)
                .map_err(err)?
                .map(|r| vec![r])
                .unwrap_or_default()),
            (None, Some(b)) => self.db.list_by_batch(b).map_err(err),
            (None, None) => Err(invalid_request(
                "provide either run_id or batch_id",
            )),
        }
    }
}

fn err(e: anyhow::Error) -> ErrorData {
    ErrorData::internal_error(e.to_string(), None)
}

fn invalid_request(msg: impl Into<String>) -> ErrorData {
    ErrorData::invalid_request(msg.into(), None)
}
