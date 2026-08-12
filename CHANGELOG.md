# Changelog

All notable changes to this project are documented in this file.
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added
- CHANGELOG.md, CONTRIBUTING.md, LICENSE.md
- GitHub Actions CI (build + test + clippy `-D warnings`)

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
