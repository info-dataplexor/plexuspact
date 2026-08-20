//! Draft a `contract.yaml` from a [`DatasetProfile`] (the `init` output).
//!
//! Suggestions are conservative and **commented out**: `init` output must parse
//! cleanly and never fail against the data it was profiled from (doc 03 §6).
//! The user reviews and uncomments the suggestions they want.

use std::fmt::Write as _;

use plexuspact_engine::{ColumnProfile, DatasetProfile};

/// Threshold under which a low-cardinality string column gets a suggested
/// `enum` comment.
const ENUM_SUGGEST_MAX_DISTINCT: u64 = 12;

/// Renders a draft contract for the profile under the given dataset name.
pub fn draft_contract(profile: &DatasetProfile, dataset: &str) -> String {
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

    out
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
                let _ = writeln!(
                    out,
                    "    # checks: [{{ enum: [...] }}]  # {} distinct value(s) observed",
                    col.distinct
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
}
