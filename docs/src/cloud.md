# Keeping a history

A check tells you about today. It answers "is this file good?" and then it is
gone — the next run starts from nothing, and so does the next conversation with
the team that sent you the file.

A *history* answers different questions, and they are the ones that change
behaviour. Is this feed getting worse? Was this the third failure this month or
the first? When the supplier says "that was a one-off", is it? None of those can
be answered by a single exit code, however good it is.

[PlexusPact Cloud](https://plexuspact.com) keeps them. The CLI stays free and
works entirely on its own; this page is about the one variable that connects the
two, for people who want the history.

## Turning it on

Create a project API key in the cloud (Settings → API keys — it is shown once),
then export it:

```sh
export PLEXUSPACT_API_KEY=ck_live_xxxxxxxxxxxxxxxx
```

That is the whole setup. Run a check exactly as before:

```sh
plexuspact check data.csv --contract contract.yaml
```

```text
✓ user_signups — all 18 checks passed (45,231 rows in 0.4s)
✓ reported run to https://app.plexuspact.com/projects/01a04f0c-d8da-7312-9c4d-c0118a0125d8/runs/01a04f0c-e171-7103-8ea4-d928c2ef4a15
```

Nothing to pipe, nothing to parse, no glue to maintain. The run is matched to a
registered contract by its dataset name and the SHA-256 of the contract file, so
a contract you edit and re-commit lands as a new version rather than corrupting
the old one's history.

Self-hosted installs point somewhere else:

```sh
export PLEXUSPACT_API=https://plexuspact.internal.example.com/api/v1
```

or per-invocation, `--api https://…`.

## In CI

The [GitHub Action](ci-recipes.md#github-actions) takes the key as an input and
does the same thing:

```yaml
      - uses: dataplexor/plexuspact/action@v0.1.0
        with:
          contract: contracts/user_signups.yaml
          data: export/signups.csv
          api-key: ${{ secrets.PLEXUSPACT_API_KEY }}
```

It also publishes the recorded run's URL as the `run-url` output and as a
workflow notice, so the link is one click from the job summary.

For any other CI system, exporting the secret into the environment is enough —
there is nothing else to add to the pipeline.

## Reporting something that already exists

`plexuspact push` sends a result document written earlier: a file from a previous
step, a nightly batch, a retry of something the network ate.

```sh
plexuspact check data.csv --contract contract.yaml --format json > run.json
plexuspact push run.json
```

It reads `-` as stdin, prints the run URL on stdout (so it can be captured) and
the confirmation on stderr. Unlike `check`, reporting is the entire job here, so
a missing key is an error rather than a silence.

## What it does not do

**It never changes the verdict.** The exit code belongs to the data. If the
report fails — network down, key revoked, plan out of runs — you get a warning on
stderr and the code the check earned. This is the whole reason the CLI carries
the bridge itself: the obvious hand-written version,

```sh
plexuspact check … --format json | curl -X POST …   # don't
```

quietly hands the pipeline's exit status to `curl`, so a failing check goes green
under `set -e`; add `pipefail` and the failing check kills the pipeline before
`curl` runs, so the runs never recorded are exactly the ones worth recording.

If a missing run *is* as serious as a failing check — a compliance pipeline where
the evidence matters as much as the result — say so explicitly:

```sh
plexuspact check data.csv --contract contract.yaml --push
```

`--push` makes an unwired secret an error on the first run rather than a
discovery a month later, and turns an unreported clean run into [exit
`3`](exit-codes.md). It still never overwrites a `1`.

## Turning it off

| | Effect |
|---|---|
| No `PLEXUSPACT_API_KEY` | Nothing is reported. This is the default. |
| `check --no-push` | Ignores a key that is set, for one run. |
| `--offline` | No network activity at all, for one run. |
| `PLEXUSPACT_NO_NETWORK=1` | The same, for every run in that environment. |

The last two outrank the key deliberately: set the variable in a CI base image
and no pipeline underneath it can re-enable the network, whatever it exports.
Under any of them, `--push` is a usage error rather than a silent no-op.

## What travels

Exactly the JSON document `check --format json` prints — the [result
schema](result-schema.md), verbatim. That includes **sample failing values**,
which are real rows from your dataset, along with column names and the contract's
own contents. If the failing rows are sensitive, mask them:

```sh
plexuspact check data.csv --contract contract.yaml --redact-samples
```

Row numbers survive; the values become `***`. The counts, rates, and verdict are
unchanged, so the history is still worth having.

Two more properties, by construction: the key is read only from the environment —
there is no `--token` flag to leak into shell history, `ps`, or a CI log that
echoes its own command line — and it is refused over plain `http://` to anything
but localhost.

This is not telemetry. Nothing is collected about you, and nothing is sent that
you did not address to your own project. See the [telemetry and network
policy](telemetry.md) and [ADR-013](https://github.com/dataplexor/plexuspact/blob/main/docs/adr/013-cloud-reporting-boundary.md).
