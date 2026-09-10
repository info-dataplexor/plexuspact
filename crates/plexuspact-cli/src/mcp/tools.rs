//! The five questions, as MCP tools.
//!
//! Two of them are local and need nothing: what does this contract promise, and
//! what changed between these two. One reads a data file. Two ask the registry,
//! and those two are the reason this exists — an assistant can find out *before*
//! it writes the change which version is in force and who reads the column it
//! was about to rename.
//!
//! Every answer comes back twice: as prose in `content`, because that is what
//! the model actually reads, and as JSON in `structuredContent`, because that is
//! what a client can render or a script can act on. The prose is the primary
//! one. A tool that returns only a JSON blob makes the model do the summarising,
//! and a model summarising a blast radius is a model that can round "billing
//! breaks" down to "some changes were detected".

use std::path::Path;

use plexuspact_contract::{
    diff, odcs, validate, Change, ColumnDef, Contract, Impact, LintLevel, Stability,
};
use plexuspact_core::{run_check_full, CheckOptions, InputOverrides, ReferenceSets};
use plexuspact_report::{render_human, HumanOptions};
use serde_json::{json, Value};

use crate::preflight;
use crate::push::{self, Reporting};

/// How many tools this server offers. Asserted against [`list`] so the number
/// in the startup banner cannot drift from the number in the list.
pub const COUNT: usize = 5;

/// The switches the session was started under, settled once in [`super::serve`].
#[derive(Debug, Clone)]
pub struct Context {
    /// The resolved `--api` value, or `None` for the default root.
    pub api: Option<String>,
    /// The global `--offline` flag.
    pub offline: bool,
}

/// The tool list, as `tools/list` returns it.
pub fn list() -> Vec<Value> {
    vec![
        json!({
            "name": "explain_contract",
            "title": "Explain a data contract",
            "description":
                "Read a contract file and say what it promises: the dataset, its owner, every \
                 column with its type and rules, which columns are on their way out, and which \
                 downstream consumers have declared that they read it. Use this before writing \
                 or editing code that produces the dataset.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to a contract.yaml (or an ODCS document, which is converted)." },
                    "content": { "type": "string", "description": "The contract as text, instead of a path." }
                },
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false },
        }),
        json!({
            "name": "check_data",
            "title": "Validate a data file against a contract",
            "description":
                "Run the contract against a data file (CSV, TSV, Parquet, NDJSON, JSON, Excel, \
                 XML, fixed-width) and report every check that failed, with sample failing rows. \
                 Local only: nothing is uploaded and no run is recorded.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "data_path": { "type": "string", "description": "The data file to validate." },
                    "contract_path": { "type": "string", "description": "The contract to validate it against." },
                    "strict": { "type": "boolean", "description": "Treat warn-severity failures as failures. Default false." },
                    "redact_samples": { "type": "boolean", "description": "Mask the sample values and keep only row numbers. Default false. Use it when the data is sensitive." }
                },
                "required": ["data_path", "contract_path"],
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false },
        }),
        json!({
            "name": "diff_contracts",
            "title": "Compare two contracts",
            "description":
                "Classify every difference between two contracts as breaking, semantic \
                 (the meaning changed while the rules did not), non-breaking, or cosmetic. \
                 Local only. To find out who actually breaks, use preflight_contract instead.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "before_path": { "type": "string", "description": "The earlier contract, as a path." },
                    "before_content": { "type": "string", "description": "The earlier contract, as text." },
                    "after_path": { "type": "string", "description": "The proposed contract, as a path." },
                    "after_content": { "type": "string", "description": "The proposed contract, as text." }
                },
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false },
        }),
        json!({
            "name": "preflight_contract",
            "title": "Ask the registry what this change would do",
            "description":
                "Judge a proposed contract against the version in force in PlexusPact Cloud and \
                 name the downstream consumers a breaking change would reach. This is the one to \
                 call before editing anything that produces a dataset — a schema, a \
                 transformation, a column's type or name, a dbt model. Writes nothing: no \
                 version is registered. Needs PLEXUSPACT_API_KEY.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "The proposed contract, as a path." },
                    "content": { "type": "string", "description": "The proposed contract, as text." },
                    "dataset": { "type": "string", "description": "The registered dataset to judge against, when this version renames it. Without it a rename reads as a brand new dataset with nothing to break." }
                },
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": true },
        }),
        json!({
            "name": "dataset_status",
            "title": "Recent deliveries of a dataset",
            "description":
                "How the deliveries have been going. With no dataset, lists every dataset in the \
                 project with its most recent run. With one, returns that dataset's recent runs, \
                 newest first. Needs PLEXUSPACT_API_KEY.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "dataset": { "type": "string", "description": "The dataset to report on. Omit to list all of them." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 200, "description": "How many runs to return. Default 10." }
                },
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": true },
        }),
    ]
}

