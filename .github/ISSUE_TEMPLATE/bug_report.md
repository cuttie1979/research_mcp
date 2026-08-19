---
name: Bug report
about: Report a bug so we can fix it
title: "[bug] "
labels: bug
assignees: ""
---

## Describe the bug

A clear and concise description of what the bug is.

## To reproduce

Steps to reproduce the behavior:

```txt
1. Research topic / command used, e.g.:
   research_mcp "The latest news on the energy market over the past one week"
2. Expected: web results appear
3. Actual: zero web results
```

For MCP usage, include the run id if available:

```
Run ID: <id>      (or paste the `research_status <id>` output)
```

## Observed log

Paste the phase log or the relevant stdout/stderr lines, especially any
`⚠` warnings (e.g. "0 web results — all backends gated/empty").

```txt
```

## Expected behavior

What you expected to happen.

## Actual behavior

What actually happened, including any error message.

## Environment

- OS: (e.g. Linux x86_64, macOS arm64)
- Version / build: (e.g. `research_mcp 0.8.4`, or the commit hash)
- Source adapter(s) involved, if known: (web / arXiv / PubMed / Scopus / debate)

## Additional context

- Config relevant to the issue (redact API keys).
- Whether the run was via MCP (`--mcp`) or CLI.
- Any workaround you tried.
