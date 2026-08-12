# research_mcp

Rust deep research engine with multi-agent debate — a Feynman-style deepresearch
pipeline with an SQLite-backed queue, MCP server, and five source types.

## Features

- **7-phase pipeline**: planning → searching → fetching → drafting → citing →
  debating → reviewing → delivering
- **5 source types**: web (DuckDuckGo + Brave fallback), arXiv (HTML abs page,
  immune to export API rate limits), PubMed (E-utilities), Scopus (Elsevier,
  metadata + CrossRef abstract fallback)
- **Multi-agent debate**: 5 agents with distinct beliefs critique the brief's
  content AND references, converge over rounds (spread/convergence measured),
  then answer targeted follow-up interviews on open questions
- **SQLite state machine**: every phase commits to `research.db` — crash
  recovery, resume (`--resume <id>`), audit trail (`research_phase_log`)
- **Resilient worker**: max 1 concurrent run, batch coherence, unlimited retry
  with backoff, auth errors (401/403) fail fast
- **MCP server** (`--mcp`): 7 tools over stdio for OpenCode/Claude/etc.
- **Language-aware**: Hungarian topics produce Hungarian reports and debates
- **CLI**: run research, `--list`, `--status <id>`, `--resume <id>`

## Pipeline

```
topic → planning → searching → fetching → drafting → citing
      → debating (5 agents, belief update, consensus) → reviewing → delivering
```

## Install

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

# Run as MCP server (stdio)
research_mcp --mcp
```

### MCP (OpenCode)

Add to `opencode.jsonc`:

```jsonc
"research_mcp": {
  "type": "local",
  "command": ["/path/to/research_mcp", "--mcp"],
  "enabled": true
}
```

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

## Output

Each run produces (in `out_dir`):
- `<slug>.md` — the research brief with inline citations, Sources, and an
  Agent Debate Summary (positions, consensus, dissensus, interviews)
- `<slug>.provenance.md` — source tracking (date, accepted/rejected, plan paths)
- `.drafts/` — intermediate artifacts (plan, research notes, draft, cited,
  verification)

## License

[MIT](LICENSE.md) (or AGPL-3.0 — see LICENSE).

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