/// Run one `tools/call`.
///
/// `Err` is for the protocol — a call with no name, or a name this server does
/// not have. Everything that goes wrong *inside* a tool comes back as `Ok` with
/// `isError` set, because that is the answer the model needs to read and act on
/// rather than a transport fault it can only retry.
pub fn call(params: &Value, ctx: &Context) -> Result<Value, String> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "a tools/call needs a `name`".to_owned())?;
    let args = params.get("arguments").cloned().unwrap_or(json!({}));

    let outcome = match name {
        "explain_contract" => explain(&args),
        "check_data" => check(&args),
        "diff_contracts" => compare(&args),
        "preflight_contract" => flight(&args, ctx),
        "dataset_status" => status(&args, ctx),
        other => {
            return Err(format!(
                "no tool named `{other}`; call tools/list for what this server has"
            ))
        }
    };
    Ok(match outcome {
        Ok(result) => result,
        Err(message) => failed(&message),
    })
}

// ─────────────────────────────── the tools ──────────────────────────────

/// `explain_contract`
fn explain(args: &Value) -> Result<Value, String> {
    let (contract, _, notes) = load(args, "path", "content")?;
    let mut text = describe(&contract);
    for note in &notes {
        text.push_str(&format!("\nNote: {note}\n"));
    }
    for finding in validate(&contract) {
        let level = match finding.level {
            LintLevel::Error => "problem",
            LintLevel::Warning => "warning",
        };
        text.push_str(&format!("\n{level}: {}\n", finding.message));
    }
    let structured = serde_json::to_value(&contract)
        .map_err(|e| format!("the contract could not be rendered as JSON: {e}"))?;
    Ok(answer(text, json!({ "contract": structured })))
}

/// `check_data`
fn check(args: &Value) -> Result<Value, String> {
    let data_path = string(args, "data_path").ok_or("`data_path` is required")?;
    let contract_path = string(args, "contract_path").ok_or("`contract_path` is required")?;
    let strict = flag(args, "strict");

    let (contract, bytes) = read_contract(Path::new(&contract_path))?;
    let blocking: Vec<_> = validate(&contract)
        .into_iter()
        .filter(|f| f.level == LintLevel::Error)
        .collect();
    if !blocking.is_empty() {
        let lines: Vec<_> = blocking.iter().map(|f| f.message.clone()).collect();
        return Err(format!(
            "{contract_path} cannot be used as it stands:\n- {}",
            lines.join("\n- ")
        ));
    }

    let options = CheckOptions {
        sample_failures: 5,
        redact_samples: flag(args, "redact_samples"),
        now: None,
        references: ReferenceSets::none(),
        profile: false,
    };
    let (result, _) = run_check_full(
        contract,
        bytes.into_bytes(),
        Some(contract_path.clone()),
        &data_path,
        &InputOverrides::default(),
        &options,
        env!("CARGO_PKG_VERSION"),
    )
    .map_err(|e| format!("{data_path} could not be checked: {e}"))?;

    let human = render_human(
        &result,
        &HumanOptions {
            color: false,
            max_samples: 5,
        },
    );
    let failing = result.is_failure(strict);
    let verdict = if failing {
        "The data does not keep this contract."
    } else {
        "The data keeps this contract."
    };
    let structured = serde_json::to_value(&result)
        .map_err(|e| format!("the result could not be rendered as JSON: {e}"))?;
    Ok(answer(format!("{verdict}\n\n{human}"), structured))
}

