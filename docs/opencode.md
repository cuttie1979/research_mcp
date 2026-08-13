# OpenCode Integration

How to set up `research_mcp` as an MCP server in [OpenCode](https://opencode.ai)
so the agent can run deep research directly from chat.

## 1. Build the binary

```bash
cargo build --release
```

The binary lands at `target/release/research_mcp`. Use the absolute path in the
config below.

## 2. Configure API keys

Copy the example config next to the binary (or in the project dir):

```bash
cp config.example.toml config.toml
```

Required: the OpenCode Go API key (used for the LLM):

```toml
api_key = "sk-..."          # https://opencode.ai/auth
```

Optional: the Elsevier key enables Scopus search:

```toml
elsevier_api_key = "..."    # https://dev.elsevier.com/apikey/manage
```

`config.toml` is gitignored. The server also accepts `OPENCODE_GO_API_KEY`
and `ELSEVIER_API_KEY` environment variables.

## 3. Register the MCP server

OpenCode reads MCP servers from `opencode.jsonc` (global:
`~/.config/opencode/opencode.jsonc`, or a project-local `opencode.jsonc`).

Add under the `mcp` key:

```jsonc
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "research_mcp": {
      "type": "local",
      "command": ["/absolute/path/to/research_mcp", "--mcp"],
      "enabled": true,
      "timeout": 60000
    }
  }
}
```

- `command` must point at the built binary (absolute path).
- `timeout` is the tool-list fetch timeout; 60s is safe.
- The server discovers `config.toml` next to the executable, so it works from
  any working directory OpenCode launches it from.

## 4. Verify the connection

Restart OpenCode (or run `/mcp` to check), then ask:

```
use the research_mcp tool to research "benefits of intermittent fasting"
```

You should see the tool list (8 tools: `research_submit`, `research_batch_submit`,
`research_list`, `research_status`, `research_cancel`, `research_resume`,
`research_output`, `research_rerun`).

## 5. Using the tools

| Step | Tool call | What happens |
|---|---|---|
| Start research | `research_submit(topic)` | Enqueues, returns `run_id` |
| Watch progress | `research_status(run_id)` | Phase, progress %, phase log |
| Batch | `research_batch_submit(topics)` | Multiple runs, one `batch_id` |
| List | `research_list(status?)` | Filterable run history |
| Cancel | `research_cancel(run_id \| batch_id)` | Stops queued/running work |
| Resume | `research_resume(run_id \| batch_id)` | Re-queues failed/cancelled |
| Read output | `research_output(run_id)` | Returns `.md` + `.provenance.md` |
| Re-run | `research_rerun(run_id)` | New run, same topic, fresh session (testing fixes) |

Recommended agent flow:

1. `research_submit` the topic → capture `run_id`.
2. Poll `research_status` every ~30-60s (research takes minutes; the debate
   phase adds ~5-10 min on large source sets).
3. On `complete`, call `research_output` and summarize the report for the user.
4. If a run ends `failed`/`blocked`, check `research_status` for `error`, fix
   the cause (e.g. API key), then `research_resume`.

## 6. Where output lands

Reports are written to `out_dir` (default
`/home/user/AIDocumentStore/raw/research`):

```
<slug>.md                  # the research brief (+ Agent Debate Summary)
<slug>.provenance.md       # source tracking
.drafts/                   # intermediate artifacts (plan, notes, draft, ...)
```

## 7. Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `API key missing` | `config.toml` not found / empty | Set `api_key` or `OPENCODE_GO_API_KEY` |
| `LLM API error 401` | Invalid OpenCode Go key | Regenerate at opencode.ai/auth |
| `run stuck in queued` | Worker busy with another run | Wait; worker is max 1 concurrent |
| `run failed (auth)` | `elsevier_api_key` invalid | Check Scopus key; run works without it |
| Tools not listed | Binary path wrong | Check `command` path, restart OpenCode |
| Slow runs | Debate phase (14 LLM calls) | Normal; use MCP (no CLI timeout) |
| Interrupted run | CLI timeout / crash | `research_resume(run_id)` — resumes from saved session |

## Skill

A ready-made OpenCode skill (`research-deepresearch`) ships with this repo —
see `.opencode/skills/research-deepresearch/SKILL.md`. It teaches the agent the
recommended tool flow, polling, and output handling.
