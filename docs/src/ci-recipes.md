# CI recipes

`plexuspact check` is designed to drop into any pipeline: one binary, deterministic [exit codes](exit-codes.md), machine-readable output. These recipes are copy-paste starting points.

## GitHub Actions

Use the official composite action — it downloads a pinned, checksum-verified binary, annotates the PR with each failed check, and uploads the HTML report as an artifact:

```yaml
name: data-contract
on: [pull_request]

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: dataplexor/plexuspact/action@v0.1.0
        with:
          contract: contracts/user_signups.yaml
          data: export/signups.csv
          strict: "true"
```

Inputs, outputs, and non-blocking mode are documented in the [action README](https://github.com/dataplexor/plexuspact/blob/main/action/README.md).

Add `api-key: ${{ secrets.PLEXUSPACT_API_KEY }}` and every run this workflow
produces is [kept as history](cloud.md), with the run's URL surfaced as a workflow
notice. Nothing else changes; the gate still passes or fails on the data alone.

### Gate contract changes, not just data

Comparing two files in the working copy answers a question the author already
knows the answer to — they wrote both files. The question a pull request
actually raises is different: *what is in force right now, and who breaks if
this lands?* Only the registry can answer that, because only the registry knows
which version suppliers are being judged against today and who has declared they
read the dataset.

```yaml
name: contract
on: [pull_request]

permissions:
  contents: read
  pull-requests: write     # to leave the summary on the PR

jobs:
  preflight:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dataplexor/plexuspact/action/preflight@v0.1.0
        with:
          contract: contracts/user_signups.yaml
          api-key: ${{ secrets.PLEXUSPACT_API_KEY }}
```

It comments on the pull request with what changes and who breaks, annotates the
contract file, and fails the job when the change tightens terms somebody relies
on. It writes nothing to the registry: a contract that has not been merged has
not been agreed.

Without a key — or in a repo that does not use the cloud — the local form still
catches a tightening against the base branch:

```yaml
      - name: Block breaking contract changes
        run: |
          git show origin/main:contracts/user_signups.yaml > /tmp/old.yaml
          plexuspact diff /tmp/old.yaml contracts/user_signups.yaml
```

### Register on merge, with the pull request attached

An API key is one account, not one person. A contract pushed from CI otherwise
arrives from nowhere — the record says a machine did it and stops. The merge job
is the only place that knows the pull request the change was actually agreed in,
so it passes the link, and the registry keeps it on the version *and* in the
audit trail.

```yaml
name: contract-register
on:
  push:
    branches: [main]
    paths: ["contracts/**"]

jobs:
  register:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dataplexor/plexuspact/action/register@v0.1.0
        with:
          contract: contracts/user_signups.yaml
          api-key: ${{ secrets.PLEXUSPACT_API_KEY }}
```

The pull request URL and the short commit SHA are worked out from the event; set
`source-url` and `version-label` yourself to override either. If the project
requires a second person to approve a tightening, the version is recorded as
*proposed* and the one in force keeps applying — that is a working project, not
a failed job, so the step still passes. Set `fail-if-pending: "true"` if you
want the merge to go red until somebody approves.

## GitLab CI

```yaml
data-contract:
  stage: test
  image: alpine:3.20
  variables:
    PLEXUSPACT_VERSION: "0.1.0"
  before_script:
    - apk add --no-cache curl
    - |
      curl -fsSL -o plexuspact.tar.gz \
        "https://github.com/dataplexor/plexuspact/releases/download/v${PLEXUSPACT_VERSION}/plexuspact-v${PLEXUSPACT_VERSION}-x86_64-unknown-linux-musl.tar.gz"
      curl -fsSL -o SHA256SUMS \
        "https://github.com/dataplexor/plexuspact/releases/download/v${PLEXUSPACT_VERSION}/SHA256SUMS"
      grep "plexuspact-v${PLEXUSPACT_VERSION}-x86_64-unknown-linux-musl.tar.gz" SHA256SUMS | sed 's| .*| plexuspact.tar.gz|' | sha256sum -c -
      tar -xzf plexuspact.tar.gz plexuspact && install -m 755 plexuspact /usr/local/bin/
  script:
    - plexuspact check export/signups.csv --contract contracts/user_signups.yaml
        --format junit --report report.html > junit.xml || status=$?
    - exit ${status:-0}
  artifacts:
    when: always
    reports:
      junit: junit.xml
    paths: [report.html]
```

The musl binary runs on any Linux image, including Alpine.

### Contract preflight on a merge request

The same shift-left gate, as an includable template — it verifies the download's
checksum, asks the registry what would change and who breaks, and leaves one
note on the merge request that it updates on every push rather than adding a new
one each time:

```yaml
include:
  - remote: "https://raw.githubusercontent.com/dataplexor/plexuspact/v0.1.0/ci/gitlab/plexuspact-preflight.yml"

plexuspact:preflight:
  variables:
    PLEXUSPACT_CONTRACT: contracts/user_signups.yaml
```

`PLEXUSPACT_API_KEY` goes in the project's CI/CD settings as a masked, protected
variable. The note needs a project access token with the `api` scope in
`PLEXUSPACT_GITLAB_TOKEN`; without it the job still gates, it just has nowhere
to leave the summary.

## Azure Pipelines

```yaml
resources:
  repositories:
    - repository: plexuspact
      type: github
      name: dataplexor/plexuspact
      ref: refs/tags/v0.1.0
      endpoint: github

steps:
  - template: ci/azure/plexuspact-preflight.yml@plexuspact
    parameters:
      contract: contracts/user_signups.yaml
      apiKey: $(PLEXUSPACT_API_KEY)
```

Azure does not map secret variables into the environment on its own, which is
why the key is passed as a parameter and handed to the step as an env var rather
than put on a command line where it would land in the log. The comment on the
pull request needs the build service to have *Contribute to pull requests* on
the repository.

## pre-commit hook

Validate the contract file itself (and small committed fixtures) before every commit:

```yaml
# .pre-commit-config.yaml
repos:
  - repo: local
    hooks:
      - id: plexuspact-validate-contract
        name: plexuspact validate-contract
        entry: plexuspact validate-contract
        language: system
        files: ^contracts/.*\.ya?ml$
      - id: plexuspact-check-fixture
        name: plexuspact check (fixture)
        entry: plexuspact check fixtures/signups_small.csv --contract contracts/user_signups.yaml
        language: system
        pass_filenames: false
        files: ^(contracts/|fixtures/)
```

Full-dataset checks belong in CI, not pre-commit — keep hook inputs small.

## Airflow

`BashOperator` composes directly — no wrapper needed. Exit code 1 fails the task; the JSON result lands next to the data for downstream tasks or alerting:

```python
from airflow.operators.bash import BashOperator

validate_signups = BashOperator(
    task_id="validate_signups",
    bash_command=(
        "plexuspact check {{ params.data }} "
        "--contract {{ params.contract }} "
        "--format json > {{ params.out }}/run.json"
    ),
    params={
        "data": "/data/exports/signups.csv",
        "contract": "/opt/contracts/user_signups.yaml",
        "out": "/data/exports",
    },
)
```

Treat warn-only runs as soft failures by checking `.summary.failed_warn` in the JSON from a downstream `PythonOperator`, or pass `--strict` to hard-fail on warnings.

## Cron + shell

A minimal nightly gate that keeps the last report and alerts on failure:

```sh
#!/bin/sh
# /etc/cron.daily/check-vendor-feed
set -eu
cd /srv/feeds

if ! plexuspact check vendor_latest.csv \
      --contract /etc/contracts/vendor_feed.yaml \
      --format json > last_run.json \
      --report last_report.html; then
  code=$?
  # 1 = contract failures; 2/3 = usage or internal error — page differently if you like
  mail -s "vendor_feed contract FAILED (exit ${code})" data-team@acme.com < last_run.json
  exit "$code"
fi
```

## Streaming from stdin

Anywhere you can pipe, you can validate — NDJSON streams in constant memory:

```sh
kafka-console-consumer --topic signups --max-messages 100000 \
  | plexuspact check - --input-format ndjson --contract contracts/user_signups.yaml
```

## General tips

- **Pin the version** everywhere (action input, `PLEXUSPACT_VERSION` variable) and upgrade deliberately.
- **Verify checksums** when hand-downloading — every release publishes `SHA256SUMS`.
- Use `--format json` for machines, `--report report.html` for humans, both in one run.
- Budget ~10 seconds of CI overhead for a 100 MB fixture; the binary itself starts in under 50 ms.