/// `diff_contracts`
fn compare(args: &Value) -> Result<Value, String> {
    let (before, _, _) = load(args, "before_path", "before_content")?;
    let (after, _, _) = load(args, "after_path", "after_content")?;
    let changes = diff(&before, &after);
    Ok(answer(
        render_changes(&changes),
        json!({
            "changes": changes.iter().map(change_json).collect::<Vec<_>>(),
            "breaking": changes.iter().filter(|c| c.impact == Impact::Breaking).count(),
        }),
    ))
}

/// `preflight_contract`
fn flight(args: &Value, ctx: &Context) -> Result<Value, String> {
    let (_, yaml, _) = load(args, "path", "content")?;
    let dest = destination(ctx, "contracts/preflight")?;
    let result = preflight::fetch(&dest, &yaml, string(args, "dataset").as_deref())
        .map_err(|e| describe_push_error(&e))?;

    let mut text = preflight::to_markdown(&result);
    if result.is_breaking() {
        text.push_str(
            "\nThis is a breaking change. Say so before making it, name the consumers above, \
             and offer the alternatives: keep the old column alongside the new one, or set a \
             migration window with a date.\n",
        );
    }
    Ok(answer(
        text,
        json!({
            "verdict": result.verdict,
            "dataset": result.dataset,
            "breaking": result.breaking,
            "requires_review": result.requires_review,
            "consumers_affected": result.consumers.iter().map(|(n, _, _)| n.clone()).collect::<Vec<_>>(),
            "consumers_declared": result.declared,
        }),
    ))
}

/// `dataset_status`
fn status(args: &Value, ctx: &Context) -> Result<Value, String> {
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(10)
        .clamp(1, 200);

    let (endpoint, query) = match string(args, "dataset") {
        Some(dataset) if !dataset.trim().is_empty() => (
            format!("datasets/{}/runs", path_segment(dataset.trim())),
            Some(format!("limit={limit}")),
        ),
        _ => ("datasets".to_owned(), None),
    };
    let mut dest = destination(ctx, &endpoint)?;
    if let Some(q) = query {
        dest = dest.with_query(&q);
    }
    let body = push::get_json(&dest).map_err(|e| describe_push_error(&e))?;
    let parsed: Value = serde_json::from_str(&body)
        .map_err(|_| "the registry replied with something that is not JSON".to_owned())?;
    Ok(answer(render_status(&parsed), parsed))
}

// ───────────────────────────── shared pieces ────────────────────────────

/// Resolve where to call, turning the two "not calling anywhere" outcomes into
/// the sentence that says what to do about it.
///
/// This is the only place the cloud tools reach the network, and it goes
/// through [`push::resolve_endpoint`] so the offline switch, the single home
/// for the token, and the refusal to send a key over plain http apply here
/// exactly as they do at a command line.
fn destination(ctx: &Context, endpoint: &str) -> Result<push::Destination, String> {
    match push::resolve_endpoint(ctx.api.as_deref(), ctx.offline, endpoint) {
        Ok(Reporting::To(dest)) => Ok(dest),
        Ok(Reporting::Off) => Err(format!(
            "this needs a PlexusPact Cloud project: set {} in the environment this server was \
             started from. The local tools — explain_contract, check_data, diff_contracts — \
             work without it.",
            push::TOKEN_VAR
        )),
        Ok(Reporting::Suppressed(why)) => Err(format!(
            "this server was started with no network access ({why}), so the registry cannot be \
             asked. explain_contract, check_data and diff_contracts still work."
        )),
        Err(e) => Err(describe_push_error(&e)),
    }
}

/// A push error, with its hint, as one sentence a model can act on.
fn describe_push_error(e: &push::PushError) -> String {
    match &e.hint {
        Some(hint) => format!("{}. {hint}", e.message),
        None => e.message.clone(),
    }
}

/// Load a contract from whichever of the two argument names carries it.
///
/// Returns the parsed contract, the exact text the registry should be sent
/// (converted, if what arrived was ODCS), and anything the conversion could not
/// carry across.
fn load(
    args: &Value,
    path_key: &str,
    content_key: &str,
) -> Result<(Contract, String, Vec<String>), String> {
    if let Some(inline) = string(args, content_key) {
        return parse(&inline, content_key);
    }
    if let Some(path) = string(args, path_key) {
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
        return parse(&text, &path);
    }
    Err(format!("give either `{path_key}` or `{content_key}`"))
}

