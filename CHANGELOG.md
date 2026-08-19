# Changelog

All notable changes to this project are documented in this file.
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

## [0.8.4] — 2026-08-19

### Fixed
- **Web search resilience (zero web results for news/current-events):** the
  keyless search previously had only DuckDuckGo + Brave; DuckDuckGo is now
  frequently anti-bot-gated (`anomaly`/`challenge-form`) and Brave is a
  fragile SvelteKit SPA with no bot detection, so a transient challenge on
  both produced a silent `Ok(vec![])` and an empty/thin brief. Now:
  - Added a **third keyless backend: Bing HTML** (highest reliability, real
    URLs decoded from the `ck/a` redirect's base64url `u=` param).
  - **Explicit bot-gating detection** for Bing/Brave (real challenge markers
    only, checked *after* parsing so real results are never thrown away).
  - **Retry/backoff**: the whole DDG→Bing→Brave sweep is retried after
    transient gating before giving up.
  - **Warning surfaced**: "0 web results — all backends gated/empty" is logged
    and the run's searching phase no longer silently proceeds as if the web
    worked.
  Verified live: `search_live_energy_news` (energy-market news) returns 6+
    results; `bing_live` returns 8 with decoded URLs.

### Added
- **Adaptive web coverage (LLM-driven)**: the research planner now emits a
  `web_per_query` value based on topic scale/urgency (Feynman-style Scale
  decision). Narrow explainers → ~6 results/query; broad surveys → 10-15;
  fast-moving/current-events or news-heavy topics → 15-20. The gather phase
  uses this per-query result count instead of a hardcoded 6, so news queries
  pull in more sources when the topic warrants it. Backward compatible: plans
  without `web_per_query` default to 6.
- **Adaptive download budget (LLM-driven, double cap)**: the planner also emits
  `download_budget`; the actual full-text download count is
  `min(config.max_sources, download_budget)` — the user keeps a hard cost cap
  while coverage scales up for news/current-events. Narrow → ~6, surveys →
  10-15, news → up to 20. Backward compatible: missing value defaults to 8.
- **Community/contribution scaffolding**: `CODE_OF_CONDUCT.md`,
  `PULL_REQUEST_TEMPLATE.md`, and `.github/ISSUE_TEMPLATE/`
  (`bug_report.md`, `feature_request.md`).

## [0.8.3] — 2026-08-17

### Fixed
- **Drafting excerpt (Bug 2603 root cause):** `build_evidence_block` capped every
  source at 3000 chars before the drafting LLM, so even when the fetch layer had
  the full arXiv HTML the brief still cited only abstract + Introduction.
  arXiv full-text evidence (>20k chars, marked `--- FULL TEXT ---`) now gets up
  to a 60k excerpt; generic/abstract sources stay at 3000.
- arXiv full-text cap raised 100k → 250k chars so very large LaTeX-rendered
  papers (e.g. 2603.10145, 83k cleaned chars) are fully captured. The web
  fetch path already retrieves `arxiv.org/html/{id}` full text for by_id
  papers (since 0.8.2); this removes the ceiling for the largest papers.
## [0.8.2] — 2026-08-13

### Fixed
- arXiv full-text retrieval: `by_id` papers now fetch the full HTML page
  (`arxiv.org/html/{id}`, up to 100k chars) so methodology, results, and
  limitations reach the LLM — previously only abstract + intro reached it.
  Web-fetch cap is now per-domain (arxiv.org/html → 100k, generic → 12k) and
  truncation appends a `[...TRUNCATED...]` marker instead of cutting silently.

## [0.8.1] — 2026-08-13

### Fixed
- arXiv ID extraction: regex now case-insensitive, accepts `arXiv:` (capital A),
  `arXiv :` (space before colon), bare `arxiv`, and URL forms. Previously
  `arXiv:NNNN.NNNNN` topics skipped the by-id fetch and produced incorrect
  "paper not verified" reports (issue from 2026-08-13 batch).

### Added
- `research_rerun` MCP tool + CLI `--rerun <run_id>`: creates a NEW run with
  the same topic and fresh session state — for testing fixes on previously
  broken jobs (unlike `--resume`, which continues the saved session)
- SECURITY.md — vulnerability reporting policy
- docs/opencode.md — full OpenCode setup guide
- `.opencode/skills/research-deepresearch/` — agent skill for the MCP tools
- Prebuilt binary assets in releases: Linux x86-64, macOS arm64
- README: Download section with per-platform install instructions

## [0.8.0] — 2026-08-12

### Added
- Post-debate interviews: moderator asks the two most-confident agents
  targeted follow-ups on open (dissensus) points; answers rendered in the
  report's debate section
- Language-aware debate: Hungarian topics produce Hungarian arguments,
  belief-update reasons, and moderator points

### Changed
- Interviews capped at 2 agents / 150 words for latency

## [0.7.0] — 2026-08-12

### Added
- Slug collision handling: duplicate slugs get `-2`, `-3` suffixes so report
  files never overwrite each other
- LLM request timeout (`llm_timeout_secs`, default 300s)
- Batch coherence: worker prefers the next run of an already-running batch
- Context budget: web fetching stops when accumulated evidence reaches the cap

### Fixed
- Review stalls on large source sets (timeout + oversized evidence context)

## [0.6.0] — 2026-08-12

### Added
- Multi-agent debate phase (before review): 5 agents with distinct beliefs
  critique the brief's content AND references, converge over rounds
- Pipeline is now 8 phases: planning → searching → fetching → drafting →
  citing → debating → reviewing → delivering
- Consensus measurement: mean position, spread, convergence
- Debate consensus/dissensus points fed into the review pass with
  deterministic weighting
- `## Agent Debate Summary` section in the final report
- `debate_text` column in SQLite session storage + ALTER TABLE migration
- CLI `--resume <run_id>` to continue interrupted runs

## [0.5.0] — 2026-08-12

### Added
- Scopus integration (Elsevier API): search metadata + CrossRef abstract fallback
- `elsevier_api_key` config option

## [0.4.0] — 2026-08-12

### Added
- SQLite persistence: `research_runs` state machine, `research_session`
  phase artifacts, `research_phase_log` audit trail
- Background worker: max 1 concurrent run, crash recovery (`running` → `queued`),
  unlimited retry with backoff, auth errors (401/403) fail fast
- MCP server (rmcp 3.x) over stdio: 7 tools (submit, batch_submit, list,
  status, cancel, resume, output)
- Logging routed to stderr in MCP mode (JSON-RPC stream stays clean)
- Config discovered next to the executable (MCP servers run from any CWD)
- CLI query commands: `--list`, `--status <run_id>`

### Fixed
- arXiv export API rate limits bypassed via HTML abs-page route
- MCP `RunningService` kept alive via `waiting()` (drop killed the loop)

## [0.3.0] — 2026-08-12

### Added
- PubMed integration (NCBI E-utilities): search + fetch-by-ID, PMID detection
  in topics

## [0.2.0] — 2026-08-12

### Added
- arXiv integration: Atom API search + fetch-by-ID, arXiv-ID detection in topics
- Brave HTML search as fallback for DuckDuckGo (own cookie jar + warmup)
- LLM retry with exponential backoff on transient errors (5xx, 429, network)
- `config.toml` support with example template (API key kept out of git)
- Slug generation strips URLs and paper IDs

## [0.1.0] — 2026-08-12

### Added
- Initial release: Feynman deepresearch port in Rust
- 7-phase pipeline: plan → search → fetch → draft → cite → review → deliver
- OpenCode Go LLM integration (deepseek-v4-flash default)
- DuckDuckGo HTML search backend (no key required)
- URL fetch with HTML → text extraction
- Output: `<slug>.md` + `<slug>.provenance.md`
