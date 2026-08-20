//! Contract loading with human-friendly, span-labeled diagnostics.
//!
//! Parse errors implement [`miette::Diagnostic`]: they carry the contract
//! source, a labeled span pointing at the offending line/column, and a help
//! message with an example fix. Error-message rule (implementation guide §0):
//! *file, location, what was expected, one example fix.*

use std::fmt;
use std::path::Path;
use std::sync::OnceLock;

use miette::{Diagnostic, NamedSource, SourceSpan};
use regex::Regex;
use serde::de::{EnumAccess, Error as DeError, MapAccess, SeqAccess, VariantAccess, Visitor};
use serde::{Deserialize, Deserializer};
use thiserror::Error;

use crate::model::Contract;
use crate::suggest::did_you_mean;

/// Errors produced while loading or parsing a contract file.
#[derive(Debug, Error, Diagnostic)]
pub enum ParseError {
    /// The file could not be read at all.
    #[error("failed to read contract file `{path}`: {source}")]
    #[diagnostic(
        code(plexuspact::contract::io),
        help("check that the path exists and is readable")
    )]
    Io {
        /// Path that failed to open.
        path: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The contract text is not a valid contract.
    ///
    /// Boxed so `Result<Contract, ParseError>` stays small; the payload
    /// carries the whole source text for span rendering.
    #[error(transparent)]
    #[diagnostic(transparent)]
    Invalid(Box<InvalidContract>),
}

/// Details of an invalid contract: message, source, span, and fix help.
#[derive(Debug, Error, Diagnostic)]
#[error("{message}")]
#[diagnostic(code(plexuspact::contract::parse))]
pub struct InvalidContract {
    /// What went wrong, with a "did you mean" hint where possible.
    pub message: String,
    /// The contract source, so reports can show the offending line.
    #[source_code]
    pub src: NamedSource<String>,
    /// Byte span of the offending token, when known.
    #[label("{label}")]
    pub span: Option<SourceSpan>,
    /// Short label shown under the span.
    pub label: String,
    /// How to fix it, with an example.
    #[help]
    pub help: Option<String>,
}

/// Parses a contract from a YAML string.
///
/// `source_name` is used in diagnostics (usually the file path; use something
/// like `"<stdin>"` for piped input).
pub fn parse_str(src: &str, source_name: &str) -> Result<Contract, ParseError> {
    if src.trim().is_empty() {
        return Err(ParseError::Invalid(Box::new(InvalidContract {
            message: "the contract is empty".to_string(),
            src: NamedSource::new(source_name, src.to_string()),
            span: None,
            label: String::new(),
            help: Some(
                "a minimal contract looks like:\n\
                 apiVersion: v1\n\
                 dataset: my_table\n\
                 columns:\n  \
                   id: { type: string, required: true }"
                    .to_string(),
            ),
        })));
    }
    // Pre-pass: YAML keeps only the *last* of duplicate mapping keys, which
    // would silently drop columns or checks (doc 04 task 1.5: "duplicate
    // column ⇒ typed error"). Reject duplicates anywhere in the document.
    serde_yaml::from_str::<DupCheck>(src).map_err(|e| convert_yaml_error(&e, src, source_name))?;
    serde_yaml::from_str(src).map_err(|e| convert_yaml_error(&e, src, source_name))
}

/// Parses a contract from a file on disk.
pub fn parse_file(path: impl AsRef<Path>) -> Result<Contract, ParseError> {
    let path = path.as_ref();
    let src = std::fs::read_to_string(path).map_err(|source| ParseError::Io {
        path: path.display().to_string(),
        source,
    })?;
    parse_str(&src, &path.display().to_string())
}

/// Zero-sized value whose deserialization walks the entire YAML tree and
/// fails on duplicate mapping keys (anywhere: top level, `columns`, nested
/// check maps). YAML itself would keep only the last entry.
struct DupCheck;

impl<'de> Deserialize<'de> for DupCheck {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(DupCheckVisitor)
    }
}

struct DupCheckVisitor;

