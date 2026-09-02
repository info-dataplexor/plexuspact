# Exit codes

Exit codes are a contract too. They are deterministic, documented, and stable — scripts and CI systems can branch on them safely. See [ADR-004](https://github.com/dataplexor/plexuspact/blob/main/docs/adr/004-exit-code-convention.md) for the rationale.

| Code | Meaning | Typical causes |
|---|---|---|
| `0` | **Pass.** All error-severity checks passed. | Clean data; or only `warn`-severity failures without `--strict`. |
| `1` | **Check failures.** At least one error-severity check failed (or a warn-severity one under `--strict`). | Bad data. This is the code your CI gate keys on. |
| `2` | **Usage or contract error.** The run never validated anything. | Missing file, unreadable input, malformed YAML, unknown check name, invalid regex, bad CLI flags; `--push` with no API key set, or with `--offline`. |
| `3` | **Internal error.** A bug in plexuspact or an environmental failure. | Out of disk while writing a report, a panic-class defect. Please [file a bug](https://github.com/dataplexor/plexuspact/issues). Also: `--push` was given and the run could not be [recorded](cloud.md). |

## Severity interaction

- A check with `severity: warn` that fails is **reported** but does not affect the exit code: the run exits `0`.
- `--strict` promotes warn failures to exit `1`. Use it when a pipeline should be paused for *any* anomaly.
- `plexuspact diff` uses the same convention: exit `0` for semantic/non-breaking/cosmetic changes, `1` when any change is classified breaking, `2` for unparseable contracts. A `semantic` change exits 0 deliberately — a redefinition breaks no pipeline, so blocking the merge would be wrong; it is reported loudly instead, because it is the one change that never announces itself.

## Reporting never changes the verdict

[Reporting a run](cloud.md) to PlexusPact Cloud is something that happens *after*
the exit code has been decided. If the report fails — the network is down, the key
was revoked, the plan is out of runs — you get a warning on stderr and the code
the data earned. A green dataset does not turn red because a server was slow.

The single exception is opt-in and loud. `check --push` declares that a missing
run is as serious as a failing check, and a clean run that could not be recorded
then exits `3` instead of `0`. Even then it never overwrites a `1`: if the data
broke the contract, that is the more important of the two things that went wrong,
and it is the one your gate should key on.

## Scripting patterns

Distinguish "bad data" from "broken pipeline":

```sh
plexuspact check data.csv --contract contract.yaml --format json > run.json
case $? in
  0) echo "clean" ;;
  1) echo "data violates contract — quarantine the file, notify the producer" ;;
  2) echo "misconfiguration — fix the invocation or the contract, do not retry" ;;
  3) echo "internal error — retry once, then page the platform team" ;;
esac
```

In GitHub Actions the [official action](ci-recipes.md#github-actions) propagates the code and also exposes it as the `exit-code` output for non-blocking workflows.
