---
name: research-deepresearch
description: Run deep research via the research_mcp MCP server — submit a topic, poll the queue, read the generated brief. Use when the user asks for deep research, a comprehensive analysis, an in-depth report, a multi-source investigation, or a research brief on any topic.
---

# Deep Research via research_mcp

The `research_mcp` MCP server runs a Feynman-style deep research pipeline:
5 source types (web, arXiv, PubMed, Scopus, CrossRef), a multi-agent debate
phase, and a cited markdown brief with provenance.

## Tools

| Tool | Purpose |
|---|---|
| `research_submit` | Enqueue a topic → `run_id` |
| `research_batch_submit` | Enqueue multiple topics → `batch_id` + `run_ids` |
| `research_list` | List runs (status filter, pagination) |
| `research_status` | Phase, progress %, error, phase log |
| `research_cancel` | Cancel a run or batch |
| `research_resume` | Re-queue failed/blocked/cancelled runs |
| `research_output` | Read generated `.md` + `.provenance.md` |

## Workflow

### 1. Submit

Call `research_submit` with the user's topic (1-2 sentences). Capture the
`run_id` from the response. If the user gave multiple topics, use
`research_batch_submit` once instead.

### 2. Poll

Research takes minutes (the debate phase adds ~5-10 min on large source sets).
Poll `research_status(run_id)` every ~30-60 seconds:

- `status=queued` → waiting for the worker (max 1 concurrent run).
- `status=running` → check `phase`: planning → searching → fetching → drafting
  → citing → debating → reviewing → delivering.
- `status=complete` → done, `report_path` + `provenance_path` set.
- `status=failed` → read `error`; fix the cause (e.g. API key) then
  `research_resume(run_id)`.
- `status=blocked` / `cancelled` → `research_resume(run_id)` to retry.

### 3. Read and report

On `complete`, call `research_output(run_id)` and read the `report` field.
Summarize the brief for the user:

- Executive summary + key findings (with `[Sn]` citation markers).
- The "Agent Debate Summary" section shows where the debate agents agreed
  (consensus) and disagreed (dissensus) — highlight contested claims.
- Point to the `provenance` for source tracking.

### 4. Handle failures

- `run not found` → verify the `run_id` (use `research_list` to find it).
- Auth errors → the run is marked `failed` with the API error; surface it to
  the user, don't retry blindly.
- If a run was interrupted (crash/timeout), `research_resume` continues from
  the saved session — phases already completed are not redone.

## Output format

Reports contain:

- **# Title** + Executive Summary
- Findings organized by theme, inline `[Sn]` citations
- **Open Questions**
- **Sources** (full bibliography with URLs)
- **Agent Debate Summary** (positions, consensus, dissensus, interviews)

Always caveat: AI-generated briefs may contain errors — verify critical claims
against the cited primary sources.