impl<'de> Visitor<'de> for DupCheckVisitor {
    type Value = DupCheck;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any YAML value")
    }

    fn visit_bool<E: DeError>(self, _: bool) -> Result<DupCheck, E> {
        Ok(DupCheck)
    }

    fn visit_i64<E: DeError>(self, _: i64) -> Result<DupCheck, E> {
        Ok(DupCheck)
    }

    fn visit_u64<E: DeError>(self, _: u64) -> Result<DupCheck, E> {
        Ok(DupCheck)
    }

    fn visit_f64<E: DeError>(self, _: f64) -> Result<DupCheck, E> {
        Ok(DupCheck)
    }

    fn visit_str<E: DeError>(self, _: &str) -> Result<DupCheck, E> {
        Ok(DupCheck)
    }

    fn visit_unit<E: DeError>(self) -> Result<DupCheck, E> {
        Ok(DupCheck)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<DupCheck, A::Error> {
        while seq.next_element::<DupCheck>()?.is_some() {}
        Ok(DupCheck)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<DupCheck, A::Error> {
        let mut seen: Vec<serde_yaml::Value> = Vec::new();
        while let Some(key) = map.next_key::<serde_yaml::Value>()? {
            if seen.contains(&key) {
                return Err(A::Error::custom(format!(
                    "duplicate key `{}` in mapping",
                    yaml_key_repr(&key)
                )));
            }
            seen.push(key);
            map.next_value::<DupCheck>()?;
        }
        Ok(DupCheck)
    }

    fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<DupCheck, A::Error> {
        // YAML `!tag value` forms: recurse into the tagged value.
        let (_, variant) = data.variant::<serde_yaml::Value>()?;
        variant.newtype_variant::<DupCheck>()?;
        Ok(DupCheck)
    }
}

/// Renders a mapping key for the duplicate-key error message.
fn yaml_key_repr(key: &serde_yaml::Value) -> String {
    match key {
        serde_yaml::Value::String(s) => s.clone(),
        other => serde_yaml::to_string(other)
            .map(|s| s.trim_end().to_string())
            .unwrap_or_else(|_| "<key>".to_string()),
    }
}

/// Converts a `serde_yaml` error into a [`ParseError`] with span and hints.
fn convert_yaml_error(err: &serde_yaml::Error, src: &str, source_name: &str) -> ParseError {
    let raw = strip_location_suffix(&err.to_string());
    let (message, label, help) = enrich_message(&raw);
    let span = err.location().map(|loc| token_span(src, loc.index()));
    ParseError::Invalid(Box::new(InvalidContract {
        message,
        src: NamedSource::new(source_name, src.to_string()),
        span,
        label,
        help,
    }))
}

/// Removes serde_yaml's ` at line L column C` fragments (the labeled span
/// already shows the location).
fn strip_location_suffix(msg: &str) -> String {
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\s+at (line \d+ column \d+|position \d+)").ok());
    match re {
        Some(re) => re.replace_all(msg, "").into_owned(),
        None => msg.to_string(),
    }
}

/// Byte span of the token starting at `index` (identifier-ish characters),
/// clamped to the source and at least one byte long.
fn token_span(src: &str, index: usize) -> SourceSpan {
    if src.is_empty() {
        return (0, 0).into();
    }
    let mut start = index.min(src.len() - 1);
    while start > 0 && !src.is_char_boundary(start) {
        start -= 1;
    }
    let len: usize = src[start..]
        .chars()
        .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .map(char::len_utf8)
        .sum();
    let len = len.clamp(1, src.len() - start);
    (start, len).into()
}

/// Extracts the backtick-quoted tokens from a serde error message.
fn backticked(msg: &str) -> Vec<String> {
    msg.split('`')
        .enumerate()
        .filter(|(i, _)| i % 2 == 1)
        .map(|(_, part)| part.to_string())
        .collect()
}

/// Splits serde_yaml's leading field-path prefix (`columns.age.checks[0]: …`)
/// off a message, if present.
fn split_path_prefix(msg: &str) -> (Option<&str>, &str) {
    if let Some((head, rest)) = msg.split_once(": ") {
        let head_is_path = !head.is_empty()
            && head
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | '[' | ']'));
        if head_is_path && !rest.is_empty() {
            return (Some(head), rest);
        }
    }
    (None, msg)
}