/// Read and parse a contract file, keeping the bytes the checker needs.
fn read_contract(path: &Path) -> Result<(Contract, String), String> {
    let display = path.display().to_string();
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read contract {display}: {e}"))?;
    let (contract, yaml, _) = parse(&text, &display)?;
    Ok((contract, yaml))
}

/// Parse contract text, accepting an ODCS document wherever a contract is
/// accepted — the same rule the command line follows, so a repository that
/// keeps its contracts as ODCS is not a repository where these tools go quiet.
fn parse(text: &str, origin: &str) -> Result<(Contract, String, Vec<String>), String> {
    let (yaml, notes) = if odcs::looks_like_odcs(text) {
        odcs::import_yaml(text).map_err(|e| format!("{origin}: {e}"))?
    } else {
        (text.to_owned(), Vec::new())
    };
    let contract = plexuspact_contract::parse_str(&yaml, origin)
        .map_err(|e| format!("{origin} is not a contract this version can read: {e}"))?;
    Ok((contract, yaml, notes))
}

/// A successful tool result: prose first, JSON alongside.
fn answer(text: String, structured: Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": structured,
    })
}

/// A tool that could not do its job. `isError` rather than a JSON-RPC error, so
/// the model is handed the reason and can fix it.
fn failed(message: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": message }],
        "isError": true,
    })
}

/// A string argument, empty treated as absent.
fn string(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|s| !s.is_empty())
}

/// A boolean argument, absent meaning false.
fn flag(args: &Value, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool) == Some(true)
}

/// Percent-encode one path segment.
///
/// A dataset name is whatever the supplier called it, which has included a
/// space, a slash and a hash in the wild. Encoding here rather than trusting
/// URL assembly keeps `orders/eu` one segment instead of two.
fn path_segment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

// ──────────────────────────────── rendering ─────────────────────────────

/// A contract in prose.
///
/// Deliberately not the YAML: the model can already read YAML, and what it
/// cannot read out of YAML is which of these facts matter. Leading with the
/// consumers and marking the deprecated columns puts the two things that change
/// what an author does at the top, where a serialisation would have buried them
/// in declaration order.
fn describe(c: &Contract) -> String {
    let mut out = format!("Dataset `{}`", c.dataset);
    if let Some(owner) = &c.owner {
        out.push_str(&format!(", owned by {owner}"));
    }
    out.push_str(".\n");
    if let Some(description) = &c.description {
        out.push_str(&format!("{description}\n"));
    }
    if let Some(version) = &c.version {
        out.push_str(&format!("Contract version {version}.\n"));
    }
    if !c.primary_key.is_empty() {
        out.push_str(&format!(
            "One row is identified by {}.\n",
            c.primary_key.join(" + ")
        ));
    }

    out.push_str("\nWho reads it: ");
    if c.consumers.is_empty() {
        out.push_str(
            "nobody has declared what they read from this dataset. That is an unanswered \
             question, not permission to change it freely.\n",
        );
    } else {
        out.push('\n');
        for consumer in &c.consumers {
            let reads = if consumer.reads_whole_dataset() {
                "the whole dataset (nothing narrower was declared)".to_owned()
            } else {
                consumer.reads.join(", ")
            };
            let tier = match consumer.tier {
                Some(t) => format!(", tier {t}"),
                None => String::new(),
            };
            out.push_str(&format!("  {} — reads {reads}{tier}\n", consumer.name));
        }
    }

    out.push_str(&format!("\nColumns ({}):\n", c.columns.len()));
    for (name, column) in &c.columns {
        out.push_str(&describe_column(name, column));
    }

    if !c.dataset_checks.is_empty() {
        out.push_str("\nRules over the whole dataset:\n");
        for check in &c.dataset_checks {
            out.push_str(&format!(
                "  {}
",
                render_rule(&serde_json::to_value(check).unwrap_or(Value::Null))
            ));
        }
    }

    if let Some(migration) = &c.migration {
        out.push_str(&format!(
            "\nA migration window is open until {}.",
            migration.window_ends
        ));
        match &migration.note {
            Some(note) => out.push_str(&format!(" {note}\n")),
            None => out.push('\n'),
        }
    }

    if !c.settings.allow_extra_columns || c.settings.columns_exact {
        out.push_str("\nUndeclared columns are not accepted in this feed.\n");
    }
    out
}

