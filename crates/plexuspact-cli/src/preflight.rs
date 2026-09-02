//! Asking the registry what a branch would do, before it is merged.
//!
//! `diff <old> <new>` compares two files, which answers a question the author
//! already knows the answer to: they wrote both files. The question a pull
//! request actually raises is different — *what is in force right now, and who
//! breaks if this lands?* — and only the registry can answer it, because only
//! the registry knows which version the suppliers are being judged against
//! today and who has declared that they read the dataset.
//!
//! Nothing here writes. Merging is what registers a contract; a CI job that
//! registered on the way past would make the pipeline the author of terms
//! nobody had agreed to.

use crate::push::{post_json, Destination, PushError};

/// The registry's answer, kept deliberately loose.
///
/// Parsed field by field out of the JSON rather than deserialised into a
/// mirror of the server's struct: a CLI that is one release behind should
/// report what it understands, not refuse the whole reply because the server
/// grew a field. The one thing it will not do is invent a verdict — an
/// unrecognised one is carried through verbatim so it shows up in the output
/// instead of being quietly rounded down to "safe".
#[derive(Debug, Clone)]
pub struct Preflight {
    pub verdict: String,
    pub dataset: String,
    pub summary: String,
    /// `(impact, path, description)`, in the order the server sent them.
    pub changes: Vec<(String, String, String)>,
    pub breaking: usize,
    /// `(level, path, message)` — parse and lint diagnostics.
    pub findings: Vec<(String, String, String)>,
    pub requires_review: bool,
    pub baseline_version_label: Option<String>,
    /// Consumers this reaches: `(name, whole_dataset, muted)`.
    pub consumers: Vec<(String, bool, bool)>,
    /// Consumers the contract declares at all. Zero means the question has
    /// never been answered, which is not the same as nobody being affected.
    pub declared: usize,
}

impl Preflight {
    /// Whether this should stop a pipeline.
    pub fn is_breaking(&self) -> bool {
        self.verdict == "breaking"
    }

    /// Whether the contract could not be read at all.
    pub fn is_invalid(&self) -> bool {
        self.verdict == "invalid"
    }
}

/// Ask the registry to judge `content` against the version in force.
pub fn fetch(
    dest: &Destination,
    content: &str,
    dataset: Option<&str>,
) -> Result<Preflight, PushError> {
    let mut body = serde_json::Map::new();
    body.insert("content".to_owned(), content.into());
    if let Some(d) = dataset {
        body.insert("dataset".to_owned(), d.into());
    }
    let request = serde_json::Value::Object(body).to_string();
    let text = post_json(dest, &request)?;
    let v: serde_json::Value = serde_json::from_str(&text)
        .map_err(|_| PushError::new("the registry replied with something that is not JSON"))?;
    Ok(parse(&v))
}

