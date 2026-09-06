# Contributing to PlexusPact

Thanks for helping make bad data fail fast. This document covers dev setup, the ground rules every PR is reviewed against, and how to run the test suite.

## Dev setup

1. **Install Rust** via [rustup](https://rustup.rs/). The repo pins the toolchain in `rust-toolchain.toml`; rustup picks it up automatically on first build.
2. **Windows note:** the pinned toolchain is `stable-x86_64-pc-windows-gnu`, deliberately — it ships a self-contained linker, so you do **not** need Visual Studio Build Tools to hack on this repo. CI additionally builds and tests the MSVC target; release binaries for Windows are MSVC. If you already have MSVC installed you can override locally with `rustup override set stable-x86_64-pc-windows-msvc`.
3. Clone and verify:

   ```sh
   git clone https://github.com/info-dataplexor/plexuspact
   cd plexuspact
   cargo test --workspace
   cargo run -p plexuspact-cli -- --help
   ```

4. Useful extras: `cargo install cargo-deny cargo-insta mdbook`.

## Ground rules

These are non-negotiable; reviewers will hold the line.

1. **Stay inside MVP scope.** If a feature idea appears mid-PR, write it in `docs/ideas.md` and move on. The PRD's non-goals are binding.
2. **Every PR** compiles with zero clippy warnings (`cargo clippy --all-targets -- -D warnings`), is `cargo fmt`-clean, adds or updates tests, and stays under ~400 lines of diff where possible. Small PRs get reviewed same-day; big ones rot.
3. **Errors are product.** Any time you write an error message, ask: does this tell the user the file, the line, what was expected, and one example fix? If not, it's not done.
4. **No `unwrap()`/`expect()` on user-controlled input** anywhere outside tests. `clippy::unwrap_used` is deny-level in the workspace lints — don't `#[allow]` it away without a review conversation.
5. **Decisions get ADRs, not silent code.** If you face a choice the docs don't answer, propose a one-page ADR in `docs/adr/` (Context / Decision / Consequences) rather than deciding in code. See existing ADRs for the format.
6. **Respect the layering.** `cli → core → {contract, engine, report}`, `engine → {contract, io}`. The `plexuspact-contract` crate must never depend on Polars (ADR-002).
7. **Conventional commits** (`feat:`, `fix:`, `perf:`, `docs:`, `test:`, `chore:`) — the changelog is generated from them. Trunk-based flow: short-lived branch → PR → squash merge to `main`; `main` is always releasable.

## Test strategy

| Layer | Tool | What it covers |
|---|---|---|
| Unit | `cargo test` | Every check against hand-built tiny DataFrames (pass, fail, null handling, empty input) |
| Snapshot | [`insta`](https://insta.rs/) | Human output, JSON output, error messages, HTML golden files — locks the output format |
| CLI e2e | [`assert_cmd`](https://docs.rs/assert_cmd) + `fixtures/` | Every command × every exit code |
| Property | [`proptest`](https://docs.rs/proptest) | Contract round-trip (serialize → parse → equal); diff invariants |
| Fuzz | `cargo-fuzz` | Contract YAML parser, reader options (nightly CI job) |
| Perf | `criterion` | Fail on >15% regression vs committed baselines |

### Running tests

```sh
cargo test --workspace                 # unit + integration + e2e
cargo insta review                     # review changed snapshots (after cargo test)
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
cargo deny check                       # licenses + advisories
```

If a snapshot test fails because you *intentionally* changed output, run `cargo insta review` and accept — the diff then shows up in your PR for review. Never hand-edit `.snap` files.

Coverage target: ≥80% on the `plexuspact-contract` and `plexuspact-engine` crates (`cargo llvm-cov`), reported informationally in PRs, not enforced as a hard gate.

## Docs

The book lives in `docs/` (mdBook): `mdbook serve docs` and open http://localhost:3000. Docs PRs follow the same conventions (conventional commits, small diffs).

## Questions

Open a [discussion](https://github.com/info-dataplexor/plexuspact/discussions) or an issue. For security reports, do **not** open an issue — see [SECURITY.md](SECURITY.md).
