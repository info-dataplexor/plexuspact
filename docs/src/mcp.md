# Asking before you break something: the MCP server

Shift-left has always had a limit, and the limit was the pull request. `plexuspact
diff --against-registry` tells an author exactly who breaks — but only once they
have written the change, committed it, and pushed. By then the rename is
everywhere and the argument is about backing it out.

More and more of that change is now written by an assistant sitting in the
editor. An assistant can do the thing a person will not: ask, before every edit,
what the dataset already promises and who is relying on it.

`plexuspact mcp` is a [Model Context Protocol](https://modelcontextprotocol.io)
server that puts those questions in the assistant's hands.

```
plexuspact mcp
```

It speaks JSON-RPC 2.0 over stdin and stdout. You do not run it by hand — you
tell your editor or agent how to start it, and it starts it for you.

## Three rules it keeps

**Nothing here writes.** Not a contract, not a run, not a file. An assistant that
could register terms on your behalf would make the assistant the author of terms
nobody agreed to. Registering a version stays a deliberate act at a command line
or in a merge job.

**The credential rules do not change because the caller is a model.** The API key
still has one home, `PLEXUSPACT_API_KEY`, and there is still no flag to pass it
on a command line. `--offline` and `PLEXUSPACT_NO_NETWORK` still win over
everything, and a key is still never sent over plain `http` to anything but
localhost. Started with `--offline`, the three local tools work and the two cloud
tools say so plainly.

**stdout belongs to the protocol.** Everything meant for a person goes to stderr,
so a stray log line can never corrupt the conversation.

## The five tools

| Tool | Answers | Needs a key |
| --- | --- | --- |
| `explain_contract` | What does this dataset promise? Columns, rules, who reads it, what is being retired. | no |
| `check_data` | Does this file keep the contract? Every failing check, with sample rows. | no |
| `diff_contracts` | What did I just change, and is any of it breaking? | no |
| `preflight_contract` | What is in force right now, and **who breaks** if this lands? | yes |
| `dataset_status` | How have the recent deliveries of this dataset gone? | yes |

`preflight_contract` is the reason the server exists. The other four can be
answered from the working copy; only the registry knows which version suppliers
are being judged against today, and only the registry knows which teams have
declared that they read the column you were about to rename.

The server tells the assistant, in its `initialize` instructions, to call it
*before* editing anything that produces a dataset — a schema, a transformation,
a column's type or name, a dbt model.

## Configuring a client

The shape is the same everywhere: a command, its arguments, and an environment.

**Claude Code** — `.mcp.json` in the repository root:

```json
{
  "mcpServers": {
    "plexuspact": {
      "command": "plexuspact",
      "args": ["mcp"],
      "env": { "PLEXUSPACT_API_KEY": "${PLEXUSPACT_API_KEY}" }
    }
  }
}
```

**Cursor** — `.cursor/mcp.json`, and **Windsurf** — `~/.codeium/windsurf/mcp_config.json`,
take the same object. **Claude Desktop** uses `claude_desktop_config.json`, where
the environment has to be spelled out rather than inherited:

```json
{
  "mcpServers": {
    "plexuspact": {
      "command": "plexuspact",
      "args": ["mcp"],
      "env": { "PLEXUSPACT_API_KEY": "ck_live_…" }
    }
  }
}
```

Point it at a self-hosted install with `"args": ["mcp", "--api", "https://plexuspact.internal/api/v1"]`,
or with `PLEXUSPACT_API` in the environment.

A read-only key is enough. `preflight_contract` and `dataset_status` both need
only the `read` scope — mint one under **Settings → API keys** and give the
assistant nothing more than that.

## Running it without a project

Every local tool works with no key at all:

```
plexuspact mcp --offline
```

The assistant can still read contracts, validate files, and classify a diff. It
just cannot find out who breaks, because that is a question about your
organisation and not about your working copy.

## Trying it by hand

The protocol is newline-delimited JSON, so a pipe is enough to see it work:

```bash
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  | plexuspact mcp --offline
```

## What it is not

It is not a way to let an assistant approve its own changes. `preflight_contract`
returns a verdict and a list of names; deciding that a breaking change is worth
making, and telling the people on that list, is still work for a person. The
server is deliberately built so that an assistant cannot skip that step by
registering the contract itself.
