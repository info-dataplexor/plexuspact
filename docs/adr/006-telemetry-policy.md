# ADR-006: Telemetry policy — opt-in only, none in the MVP

**Status:** Accepted · **Date:** 2026-07 · **Amended by:** [ADR-013](013-cloud-reporting-boundary.md) (§4 only)

## Context

Adoption metrics (weekly active CLI users) are a stage-gate input for the business, which argues for telemetry. But the product's core audience runs it on sensitive data inside locked-down CI, and its core promise is a deterministic, network-silent check path. Developer-tool telemetry controversies show that even anonymous, opt-out telemetry burns trust that a data-security-adjacent tool cannot afford. There is also an unresolved vendor question (self-hosted PostHog vs nothing at launch).

## Decision

1. **The MVP ships with no telemetry code at all.** Not disabled — absent. No usage data is collected or transmitted by any 0.1.x release.
2. If telemetry is ever introduced, it is **opt-in only** (explicit user action to enable), limited to coarse usage events (command, version, OS, duration bucket, exit code) — never dataset contents, column names, file paths, or contract contents.
3. **Kill switches are contractual:** `PLEXUSPACT_NO_TELEMETRY` disables it unconditionally regardless of config, and `--offline` disables *all* network activity. Both exist and are documented from v0.1, ahead of any implementation.
4. **The check path never carries telemetry.** Anything network-shaped stays outside `check`'s code path (see doc/security threat model), so validation remains provably network-free.
   > **Amended by [ADR-013](013-cloud-reporting-boundary.md).** `check` now reports its result to PlexusPact Cloud when the user supplies an API key. That is not telemetry — decisions 1–3 and 5 above are untouched — but the *code path* claim in this decision is no longer literally true, and ADR-013 restates the boundary that actually holds: the validation engine has no HTTP client, and the reporting hop runs after the verdict, never before it.
5. The public policy lives in `docs/src/telemetry.md` and any future payload schema must be published there before release.

## Consequences

- We lose precise adoption numbers for the MVP; stage-gate metrics fall back to proxies (downloads, action usage, GitHub traffic) — accepted cost.
- Publishing the policy before any code exists makes it enforceable by the community and cheap to honor.
- If opt-in telemetry ships later, its expected volume is low; it must be justified against that reality, not assumed.
