# PlexusPact GitHub Action

The official composite action for running [`plexuspact check`](https://github.com/info-dataplexor/plexuspact) in CI. It:

1. downloads a **pinned** release binary for the runner OS (Linux musl, macOS x86_64/arm64, Windows MSVC),
2. verifies it against the release's `SHA256SUMS`,
3. runs `plexuspact check <data> --contract <contract> --format json --report plexuspact-report.html`,
4. emits one `::error` (or `::warning` for warn-severity) workflow annotation per failed check, anchored to the contract file,
5. uploads the self-contained HTML report as a build artifact,
6. fails the job when the check fails (exit code is propagated),
7. records the run in PlexusPact Cloud, if you gave it a key.

There is no business logic in the action — everything meaningful happens inside the CLI, so local runs and CI runs behave identically.

## Usage

```yaml
name: data-contract
on: [pull_request]

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Validate signups export against its contract
        uses: info-dataplexor/plexuspact/action@v0.2.0
        with:
          contract: contracts/user_signups.yaml
          data: export/signups.csv
          strict: "true"
```

## Inputs

| Input | Required | Default | Description |
|---|---|---|---|
| `contract` | yes | — | Path to the contract YAML (annotations are anchored to this file) |
| `data` | yes | — | Dataset to validate (CSV/Parquet/NDJSON/JSON), or `-` for stdin |
| `version` | no | `0.2.0` | plexuspact release to download, without the leading `v` |
| `strict` | no | `"false"` | Treat `warn`-severity failures as errors (`--strict`) |
| `format` | no | `json` | Results file format (`json` \| `junit` \| `human`). PR annotations require `json` |
| `report-artifact` | no | `plexuspact-report` | Artifact name for the HTML report; empty string skips the upload |
| `api-key` | no | `""` | PlexusPact Cloud project API key. Given one, every run is recorded against the project |
| `api` | no | `""` | API root for self-hosted installs (default `https://api.plexuspact.com/api/v1`) |
| `redact-samples` | no | `"false"` | Mask sample failing values, keeping row numbers (`--redact-samples`) |
| `require-push` | no | `"false"` | Fail the job when a run could not be recorded |

## Outputs

| Output | Description |
|---|---|
| `exit-code` | `0` pass, `1` failures, `2` usage/contract error, `3` internal error |
| `results-file` | Path to `plexuspact-results.<ext>` in the workspace |
| `run-url` | The recorded run in PlexusPact Cloud. Empty without an `api-key`, or if the run could not be recorded |

## Keeping the history

The check tells you about today. A history tells you whether a supplier's feed
is getting better or worse, and that is the thing you can take into a renewal.
Add one secret and every run this workflow produces is kept:

```yaml
      - uses: info-dataplexor/plexuspact/action@v0.2.0
        with:
          contract: contracts/user_signups.yaml
          data: export/signups.csv
          api-key: ${{ secrets.PLEXUSPACT_API_KEY }}
```

Nothing else changes. The check still passes or fails on the data alone — a run
that cannot be recorded is a warning in the log, not a red build, unless you ask
for `require-push: "true"`. The key is read from the environment and never
appears on a command line.

## Non-blocking mode

To surface failures without failing the job (e.g. while rolling a contract out), use `continue-on-error` and read the output:

```yaml
      - id: contract
        uses: info-dataplexor/plexuspact/action@v0.2.0
        continue-on-error: true
        with:
          contract: contracts/user_signups.yaml
          data: export/signups.csv

      - run: echo "plexuspact exited with ${{ steps.contract.outputs.exit-code }}"
```

## Notes

- Pin the action to a release tag (`@v0.2.0`). The `version` input and the action tag are independent, but keeping them equal is the supported configuration.
- Runner requirements: `curl` and `jq` (preinstalled on all GitHub-hosted runners).
- Without an `api-key`, the check runs fully offline apart from the one-time binary download from GitHub Releases — your data never leaves the runner.
- With an `api-key`, the JSON result is sent to PlexusPact Cloud over HTTPS. That result includes sample failing rows; set `redact-samples: "true"` if those rows are sensitive.
