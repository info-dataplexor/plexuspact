//! JSON renderer — the versioned wire format (doc 03 §8).
//!
//! The output is the exact serde serialization of [`RunResult`], with no
//! reshaping: field names, optionality, and nesting are owned by
//! `plexuspact-core` and stable within `result_schema_version: 1`.

use plexuspact_core::result::RunResult;

use crate::RenderError;

/// Value substituted for every sample value by [`render_json_redacted`].
pub const REDACTED_VALUE: &str = "<redacted>";

/// Serializes `result` as JSON — the exact `RunResult` wire format.
///
/// With `pretty` the document is indented for human eyes; otherwise it is a
/// single compact line suitable for piping. Both forms carry identical data.
pub fn render_json(result: &RunResult, pretty: bool) -> Result<String, RenderError> {
    let rendered = if pretty {
        serde_json::to_string_pretty(result)?
    } else {
        serde_json::to_string(result)?
    };
    Ok(rendered)
}

/// Serializes `result` as JSON with every `samples[].value` replaced by
/// [`REDACTED_VALUE`], for `--redact-samples` (doc 03 §11: sample values may
/// contain PII). Row numbers are preserved so users can still locate the
/// offending rows in the source; everything else is untouched.
pub fn render_json_redacted(result: &RunResult, pretty: bool) -> Result<String, RenderError> {
    let mut redacted = result.clone();
    for check in &mut redacted.checks {
        for sample in &mut check.samples {
            sample.value = REDACTED_VALUE.to_owned();
        }
    }
    render_json(&redacted, pretty)
}
