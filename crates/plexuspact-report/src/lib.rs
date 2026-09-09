//! Report renderers for PlexusPact.
//!
//! **Every renderer in this crate is a pure function of [`RunResult`]** (doc 03
//! §1/§8): renderers format what the engine already computed and never derive
//! new verdicts, counts, or metrics. Given the same `RunResult`, every renderer
//! produces byte-identical output on every platform.
//!
//! # Cross-renderer consistency guarantee
//!
//! The human header, the JSON `summary` object, the JUnit `<testsuite>`
//! counters, and the HTML summary tiles all reflect the *same* fields of the
//! same document — they cannot disagree, and integration tests assert this by
//! rendering one `RunResult` through all four renderers and comparing counts.
//!
//! # Renderers
//!
//! | Function | Output |
//! |---|---|
//! | [`render_human`] | Terminal output per PRD §7 (optional ANSI color) |
//! | [`render_json`] | The versioned wire format (doc 03 §8), exact serde form |
//! | [`render_json_redacted`] | Same, with every sample value masked |
//! | [`render_junit`] | JUnit XML for CI systems |
//! | [`render_html`] | Self-contained single-file HTML report (no external requests) |
//! | [`render_openlineage`] | OpenLineage `RunEvent` with data-quality facets (Databricks/dbt/Airflow) |

pub mod human;
pub mod json;
pub mod junit;
pub mod openlineage;

mod html;
mod util;

pub use html::render_html;
pub use human::{render_human, HumanOptions};
pub use json::{render_json, render_json_redacted};
pub use junit::render_junit;
pub use openlineage::{openlineage_event, render_openlineage, OpenLineageOptions};

// Re-exported so downstream callers can construct/consume results without
// naming plexuspact-core directly.
pub use plexuspact_core::result::RunResult;

/// Errors produced while rendering a [`RunResult`].
///
/// Rendering is infallible for the human format; the structured formats can
/// fail on serialization (JSON), XML writing (JUnit), or template evaluation
/// (HTML). All of these indicate a bug or an out-of-memory condition rather
/// than bad user data — a valid `RunResult` always renders.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// JSON serialization of the result document failed.
    #[error("failed to serialize result as JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// Writing the JUnit XML document failed.
    #[error("failed to serialize result as JUnit XML: {0}")]
    Junit(#[from] quick_junit::SerializeError),
    /// Evaluating the embedded HTML template failed.
    #[error("failed to render HTML report: {0}")]
    Html(#[from] tera::Error),
}
