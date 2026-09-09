//! Draft a `contract.yaml` from a [`DatasetProfile`] (the `init` output).
//!
//! Suggestions are conservative and **commented out**: `init` output must parse
//! cleanly and never fail against the data it was profiled from (doc 03 §6).
//! The user reviews and uncomments the suggestions they want.

use std::fmt::Write as _;

use plexuspact_contract::InputSettings;
use plexuspact_engine::{ColumnProfile, DatasetProfile};

/// Threshold under which a low-cardinality string column gets a suggested
/// `enum` comment.
const ENUM_SUGGEST_MAX_DISTINCT: u64 = 12;

/// Renders a draft contract for the profile under the given dataset name.
pub fn draft_contract(profile: &DatasetProfile, dataset: &str) -> String {
    draft_contract_with_input(profile, dataset, &InputSettings::default())
}

/// [`draft_contract`], recording the reading instructions the data was
/// profiled with under `settings.input` — the sheet, the record element, the
/// fixed-width layout — so `check` reads the feed the same way without the
/// flags.
pub fn draft_contract_with_input(
    profile: &DatasetProfile,
    dataset: &str,
    input: &InputSettings,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "apiVersion: v1");
    let _ = writeln!(out, "dataset: {dataset}");
    let _ = writeln!(out, "# owner: your-team@example.com");
    let _ = writeln!(out, "# description: Describe this dataset.");
    let _ = writeln!(out);
    let _ = writeln!(out, "columns:");
    for col in &profile.columns {
        render_column(&mut out, col);
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "dataset_checks:");
    let _ = writeln!(out, "  - row_count_min: 1");
    if let Some(ts) = profile
        .columns
        .iter()
        .find(|c| c.inferred_type == "datetime")
    {
        let _ = writeln!(
            out,
            "  # - freshness: {{ column: {}, max_age: 48h, severity: warn }}",
            ts.name
        );
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "settings:");
    let _ = writeln!(out, "  allow_extra_columns: true");
    let _ = writeln!(out, "  on_type_mismatch: error");
    render_input(&mut out, input);

    out
}

/// Renders `settings.input` from what was given; nothing when nothing was.
fn render_input(out: &mut String, input: &InputSettings) {
    if input.is_empty() {
        return;
    }
    let _ = writeln!(out, "  input:");
    if let Some(f) = input.format {
        let _ = writeln!(out, "    format: {}", f.name());
    }
    if let Some(d) = &input.delimiter {
        let _ = writeln!(out, "    delimiter: {}", yaml_str(d));
    }
    if let Some(h) = input.has_header {
        let _ = writeln!(out, "    has_header: {h}");
    }
    if let Some(n) = input.skip_rows {
        let _ = writeln!(out, "    skip_rows: {n}");
    }
    if let Some(p) = &input.json_path {
        let _ = writeln!(out, "    json_path: {}", yaml_str(p));
    }
    if let Some(s) = &input.sheet {
        let _ = writeln!(out, "    sheet: {}", yaml_str(s));
    }
    if let Some(r) = &input.xml_record {
        let _ = writeln!(out, "    xml_record: {}", yaml_str(r));
    }
    if !input.fixed_width.is_empty() {
        let _ = writeln!(out, "    fixed_width:");
        for f in &input.fixed_width {
            let mut parts = vec![format!("name: {}", yaml_str(&f.name))];
            if let Some(s) = f.start {
                parts.push(format!("start: {s}"));
            }
            if let Some(e) = f.end {
                parts.push(format!("end: {e}"));
            }
            if let Some(w) = f.width {
                parts.push(format!("width: {w}"));
            }
            let _ = writeln!(out, "      - {{ {} }}", parts.join(", "));
        }
    }
}

/// A double-quoted YAML scalar: safe for `;`, `|`, `\t`, a sheet called `2`.
fn yaml_str(s: &str) -> String {
    let mut q = String::with_capacity(s.len() + 2);
    q.push('"');
    for ch in s.chars() {
        match ch {
            '"' => q.push_str("\\\""),
            '\\' => q.push_str("\\\\"),
            '\t' => q.push_str("\\t"),
            '\n' => q.push_str("\\n"),
            c => q.push(c),
        }
    }
    q.push('"');
    q
}

