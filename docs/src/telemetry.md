# Telemetry policy

Short version: **the current release contains no telemetry code at all.** Nothing is collected, nothing is sent, there is nothing to opt out of. This page exists so the policy is public *before* any telemetry ever ships, and so you can hold us to it.

## The policy

If usage telemetry is ever added (it is under consideration to measure adoption — see [ADR-006](https://github.com/dataplexor/plexuspact/blob/main/docs/adr/006-telemetry-policy.md)), it will follow these rules, in order of precedence:

1. **Opt-in only.** Telemetry will be off by default. No dark patterns, no "anonymous by default", no opt-out-buried-in-docs. You would have to explicitly enable it.
2. **Kill switches always win.** Setting the environment variable `PLEXUSPACT_NO_TELEMETRY` (to any value) disables telemetry unconditionally, regardless of any config file — suitable for org-wide enforcement in CI images.
3. **`--offline` disables all network activity** — telemetry, update checks, everything. A run with `--offline` is guaranteed to open zero sockets.
4. **Never data, never contracts.** Telemetry would carry coarse usage events only (command name, tool version, OS, duration bucket, exit code). Never dataset contents, never sample values, never column names, never contract contents, never file paths.
5. **Visible and documented.** Any transmission would be logged to stderr, and the exact payload schema documented on this page before release.

## The check path is network-free by design

Independent of telemetry: `plexuspact check` performs **no network I/O, ever**. This is an architectural guarantee (see [SECURITY.md](https://github.com/dataplexor/plexuspact/blob/main/SECURITY.md)) — validation is deterministic and works identically on an air-gapped machine. Anything network-shaped now or in the future (telemetry, update checks, Phase 2 `push` to the registry, Phase 1.5 AI assist) lives outside the check path, is explicit opt-in, and respects `--offline` and `PLEXUSPACT_NO_NETWORK`.

## Verifying

Don't take our word for it:

```sh
# Linux: run a check under strace and observe zero socket syscalls
strace -f -e trace=network plexuspact check data.csv --contract contract.yaml
```

The source is Apache-2.0 — grep it: there is no HTTP client in the dependency tree of the check path.
