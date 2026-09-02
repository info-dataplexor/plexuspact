# Telemetry and network policy

Short version: **there is no telemetry.** No usage data is collected, none is
sent, there is nothing to opt out of. This page exists so the policy is public
*before* any telemetry ever ships, and so you can hold us to it.

`plexuspact` does open one network connection, in one situation, and it is worth
being precise about which: when *you* give it an API key, `check` reports its own
result to *your* PlexusPact Cloud project. That is a feature you switch on, not
data we collect. The rest of this page draws that line properly.

## The telemetry policy

If usage telemetry is ever added (it is under consideration to measure adoption —
see [ADR-006](https://github.com/dataplexor/plexuspact/blob/main/docs/adr/006-telemetry-policy.md)),
it will follow these rules, in order of precedence:

1. **Opt-in only.** Telemetry will be off by default. No dark patterns, no
   "anonymous by default", no opt-out-buried-in-docs. You would have to
   explicitly enable it.
2. **Kill switches always win.** Setting `PLEXUSPACT_NO_TELEMETRY` (to any value)
   disables telemetry unconditionally, regardless of any config file — suitable
   for org-wide enforcement in CI images.
3. **`--offline` disables all network activity** — telemetry, reporting, update
   checks, everything. A run with `--offline` opens zero sockets.
4. **Never data, never contracts.** Telemetry would carry coarse usage events
   only (command name, tool version, OS, duration bucket, exit code). Never
   dataset contents, never sample values, never column names, never contract
   contents, never file paths.
5. **Visible and documented.** Any transmission would be logged to stderr, and
   the exact payload schema documented on this page before release.

## Reporting is not telemetry

[Reporting a run](cloud.md) sends the JSON result document to the cloud project
whose key you exported. The difference from telemetry is not a matter of degree:

|  | Telemetry | Reporting |
|---|---|---|
| Whose question does it answer? | Ours | Yours |
| Where does it go? | Our analytics | Your project |
| What turns it on? | Nothing — it does not exist | You, by exporting a key |
| What is in it? | — | The result you already print with `--format json` |
| Can you read it afterwards? | — | Yes; it is the run page you opened |

Two consequences worth stating plainly, because they cut the other way:

- **Reporting carries your data.** A result document quotes sample failing values,
  which are real rows from your dataset. If those rows are sensitive, run with
  `--redact-samples`, which keeps row numbers and masks the values. Column names
  and the contract's own contents travel too.
- **It is on whenever the key is.** There is no second confirmation. Exporting
  `PLEXUSPACT_API_KEY` in a shell profile means every check in that shell reports.
  That is the intended ergonomics, and it is why every reported run prints
  `✓ reported run to <url>` on stderr — you can always see that it happened.

## Guarantees about the network

These hold for every 0.1.x release:

- **The validation engine cannot open a socket.** `plexuspact-core`, `-engine`,
  `-io` and `-contract` have no HTTP client in their dependency trees and cannot
  acquire one without a change to the workspace manifest. Validation is
  deterministic and identical on an air-gapped machine.
- **Reporting happens after the verdict, never before it.** The result is
  computed, printed, and only then sent. Nothing about what the network does can
  change what the check decided.
- **A failed report is a warning, not a failure.** The exit code belongs to the
  data. The one exception is explicit: `check --push` asks for a missing run to be
  treated as an error (see [exit codes](exit-codes.md)).
- **No key, no connection.** With `PLEXUSPACT_API_KEY` unset, `check` behaves
  exactly as it did before reporting existed.
- **Two switches outrank the key.** `--offline`, or `PLEXUSPACT_NO_NETWORK` set to
  anything other than empty or `0`, suppresses reporting even when a key is
  present. Put the variable in a CI base image and no pipeline underneath it can
  re-enable the network.
- **Keys never travel in the clear.** Plain `http://` is refused for any host that
  is not loopback, and the key is only ever read from the environment — there is
  no `--token` flag to leak into shell history or a CI log.

## Verifying

Don't take our word for it:

```sh
# Zero socket syscalls, guaranteed, whatever else is in the environment:
strace -f -e trace=network plexuspact check data.csv --contract contract.yaml --offline
```

Without `--offline` the same command opens exactly one connection if — and only
if — `PLEXUSPACT_API_KEY` is set, and tells you so on stderr.

The source is Apache-2.0. The HTTP client (`ureq`) appears in exactly one crate,
`plexuspact-cli`, and is reached from exactly one module, `push.rs`:

```sh
cargo tree -p plexuspact-core -i ureq   # error: nothing depends on it
```
