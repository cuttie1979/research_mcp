# Contributing

Thanks for considering a contribution to `research_mcp`.

## Development setup

```bash
cargo build
cargo test
cargo clippy -- -D warnings
```

The fast test suite must stay green before any PR:
- Unit tests are offline (no live API calls).
- Live tests (arXiv, PubMed, Scopus, web search) are marked `#[ignore]` and
  only run with `cargo test -- --ignored` — external services rate-limit
  bursty traffic.

## Conventions

- `cargo fmt` before committing.
- `cargo clippy -- -D warnings` must pass clean.
- Keep unit tests offline: no live HTTP, no SQLite in CI temp paths is fine
  (uses `std::env::temp_dir()`).
- API keys never enter git. `config.toml`, `research.db`, and `*.db` are
  gitignored — put secrets only in `config.toml` or environment variables.

## Code layout

| Path | Purpose |
|---|---|
| `src/main.rs` | CLI entry: research, `--mcp`, `--list`, `--status`, `--resume` |
| `src/workflow.rs` | Phase engine: planning → … → delivering, session persistence |
| `src/debate.rs` | Multi-agent debate + interviews + consensus measurement |
| `src/db.rs` | SQLite state machine, session storage, phase log |
| `src/worker.rs` | Queue worker: retry policy, crash recovery, batch coherence |
| `src/mcp.rs` | MCP server (stdio) exposing the queue as tools |
| `src/arxiv.rs` `src/pubmed.rs` `src/scopus.rs` `src/search.rs` `src/fetch.rs` | Source adapters |
| `src/llm.rs` | OpenCode Go client (OpenAI-compatible), retry + timeout |
| `src/slug.rs` | Topic → slug, URL/ID stripping, collision handling |
| `src/config.rs` | `config.toml` loading (env fallbacks) |
| `src/log.rs` | `log_info!` / `log_warn!` macros (stdout in CLI, stderr in MCP) |

## Adding a source type

1. Add a module (e.g. `src/example.rs`) following the `arXiv`/`PubMed`/
   `Scopus` pattern: a struct with `search()` and/or `by_id()` returning a
   serde-serializable paper struct.
2. Add it to `Gathered` in `src/workflow.rs`, wire the search into `gather()`,
   and convert papers into `EvidenceItem`s in `fetch_sources()`.
3. Add a live test marked `#[ignore]`.

## Phases

The pipeline is a state machine; every phase commits its output to the
SQLite session before moving on. This is what makes crash recovery and
`--resume` work. When adding a phase:
- Add it to `PROGRESS` in `src/workflow.rs`.
- Persist its output in `SessionData` (`src/db.rs`) — add a column + the
  `ALTER TABLE` migration for existing databases.
- Log a phase event via `db.log_phase(...)`.

## License

By contributing you agree your work is licensed under [MIT](LICENSE.md).