fn render_column(out: &mut String, col: &ColumnProfile) {
    let name = &col.name;
    let ty = &col.inferred_type;
    // `required: true` only when the data had no nulls (safe by construction).
    if col.null_count == 0 {
        let _ = writeln!(out, "  {name}: {{ type: {ty}, required: true }}");
    } else {
        let _ = writeln!(out, "  {name}: {{ type: {ty} }}");
    }

    // Conservative, commented suggestions that the current data would pass.
    match ty.as_str() {
        "int" | "float" => {
            if let (Some(min), Some(max)) = (&col.min, &col.max) {
                let _ = writeln!(
                    out,
                    "    # checks: [{{ min: {min} }}, {{ max: {max} }}]  # observed range: {min}..{max}"
                );
            }
        }
        "string" => {
            if col.distinct > 0
                && col.distinct <= ENUM_SUGGEST_MAX_DISTINCT
                && !col.distinct_at_least
            {
                // Name the values when the profile kept them. "9 distinct values
                // observed" tells the reader an enum is plausible and then makes
                // them go and find out what it is; the list is the whole point.
                if col.values_complete && !col.values.is_empty() {
                    let _ = writeln!(
                        out,
                        "    # checks: [{{ enum: [{}] }}]  # every value observed",
                        col.values.join(", ")
                    );
                } else {
                    let _ = writeln!(
                        out,
                        "    # checks: [{{ enum: [...] }}]  # {} distinct value(s) observed",
                        col.distinct
                    );
                }
            } else if let Some(format) = &col.format {
                let _ = writeln!(
                    out,
                    "    # checks: [{{ format: {format} }}]  # every value matched"
                );
            } else if let (Some(min_len), Some(max_len)) = (col.min_length, col.max_length) {
                if min_len == max_len {
                    let _ = writeln!(
                        out,
                        "    # checks: [{{ length: {min_len} }}]  # all values are {min_len} chars"
                    );
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn col(name: &str, ty: &str, nulls: u64) -> ColumnProfile {
        ColumnProfile {
            name: name.to_owned(),
            inferred_type: ty.to_owned(),
            null_count: nulls,
            null_ratio: 0.0,
            distinct: 3,
            distinct_at_least: false,
            min: Some("1".to_owned()),
            max: Some("9".to_owned()),
            mean: None,
            std: None,
            min_length: Some(2),
            max_length: Some(2),
            format: None,
            values: Vec::new(),
            values_complete: false,
        }
    }

    #[test]
    fn draft_parses_cleanly() {
        let profile = DatasetProfile {
            rows: 10,
            columns: vec![col("id", "int", 0), col("plan", "string", 1)],
        };
        let yaml = draft_contract(&profile, "my_dataset");
        // The draft must be a valid contract.
        let parsed = plexuspact_contract::parse_str(&yaml, "draft.yaml").unwrap();
        assert_eq!(parsed.dataset, "my_dataset");
        assert!(parsed.columns.contains_key("id"));
        // `id` had no nulls → required; `plan` had a null → not required.
        assert!(parsed.columns["id"].required);
        assert!(!parsed.columns["plan"].required);
    }

    #[test]
    fn draft_records_input_settings() {
        use plexuspact_contract::{FixedWidthField, InputFormat};
        let profile = DatasetProfile {
            rows: 1,
            columns: vec![col("id", "int", 0)],
        };
        let input = InputSettings {
            format: Some(InputFormat::FixedWidth),
            delimiter: Some("\t".into()),
            sheet: Some("2".into()),
            fixed_width: vec![FixedWidthField {
                name: "id".into(),
                start: Some(1),
                end: Some(4),
                width: None,
            }],
            ..Default::default()
        };
        let yaml = draft_contract_with_input(&profile, "ds", &input);
        let parsed = plexuspact_contract::parse_str(&yaml, "draft").unwrap();
        assert_eq!(parsed.settings.input, input);
        assert!(yaml.contains("    format: fixed_width\n"), "{yaml}");
        assert!(
            yaml.contains("      - { name: \"id\", start: 1, end: 4 }\n"),
            "{yaml}"
        );
        assert!(!draft_contract(&profile, "ds").contains("input:"));
    }
}
