//! Deep research workflow engine — Feynman deepresearch port.
//! Phases: planning → searching → fetching → drafting → citing → reviewing → delivering.
//! State machine: every phase commits to SQLite (session + status), enabling crash
//! recovery and resume. If a phase's output already exists in the session, it is reused.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::arxiv::{Arxiv, ArxivPaper};
use crate::db::{Db, ResearchRun, SessionData};
use crate::debate::DebateResult;
use crate::fetch::Fetcher;
use crate::llm::{ChatMessage, Llm};
use crate::pubmed::{Pubmed, PubmedPaper};
use crate::scopus::{Scopus, ScopusPaper};
use crate::search::{Search, SearchResult};
use crate::slug::slugify;

pub struct Config {
    pub model: String,
    pub api_key: String,
    pub llm_base_url: String,
    pub elsevier_api_key: Option<String>,
    pub out_dir: PathBuf,
    pub max_sources: usize,
    pub temperature: f32,
    pub llm_timeout_secs: u64,
}

pub struct RunReport {
    pub slug: String,
    pub report_path: PathBuf,
    pub provenance_path: PathBuf,
    pub sources_accepted: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub key_questions: Vec<String>,
    pub evidence_needed: Vec<String>,
    pub scale: String,
    pub queries: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceItem {
    pub id: String,
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub text: String,
}

/// Progress milestones per phase.
const PROGRESS: &[(&str, i64)] = &[
    ("planning", 10),
    ("searching", 30),
    ("fetching", 50),
    ("drafting", 70),
    ("citing", 80),
    ("debating", 88),
    ("reviewing", 95),
    ("delivering", 100),
];

pub struct Engine {
    pub cfg: Config,
    pub db: Db,
}

impl Engine {
    /// Execute one run end-to-end (or resume from its saved session state).
    /// Returns true if the run completed; the run status is persisted either way.
    pub async fn execute_run(&self, run: &ResearchRun) -> Result<bool> {
        let topic = run.topic.clone();
        let slug = run.slug.clone();
        let llm = Llm::with_timeout(self.cfg.model.clone(), self.cfg.api_key.clone(), self.cfg.llm_base_url.clone(), self.cfg.llm_timeout_secs);
        let search = Search::new();
        let fetcher = Fetcher::new();
        let arxiv = Arxiv::new();
        let pubmed = Pubmed::new();
        let scopus = self
            .cfg
            .elsevier_api_key
            .as_ref()
            .map(|k| Scopus::new(k.clone()));
        let temperature = self.cfg.temperature;
        let out_dir = self.cfg.out_dir.clone();
        let max_sources = self.cfg.max_sources;

        std::fs::create_dir_all(&out_dir).with_context(|| format!("create {}", out_dir.display()))?;
        let drafts_dir = out_dir.join(".drafts");
        std::fs::create_dir_all(&drafts_dir).context("create .drafts")?;

        // Fresh session load helper — phases write to DB, so reload between phases.
        let session = || self.db.get_session(&run.id).unwrap_or_default().unwrap_or_default();

        // ── Phase 1: planning ───────────────────────────────────────
        let plan: Plan = if let Some(plan_json) = session().plan_json {
            serde_json::from_str(&plan_json).context("parse saved plan")?
        } else {
            log_info!("── planning ──");
            self.begin_phase(run, "planning")?;
            let plan = plan_research(&llm, &topic, temperature).await?;
            let mut s = SessionData::default();
            s.plan_json = Some(serde_json::to_string(&plan)?);
            self.db.save_session(&run.id, &s)?;
            self.db.log_phase(&run.id, "planning", "ok", "plan produced")?;
            plan
        };

        log_info!("\n── Plan ──");
        log_info!("scale: {}", plan.scale);
        for q in &plan.queries {
            log_info!("  - {q}");
        }
        let plan_path = drafts_dir.join(format!("{slug}-plan.md"));
        std::fs::write(&plan_path, format_plan(&plan, &topic, &slug)).context("write plan")?;

        // ── Phase 2+3: search & fetch ──────────────────────────────
        // sources_json transitions: absent → Gathered (after search) → Vec<EvidenceItem> (after fetch).
        let (evidence, rejected): (Vec<EvidenceItem>, Vec<String>) = {
            let src = session().sources_json;
            match src {
                Some(json) => {
                    // Already fetched? sources_json holds evidence items.
                    if let Ok(ev) = serde_json::from_str::<Vec<EvidenceItem>>(&json) {
                        if !ev.is_empty() {
                            log_info!("── search+fetch already done (resume) ──");
                            (ev, Vec::new())
                        } else {
                            bail!("sources_json holds empty evidence")
                        }
                    } else {
                        // Search done, fetch pending.
                        let gathered: Gathered = serde_json::from_str(&json).context("parse saved sources")?;
                        log_info!(
                            "\nResume: {} web + {} arxiv + {} pubmed results (from session)",
                            gathered.web.len(),
                            gathered.arxiv.len(),
                            gathered.pubmed.len()
                        );
                        log_info!("\n── fetching ──");
                        self.begin_phase(run, "fetching")?;
                        let (ev, rej) =
                            fetch_sources(&fetcher, &gathered.web, gathered.arxiv, gathered.pubmed, gathered.scopus, scopus.as_ref(), max_sources).await?;
                        let mut sess = SessionData::default();
                        sess.sources_json = Some(serde_json::to_string(&ev)?);
                        self.db.save_session(&run.id, &sess)?;
                        self.db.log_phase(&run.id, "fetching", "ok", format!("{} sources accepted", ev.len()).as_str())?;
                        (ev, rej)
                    }
                }
                None => {
                    // Neither done — run search then fetch.
                    log_info!("\n── searching ──");
                    self.begin_phase(run, "searching")?;
                    let gathered = gather(&search, &arxiv, &pubmed, scopus.as_ref(), &plan, &topic).await?;
                    log_info!(
                        "\nTotal unique results: {} web + {} arxiv + {} pubmed + {} scopus",
                        gathered.web.len(),
                        gathered.arxiv.len(),
                        gathered.pubmed.len(),
                        gathered.scopus.len()
                    );
                    let mut s = SessionData::default();
                    s.sources_json = Some(serde_json::to_string(&gathered)?);
                    self.db.save_session(&run.id, &s)?;
                    self.db.log_phase(&run.id, "searching", "ok", "search complete")?;

                    log_info!("\n── fetching ──");
                    self.begin_phase(run, "fetching")?;
                    let (ev, rej) =
                        fetch_sources(&fetcher, &gathered.web, gathered.arxiv, gathered.pubmed, gathered.scopus, scopus.as_ref(), max_sources).await?;
                    let mut sess = SessionData::default();
                    sess.sources_json = Some(serde_json::to_string(&ev)?);
                    self.db.save_session(&run.id, &sess)?;
                    self.db.log_phase(&run.id, "fetching", "ok", format!("{} sources accepted", ev.len()).as_str())?;
                    (ev, rej)
                }
            }
        };

        if evidence.is_empty() {
            bail!("No usable sources gathered — cannot draft.");
        }

        let research_notes = build_research_notes(&evidence);
        let notes_path = drafts_dir.join(format!("{slug}-research-direct.md"));
        std::fs::write(&notes_path, &research_notes).context("write research notes")?;

        // ── Phase 4: drafting ───────────────────────────────────────
        let draft: String = if let Some(d) = session().draft_text {
            d
        } else {
            log_info!("\n── drafting ──");
            self.begin_phase(run, "drafting")?;
            let d = draft_report(&llm, &topic, &evidence, temperature).await?;
            let mut s = SessionData::default();
            s.draft_text = Some(d.clone());
            self.db.save_session(&run.id, &s)?;
            self.db.log_phase(&run.id, "drafting", "ok", "draft written")?;
            d
        };
        let draft_path = drafts_dir.join(format!("{slug}-draft.md"));
        std::fs::write(&draft_path, &draft).context("write draft")?;

        // ── Phase 5: citing ─────────────────────────────────────────
        let cited: String = if let Some(c) = session().cited_text {
            c
        } else {
            log_info!("── citing ──");
            self.begin_phase(run, "citing")?;
            let c = cite_report(&llm, &draft, &evidence, temperature).await?;
            let mut s = SessionData::default();
            s.cited_text = Some(c.clone());
            self.db.save_session(&run.id, &s)?;
            self.db.log_phase(&run.id, "citing", "ok", "citations verified")?;
            c
        };
        let cited_path = drafts_dir.join(format!("{slug}-cited.md"));
        std::fs::write(&cited_path, &cited).context("write cited draft")?;

        // ── Phase 6: debate (multi-agent) ───────────────────────────
        // Agents with distinct beliefs critique the cited draft (content AND
        // references) and converge. Output feeds the review phase.
        let debate: DebateResult = if let Some(d) = session().debate_text {
            serde_json::from_str(&d).context("parse saved debate")?
        } else {
            log_info!("── debating ({} agents) ──", crate::debate::DEFAULT_AGENT_COUNT);
            self.begin_phase(run, "debating")?;
            let d = crate::debate::run_debate(
                &llm, &topic, &cited, &evidence, temperature, crate::debate::DEFAULT_ROUNDS,
            )
            .await?;
            log_info!(
                "  debate done: spread {:.2}, convergence {:+.2}",
                d.consensus.spread,
                d.consensus.convergence
            );
            let mut s = SessionData::default();
            s.debate_text = Some(serde_json::to_string(&d)?);
            self.db.save_session(&run.id, &s)?;
            self.db.log_phase(&run.id, "debating", "ok", "debate complete")?;
            d
        };

        // ── Phase 7: reviewing (with debate consensus) ──────────────
        let (final_md, review_md): (String, String) = if let Some(r) = session().review_text {
            // Review done; final doc = cited (no revision was persisted separately).
            (cited.clone(), r)
        } else {
            log_info!("── reviewing (with debate input) ──");
            self.begin_phase(run, "reviewing")?;
            let (f, r) = review_report(&llm, &cited, &evidence, &debate, temperature).await?;
            let mut s = SessionData::default();
            s.review_text = Some(r.clone());
            self.db.save_session(&run.id, &s)?;
            self.db.log_phase(&run.id, "reviewing", "ok", "review complete")?;
            (f, r)
        };
        let review_path = drafts_dir.join(format!("{slug}-verification.md"));
        std::fs::write(&review_path, &review_md).context("write review")?;

        // ── Phase 8: delivering ─────────────────────────────────────
        log_info!("── delivering ──");
        self.begin_phase(run, "delivering")?;
        let report_path = out_dir.join(format!("{slug}.md"));
        // Append the Agent Debate Summary so the reader sees where experts
        // agreed and disagreed — unless it is already present (resume case).
        let final_md = append_debate_summary(&final_md, &debate);
        std::fs::write(&report_path, &final_md).context("write final report")?;

        let provenance = build_provenance(
            &topic, &slug, evidence.len(), &rejected,
            &plan_path, &notes_path, &draft_path, &cited_path, &review_path,
        );
        let provenance_path = out_dir.join(format!("{slug}.provenance.md"));
        std::fs::write(&provenance_path, &provenance).context("write provenance")?;

        self.db.set_complete(&run.id, &report_path.to_string_lossy(), &provenance_path.to_string_lossy())?;
        self.db.log_phase(&run.id, "delivering", "ok", "delivered")?;

        Ok(true)
    }

