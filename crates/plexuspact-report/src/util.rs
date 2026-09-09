//! Shared, presentation-only formatting helpers.
//!
//! Everything in here formats values that already exist on the `RunResult` —
//! nothing computes new results (doc 03 §1).

use serde_json::Value;

/// Formats an integer with `,` thousands separators: `1204441` → `"1,204,441"`.
pub(crate) fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Renders a millisecond duration as seconds with one decimal: `6231` → `"6.2s"`.
pub(crate) fn duration_secs(ms: u64) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
}

/// Formats a fail ratio (`rows_failed / rows_evaluated`) as a percentage.
///
/// Ratios at or above 0.01% use two decimals with trailing zeros trimmed
/// (`0.0425` → `"0.04%"`); smaller ratios keep enough decimals to show the
/// first significant digit (`2.49e-6` → `"0.0002%"`), matching PRD §7.
pub(crate) fn percent(ratio: f64) -> String {
    let pct = ratio * 100.0;
    if !pct.is_finite() || pct <= 0.0 {
        return "0%".to_owned();
    }
    let formatted = if pct >= 0.01 {
        format!("{pct:.2}")
    } else {
        // Enough decimals to expose the first significant digit, capped so a
        // denormal ratio cannot produce an absurdly long string.
        let decimals = (-pct.log10()).ceil() as usize;
        let decimals = decimals.clamp(2, 12);
        format!("{pct:.decimals$}")
    };
    let trimmed = formatted.trim_end_matches('0').trim_end_matches('.');
    format!("{trimmed}%")
}

/// Derives the human-readable parameter summary for a check, generically from
/// `kind` + `params` as declared in the contract.
///
/// Examples: `format` + `{"format":"email"}` → `format: email`; `min` +
/// `{"min":18}` → `min: 18`; `freshness` + `{"max_age":"48h",...}` →
/// `freshness ≤ 48h`; `unique` + `null` → `unique`.
pub(crate) fn param_summary(kind: &str, params: &Value) -> String {
    // Freshness reads as a bound, per PRD §7: `freshness ≤ 48h`.
    if kind == "freshness" {
        if let Some(max_age) = params.get("max_age") {
            return format!("{kind} \u{2264} {}", render_value(max_age));
        }
    }
    match params {
        Value::Null => kind.to_owned(),
        Value::Object(map) => {
            // `severity` and `column` are routing metadata, not check
            // parameters worth echoing next to the column name.
            let mut entries: Vec<(&String, &Value)> = map
                .iter()
                .filter(|(k, _)| k.as_str() != "severity" && k.as_str() != "column")
                .collect();
            // Sort by key so output is deterministic regardless of whether
            // serde_json's `preserve_order` feature is unified on in the build.
            entries.sort_by(|a, b| a.0.cmp(b.0));
            match entries.as_slice() {
                [] => kind.to_owned(),
                [(_, value)] => format!("{kind}: {}", render_value(value)),
                many => {
                    let joined = many
                        .iter()
                        .map(|(k, v)| format!("{k}={}", render_value(v)))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{kind}: {joined}")
                }
            }
        }
        scalar => format!("{kind}: {}", render_value(scalar)),
    }
}

/// Renders a JSON parameter value compactly for display (strings unquoted,
/// arrays bracketed and comma-joined).
pub(crate) fn render_value(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(items) => {
            let inner = items
                .iter()
                .map(render_value)
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{inner}]")
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn thousands_groups_digits() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_204_441), "1,204,441");
        assert_eq!(thousands(123_456_789), "123,456,789");
    }

    #[test]
    fn duration_is_seconds_with_one_decimal() {
        assert_eq!(duration_secs(6231), "6.2s");
        assert_eq!(duration_secs(0), "0.0s");
        assert_eq!(duration_secs(754_321), "754.3s");
    }

    #[test]
    fn percent_matches_prd_examples() {
        assert_eq!(percent(2.49e-6), "0.0002%");
        assert_eq!(percent(4.25e-4), "0.04%");
        assert_eq!(percent(0.01), "1%");
        assert_eq!(percent(1.0), "100%");
        assert_eq!(percent(0.0), "0%");
        assert_eq!(percent(f64::NAN), "0%");
    }

    #[test]
    fn param_summaries_are_generic() {
        use serde_json::json;
        assert_eq!(
            param_summary("format", &json!({"format": "email"})),
            "format: email"
        );
        assert_eq!(param_summary("min", &json!({"min": 18})), "min: 18");
        assert_eq!(
            param_summary(
                "freshness",
                &json!({"column": "signed_up", "max_age": "48h"})
            ),
            "freshness \u{2264} 48h"
        );
        assert_eq!(param_summary("unique", &json!(null)), "unique");
        assert_eq!(
            param_summary("enum", &json!({"enum": ["free", "pro"]})),
            "enum: [free, pro]"
        );
        assert_eq!(
            param_summary("length", &json!({"max": 254, "min": 3})),
            "length: max=254, min=3"
        );
        assert_eq!(param_summary("min", &json!(18)), "min: 18");
    }
}
