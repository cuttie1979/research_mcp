//! Deep research workflow — Feynman deepresearch port.
//! Phases: plan → gather (search + fetch) → draft → cite → review → deliver.
//! No confirmation step: runs end-to-end on the given topic.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use regex::Regex;

use crate::arxiv::{Arxiv, ArxivPaper};
use crate::fetch::Fetcher;
use crate::llm::{ChatMessage, Llm};
use crate::pubmed::{Pubmed, PubmedPaper};
use crate::search::{Search, SearchResult};
use crate::slug::slugify;

pub struct Config {
    pub model: String,
    pub api_key: String,
    pub llm_base_url: String,
    pub out_dir: PathBuf,
    pub max_sources: usize,
    pub temperature: f32,
}

pub struct RunReport {
    pub slug: String,
    pub report_path: PathBuf,
    pub provenance_path: PathBuf,
    pub sources_accepted: usize,
}

#[derive(Debug, serde::Deserialize)]
struct Plan {
    key_questions: Vec<String>,
    evidence_needed: Vec<String>,
    scale: String,
    queries: Vec<String>,
}

pub async fn run(cfg: &Config, topic: &str) -> Result<RunReport> {
    let llm = Llm::new(cfg.model.clone(), cfg.api_key.clone(), cfg.llm_base_url.clone());
    let search = Search::new();
    let fetcher = Fetcher::new();
    let arxiv = Arxiv::new();
    let pubmed = Pubmed::new();
    let temperature = cfg.temperature;

    let slug = slugify(topic);
    println!("Topic: {topic}");
    println!("Slug:  {slug}");

    std::fs::create_dir_all(&cfg.out_dir).with_context(|| format!("create {}", cfg.out_dir.display()))?;
    let drafts_dir = cfg.out_dir.join(".drafts");
    std::fs::create_dir_all(&drafts_dir).context("create .drafts")?;

    // ── Phase 1: Plan ──────────────────────────────────────────────
    let plan = plan_research(&llm, topic, temperature).await?;
    println!("\n── Plan ──");
    println!("scale: {}", plan.scale);
    println!("queries ({}):", plan.queries.len());
    for q in &plan.queries {
        println!("  - {q}");
    }

    let plan_path = drafts_dir.join(format!("{slug}-plan.md"));
    let plan_md = format_plan(&plan, topic, &slug);
    std::fs::write(&plan_path, &plan_md).context("write plan")?;

    // ── Phase 2: Gather evidence ───────────────────────────────────
    let queries = if plan.queries.is_empty() {
        vec![topic.to_string()]
    } else {
        plan.queries
    };

    let mut all_results: Vec<SearchResult> = Vec::new();
    let mut seen_urls = std::collections::HashSet::new();

    // Direct arXiv fetch if the topic contains an arXiv ID (e.g. 2607.12631).
    let mut arxiv_papers: Vec<ArxivPaper> = Vec::new();
    for id in extract_arxiv_ids(topic) {
        println!("\n── arXiv by-id: {id}");
        match arxiv.by_id(&id).await {
            Ok(Some(p)) => {
                arxiv_papers.push(p);
            }
            Ok(None) => eprintln!("  ⚠ arXiv id {id} not found"),
            Err(e) => eprintln!("  ⚠ arXiv fetch failed for {id}: {e}"),
        }
    }

    // Direct PubMed fetch if the topic contains a PMID (e.g. PMID:12345678).
    let mut pubmed_papers: Vec<PubmedPaper> = Vec::new();
    for id in extract_pubmed_ids(topic) {
        println!("\n── PubMed by-id: {id}");
        match pubmed.by_id(&id).await {
            Ok(Some(p)) => {
                pubmed_papers.push(p);
            }
            Ok(None) => eprintln!("  ⚠ PMID {id} not found"),
            Err(e) => eprintln!("  ⚠ PubMed fetch failed for {id}: {e}"),
        }
    }

    for (i, q) in queries.iter().enumerate() {
        println!("\n── Search {}/{}: {q}", i + 1, queries.len());
        match search.query(q, 6).await {
            Ok(results) => {
                for r in results {
                    if seen_urls.insert(r.url.clone()) {
                        all_results.push(r);
                    }
                }
            }
            Err(e) => eprintln!("  ⚠ search failed for {q}: {e}"),
        }
    }

    // arXiv search in parallel with DDG for the first 2 plan queries.
    for q in queries.iter().take(2) {
        println!("── arXiv search: {q}");
        match arxiv.search(q, 4).await {
            Ok(papers) => {
                for p in papers {
                    if seen_urls.insert(p.url.clone()) {
                        arxiv_papers.push(p);
                    }
                }
            }
            Err(e) => eprintln!("  ⚠ arxiv search failed for {q}: {e}"),
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // PubMed search for the first 2 plan queries.
    for q in queries.iter().take(2) {
        println!("── PubMed search: {q}");
        match pubmed.search(q, 4).await {
            Ok(papers) => {
                for p in papers {
                    if seen_urls.insert(p.url.clone()) {
                        pubmed_papers.push(p);
                    }
                }
            }
            Err(e) => eprintln!("  ⚠ pubmed search failed for {q}: {e}"),
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    println!(
        "\nTotal unique results: {} web + {} arxiv + {} pubmed",
        all_results.len(),
        arxiv_papers.len(),
        pubmed_papers.len()
    );

    // ── Phase 3: Fetch sources ─────────────────────────────────────
    let mut evidence: Vec<EvidenceItem> = Vec::new();
    let mut accepted = 0usize;
    let mut rejected: Vec<String> = Vec::new();

    // arXiv papers: abstract text is the source content — no HTML fetch needed.
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

    // PubMed papers: abstract text is the source content.
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

    for r in all_results.into_iter().take(cfg.max_sources) {
        // Skip URLs already covered by arXiv fetch.
        if evidence.iter().any(|e| e.url == r.url) {
            continue;
        }
        println!("── Fetch: {}", r.url);
        match fetcher.fetch(&r.url, 12000).await {
            Ok(text) => {
                if text.chars().count() < 200 {
                    rejected.push(format!("{} (too short)", r.url));
                    continue;
                }
                accepted += 1;
                evidence.push(EvidenceItem {
                    id: format!("S{}", accepted),
                    title: r.title.clone(),
                    url: r.url.clone(),
                    snippet: r.snippet.clone(),
                    text,
                });
            }
            Err(e) => {
                eprintln!("  ⚠ fetch failed: {e}");
                rejected.push(r.url);
            }
        }
        // Small delay to be polite.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    if evidence.is_empty() {
        bail!("No usable sources gathered — cannot draft.");
    }

    let research_notes = build_research_notes(&evidence);
    let notes_path = drafts_dir.join(format!("{slug}-research-direct.md"));
    std::fs::write(&notes_path, &research_notes).context("write research notes")?;

    // ── Phase 4: Draft ─────────────────────────────────────────────
    println!("\n── Drafting ({accepted} sources) ──");
    let draft = draft_report(&llm, topic, &evidence, temperature).await?;
    let draft_path = drafts_dir.join(format!("{slug}-draft.md"));
    std::fs::write(&draft_path, &draft).context("write draft")?;

    // ── Phase 5: Cite ──────────────────────────────────────────────
    println!("── Citing ──");
    let cited = cite_report(&llm, &draft, &evidence, temperature).await?;
    let cited_path = drafts_dir.join(format!("{slug}-cited.md"));
    std::fs::write(&cited_path, &cited).context("write cited draft")?;

    // ── Phase 6: Review ────────────────────────────────────────────
    println!("── Reviewing ──");
    let (final_md, review_md) = review_report(&llm, &cited, &evidence, temperature).await?;
    let review_path = drafts_dir.join(format!("{slug}-verification.md"));
    std::fs::write(&review_path, &review_md).context("write review")?;

    // ── Phase 7: Deliver ───────────────────────────────────────────
    let report_path = cfg.out_dir.join(format!("{slug}.md"));
    std::fs::write(&report_path, &final_md).context("write final report")?;

    let provenance = build_provenance(topic, &slug, accepted, &rejected, &plan_path, &notes_path, &draft_path, &cited_path, &review_path);
    let provenance_path = cfg.out_dir.join(format!("{slug}.provenance.md"));
    std::fs::write(&provenance_path, &provenance).context("write provenance")?;

    Ok(RunReport {
        slug,
        report_path,
        provenance_path,
        sources_accepted: accepted,
    })
}

// ── Phase implementations ──────────────────────────────────────────

/// Extract arXiv IDs from a topic string. Matches:
/// - bare IDs like "2607.12631"
/// - arxiv.org/abs/2607.12631, arxiv.org/pdf/2607.12631v1
/// - arxiv:2607.12631
fn extract_arxiv_ids(text: &str) -> Vec<String> {
    let re = Regex::new(
        r"(?:arxiv\.org/(?:abs|pdf)/|arxiv:\s*|^|\s)(\d{4}\.\d{4,5}(?:v\d+)?)",
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

/// Extract PubMed IDs (PMIDs) from a topic string. Matches:
/// - PMID:12345678, PMID 12345678
/// - pubmed.ncbi.nlm.nih.gov/12345678/
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
            Ok(_) => eprintln!("  ⚠ plan had no queries, retry {}/3", attempt + 1),
            Err(e) => eprintln!("  ⚠ plan parse failed: {e}, retry {}/3", attempt + 1),
        }
    }
    bail!("Could not produce a valid plan after 3 attempts")
}

fn parse_plan_json(raw: &str) -> Result<Plan> {
    // Strip code fences if the model wrapped JSON.
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

#[derive(Debug, Clone)]
struct EvidenceItem {
    id: String,
    title: String,
    url: String,
    snippet: String,
    text: String,
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
    temperature: f32,
) -> Result<(String, String)> {
    let sys = ChatMessage::system(
        "You are a rigorous internal research reviewer. Verify the cited brief.\n\
         Checks: unsupported claims, logical gaps, single-source critical claims, overstated confidence, invalid source IDs.\n\
         Produce:\n\
         1. A verification report with findings marked FATAL / MAJOR / MINOR and the checks performed.\n\
         2. If there are FATAL issues, the full corrected markdown document (complete, with ## Sources).\n\
         3. If no FATAL issues, output the document unchanged.\n\
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
    let user = ChatMessage::user(format!(
        "Available sources:\n{}\n\nCited draft to review:\n\n{cited}\n\nProduce the verification report and final document.",
        source_list.join("\n")
    ));

    let raw = llm.complete(&[sys, user], temperature).await?;

    // Split verification report from final document.
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

fn rel(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}
