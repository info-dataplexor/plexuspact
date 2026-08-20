<!-- Title must follow conventional commits: feat: / fix: / perf: / docs: / test: / chore: -->

## What & why

<!-- One or two sentences. Link the issue: Closes #123 -->

## How

<!-- Anything a reviewer needs to know about the approach. If you made a design
     decision the docs don't cover, it needs an ADR in docs/adr/ — link it here. -->

## Checklist

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --all-targets -- -D warnings` passes
- [ ] `cargo test --workspace` passes; new behavior has tests
- [ ] No `unwrap()`/`expect()` on user-controlled input outside tests
- [ ] Error messages tell the user the file, what was expected, and an example fix
- [ ] Snapshot changes (if any) were accepted via `cargo insta review` and are intentional
- [ ] User-facing changes are noted in `CHANGELOG.md` under **Unreleased**
- [ ] Diff is focused (< ~400 lines where possible); unrelated ideas went to `docs/ideas.md`
