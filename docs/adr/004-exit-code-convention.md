# ADR-004: Exit-code convention

**Status:** Accepted · **Date:** 2026-07

## Context

The primary consumer of `plexuspact check` is a CI job or shell script, not a human. Scripts need to distinguish at least three situations that demand different responses: the data is bad (block the merge / quarantine the file), the invocation or contract is bad (fix configuration, don't retry), and the tool itself broke (retry / file a bug). Many tools conflate these into a single non-zero code, which forces output parsing.

## Decision

Four stable exit codes, uniform across commands:

| Code | Meaning |
|---|---|
| `0` | Pass — no error-severity failures. |
| `1` | Check failures — at least one error-severity check failed. |
| `2` | Usage or contract error — bad flags, missing/unreadable input, malformed or semantically invalid contract. Nothing was validated. |
| `3` | Internal error — a plexuspact bug or environmental failure (e.g. disk full writing a report). |

Severity interaction: `warn`-severity failures are reported but do not affect the exit code; `--strict` promotes them to code `1`. `plexuspact diff` maps breaking changes to `1` with the same `2`/`3` semantics. Codes are frozen: changing their meaning is a breaking change requiring a major (0.x: minor) version bump and a migration note.

## Consequences

- CI gates are one line; orchestrators can branch (retry on `3`, never on `2`) without parsing output.
- Every failure path in the codebase must be classified into exactly one bucket — e2e tests assert command × exit code exhaustively.
- We give up per-failure-type granularity in the code itself; that detail belongs in the JSON result, not the exit status.
