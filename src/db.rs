//! SQLite persistence layer.
//!
//! Tables:
//! - research_runs: state machine per research run
//! - research_session: per-run phase artifacts (plan, sources, draft/cited/review text)
//! - research_phase_log: timestamped phase events (audit trail)

use anyhow::{Context, Result};
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params};

pub const RUN_STATUSES: &[&str] = &[
    "queued", "running", "complete", "failed", "blocked", "cancelled",
];

pub const PHASES: &[&str] = &[
    "planning", "searching", "fetching", "drafting", "citing", "reviewing", "delivering",
];

#[derive(Debug, Clone)]
pub struct ResearchRun {
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

#[derive(Debug, Clone, Default)]
pub struct SessionData {
    pub plan_json: Option<String>,
    pub sources_json: Option<String>,
    pub draft_text: Option<String>,
    pub cited_text: Option<String>,
    pub review_text: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PhaseEvent {
    pub id: i64,
    pub run_id: String,
    pub phase: String,
    pub status: String,
    pub message: String,
    pub created_at: String,
}

#[derive(Clone)]
pub struct Db {
    conn: std::sync::Arc<Mutex<Connection>>,
}

impl Db {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("create db parent dir")?;
        }
        let conn = std::sync::Arc::new(Mutex::new(Connection::open(path).with_context(|| format!("open db {}", path.display()))?));
        conn.lock().unwrap().execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        let db = Db { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS research_runs (
                id TEXT PRIMARY KEY,
                topic TEXT NOT NULL,
                slug TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'queued',
                phase TEXT NOT NULL DEFAULT 'planning',
                progress INTEGER NOT NULL DEFAULT 0,
                error TEXT,
                report_path TEXT,
                provenance_path TEXT,
                batch_id TEXT,
                priority INTEGER NOT NULL DEFAULT 0,
                attempt INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                started_at TEXT,
                completed_at TEXT
            );
            CREATE TABLE IF NOT EXISTS research_session (
                run_id TEXT PRIMARY KEY REFERENCES research_runs(id) ON DELETE CASCADE,
                plan_json TEXT,
                sources_json TEXT,
                draft_text TEXT,
                cited_text TEXT,
                review_text TEXT
            );
            CREATE TABLE IF NOT EXISTS research_phase_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id TEXT NOT NULL REFERENCES research_runs(id) ON DELETE CASCADE,
                phase TEXT NOT NULL,
                status TEXT NOT NULL,
                message TEXT,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_runs_status ON research_runs(status);
            CREATE INDEX IF NOT EXISTS idx_runs_batch ON research_runs(batch_id);
            CREATE INDEX IF NOT EXISTS idx_phase_log_run ON research_phase_log(run_id);
            ",
        )?;
        Ok(())
    }

    pub fn now() -> String {
        chrono::Utc::now().to_rfc3339()
    }

    // ── runs ────────────────────────────────────────────────────────

