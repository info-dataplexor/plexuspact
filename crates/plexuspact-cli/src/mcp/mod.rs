//! An MCP server, so the agent writing the pipeline can ask before it breaks
//! something.
//!
//! Shift-left has a limit, and for most of this tool's life that limit was the
//! pull request: `diff --against-registry` tells an author who breaks, but only
//! once they have written the change and pushed it. The change is now often
//! being written by an assistant sitting in the editor, and that assistant can
//! ask a question a person would not bother to ask — *before* the edit, for
//! every edit, without being reminded.
//!
//! So this exposes the questions the CLI already answers, over the Model
//! Context Protocol, on stdin and stdout:
//!
//! - what does this contract actually promise (`explain_contract`)
//! - does this file keep it (`check_data`)
//! - what did I just change (`diff_contracts`)
//! - what is in force right now and who breaks if this lands
//!   (`preflight_contract`)
//! - how have the deliveries been going (`dataset_status`)
//!
//! Three rules the rest of this module exists to keep.
//!
//! 1. **stdout belongs to the protocol.** One JSON object per line and nothing
//!    else, ever. Every message meant for a person goes to stderr. This is why
//!    the tools re-render their own answers instead of calling the printing
//!    helpers in [`crate::commands`], which write to stdout by design.
//! 2. **Nothing here writes anything.** Not a file, not a contract, not a run.
//!    An assistant that could register terms on the author's behalf would make
//!    the assistant the author of terms nobody agreed to. Registering stays a
//!    deliberate act at a command line.
//! 3. **The credential rules do not change because the caller is a model.** The
//!    key still has one home in `PLEXUSPACT_API_KEY`, `--offline` and
//!    `PLEXUSPACT_NO_NETWORK` still win over everything, and a key is still
//!    never sent over plain http to anywhere but localhost. The cloud tools ask
//!    [`crate::push::resolve_endpoint`] exactly as every other caller does.

mod tools;

use std::io::{BufRead, Write};

use serde_json::{json, Value};

/// MCP revisions this server can speak, newest first.
///
/// A client that asks for one of these gets it back and both sides know where
/// they are. A client that asks for anything else — including something newer —
/// is told what we do speak and decides for itself whether to continue, which
/// is what the specification asks for and is also the only honest answer.
const SUPPORTED: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// Run the server until stdin closes.
///
/// `offline` is the global `--offline` flag and `api` the resolved `--api`
/// value; both are settled once here and handed to every cloud tool, so a
/// session cannot drift from the switches it was started under.
pub fn serve(api: Option<String>, offline: bool) -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let ctx = tools::Context { api, offline };

    eprintln!(
        "plexuspact mcp: ready on stdio ({} tools, protocol {})",
        tools::COUNT,
        SUPPORTED[0]
    );

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Some(response) = handle_line(&line, &ctx) else {
            continue;
        };
        writeln!(stdout, "{response}")?;
        stdout.flush()?;
    }
    Ok(())
}

/// Turn one line of input into at most one line of output.
///
/// `None` means "say nothing", which is the correct response to a notification
/// and to a reply we were sent by mistake. A JSON-RPC notification carries no
/// id, and answering one is a protocol error rather than a harmless extra.
fn handle_line(line: &str, ctx: &tools::Context) -> Option<String> {
    let message: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Some(error_response(
                Value::Null,
                -32700,
                &format!("could not parse the request as JSON: {e}"),
            ));
        }
    };
    if !message.is_object() {
        return Some(error_response(
            Value::Null,
            -32600,
            "expected one JSON-RPC object per line; batches are not supported",
        ));
    }
    let id = message.get("id").cloned();
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        // No method and an id means this is somebody's response to us. We send
        // no requests, so it is not ours to answer.
        return id.map(|id| error_response(id, -32600, "expected a `method`"));
    };
    let params = message.get("params").cloned().unwrap_or(Value::Null);

    // Notifications (no id) are acted on and never answered.
    let id = id?;

    match method {
        "initialize" => Some(ok_response(id, initialize(&params))),
        "ping" => Some(ok_response(id, json!({}))),
        "tools/list" => Some(ok_response(id, json!({ "tools": tools::list() }))),
        "tools/call" => Some(match tools::call(&params, ctx) {
            Ok(result) => ok_response(id, result),
            Err(message) => error_response(id, -32602, &message),
        }),
        // Advertised in neither the capabilities nor the tool list, so a client
        // asking for one of these has guessed. Say so, rather than returning an
        // empty list, which reads as "there are none of those here".
        other => Some(error_response(
            id,
            -32601,
            &format!("this server implements tools only; `{other}` is not available"),
        )),
    }
}

