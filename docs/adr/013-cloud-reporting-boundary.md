# ADR-013: Reporting a run to the cloud — the CLI carries its own bridge

**Status:** Accepted · **Date:** 2026-08 · **Amends:** [ADR-006](006-telemetry-policy.md) §4

## Context

A check produces a verdict on one machine at one moment. The value of a *history*
of verdicts — is this supplier's feed getting better or worse, was this the third
time this month — only exists once results are kept somewhere shared. PlexusPact
Cloud has kept them since it shipped: `POST /api/v1/runs` accepts exactly the JSON
document `check --format json` already emits.

Nothing carried the document across. The documented method was a shell pipeline:

```sh
plexuspact check data.csv --contract contract.yaml --format json \
  | curl -sS -X POST "$API/runs" -H "Authorization: Bearer $KEY" -d @-
```

Three problems, in increasing order of seriousness.

1. Every user writes it, and every user maintains it.
2. Under `set -e` (and without `set -o pipefail`) the pipeline's exit status is
   curl's, so a failing check goes green. The gate silently stops being a gate.
3. Worse, with `pipefail` the failing check kills the pipeline before curl runs —
   so the runs that are never recorded are precisely the runs worth recording.

A bridge whose failure mode is "loses the bad news" is not a bridge.

## Decision

1. **The CLI reports for itself.** `check` sends its result to the cloud when
   `PLEXUSPACT_API_KEY` is set, and `plexuspact push <result.json>` does the same
   hop for a document already on disk. No glue.
2. **The verdict is never changed by the reporting.** A push that fails is a
   warning on stderr; the exit code is the data's alone. The single exception is
   explicit: `check --push` says a missing run is as serious as a failing check,
   and turns an unreported *clean* run into exit 3. It never overwrites exit 1 —
   "the data broke the contract" is the more important of the two failures.
3. **The key has one home, the environment.** No `--token` flag: a secret on a
   command line ends up in shell history, in `ps`, and in CI logs that echo the
   command they ran.
4. **The key is never sent in the clear.** Plain `http://` is refused for any host
   that is not loopback.
5. **Off is the default and stays reachable.** No key means no socket. `--no-push`
   ignores a key that is set. `--offline` and `PLEXUSPACT_NO_NETWORK` outrank
   everything, including a key: a base image can forbid the network and no
   pipeline underneath it can re-enable it. Under any of those, `--push` is a
   usage error rather than a silent no-op.
6. **This is not telemetry, and the distinction is load-bearing.** Telemetry is
   data about *you*, collected by *us*, to answer our questions. This is your
   result, sent by you, to your project, because you asked for it and gave it a
   key. ADR-006 stands unchanged: there is still no telemetry code, and no
   payload leaves the machine unaddressed by the person running it.

## Consequences

- **ADR-006 §4 is amended.** "Anything network-shaped stays outside `check`'s code
  path" is no longer true as written: `cmd_check` calls the reporting hop after
  the verdict is computed. The engine below it is untouched — `plexuspact-core`,
  `-engine`, `-io` and `-contract` have no HTTP client and cannot acquire one
  without a workspace-manifest change — so validation remains deterministic and
  provably network-free. What moved is the boundary: from "the binary cannot
  speak" to "the binary speaks only when handed a key, and says so when it does".
- The claim "there is no HTTP client in the dependency tree" is retired. There is
  one, `ureq`, in `plexuspact-cli` only. The check path's own tree is unchanged,
  which is the claim actually worth making, and it is now the claim we make.
- `--offline` and `PLEXUSPACT_NO_NETWORK` stop being forward-looking promises and
  become implemented switches with tests. `PLEXUSPACT_NO_TELEMETRY` remains
  reserved for telemetry that still does not exist.
- Adoption cost for the funnel's most important step is one environment variable.
  That is deliberate: the free tool spreads on its own merits, and the moment
  someone wants the history, there is nothing to build.
