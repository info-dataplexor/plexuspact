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

Gate contract *changes* too — in the repo that owns the contract:

```yaml
      - name: Block breaking contract changes
        run: |
          git show origin/main:contracts/user_signups.yaml > /tmp/old.yaml
          plexuspact diff /tmp/old.yaml contracts/user_signups.yaml
```

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
