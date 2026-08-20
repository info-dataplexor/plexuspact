# Quickstart: 5 minutes to a green check

This tutorial takes you from nothing to a passing validation on a real CSV. Total time on a typical machine: under five minutes.

## 1. Install (1 minute)

```sh
curl -fsSL https://raw.githubusercontent.com/dataplexor/plexuspact/main/install.sh | sh
plexuspact --version
```

(Windows: `irm https://raw.githubusercontent.com/dataplexor/plexuspact/main/install.ps1 | iex` — see [Installation](installation.md) for Homebrew/Scoop/cargo.)

## 2. Get some data

Use any CSV you have, or save this as `signups.csv`:

```csv
user_id,email,age,country,plan,signed_up
u_001,alice@example.com,34,DE,pro,2026-07-09T08:12:44Z
u_002,bob@example.com,28,US,free,2026-07-09T09:03:11Z
u_003,carla@example.com,41,FR,enterprise,2026-07-09T09:15:02Z
u_004,dev@example.com,23,IN,free,2026-07-09T10:44:37Z
```

## 3. Bootstrap a contract with `init` (1 minute)

Never write a contract from scratch — profile the data and let `init` draft it:

```sh
$ plexuspact init signups.csv --out contract.yaml
Profiled 4 rows × 6 columns in 0.0s
Wrote contract.yaml (6 columns typed, suggestions commented out)
```

Open `contract.yaml`. You'll find every column with an inferred type, `required: true` where no nulls were observed, and conservative suggestions left as comments (for example `# checks: [{ min: 23 }]  # observed min: 23`). Suggestions are never enabled automatically — `init` output always passes on the data it profiled.

## 4. Say what must be true

Uncomment and tighten the rules you actually want to enforce. A reasonable contract for this dataset:

```yaml
apiVersion: v1
dataset: user_signups
owner: growth-team@acme.com
description: Daily signup export from the app database.

columns:
  user_id:   { type: string, required: true, checks: [unique] }
  email:     { type: string, required: true, pii: email, checks: [{ format: email }] }
  age:       { type: int,    checks: [{ min: 18 }, { max: 120 }] }
  country:   { type: string, checks: [{ length: 2 }, { regex: "^[A-Z]{2}$" }] }
  plan:      { type: string, checks: [{ enum: [free, pro, enterprise] }] }
  signed_up: { type: datetime, required: true }

dataset_checks:
  - row_count_min: 1

settings:
  allow_extra_columns: true
  on_type_mismatch: error
```

Sanity-check it without touching any data:

```sh
$ plexuspact validate-contract contract.yaml
✓ contract.yaml is valid (6 columns, 10 checks)
```

## 5. Run your first check (30 seconds)

```
$ plexuspact check signups.csv --contract contract.yaml

✓ user_signups — 10 of 10 checks passed (4 rows in 0.0s)

$ echo $?
0
```

Green. That exit code `0` is the whole point: this command now drops into any CI job, cron script, or Makefile as a gate.

## 6. Watch it catch something

Break a row — change `bob@example.com` to `bob@@example` and an age to `11` — and rerun:

```
$ plexuspact check signups.csv --contract contract.yaml

✗ user_signups — 2 of 10 checks failed (4 rows in 0.0s)

  ✗ email · format: email          1 row failed (25%)
      row 2: "bob@@example"
  ✗ age · min: 18                  1 row failed (25%)
      min observed: 11

$ echo $?
1
```

Column, rule, observed value, row number. The producer who broke it can fix it without asking anyone.

## Where next

- Ship it to CI: [CI recipes](ci-recipes.md) — the GitHub Action takes four lines.
- Add a shareable report: `--report report.html` (self-contained, mail it to a vendor).
- Machine-readable results: `--format json` ([schema](result-schema.md)).
- Learn every field and all 20 checks: [contract reference](contract-reference.md).