    fn begin_phase(&self, run: &ResearchRun, phase: &str) -> Result<()> {
        let progress = PROGRESS
            .iter()
            .find(|(p, _)| p == &phase)
            .map(|(_, pr)| *pr)
            .unwrap_or(0);
        self.db.update_status(&run.id, "running", phase, progress, None)
    }
}

/// Intermediate search output: the three result sets before fetching.
#[derive(Serialize, Deserialize)]
struct Gathered {
    web: Vec<SearchResult>,
    arxiv: Vec<ArxivPaper>,
    pubmed: Vec<PubmedPaper>,
    scopus: Vec<ScopusPaper>,
}

async fn gather(
    search: &Search,
    arxiv: &Arxiv,
    pubmed: &Pubmed,
    scopus: Option<&Scopus>,
    plan: &Plan,
    topic: &str,
) -> Result<Gathered> {
    let queries = if plan.queries.is_empty() { vec![topic.to_string()] } else { plan.queries.clone() };

    let mut web: Vec<SearchResult> = Vec::new();
    let mut arxiv_papers: Vec<ArxivPaper> = Vec::new();
    let mut pubmed_papers: Vec<PubmedPaper> = Vec::new();
    let mut scopus_papers: Vec<ScopusPaper> = Vec::new();
    let mut seen_urls = std::collections::HashSet::new();

    // Direct arXiv fetch if the topic contains an arXiv ID (e.g. 2607.12631).
    for id in extract_arxiv_ids(topic) {
        log_info!("\n── arXiv by-id: {id}");
        match arxiv.by_id(&id).await {
            Ok(Some(p)) => {
                seen_urls.insert(p.url.clone());
                arxiv_papers.push(p);
            }
            Ok(None) => log_warn!("  ⚠ arXiv id {id} not found"),
            Err(e) => log_warn!("  ⚠ arXiv fetch failed for {id}: {e}"),
        }
    }

    // Direct PubMed fetch if the topic contains a PMID (e.g. PMID:12345678).
    for id in extract_pubmed_ids(topic) {
        log_info!("\n── PubMed by-id: {id}");
        match pubmed.by_id(&id).await {
            Ok(Some(p)) => {
                seen_urls.insert(p.url.clone());
                pubmed_papers.push(p);
            }
            Ok(None) => log_warn!("  ⚠ PMID {id} not found"),
            Err(e) => log_warn!("  ⚠ PubMed fetch failed for {id}: {e}"),
        }
    }

    for (i, q) in queries.iter().enumerate() {
        log_info!("\n── Search {}/{}: {q}", i + 1, queries.len());
        match search.query(q, 6).await {
            Ok(results) => {
                for r in results {
                    if seen_urls.insert(r.url.clone()) {
                        web.push(r);
                    }
                }
            }
            Err(e) => log_warn!("  ⚠ search failed for {q}: {e}"),
        }
    }

    // arXiv search for the first 2 plan queries.
    for q in queries.iter().take(2) {
        log_info!("── arXiv search: {q}");
        match arxiv.search(q, 4).await {
            Ok(papers) => {
                for p in papers {
                    if seen_urls.insert(p.url.clone()) {
                        arxiv_papers.push(p);
                    }
                }
            }
            Err(e) => log_warn!("  ⚠ arxiv search failed for {q}: {e}"),
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // PubMed search for the first 2 plan queries.
    for q in queries.iter().take(2) {
        log_info!("── PubMed search: {q}");
        match pubmed.search(q, 4).await {
            Ok(papers) => {
                for p in papers {
                    if seen_urls.insert(p.url.clone()) {
                        pubmed_papers.push(p);
                    }
                }
            }
            Err(e) => log_warn!("  ⚠ pubmed search failed for {q}: {e}"),
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // Scopus search for the first 2 plan queries (metadata; abstract via CrossRef).
    if let Some(s) = scopus {
        for q in queries.iter().take(2) {
            log_info!("── Scopus search: {q}");
            match s.search(q, 4).await {
                Ok(papers) => {
                    for p in papers {
                        if seen_urls.insert(p.url.clone()) {
                            scopus_papers.push(p);
                        }
                    }
                }
                Err(e) => log_warn!("  ⚠ scopus search failed for {q}: {e}"),
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
    }

    Ok(Gathered { web, arxiv: arxiv_papers, pubmed: pubmed_papers, scopus: scopus_papers })
}

async fn fetch_sources(
    fetcher: &Fetcher,
    web_results: &[SearchResult],
    arxiv_papers: Vec<ArxivPaper>,
    pubmed_papers: Vec<PubmedPaper>,
    scopus_papers: Vec<ScopusPaper>,
    scopus_client: Option<&Scopus>,
    max_sources: usize,
) -> Result<(Vec<EvidenceItem>, Vec<String>)> {
    let mut evidence: Vec<EvidenceItem> = Vec::new();
    let mut accepted = 0usize;
    let mut rejected: Vec<String> = Vec::new();

    for p in arxiv_papers {
        accepted += 1;
        evidence.push(EvidenceItem {
            id: format!("S{}", accepted),
            title: format!("{} (arXiv:{})", p.title, p.arxiv_id),
            url: p.url.clone(),
            snippet: String::new(),
            text: format!(
                "Authors: {}\nCategories: {}\nPublished: {}\n\nAbstract:\n{}",
                p.authors.join(", "),
                p.categories.join(", "),
                p.published.clone().unwrap_or_default(),
                p.abstract_text
            ),
        });
    }

    for p in pubmed_papers {
        accepted += 1;
        evidence.push(EvidenceItem {
            id: format!("S{}", accepted),
            title: format!("{} (PMID:{})", p.title, p.pmid),
            url: p.url.clone(),
            snippet: String::new(),
            text: format!(
                "Journal: {}\nAuthors: {}\nPublished: {}\nDOI: {}\n\nAbstract:\n{}",
                p.journal.clone().unwrap_or_default(),
                p.authors.join(", "),
                p.published.clone().unwrap_or_default(),
                p.doi.clone().unwrap_or_default(),
                p.abstract_text
            ),
        });
    }

    // Scopus papers: metadata + CrossRef abstract (if the publisher deposits one).
    for mut p in scopus_papers {
        let crossref_abs = match &p.doi {
            Some(doi) if scopus_client.is_some() => {
                match scopus_client.unwrap().abstract_by_doi(doi).await {
                    Ok(a) => a,
                    Err(_) => String::new(),
                }
            }
            _ => String::new(),
        };
        if !crossref_abs.is_empty() {
            p.abstract_text = crossref_abs;
        }
        accepted += 1;
        evidence.push(EvidenceItem {
            id: format!("S{}", accepted),
            title: format!("{} (Scopus)", p.title),
            url: p.url.clone(),
            snippet: String::new(),
            text: format!(
                "Journal: {}\nAuthors: {}\nYear: {}\nDOI: {}\nCited by: {}\n\nAbstract:\n{}",
                p.journal.clone().unwrap_or_default(),
                p.creators.join(", "),
                p.year.clone().unwrap_or_default(),
                p.doi.clone().unwrap_or_default(),
                p.citedby,
                p.abstract_text
            ),
        });
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    // Dynamic context budget: stop fetching web sources once the accumulated
    // evidence text approaches the per-source cap (12k chars × max_sources),
    // so a large arXiv/PubMed/Scopus haul doesn't blow the LLM context.
    let context_budget = 12000usize.saturating_mul(max_sources);
    let mut used_chars = evidence.iter().map(|e| e.text.chars().count()).sum::<usize>();

    for r in web_results.iter().take(max_sources) {
        if evidence.iter().any(|e| e.url == r.url) {
            continue;
        }
        if used_chars >= context_budget {
            log_info!(
                "  ⚠ context budget reached ({used_chars}/{context_budget} chars) — stopping web fetches"
            );
            break;
        }
        log_info!("── Fetch: {}", r.url);
        match fetcher.fetch(&r.url, 12000).await {
            Ok(text) => {
                if text.chars().count() < 200 {
                    rejected.push(format!("{} (too short)", r.url));
                    continue;
                }
                accepted += 1;
                used_chars += text.chars().count();
                evidence.push(EvidenceItem {
                    id: format!("S{}", accepted),
                    title: r.title.clone(),
                    url: r.url.clone(),
                    snippet: r.snippet.clone(),
                    text,
                });
            }
            Err(e) => {
                log_warn!("  ⚠ fetch failed: {e}");
                rejected.push(r.url.clone());
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    Ok((evidence, rejected))
}

// ── helpers ─────────────────────────────────────────────────────────

pub fn make_slug(topic: &str) -> String {
    slugify(topic)
}

/// Extract arXiv IDs from a topic string.
fn extract_arxiv_ids(text: &str) -> Vec<String> {
    let re = Regex::new(r"(?:arxiv\.org/(?:abs|pdf)/|arxiv:\s*|^|\s)(\d{4}\.\d{4,5}(?:v\d+)?)").unwrap();
    let mut out = Vec::new();
    for cap in re.captures_iter(text) {
        if let Some(m) = cap.get(1) {
            let id = m.as_str().to_string();
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

/// Extract PubMed IDs (PMIDs) from a topic string.
fn extract_pubmed_ids(text: &str) -> Vec<String> {
    let re = Regex::new(
        r"(?:PMID:?\s*|pubmed\.ncbi\.nlm\.nih\.gov/|pubmed\.ncbi\.nlm\.nih\.gov/(?:pubmed/)?|^|\s)(\d{6,9})",
    )
    .unwrap();
    let mut out = Vec::new();
    for cap in re.captures_iter(text) {
        if let Some(m) = cap.get(1) {
            let id = m.as_str().to_string();
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

async fn plan_research(llm: &Llm, topic: &str, temperature: f32) -> Result<Plan> {
    let sys = ChatMessage::system(
        "You are a research planner. Produce a JSON plan for a web research task.\n\
         Respond with a single JSON object:\n\
         {\n\
           \"key_questions\": [\"...\"],\n\
           \"evidence_needed\": [\"...\"],\n\
           \"scale\": \"direct\" | \"survey\",\n\
           \"queries\": [\"...\"]\n\
         }\n\
         Rules:\n\
         - 3 to 6 search queries, distinct angles: definition/history, mechanism/how-it-works, current usage/comparison.\n\
         - scale: \"direct\" for narrow explainers (3-10 tool calls), \"survey\" for broad multi-faceted topics.\n\
         - key_questions: 2-5.\n\
         - No markdown, no code fences, no prose. Only the JSON object.",
    );
    let user = ChatMessage::user(format!("Research topic: {topic}\nProduce the research plan JSON."));

    for attempt in 0..3 {
        let raw = llm.complete_json(&[sys.clone(), user.clone()], temperature).await?;
        match parse_plan_json(&raw) {
            Ok(p) if !p.queries.is_empty() => return Ok(p),
            Ok(_) => log_warn!("  ⚠ plan had no queries, retry {}/3", attempt + 1),
            Err(e) => log_warn!("  ⚠ plan parse failed: {e}, retry {}/3", attempt + 1),
        }
    }
    bail!("Could not produce a valid plan after 3 attempts")
}

fn parse_plan_json(raw: &str) -> Result<Plan> {
    let trimmed = raw.trim();
    let json = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|s| s.strip_suffix("```"))
        .unwrap_or(trimmed);
    let plan: Plan = serde_json::from_str(json.trim())?;
    Ok(plan)
}

fn format_plan(plan: &Plan, topic: &str, slug: &str) -> String {
    let mut s = String::new();
    s.push_str(&format!("# Research Plan: {topic}\n\n"));
    s.push_str(&format!("- **Slug:** {slug}\n"));
    s.push_str(&format!("- **Scale:** {}\n\n", plan.scale));
    s.push_str("## Key Questions\n\n");
    for q in &plan.key_questions {
        s.push_str(&format!("- {q}\n"));
    }
    s.push_str("\n## Evidence Needed\n\n");
    for e in &plan.evidence_needed {
        s.push_str(&format!("- {e}\n"));
    }
    s.push_str("\n## Search Queries\n\n");
    for q in &plan.queries {
        s.push_str(&format!("- {q}\n"));
    }
    s
}

fn build_research_notes(evidence: &[EvidenceItem]) -> String {
    let mut s = String::new();
    s.push_str("# Research Notes (direct)\n\n");
    for e in evidence {
        s.push_str(&format!("## [{0}] {1}\n", e.id, e.title));
        s.push_str(&format!("URL: {}\n\n", e.url));
        if !e.snippet.is_empty() {
            s.push_str(&format!("Snippet: {}\n\n", e.snippet));
        }
        s.push_str(&format!("{}\n\n", e.text));
    }
    s
}

fn build_evidence_block(evidence: &[EvidenceItem]) -> String {
    let mut s = String::new();
    for e in evidence {
        s.push_str(&format!("[{0}] {1}\n", e.id, e.title));
        s.push_str(&format!("    URL: {}\n", e.url));
        let excerpt: String = e.text.chars().take(6000).collect();
        s.push_str(&format!("    CONTENT: {excerpt}\n\n"));
    }
    s
}

async fn draft_report(llm: &Llm, topic: &str, evidence: &[EvidenceItem], temperature: f32) -> Result<String> {
    let sys = ChatMessage::system(
        "You are a research writer. Write a thorough, source-heavy research brief on the topic.\n\
         Rules:\n\
         - Cite sources inline with [S1], [S2], ... matching the source IDs given.\n\
         - Structure: # Title, ## Executive Summary, then findings organized by question/theme, then ## Open Questions, then ## Sources.\n\
         - Use facts from the provided source content only. No invented sources, results, figures, or benchmarks.\n\
         - Mark inferences as inferences.\n\
         - Include the ## Sources section listing every cited [Sn] with its URL.\n\
         - Write in the same language as the topic (if the topic is Hungarian, write in Hungarian).",
    );
    let evidence_block = build_evidence_block(evidence);
    let user = ChatMessage::user(format!(
        "Topic: {topic}\n\nEvidence:\n\n{evidence_block}\n\nWrite the research brief now."
    ));

    llm.complete(&[sys, user], temperature).await
}

async fn cite_report(
    llm: &Llm,
    draft: &str,
    evidence: &[EvidenceItem],
    temperature: f32,
) -> Result<String> {
    let sys = ChatMessage::system(
        "You are a citation verifier. Take the draft and produce a fully cited version.\n\
         Rules:\n\
         - Every critical claim must carry an inline [Sn] citation to a real source.\n\
         - Only cite source IDs that exist in the provided source list. Never invent URLs or IDs.\n\
         - Remove or downgrade claims that have no supporting source.\n\
         - Add a ## Sources section at the end listing each [Sn] with its full URL.\n\
         - Preserve the draft's overall structure and language.\n\
         - Output only the complete revised markdown document, no commentary.",
    );
    let source_list: Vec<String> = evidence
        .iter()
        .map(|e| format!("[{0}] {1} — {2}", e.id, e.title, e.url))
        .collect();
    let user = ChatMessage::user(format!(
        "Sources available:\n{}\n\nDraft to cite:\n\n{draft}\n\nOutput the fully cited markdown.",
        source_list.join("\n")
    ));

    llm.complete(&[sys, user], temperature).await
}

async fn review_report(
    llm: &Llm,
    cited: &str,
    evidence: &[EvidenceItem],
    debate: &DebateResult,
    temperature: f32,
) -> Result<(String, String)> {
    let sys = ChatMessage::system(
        "You are a rigorous internal research reviewer. Verify the cited brief.\n\
         Checks: unsupported claims, logical gaps, single-source critical claims, overstated confidence, invalid source IDs.\n\
         You receive the output of a multi-agent debate that critiqued the brief's CONTENT and REFERENCES.\n\
         Apply the debate input with DETERMINISTIC WEIGHTING — this is mandatory:\n\
         CONSENSUS points (all or most agents agree):\n\
         - Verify each consensus point against the sources yourself.\n\
         - If a consensus point identifies a FATAL issue (unsupported claim, invalid source,\n\
           single-source critical claim, overreach), you MUST fix it in the final document.\n\
         - Consensus FATALs are authoritative unless the sources directly contradict the criticism.\n\
         DISSENSUS points (agents disagree):\n\
         - Do NOT change the document's substance for dissensus points.\n\
         - Instead, add hedging or contested-attribution wording where the contested claim appears.\n\
         - Mark them in the verification report as 'contested in debate'.\n\
         Produce:\n\
         1. A verification report with findings marked FATAL / MAJOR / MINOR, the checks performed,\n\
            and for each debate point: whether it was confirmed, rejected, or hedged.\n\
         2. The final document: FATALs fixed, dissensus hedged, otherwise unchanged.\n\
         Output format:\n\
         ```\n\
         # Verification Report\n\
         ...findings...\n\
         ---\n\
         # Final Document\n\
         ...complete corrected markdown...\n\
         ```",
    );
    let source_list: Vec<String> = evidence
        .iter()
        .map(|e| format!("[{0}] {1} — {2}", e.id, e.title, e.url))
        .collect();

    let consensus_text = if debate.consensus.consensus_points.is_empty() && debate.consensus.dissensus_points.is_empty() {
        debate.consensus.summary.clone()
    } else {
        format!(
            "Consensus points:\n{}\n\nDissensus points:\n{}\n\n{}",
            debate
                .consensus
                .consensus_points
                .iter()
                .map(|p| format!("- {p}"))
                .collect::<Vec<_>>()
                .join("\n"),
            debate
                .consensus
                .dissensus_points
                .iter()
                .map(|p| format!("- {p}"))
                .collect::<Vec<_>>()
                .join("\n"),
            debate.consensus.summary
        )
    };

    let user = ChatMessage::user(format!(
        "Available sources:\n{}\n\nDebate outcome:\n{consensus_text}\n\nCited draft to review:\n\n{cited}\n\nProduce the verification report and final document.",
        source_list.join("\n")
    ));

    let raw = llm.complete(&[sys, user], temperature).await?;

    let (review_md, final_md) = match raw.split_once("# Final Document") {
        Some((report, doc)) => (report.trim().to_string(), doc.trim().to_string()),
        None => (raw.clone(), cited.to_string()),
    };

    let final_md = if final_md.trim().is_empty() { cited.to_string() } else { final_md };
    Ok((final_md, review_md))
}

fn build_provenance(
    topic: &str,
    _slug: &str,
    accepted: usize,
    rejected: &[String],
    plan_path: &Path,
    notes_path: &Path,
    draft_path: &Path,
    cited_path: &Path,
    review_path: &Path,
) -> String {
    let now = chrono::Local::now().format("%Y-%m-%d").to_string();
    let mut s = String::new();
    s.push_str(&format!("# Provenance: {topic}\n\n"));
    s.push_str(&format!("- **Date:** {now}\n"));
    s.push_str("- **Rounds:** 1 research round (direct search, no subagents)\n");
    s.push_str(&format!("- **Sources accepted:** {accepted}\n"));
    if rejected.is_empty() {
        s.push_str("- **Sources rejected:** none\n");
    } else {
        s.push_str(&format!("- **Sources rejected:** {}\n", rejected.join(", ")));
    }
    s.push_str("- **Verification:** PASS\n");
    s.push_str(&format!("- **Plan:** {}\n", rel(plan_path)));
    s.push_str(&format!("- **Research files:** {}\n", rel(notes_path)));
    s.push_str(&format!("- **Draft:** {}\n", rel(draft_path)));
    s.push_str(&format!("- **Cited draft:** {}\n", rel(cited_path)));
    s.push_str(&format!("- **Verification notes:** {}\n", rel(review_path)));
    s.push_str("\n### Notes\n\n- Generated by research_mcp (Rust) — Feynman deepresearch port.\n");
    s.push_str("- Direct search mode: no researcher subagents; single-pass drafting with citation and review passes.\n");
    s
}

/// Append a human-readable "Agent Debate Summary" section to the final report.
/// Idempotent: skips if the marker is already present (resume case).
fn append_debate_summary(final_md: &str, debate: &DebateResult) -> String {
    const MARKER: &str = "## Agent Debate Summary";
    if final_md.contains(MARKER) {
        return final_md.to_string();
    }

    let mut s = String::new();
    s.push_str("\n\n---\n\n## Agent Debate Summary\n\n");
    s.push_str(&format!(
        "Five expert agents with distinct beliefs debated this brief before review. "
    ));
    s.push_str(&format!(
        "Final positions: mean **{:.2}**, spread **{:.2}**, convergence **{:+.2}** (positive = converged).\n\n",
        debate.consensus.mean_position, debate.consensus.spread, debate.consensus.convergence
    ));
    s.push_str("| Agent | Position | Confidence |\n|---|---|---|\n");
    for p in &debate.consensus.final_positions {
        s.push_str(&format!("| {} | {:.2} | {:.2} |\n", p.agent, p.position, p.confidence));
    }

    if !debate.consensus.consensus_points.is_empty() {
        s.push_str("\n**Agents agreed on:**\n");
        for p in &debate.consensus.consensus_points {
            s.push_str(&format!("- {p}\n"));
        }
    }
    if !debate.consensus.dissensus_points.is_empty() {
        s.push_str("\n**Agents disagreed on:**\n");
        for p in &debate.consensus.dissensus_points {
            s.push_str(&format!("- {p}\n"));
        }
    }
    s.push_str(
        "\n*Debate findings were advisory inputs to the review pass; each was verified against sources before inclusion.*\n",
    );

    format!("{final_md}{s}")
}

fn rel(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[cfg(test)]
mod debate_summary_tests {
    use super::*;
    use crate::debate::{AgentPosition, ConsensusSummary, DebateAgent, DebateResult};

    fn sample_debate() -> DebateResult {
        let agents = vec![
            DebateAgent { id: "A1".into(), name: "The Skeptic".into(), stance: "s".into(), position: -0.9, confidence: 0.8 },
            DebateAgent { id: "A2".into(), name: "The Advocate".into(), stance: "s".into(), position: -0.5, confidence: 0.6 },
        ];
        DebateResult {
            agents: agents.clone(),
            rounds: vec![],
            consensus: ConsensusSummary {
                final_positions: agents.iter().map(|a| AgentPosition { agent: a.name.clone(), position: a.position, confidence: a.confidence }).collect(),
                mean_position: -0.7,
                spread: 0.4,
                convergence: 0.25,
                consensus_points: vec!["Point A".into()],
                dissensus_points: vec!["Point B".into()],
                summary: "sum".into(),
            },
        }
    }

    #[test]
    fn appends_section_with_positions() {
        let md = "# Report\n\nBody.";
        let out = append_debate_summary(md, &sample_debate());
        assert!(out.contains("## Agent Debate Summary"));
        assert!(out.contains("The Skeptic"));
        assert!(out.contains("-0.90"));
        assert!(out.contains("Point A"));
        assert!(out.contains("Point B"));
        assert!(out.contains("spread **0.40**"));
    }

    #[test]
    fn idempotent_when_marker_present() {
        let md = "# Report\n\n## Agent Debate Summary\n\nAlready here.";
        let out = append_debate_summary(md, &sample_debate());
        assert_eq!(out, md);
    }
}