/// One column, with the two facts an author has to see: whether it may be null,
/// and whether it is on its way out.
fn describe_column(name: &str, column: &ColumnDef) -> String {
    let mut line = format!("  {name}: {}", column.r#type);
    if column.required {
        line.push_str(", never null");
    }
    match column.stability {
        Stability::Stable => {}
        Stability::Beta => line.push_str(", beta (offered, not promised)"),
        Stability::Deprecated => line.push_str(", DEPRECATED"),
    }
    if let Some(sunset) = &column.sunset {
        line.push_str(&format!(", goes away after {sunset}"));
    }
    if let Some(pii) = &column.pii {
        line.push_str(&format!(", personal data ({pii:?})"));
    }
    line.push('\n');
    if let Some(description) = &column.description {
        line.push_str(&format!("      {description}\n"));
    }
    if !column.checks.is_empty() {
        let rules: Vec<String> = column
            .checks
            .iter()
            .map(|check| render_rule(&serde_json::to_value(check).unwrap_or(Value::Null)))
            .collect();
        line.push_str(&format!("      rules: {}\n", rules.join("; ")));
    }
    line
}

/// A check as one short phrase.
///
/// Goes through serde rather than matching every variant: the check vocabulary
/// grows, and a `match` here would silently render next year's check as nothing
/// at all — which in a contract summary reads as a rule that does not exist.
fn render_rule(value: &Value) -> String {
    match value {
        Value::Null => "(unreadable rule)".to_owned(),
        Value::String(s) => s.clone(),
        Value::Object(map) => rule_parts(map).join(" "),
        other => other.to_string(),
    }
}

/// One phrase per entry in a check's map.
///
/// A nested map becomes `key(a=1, b=2)` rather than raw JSON: the reader here
/// is a model deciding whether a rule is in its way, and braces and quotes are
/// noise it has to parse back out before it can.
fn rule_parts(map: &serde_json::Map<String, Value>) -> Vec<String> {
    map.iter()
        .map(|(k, v)| match v {
            Value::Null => k.clone(),
            Value::String(s) => format!("{k}={s}"),
            Value::Object(inner) => format!("{k}({})", rule_parts(inner).join(", ")),
            other => format!("{k}={other}"),
        })
        .collect()
}

/// The classified changes, breaking ones first.
fn render_changes(changes: &[Change]) -> String {
    if changes.is_empty() {
        return "Nothing changed between these two contracts.".to_owned();
    }
    let breaking = changes
        .iter()
        .filter(|c| c.impact == Impact::Breaking)
        .count();
    let semantic = changes
        .iter()
        .filter(|c| c.impact == Impact::Semantic)
        .count();

    let mut out = match (breaking, semantic) {
        (0, 0) => {
            "Nothing here tightens the terms; data that passed before still passes.\n".to_owned()
        }
        (0, n) => format!(
            "Nothing tightens, but {n} change(s) alter what a column means. Every number built \
             on the old meaning is now wrong in a way no check will catch, so this has to be \
             announced.\n"
        ),
        (n, _) => format!(
            "{n} change(s) tighten terms somebody may be relying on. Before making them, find \
             out who reads this dataset — preflight_contract answers that.\n"
        ),
    };
    out.push('\n');

    let mut ordered: Vec<&Change> = changes.iter().collect();
    ordered.sort_by_key(|c| match c.impact {
        Impact::Breaking => 0,
        Impact::Semantic => 1,
        Impact::NonBreaking => 2,
        Impact::Cosmetic => 3,
    });
    for change in ordered {
        let label = match change.impact {
            Impact::Breaking => "BREAKS",
            Impact::Semantic => "MEANING",
            Impact::NonBreaking => "safe",
            Impact::Cosmetic => "cosmetic",
        };
        out.push_str(&format!(
            "  {label}  {}: {}\n",
            change.path, change.description
        ));
    }
    out
}

/// The registry's dataset reply in prose.
///
/// Loose on purpose, like the preflight reader: a CLI one release behind should
/// say what it understood rather than refuse a reply that grew a field.
fn render_status(v: &Value) -> String {
    let items = v
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    if let Some(dataset) = v.get("dataset").and_then(Value::as_str) {
        if items.is_empty() {
            return format!(
                "`{dataset}` has no recorded runs. That is silence, not a pass — nothing has \
                 been delivered and checked, or nothing has been reported."
            );
        }
        let mut out = format!(
            "`{dataset}` — {} recent run(s), newest first:\n",
            items.len()
        );
        for run in &items {
            let field = |k: &str| run.get(k).and_then(Value::as_str).unwrap_or("");
            out.push_str(&format!(
                "  {}  {}  {}\n",
                field("started_at"),
                field("status"),
                field("summary")
            ));
        }
        return out;
    }

    if items.is_empty() {
        return "This project has no datasets with recorded runs yet.".to_owned();
    }
    let mut out = format!("{} dataset(s) in this project:\n", items.len());
    for entry in &items {
        let field = |k: &str| entry.get(k).and_then(Value::as_str).unwrap_or("");
        out.push_str(&format!(
            "  {}  last run {} at {}\n",
            field("dataset"),
            field("last_status"),
            field("last_started_at")
        ));
    }
    out
}

/// One change as JSON, in the vocabulary the rest of the product uses.
fn change_json(c: &Change) -> Value {
    json!({
        "impact": match c.impact {
            Impact::Breaking => "breaking",
            Impact::Semantic => "semantic",
            Impact::NonBreaking => "non_breaking",
            Impact::Cosmetic => "cosmetic",
        },
        "path": c.path,
        "description": c.description,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
apiVersion: v1
dataset: orders
owner: data-platform
description: One row per accepted order.
primary_key: [order_id]
consumers:
  - name: billing
    reads: [amount]
    tier: 1
columns:
  order_id:
    type: string
    required: true
  amount:
    type: float
    required: true
    description: Gross order value in minor units.
    checks:
      - min: 0
  region:
    type: string
    stability: deprecated
    sunset: "2026-12-01"
dataset_checks:
  - row_count_min: 1
  - null_ratio_max: { column: amount, ratio: 0.01 }
"#;

    fn contract() -> Contract {
        plexuspact_contract::parse_str(SAMPLE, "sample.yaml").unwrap()
    }

    fn ctx() -> Context {
        Context {
            api: None,
            offline: true,
        }
    }

    fn text_of(result: &Value) -> String {
        result["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    }

    #[test]
    fn every_listed_tool_is_callable_and_the_count_matches() {
        let listed = list();
        assert_eq!(listed.len(), COUNT);
        for tool in &listed {
            let name = tool["name"].as_str().unwrap_or_default();
            let result = call(&json!({ "name": name, "arguments": {} }), &ctx());
            // Missing arguments is a tool error, never an unknown-tool error.
            assert!(result.is_ok(), "{name} is listed but not dispatched");
        }
    }

    #[test]
    fn rules_over_the_whole_dataset_are_named_not_swallowed() {
        // A check the renderer does not recognise must still appear as a rule.
        // Rendering it as nothing would tell a reader the dataset promises less
        // than it does, which is the one mistake a summary must never make.
        let text = describe(&contract());
        assert!(text.contains("Rules over the whole dataset:"), "{text}");
        assert!(text.contains("row_count_min=1"), "{text}");
        assert!(
            text.contains("null_ratio_max(column=amount, ratio=0.01)"),
            "{text}"
        );
    }

    #[test]
    fn an_unknown_tool_is_a_protocol_error_not_a_silent_empty_answer() {
        let err = call(&json!({ "name": "delete_everything" }), &ctx()).unwrap_err();
        assert!(err.contains("no tool named"), "{err}");
    }

    #[test]
    fn explaining_a_contract_leads_with_who_reads_it() {
        let result = explain(&json!({ "content": SAMPLE })).unwrap();
        let text = text_of(&result);
        let who = text.find("Who reads it").unwrap();
        let columns = text.find("Columns (3)").unwrap();
        assert!(who < columns, "{text}");
        assert!(text.contains("billing — reads amount, tier 1"), "{text}");
    }

    #[test]
    fn a_column_on_its_way_out_says_so_and_says_when() {
        let text = text_of(&explain(&json!({ "content": SAMPLE })).unwrap());
        assert!(
            text.contains("region: string, DEPRECATED, goes away after 2026-12-01"),
            "{text}"
        );
    }

    #[test]
    fn no_declared_consumers_is_never_rendered_as_nobody_affected() {
        let bare = "apiVersion: v1\ndataset: orders\ncolumns:\n  id:\n    type: string\n";
        let text = text_of(&explain(&json!({ "content": bare })).unwrap());
        assert!(text.contains("unanswered question"), "{text}");
    }

    #[test]
    fn a_dropped_column_is_reported_as_breaking() {
        let after = SAMPLE.replace("  amount:\n    type: float\n    required: true\n    description: Gross order value in minor units.\n    checks:\n      - min: 0\n", "");
        let result = compare(&json!({ "before_content": SAMPLE, "after_content": after })).unwrap();
        assert_eq!(result["structuredContent"]["breaking"], 1);
        let text = text_of(&result);
        assert!(text.contains("BREAKS"), "{text}");
        assert!(text.contains("preflight_contract"), "{text}");
    }

    #[test]
    fn breaking_changes_are_listed_before_cosmetic_ones() {
        let after = SAMPLE
            .replace("owner: data-platform", "owner: platform-team")
            .replace("    type: float\n", "    type: string\n");
        let text = text_of(
            &compare(&json!({ "before_content": SAMPLE, "after_content": after })).unwrap(),
        );
        let breaks = text.find("BREAKS").unwrap();
        let cosmetic = text.find("cosmetic").unwrap();
        assert!(breaks < cosmetic, "{text}");
    }

    #[test]
    fn an_unreadable_contract_is_a_tool_error_the_model_can_read() {
        let result = call(
            &json!({ "name": "explain_contract", "arguments": { "content": "not: a: contract" } }),
            &ctx(),
        )
        .unwrap();
        assert_eq!(result["isError"], true);
        assert!(!text_of(&result).is_empty());
    }

    #[test]
    fn a_cloud_tool_offline_says_which_tools_still_work() {
        let result = call(
            &json!({ "name": "preflight_contract", "arguments": { "content": SAMPLE } }),
            &ctx(),
        )
        .unwrap();
        assert_eq!(result["isError"], true);
        let text = text_of(&result);
        assert!(text.contains("no network access"), "{text}");
        assert!(text.contains("diff_contracts"), "{text}");
    }

    #[test]
    fn a_dataset_name_with_a_slash_stays_one_path_segment() {
        assert_eq!(path_segment("orders/eu"), "orders%2Feu");
        assert_eq!(path_segment("daily orders"), "daily%20orders");
        assert_eq!(path_segment("orders_v2-1.0"), "orders_v2-1.0");
    }

    #[test]
    fn silence_from_the_registry_is_not_reported_as_a_pass() {
        let text = render_status(&json!({ "dataset": "orders", "items": [] }));
        assert!(text.contains("silence, not a pass"), "{text}");
    }

    #[test]
    fn rules_are_rendered_through_serde_so_new_kinds_still_appear() {
        let text = text_of(&explain(&json!({ "content": SAMPLE })).unwrap());
        assert!(text.contains("rules: min=0"), "{text}");
    }

    #[test]
    fn checking_a_file_reports_the_failing_rows() {
        let dir = tempfile::tempdir().unwrap();
        let contract_path = dir.path().join("contract.yaml");
        let data_path = dir.path().join("orders.csv");
        std::fs::write(&contract_path, SAMPLE).unwrap();
        std::fs::write(&data_path, "order_id,amount,region\na,-5,eu\nb,10,us\n").unwrap();

        let result = check(&json!({
            "data_path": data_path.display().to_string(),
            "contract_path": contract_path.display().to_string(),
        }))
        .unwrap();
        let text = text_of(&result);
        assert!(text.contains("does not keep this contract"), "{text}");
        assert_eq!(result["structuredContent"]["status"], "failed");
    }

    #[test]
    fn checking_a_file_that_keeps_the_contract_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let contract_path = dir.path().join("contract.yaml");
        let data_path = dir.path().join("orders.csv");
        std::fs::write(&contract_path, SAMPLE).unwrap();
        std::fs::write(&data_path, "order_id,amount,region\na,5,eu\n").unwrap();

        let text = text_of(
            &check(&json!({
                "data_path": data_path.display().to_string(),
                "contract_path": contract_path.display().to_string(),
            }))
            .unwrap(),
        );
        assert!(text.contains("keeps this contract"), "{text}");
    }
}