/// Turns a raw serde message into `(message, label, help)` with typo
/// suggestions and example fixes.
fn enrich_message(msg: &str) -> (String, String, Option<String>) {
    let (path, rest) = split_path_prefix(msg);
    let (mut message, label, help) = classify_message(rest);
    if let Some(p) = path {
        if !message.contains(p) {
            message = format!("{message} (at `{p}`)");
        }
    }
    (message, label, help)
}

/// Classifies a serde message with the path prefix already removed.
fn classify_message(msg: &str) -> (String, String, Option<String>) {
    if msg.starts_with("unknown field `") {
        let tokens = backticked(msg);
        if let Some((field, expected)) = tokens.split_first() {
            let expected_refs: Vec<&str> = expected.iter().map(String::as_str).collect();
            let mut message = format!("unknown key `{field}`");
            if let Some(suggestion) = did_you_mean(field, expected_refs.iter().copied()) {
                message.push_str(&format!(" — did you mean `{suggestion}`?"));
            }
            let help = if expected_refs.is_empty() {
                format!("remove `{field}`; no keys are allowed here")
            } else {
                format!("valid keys here are: {}", expected_refs.join(", "))
            };
            return (message, "unrecognized key".to_string(), Some(help));
        }
    }
    if msg.starts_with("unknown variant `") {
        let tokens = backticked(msg);
        if let Some((variant, expected)) = tokens.split_first() {
            let expected_refs: Vec<&str> = expected.iter().map(String::as_str).collect();
            // Only `apiVersion` has exactly the single variant `v1`.
            if expected_refs == ["v1"] {
                return (
                    format!("unsupported apiVersion `{variant}`; supported versions: v1"),
                    "unsupported version".to_string(),
                    Some("set `apiVersion: v1`".to_string()),
                );
            }
            let mut message = format!("unknown value `{variant}`");
            if let Some(suggestion) = did_you_mean(variant, expected_refs.iter().copied()) {
                message.push_str(&format!(" — did you mean `{suggestion}`?"));
            }
            let help = format!("valid values here are: {}", expected_refs.join(", "));
            return (message, "unrecognized value".to_string(), Some(help));
        }
    }
    if msg.starts_with("missing field `") {
        let tokens = backticked(msg);
        if let Some(field) = tokens.first() {
            return (
                format!("missing required key `{field}`"),
                "missing key in this block".to_string(),
                Some(missing_field_help(field)),
            );
        }
    }
    if msg.starts_with("duplicate key `") {
        return (
            msg.to_string(),
            "duplicated here".to_string(),
            Some(
                "remove or rename one of the duplicate entries; YAML would otherwise \
                 silently keep only the last one"
                    .to_string(),
            ),
        );
    }
    // serde's own type errors are `invalid type: …` / `invalid value: …`;
    // our check errors say `invalid value \`x\` for check …` and pass through
    // below with their embedded examples.
    if msg.starts_with("invalid type:")
        || msg.starts_with("invalid value:")
        || msg.starts_with("invalid length")
    {
        return (
            msg.to_string(),
            "unexpected value".to_string(),
            Some(
                "give the key a value of the expected type — numbers unquoted (`min: 18`), \
                 durations as strings (`max_age: 48h`), lists in `[...]`"
                    .to_string(),
            ),
        );
    }
    // Messages from our own check deserializers already carry examples
    // (`e.g.` / `examples:`) or name the check/severity involved.
    if msg.contains("check")
        || msg.contains("severity")
        || msg.contains("did you mean")
        || msg.contains("e.g.")
        || msg.contains("examples:")
    {
        return (msg.to_string(), "invalid check".to_string(), None);
    }
    (
        msg.to_string(),
        "syntax error here".to_string(),
        Some(
            "check YAML syntax: indentation is spaces (never tabs), keys end with `:`, \
             and strings with special characters need quotes"
                .to_string(),
        ),
    )
}

