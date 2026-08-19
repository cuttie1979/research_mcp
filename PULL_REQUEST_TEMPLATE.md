# Pull Request

## Summary

<!-- Describe the change and the problem it solves. Keep it concise.
     Reference issues with #ID where applicable (e.g. Closes #12). -->

Closes #<!-- issue number -->

## Type of change

<!-- Mark the box(es) that apply. -->

- [ ] Bug fix
- [ ] New feature
- [ ] Refactor / internal
- [ ] Documentation / CI / tooling

## How it was verified

<!-- Show how you validated the change. The fast suite MUST stay green.
     Live tests are marked #[ignore] and run separately. -->

- `cargo build` — [ ] pass
- `cargo test` — [ ] pass
- `cargo clippy -- -D warnings` — [ ] pass
- `cargo fmt` — [ ] applied
- Live tests run (if relevant): `cargo test -- --ignored <filter>` — result:

```
<paste relevant output>
```

## Checklist

<!-- Before requesting review, confirm each of these. -->

- [ ] `cargo fmt` applied.
- [ ] `cargo clippy -- -D warnings` passes clean.
- [ ] Unit tests are offline (no live HTTP / API calls).
- [ ] No API keys or secrets added; `config.toml` / `*.db` remain gitignored.
- [ ] New public surface (CLI flag, tool, struct field) is backward compatible
      or the breaking change is justified and documented.
- [ ] CHANGELOG updated under `[Unreleased]` if this is a user-visible change.
- [ ] Issue templates / Code of Conduct honored.

## Additional context

<!-- Anything a reviewer should know: design trade-offs, related work,
     migration concerns, links. -->