/// Read a reply into a [`Preflight`]. Split out so it can be tested without a
/// server.
fn parse(v: &serde_json::Value) -> Preflight {
    let string = |key: &str| {
        v.get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let triples = |key: &str, a: &str, b: &str, c: &str| -> Vec<(String, String, String)> {
        v.get(key)
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|i| {
                        let f = |k: &str| {
                            i.get(k)
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default()
                                .to_owned()
                        };
                        (f(a), f(b), f(c))
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let impact = v.get("impact");
    let consumers = impact
        .and_then(|i| i.get("consumers"))
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|c| {
                    let flag =
                        |k: &str| c.get(k).and_then(serde_json::Value::as_bool) == Some(true);
                    (
                        c.get("name")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        flag("whole_dataset"),
                        flag("muted"),
                    )
                })
                .collect()
        })
        .unwrap_or_default();

    Preflight {
        verdict: string("verdict"),
        dataset: string("dataset"),
        summary: string("summary"),
        changes: triples("changes", "impact", "path", "description"),
        breaking: v
            .get("breaking")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize,
        findings: triples("findings", "level", "path", "message"),
        requires_review: v
            .get("requires_review")
            .and_then(serde_json::Value::as_bool)
            == Some(true),
        baseline_version_label: v
            .get("baseline_version_label")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        consumers,
        declared: impact
            .and_then(|i| i.get("declared"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize,
    }
}

/// The comment a pull request gets.
///
/// Written to be read by whoever is reviewing the code change, not by whoever
/// wrote the contract: it leads with the consequence, names the people, and
/// only then lists the paths. Markdown rather than a rendered image so it stays
/// searchable, diffable, and readable in an email notification.
pub fn to_markdown(p: &Preflight) -> String {
    let title = match p.verdict.as_str() {
        "invalid" => "This contract cannot be read",
        "unchanged" => "No change to the terms",
        "new" => "A new dataset comes under contract",
        "safe" => "The terms change, but nothing tightens",
        "breaking" => "This tightens terms somebody is relying on",
        _ => "Contract preflight",
    };
    let dataset = if p.dataset.is_empty() {
        "unnamed dataset".to_owned()
    } else {
        format!("`{}`", p.dataset)
    };
    let mut out = format!("## {title} — {dataset}\n\n{}\n", p.summary);

    if !p.findings.is_empty() {
        out.push_str("\n### Problems\n\n");
        for (level, path, message) in &p.findings {
            let where_ = if path.is_empty() {
                String::new()
            } else {
                format!("`{path}` — ")
            };
            out.push_str(&format!("- **{level}** {where_}{message}\n"));
        }
    }

    if !p.consumers.is_empty() {
        out.push_str("\n### Who breaks\n\n");
        for (name, whole, muted) in &p.consumers {
            let mut line = format!("- **{name}**");
            if *whole {
                line.push_str(" — reads the whole dataset (nothing narrower was declared)");
            }
            if *muted {
                line.push_str(" — asked us to stop writing to them, so they will not hear");
            }
            out.push_str(&line);
            out.push('\n');
        }
    } else if p.is_breaking() && p.declared == 0 {
        out.push_str(
            "\n### Who breaks\n\nNobody has declared what they read from this dataset. \
             That is an unanswered question, not an all-clear.\n",
        );
    }

    if !p.changes.is_empty() {
        out.push_str("\n### Changes\n\n| | change |\n| --- | --- |\n");
        for (impact, path, description) in &p.changes {
            let label = match impact.as_str() {
                "breaking" => "**breaks**",
                "non_breaking" => "safe",
                _ => "cosmetic",
            };
            out.push_str(&format!("| {label} | `{path}` — {description} |\n"));
        }
    }

    let against = match &p.baseline_version_label {
        Some(label) if !label.is_empty() => format!("the version in force (`{label}`)"),
        _ => "the version in force".to_owned(),
    };
    out.push_str(&format!(
        "\n<sub>Judged against {against}. Nothing has been registered — merging is what \
         does that.</sub>\n"
    ));
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn sample() -> serde_json::Value {
        serde_json::json!({
            "verdict": "breaking",
            "dataset": "orders",
            "summary": "1 of 2 change(s) to orders tighten the terms.",
            "breaking": 1,
            "requires_review": true,
            "baseline_version_label": "2026-08-01",
            "changes": [
                { "impact": "breaking", "path": "columns.amount", "description": "removed" },
                { "impact": "cosmetic", "path": "description", "description": "reworded" }
            ],
            "findings": [],
            "impact": {
                "declared": 3,
                "columns": ["amount"],
                "dataset_wide": false,
                "consumers": [
                    { "name": "billing", "columns": ["amount"], "whole_dataset": false, "muted": true }
                ]
            }
        })
    }

    #[test]
    fn a_reply_is_read_without_a_mirrored_struct() {
        let p = parse(&sample());
        assert!(p.is_breaking());
        assert_eq!(p.breaking, 1);
        assert_eq!(p.changes.len(), 2);
        assert_eq!(p.declared, 3);
        assert_eq!(p.consumers, vec![("billing".to_owned(), false, true)]);
        assert_eq!(p.baseline_version_label.as_deref(), Some("2026-08-01"));
    }

    #[test]
    fn an_unknown_verdict_is_carried_through_rather_than_softened() {
        let mut v = sample();
        v["verdict"] = serde_json::json!("something_new");
        let p = parse(&v);
        assert!(!p.is_breaking());
        assert!(!p.is_invalid());
        assert!(to_markdown(&p).contains("Contract preflight"));
    }

    #[test]
    fn the_comment_names_the_people_before_the_paths() {
        let md = to_markdown(&parse(&sample()));
        let who = md.find("Who breaks").unwrap();
        let what = md.find("### Changes").unwrap();
        assert!(who < what, "{md}");
        assert!(md.contains("billing"), "{md}");
        assert!(md.contains("will not hear"), "{md}");
    }

    #[test]
    fn nobody_declared_is_never_rendered_as_nobody_affected() {
        let mut v = sample();
        v["impact"] = serde_json::json!({ "declared": 0, "consumers": [] });
        let md = to_markdown(&parse(&v));
        assert!(md.contains("unanswered question"), "{md}");
    }

    #[test]
    fn the_footer_says_that_nothing_was_registered() {
        let md = to_markdown(&parse(&sample()));
        assert!(md.contains("Nothing has been registered"), "{md}");
    }
}