/// Fix-it help for a missing required key.
fn missing_field_help(field: &str) -> String {
    let example = match field {
        "apiVersion" => "apiVersion: v1",
        "dataset" => "dataset: user_signups",
        "type" => "type: string   # one of string|int|float|bool|date|datetime",
        "column" => "column: <column_name>",
        "ratio" => "ratio: 0.1",
        "expr" => r#"expr: "age >= 18""#,
        "max_age" => "max_age: 48h",
        "name" => "name: analytics-core",
        other => return format!("add `{other}: <value>` to this block"),
    };
    format!("add e.g. `{example}`")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    const MINIMAL: &str = "apiVersion: v1\ndataset: t\ncolumns:\n  id: { type: string }\n";

    fn err(src: &str) -> ParseError {
        parse_str(src, "test.yaml").unwrap_err()
    }

    fn msg(src: &str) -> String {
        err(src).to_string()
    }

    #[test]
    fn parses_minimal_contract() {
        let c = parse_str(MINIMAL, "test.yaml").unwrap();
        assert_eq!(c.dataset, "t");
        assert_eq!(c.columns.len(), 1);
    }

    #[test]
    fn unknown_top_level_key_suggests() {
        let m = msg("apiVersion: v1\ndatasett: t\ncolumns: {}\n");
        assert!(m.contains("unknown key `datasett`"), "{m}");
        assert!(m.contains("did you mean `dataset`?"), "{m}");
    }

    #[test]
    fn wrong_api_version_lists_supported() {
        let m = msg("apiVersion: v2\ndataset: t\ncolumns: {}\n");
        assert!(m.contains("unsupported apiVersion `v2`"), "{m}");
        assert!(m.contains("supported versions: v1"), "{m}");
    }

    #[test]
    fn missing_api_version_has_fix_example() {
        let e = err("dataset: t\ncolumns: {}\n");
        let m = e.to_string();
        assert!(m.contains("missing required key `apiVersion`"), "{m}");
        if let ParseError::Invalid(inv) = &e {
            assert!(inv.help.as_deref().unwrap().contains("apiVersion: v1"));
        } else {
            panic!("expected Invalid variant");
        }
    }

    #[test]
    fn empty_contract_is_friendly() {
        let e = err("   \n");
        assert!(e.to_string().contains("empty"), "{e}");
    }

    #[test]
    fn spans_point_at_offending_token() {
        let src = "apiVersion: v1\ndatasett: t\ncolumns: {}\n";
        let e = err(src);
        match &e {
            ParseError::Invalid(inv) if inv.span.is_some() => {
                let span = inv.span.unwrap();
                let start = span.offset();
                let text = &src[start..start + span.len()];
                assert_eq!(text, "datasett");
            }
            other => panic!("expected a span, got {other:?}"),
        }
    }

    #[test]
    fn unknown_check_error_propagates_through_contract_parse() {
        let m = msg("apiVersion: v1\ndataset: t\ncolumns:\n  a: { type: int, checks: [uniq] }\n");
        assert!(m.contains("did you mean `unique`?"), "{m}");
    }

    #[test]
    fn parse_file_missing_path_is_io_error() {
        let e = parse_file("does-not-exist.yaml").unwrap_err();
        assert!(matches!(e, ParseError::Io { .. }), "{e}");
        assert!(e.to_string().contains("does-not-exist.yaml"), "{e}");
    }

    #[test]
    fn strip_location_suffix_variants() {
        assert_eq!(strip_location_suffix("boom at line 3 column 7"), "boom");
        assert_eq!(strip_location_suffix("boom at position 12"), "boom");
        assert_eq!(strip_location_suffix("no location"), "no location");
    }

    #[test]
    fn path_prefix_is_split_and_appended() {
        let (path, rest) = split_path_prefix("columns.a.checks[0]: unknown check `uniq`");
        assert_eq!(path, Some("columns.a.checks[0]"));
        assert_eq!(rest, "unknown check `uniq`");
        let (path, rest) = split_path_prefix("invalid type: string, expected an integer");
        assert_eq!(path, None);
        assert!(rest.starts_with("invalid type"));
    }

    #[test]
    fn wrong_value_type_shows_path_context() {
        let m =
            msg("apiVersion: v1\ndataset: t\ncolumns:\n  id: { type: string, required: maybe }\n");
        assert!(m.contains("invalid type"), "{m}");
        assert!(m.contains("columns.id.required"), "{m}");
    }

    #[test]
    fn token_span_clamps() {
        let span = token_span("ab", 10);
        assert_eq!(span.offset(), 1);
        assert_eq!(span.len(), 1);
    }
}
