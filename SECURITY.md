# Security Policy

## Reporting a vulnerability

If you find a security vulnerability in `research_mcp`, please report it
privately — do **not** open a public issue.

- **Email:** open a private vulnerability report via
  GitHub → **Security** → **Report a vulnerability**
- **Response time:** within 5 business days you'll receive an
  acknowledgment; a fix and disclosure timeline will follow.

## What to include

- The affected version / commit
- A minimal reproduction (steps or a small script)
- Impact description (what an attacker could do)

## Scope

- The Rust codebase (`src/`)
- The MCP server (`--mcp`)
- Configuration handling (`config.toml`, environment variables)

## Out of scope

- API keys or credentials you stored yourself (rotate them, don't report)
- Issues in third-party dependencies — report upstream
- LLM output quality / hallucination — not a security issue

## Note on API keys

Never commit API keys. `config.toml`, `research.db`, and `*.db` are
gitignored for this reason. If you believe a key leaked, rotate it
immediately and report the leak.