/// The handshake.
fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = match asked {
        Some(v) if SUPPORTED.contains(&v) => v,
        _ => SUPPORTED[0],
    };
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": "plexuspact",
            "title": "PlexusPact data contracts",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": INSTRUCTIONS,
    })
}

/// What the client is told this server is for.
///
/// Written for the assistant that will read it, and deliberately about *when*
/// to ask rather than about what the tools return: a schema explains itself,
/// but nothing in a schema tells a model that the moment to ask is before the
/// edit rather than after the pipeline fails.
const INSTRUCTIONS: &str = "\
PlexusPact validates datasets against versioned data contracts.

Before changing anything that produces a dataset — a schema, a transformation, \
a column's type or name, a dbt model — call `preflight_contract` with the \
proposed contract. It answers with what is in force today, which changes break \
it, and which named downstream consumers those changes reach. A change that \
breaks a consumer is not a change to make quietly.

`explain_contract` and `diff_contracts` read local files and need no \
credentials. `preflight_contract` and `dataset_status` ask a PlexusPact Cloud \
project and need PLEXUSPACT_API_KEY in the environment.

Nothing here writes: no contract is registered and no run is reported. \
Registering a version is a deliberate act by a person.";

/// A successful JSON-RPC response.
fn ok_response(id: Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

/// A JSON-RPC error response.
fn error_response(id: Value, code: i32, message: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> tools::Context {
        tools::Context {
            api: None,
            offline: true,
        }
    }

    fn call(line: &str) -> Value {
        let out = handle_line(line, &ctx()).unwrap_or_default();
        serde_json::from_str(&out).unwrap_or(Value::Null)
    }

    #[test]
    fn initialize_answers_with_a_version_both_sides_speak() {
        let v = call(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#,
        );
        assert_eq!(v["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(v["result"]["serverInfo"]["name"], "plexuspact");
    }

    #[test]
    fn an_unknown_protocol_version_gets_ours_rather_than_a_refusal() {
        // The client decides whether it can live with the answer. Refusing the
        // handshake outright would strand every client newer than this binary.
        let v = call(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2099-01-01"}}"#,
        );
        assert_eq!(v["result"]["protocolVersion"], SUPPORTED[0]);
    }

    #[test]
    fn a_notification_is_never_answered() {
        let line = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        assert!(handle_line(line, &ctx()).is_none());
    }

    #[test]
    fn tools_are_listed_with_schemas() {
        let v = call(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let listed = v["result"]["tools"].as_array().cloned().unwrap_or_default();
        assert_eq!(listed.len(), tools::COUNT);
        for tool in listed {
            assert!(tool["name"].is_string());
            assert!(tool["description"].is_string());
            assert_eq!(tool["inputSchema"]["type"], "object");
        }
    }

    #[test]
    fn a_batch_is_refused_in_words_that_say_why() {
        let v = call(r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#);
        assert_eq!(v["error"]["code"], -32600);
        assert!(v["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("batches"));
    }

    #[test]
    fn an_unimplemented_method_says_what_this_server_is() {
        let v = call(r#"{"jsonrpc":"2.0","id":3,"method":"resources/list"}"#);
        assert_eq!(v["error"]["code"], -32601);
        assert!(v["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("tools only"));
    }

    #[test]
    fn malformed_json_is_a_parse_error_not_a_crash() {
        let v = call("{not json");
        assert_eq!(v["error"]["code"], -32700);
    }
}