    pub fn create_run(&self, topic: &str, slug: &str, batch_id: Option<&str>, priority: i64) -> Result<ResearchRun> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Self::now();
        self.conn.lock().unwrap().execute(
            "INSERT INTO research_runs
                (id, topic, slug, status, phase, progress, batch_id, priority, attempt, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'queued', 'planning', 0, ?4, ?5, 1, ?6, ?6)",
            params![id, topic, slug, batch_id, priority, now],
        )?;
        self.get_run(&id)?.context("run created but not found")
    }

    pub fn get_run(&self, id: &str) -> Result<Option<ResearchRun>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, topic, slug, status, phase, progress, error,
                        report_path, provenance_path, batch_id, priority, attempt,
                        created_at, updated_at, started_at, completed_at
                 FROM research_runs WHERE id = ?1",
                params![id],
                |r| {
                    Ok(ResearchRun {
                        id: r.get(0)?,
                        topic: r.get(1)?,
                        slug: r.get(2)?,
                        status: r.get(3)?,
                        phase: r.get(4)?,
                        progress: r.get(5)?,
                        error: r.get(6)?,
                        report_path: r.get(7)?,
                        provenance_path: r.get(8)?,
                        batch_id: r.get(9)?,
                        priority: r.get(10)?,
                        attempt: r.get(11)?,
                        created_at: r.get(12)?,
                        updated_at: r.get(13)?,
                        started_at: r.get(14)?,
                        completed_at: r.get(15)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    pub fn list_runs(&self, status: Option<&str>, limit: i64, offset: i64) -> Result<Vec<ResearchRun>> {
        let mut sql = String::from(
            "SELECT id, topic, slug, status, phase, progress, error,
                    report_path, provenance_path, batch_id, priority, attempt,
                    created_at, updated_at, started_at, completed_at
             FROM research_runs",
        );
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(s) = status {
            sql.push_str(" WHERE status = ?1");
            params.push(Box::new(s.to_string()));
        }
        sql.push_str(" ORDER BY created_at DESC LIMIT ?");
        params.push(Box::new(limit));
        sql.push_str(" OFFSET ?");
        params.push(Box::new(offset));

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())), |r| {
            Ok(ResearchRun {
                id: r.get(0)?,
                topic: r.get(1)?,
                slug: r.get(2)?,
                status: r.get(3)?,
                phase: r.get(4)?,
                progress: r.get(5)?,
                error: r.get(6)?,
                report_path: r.get(7)?,
                provenance_path: r.get(8)?,
                batch_id: r.get(9)?,
                priority: r.get(10)?,
                attempt: r.get(11)?,
                created_at: r.get(12)?,
                updated_at: r.get(13)?,
                started_at: r.get(14)?,
                completed_at: r.get(15)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn list_by_batch(&self, batch_id: &str) -> Result<Vec<ResearchRun>> {
        self.list_runs_filtered(&format!("batch_id = ?1"), vec![batch_id.to_string()])
    }

    fn list_runs_filtered(&self, where_clause: &str, args: Vec<String>) -> Result<Vec<ResearchRun>> {
        let sql = format!(
            "SELECT id, topic, slug, status, phase, progress, error,
                    report_path, provenance_path, batch_id, priority, attempt,
                    created_at, updated_at, started_at, completed_at
             FROM research_runs WHERE {where_clause} ORDER BY created_at DESC"
        );
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&sql)?;
        let params = rusqlite::params_from_iter(args.iter().map(|s| s.as_str()));
        let rows = stmt.query_map(params, |r| {
            Ok(ResearchRun {
                id: r.get(0)?,
                topic: r.get(1)?,
                slug: r.get(2)?,
                status: r.get(3)?,
                phase: r.get(4)?,
                progress: r.get(5)?,
                error: r.get(6)?,
                report_path: r.get(7)?,
                provenance_path: r.get(8)?,
                batch_id: r.get(9)?,
                priority: r.get(10)?,
                attempt: r.get(11)?,
                created_at: r.get(12)?,
                updated_at: r.get(13)?,
                started_at: r.get(14)?,
                completed_at: r.get(15)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn update_status(&self, id: &str, status: &str, phase: &str, progress: i64, error: Option<&str>) -> Result<()> {
        let now = Self::now();
        self.conn.lock().unwrap().execute(
            "UPDATE research_runs
             SET status = ?2, phase = ?3, progress = ?4, error = ?5, updated_at = ?6,
                 started_at = COALESCE(started_at, ?6),
                 completed_at = CASE WHEN ?2 IN ('complete','failed','blocked','cancelled') THEN ?6 ELSE completed_at END
             WHERE id = ?1",
            params![id, status, phase, progress, error, now],
        )?;
        Ok(())
    }

    pub fn set_run_started(&self, id: &str) -> Result<()> {
        let now = Self::now();
        self.conn.lock().unwrap().execute(
            "UPDATE research_runs SET status='running', started_at = COALESCE(started_at, ?2), updated_at = ?2 WHERE id = ?1",
            params![id, now],
        )?;
        Ok(())
    }

    pub fn set_complete(&self, id: &str, report_path: &str, provenance_path: &str) -> Result<()> {
        let now = Self::now();
        self.conn.lock().unwrap().execute(
            "UPDATE research_runs
             SET status='complete', phase='delivering', progress=100, error=NULL,
                 report_path = ?2, provenance_path = ?3, completed_at = ?4, updated_at = ?4
             WHERE id = ?1",
            params![id, report_path, provenance_path, now],
        )?;
        Ok(())
    }

    pub fn increment_attempt(&self, id: &str) -> Result<i64> {
        self.conn.lock().unwrap().execute(
            "UPDATE research_runs SET attempt = attempt + 1, updated_at = ?2 WHERE id = ?1",
            params![id, Self::now()],
        )?;
        let attempt: i64 = self.conn.lock().unwrap().query_row(
            "SELECT attempt FROM research_runs WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )?;
        Ok(attempt)
    }

    /// Crash recovery: any run left in 'running' is stale (process died).
    /// Reset to 'queued' so the worker picks it up again.
    pub fn recover_stale_runs(&self) -> Result<usize> {
        let now = Self::now();
        let n = self.conn.lock().unwrap().execute(
            "UPDATE research_runs SET status='queued', updated_at = ?2 WHERE status='running'",
            params![1, now],
        )?;
        Ok(n)
    }

    /// Next run to process: highest priority, then oldest. Only 'queued'.
    pub fn next_queued(&self) -> Result<Option<ResearchRun>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, topic, slug, status, phase, progress, error,
                        report_path, provenance_path, batch_id, priority, attempt,
                        created_at, updated_at, started_at, completed_at
                 FROM research_runs
                 WHERE status = 'queued'
                 ORDER BY priority DESC, created_at ASC
                 LIMIT 1",
                [],
                |r| {
                    Ok(ResearchRun {
                        id: r.get(0)?,
                        topic: r.get(1)?,
                        slug: r.get(2)?,
                        status: r.get(3)?,
                        phase: r.get(4)?,
                        progress: r.get(5)?,
                        error: r.get(6)?,
                        report_path: r.get(7)?,
                        provenance_path: r.get(8)?,
                        batch_id: r.get(9)?,
                        priority: r.get(10)?,
                        attempt: r.get(11)?,
                        created_at: r.get(12)?,
                        updated_at: r.get(13)?,
                        started_at: r.get(14)?,
                        completed_at: r.get(15)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    // ── session ─────────────────────────────────────────────────────

    pub fn save_session(&self, run_id: &str, data: &SessionData) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO research_session (run_id, plan_json, sources_json, draft_text, cited_text, review_text)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(run_id) DO UPDATE SET
                plan_json = COALESCE(excluded.plan_json, plan_json),
                sources_json = COALESCE(excluded.sources_json, sources_json),
                draft_text = COALESCE(excluded.draft_text, draft_text),
                cited_text = COALESCE(excluded.cited_text, cited_text),
                review_text = COALESCE(excluded.review_text, review_text)",
            params![
                run_id,
                data.plan_json,
                data.sources_json,
                data.draft_text,
                data.cited_text,
                data.review_text,
            ],
        )?;
        Ok(())
    }

    pub fn get_session(&self, run_id: &str) -> Result<Option<SessionData>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT plan_json, sources_json, draft_text, cited_text, review_text
                 FROM research_session WHERE run_id = ?1",
                params![run_id],
                |r| {
                    Ok(SessionData {
                        plan_json: r.get(0)?,
                        sources_json: r.get(1)?,
                        draft_text: r.get(2)?,
                        cited_text: r.get(3)?,
                        review_text: r.get(4)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    // ── phase log ───────────────────────────────────────────────────

    pub fn log_phase(&self, run_id: &str, phase: &str, status: &str, message: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO research_phase_log (run_id, phase, status, message, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![run_id, phase, status, message, Self::now()],
        )?;
        Ok(())
    }

    pub fn phase_log(&self, run_id: &str) -> Result<Vec<PhaseEvent>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, run_id, phase, status, message, created_at
             FROM research_phase_log WHERE run_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![run_id], |r| {
            Ok(PhaseEvent {
                id: r.get(0)?,
                run_id: r.get(1)?,
                phase: r.get(2)?,
                status: r.get(3)?,
                message: r.get(4)?,
                created_at: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn count_running(&self) -> Result<i64> {
        let n: i64 = self.conn.lock().unwrap().query_row(
            "SELECT COUNT(*) FROM research_runs WHERE status = 'running'",
            [],
            |r| r.get(0),
        )?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Db {
        let path = std::env::temp_dir().join(format!("research_mcp_test_{}.db", uuid::Uuid::new_v4()));
        Db::open(&path).unwrap()
    }

    #[test]
    fn create_and_get_run() {
        let db = test_db();
        let run = db.create_run("Test topic", "test-topic", Some("B1"), 0).unwrap();
        assert_eq!(run.status, "queued");
        let fetched = db.get_run(&run.id).unwrap().unwrap();
        assert_eq!(fetched.topic, "Test topic");
        assert_eq!(fetched.batch_id.as_deref(), Some("B1"));
    }

    #[test]
    fn status_transitions() {
        let db = test_db();
        let run = db.create_run("T", "t", None, 0).unwrap();
        db.set_run_started(&run.id).unwrap();
        assert_eq!(db.get_run(&run.id).unwrap().unwrap().status, "running");
        db.update_status(&run.id, "running", "drafting", 60, None).unwrap();
        let r = db.get_run(&run.id).unwrap().unwrap();
        assert_eq!(r.phase, "drafting");
        assert_eq!(r.progress, 60);
        db.set_complete(&run.id, "/out/x.md", "/out/x.provenance.md").unwrap();
        let r = db.get_run(&run.id).unwrap().unwrap();
        assert_eq!(r.status, "complete");
        assert_eq!(r.progress, 100);
        assert!(r.completed_at.is_some());
    }

    #[test]
    fn session_upsert() {
        let db = test_db();
        let run = db.create_run("T", "t", None, 0).unwrap();
        let s1 = SessionData { plan_json: Some("{\"q\":1}".into()), ..Default::default() };
        db.save_session(&run.id, &s1).unwrap();
        let s2 = SessionData { draft_text: Some("draft".into()), ..Default::default() };
        db.save_session(&run.id, &s2).unwrap();
        let got = db.get_session(&run.id).unwrap().unwrap();
        assert_eq!(got.plan_json.as_deref(), Some("{\"q\":1}"));
        assert_eq!(got.draft_text.as_deref(), Some("draft"));
    }

    #[test]
    fn next_queued_priority_order() {
        let db = test_db();
        let low = db.create_run("low", "low", None, 0).unwrap();
        let high = db.create_run("high", "high", None, 5).unwrap();
        assert_eq!(db.next_queued().unwrap().unwrap().id, high.id);
        db.set_run_started(&high.id).unwrap();
        assert_eq!(db.next_queued().unwrap().unwrap().id, low.id);
    }

    #[test]
    fn crash_recovery_resets_running() {
        let db = test_db();
        let run = db.create_run("T", "t", None, 0).unwrap();
        db.set_run_started(&run.id).unwrap();
        let n = db.recover_stale_runs().unwrap();
        assert_eq!(n, 1);
        assert_eq!(db.get_run(&run.id).unwrap().unwrap().status, "queued");
    }

    #[test]
    fn phase_log_records_events() {
        let db = test_db();
        let run = db.create_run("T", "t", None, 0).unwrap();
        db.log_phase(&run.id, "planning", "ok", "planned").unwrap();
        db.log_phase(&run.id, "searching", "ok", "searched").unwrap();
        let events = db.phase_log(&run.id).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].phase, "searching");
    }

    #[test]
    fn list_filters_by_status() {
        let db = test_db();
        let r1 = db.create_run("a", "a", None, 0).unwrap();
        let _r2 = db.create_run("b", "b", None, 0).unwrap();
        db.set_complete(&r1.id, "p", "q").unwrap();
        let completed = db.list_runs(Some("complete"), 10, 0).unwrap();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].id, r1.id);
        let all = db.list_runs(None, 10, 0).unwrap();
        assert_eq!(all.len(), 2);
    }
}
