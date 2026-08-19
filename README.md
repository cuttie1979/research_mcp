# research_mcp

Rust deep research engine with multi-agent debate — a Feynman-style deepresearch
pipeline with an SQLite-backed queue, MCP server, and five source types.

## Features

- **8-phase pipeline**: planning → searching → fetching → drafting → citing →
  debating → reviewing → delivering
- **5 source types**: web (DuckDuckGo → Bing → Brave, three keyless backends
  with bot-gating detection + retry), arXiv (HTML abs page, immune to export
  API rate limits), PubMed (E-utilities), Scopus (Elsevier, metadata +
  CrossRef abstract fallback)
- **Multi-agent debate**: 5 agents with distinct beliefs critique the brief's
  content AND references, converge over rounds (spread/convergence measured),
  then answer targeted follow-up interviews on open questions
- **SQLite state machine**: every phase commits to `research.db` — crash
  recovery, resume (`--resume <id>`), audit trail (`research_phase_log`)
- **Resilient worker**: max 1 concurrent run, batch coherence, unlimited retry
  with backoff, auth errors (401/403) fail fast
- **MCP server** (`--mcp`): 8 tools over stdio for OpenCode/Claude/etc.
- **Language-aware**: Hungarian topics produce Hungarian reports and debates
- **CLI**: run research, `--list`, `--status <id>`, `--resume <id>`

## Pipeline

```
topic → planning → searching → fetching → drafting → citing
      → debating (5 agents, belief update, consensus) → reviewing → delivering
```

## Install

### Download (prebuilt binaries)

Grab the package for your platform from the
[latest release](https://github.com/cuttie1979/research_mcp/releases/latest):

| Platform | Asset | Arch |
|---|---|---|
| Linux | `research_mcp-<ver>-linux-x86_64.tar.gz` | x86-64 |
| macOS | `research_mcp-<ver>-macos-arm64.tar.gz` | Apple Silicon (M1+) |

```bash
# Linux / macOS
tar xzf research_mcp-<ver>-<platform>.tar.gz
cd research_mcp-<ver>
cp config.example.toml config.toml   # fill in your API keys
./research_mcp "your research topic"
```

No Rust toolchain needed — the binaries are self-contained (only system libc).

### From source

```bash
cargo build --release
cp config.example.toml config.toml   # fill in your API keys
```

## Configuration (`config.toml`)

| Key | Required | Description |
|---|---|---|
| `api_key` | yes | OpenCode Go key (https://opencode.ai/auth) |
| `elsevier_api_key` | no | Elsevier/Scopus key (https://dev.elsevier.com/apikey/manage) |
| `model` | no | OpenCode Go model (default `deepseek-v4-flash`) |
| `out_dir` | no | Report output dir (default `/home/user/AIDocumentStore/raw/research`) |
| `db_path` | no | SQLite path (default `./research.db`) |
| `llm_timeout_secs` | no | LLM request timeout (default 300) |
| `temperature` | no | Drafting temperature (default 0.3) |
| `max_sources` | no | Hard cap on full-text downloads per run (default 8) |

### Adaptive coverage (LLM-driven)

The research planner is an LLM that sizes the search based on topic scale
(Feynman-style "Scale decision"). Two extra plan fields are emitted at planning
time and are not in `config.toml`:

| Plan field | Meaning | Narrow explainer | Broad survey | News / current-events |
|---|---|---|---|---|
| `web_per_query` | max web results to collect per query | ~6 | 10–15 | 15–20 |
| `download_budget` | desired full-text downloads | ~6 | 10–15 | up to 20 |

`download_budget` is combined with the config `max_sources` via a **double cap**:
the actual download count is `min(max_sources, download_budget)`, so you keep a
hard upper bound on cost/context while coverage expands for news-heavy topics.
Plans that omit either field fall back to defaults (`6` / `8`), keeping old
plans and saved sessions compatible.

Web search resilience: the keyless backend chain is DuckDuckGo → Bing → Brave,
each with bot-gating detection (real challenge markers, checked after parsing
so genuine results are never discarded) and the whole sweep retries before
giving up. When all three are empty/gated, a "0 web results" warning is
surfaced in the run log instead of silently producing an empty brief.

## Usage

### CLI

```bash
# Run a research (synchronous)
research_mcp "What are the tradeoffs of the ketogenic diet?"

# Query the queue
research_mcp --list
research_mcp --list --list-status complete
research_mcp --status <run_id>

# Resume an interrupted run (crash recovery)
research_mcp --resume <run_id>

# Re-run a job with the same topic and fresh session (test fixes)
research_mcp --rerun <run_id>

# Raise the hard cap on full-text downloads (news/current-events topics)
research_mcp --max-sources 20 "your topic"

# Run as MCP server (stdio)
research_mcp --mcp
```

### MCP (OpenCode)

See [docs/opencode.md](docs/opencode.md) for the full setup guide (config,
troubleshooting, tool usage). Short version:

```jsonc
"research_mcp": {
  "type": "local",
  "command": ["/absolute/path/to/research_mcp", "--mcp"],
  "enabled": true
}
```

A ready-made OpenCode skill ships in `.opencode/skills/research-deepresearch/`
— it teaches the agent the recommended submit → poll → read flow.

### MCP tools

| Tool | Purpose |
|---|---|
| `research_submit` | Enqueue a topic → `run_id` |
| `research_batch_submit` | Enqueue multiple topics → `batch_id` + `run_ids` |
| `research_list` | List runs (status filter, pagination) |
| `research_status` | Full state: phase, progress, error, phase log |
| `research_cancel` | Cancel a run or an entire batch |
| `research_resume` | Re-queue failed/blocked/cancelled runs |
| `research_output` | Read generated `.md` + `.provenance.md` |
| `research_rerun` | Re-run a job: new run, same topic, fresh session |

## Output

Each run produces (in `out_dir`):
- `<slug>.md` — the research brief with inline citations, Sources, and an
  Agent Debate Summary (positions, consensus, dissensus, interviews)
- `<slug>.provenance.md` — source tracking (date, accepted/rejected, plan paths)
- `.drafts/` — intermediate artifacts (plan, research notes, draft, cited,
  verification)

## License

[MIT](LICENSE.md).

Inspired by:
- [companion-inc/feynman](https://github.com/companion-inc/feynman) (MIT) —
  deepresearch workflow pattern
- [MiroShark/MiroShark](https://github.com/MiroShark/MiroShark) (AGPL-3.0) —
  multi-agent belief state + debate pattern (reimplemented, LLM-based)

## Disclaimer

**Use at your own risk.** The software is provided "as is", without warranty
of any kind (see [LICENSE.md](LICENSE.md)). Generated research briefs are
AI-assisted and may contain errors, omissions, or hallucinations. Always
verify claims against the cited primary sources. This tool does not provide
professional, medical, financial, or legal advice.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Changes are tracked in
[CHANGELOG.md](CHANGELOG.md).
