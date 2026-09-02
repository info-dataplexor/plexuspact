//! Putting a contract into force, from the pipeline that merged it.
//!
//! The counterpart to [`crate::preflight`]. Preflight runs on the branch and
//! writes nothing, because a contract that has not been merged has not been
//! agreed. This runs after the merge, when it has — and it carries the one
//! piece of context the registry could never work out for itself: *where* it
//! was agreed.
//!
//! That matters more than it first looks. A contract registered by an API key
//! has, until now, arrived from nowhere; the record says a machine did it and
//! stops. The question an auditor asks six months later is not "which key" but
//! "who agreed to this", and in a repository-backed workflow that question has
//! a precise answer — the pull request, with its reviewers, its approvals and
//! its argument attached. So the pipeline passes the link, and the registry
//! keeps it on the version and in the audit record both.

use crate::push::{put_json, Destination, PushError};

/// What the registry did with a version.
///
/// Read field by field for the same reason as the preflight reply: a CLI one
/// release behind should report what it understands rather than refuse an
/// answer it mostly recognises.
#[derive(Debug, Clone)]
pub struct Registered {
    pub dataset: String,
    pub content_sha256: String,
    pub version_label: Option<String>,
    /// `true` when these exact bytes had never been seen before. `false` is a
    /// re-push of a version that already existed, which is the normal outcome
    /// of re-running a job — not an error.
    pub created: bool,
    /// `active` (binding now) or `proposed` (this project requires somebody to
    /// approve a tightening, and this version is waiting for them).
    pub status: String,
    /// The registry's own words when the version is not yet binding.
    pub message: Option<String>,
}

impl Registered {
    /// Whether this version is what suppliers are now judged against.
    pub fn is_active(&self) -> bool {
        self.status == "active"
    }
}

/// Register `content` as the contract for `dataset`.
///
/// `source_url` is where it was agreed — a pull request, a merge request, a
/// work item. Optional on purpose: a pipeline that does not know must not
/// invent one, because a provenance link that goes somewhere unrelated is worse
/// than an honest blank.
pub fn put(
    dest: &Destination,
    dataset: &str,
    content: &str,
    version_label: Option<&str>,
    source_url: Option<&str>,
) -> Result<Registered, PushError> {
    let mut body = serde_json::Map::new();
    body.insert("dataset".to_owned(), dataset.into());
    body.insert("content".to_owned(), content.into());
    if let Some(label) = version_label {
        body.insert("version_label".to_owned(), label.into());
    }
    if let Some(url) = source_url {
        body.insert("source_url".to_owned(), url.into());
    }
    let request = serde_json::Value::Object(body).to_string();
    let text = put_json(dest, &request)?;
    let v: serde_json::Value = serde_json::from_str(&text)
        .map_err(|_| PushError::new("the contract was accepted but the reply could not be read"))?;
    Ok(parse(&v))
}

/// Read a reply into a [`Registered`]. Split out so it can be tested without a
/// server.
fn parse(v: &serde_json::Value) -> Registered {
    let string = |key: &str| {
        v.get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let optional = |key: &str| {
        v.get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    Registered {
        dataset: string("dataset"),
        content_sha256: string("content_sha256"),
        version_label: optional("version_label"),
        created: v
            .get("created")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        // An unreadable status is never rounded up to "active": if we cannot
        // tell whether the version is binding, we must not say that it is.
        status: string("status"),
        message: optional("message"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_activated_version_reads_as_binding() {
        let reply = serde_json::json!({
            "id": "0192f0e0-0000-7000-8000-000000000000",
            "dataset": "orders",
            "content_sha256": "abc",
            "version_label": "v3",
            "created": true,
            "created_at": "2026-08-31T00:00:00Z",
            "status": "active",
        });
        let r = parse(&reply);
        assert!(r.is_active());
        assert!(r.created);
        assert_eq!(r.version_label.as_deref(), Some("v3"));
    }

    #[test]
    fn a_proposed_version_is_not_reported_as_in_force() {
        let reply = serde_json::json!({
            "dataset": "orders",
            "content_sha256": "abc",
            "created": true,
            "status": "proposed",
            "message": "this tightens the terms; somebody has to approve it",
        });
        let r = parse(&reply);
        assert!(!r.is_active());
        assert_eq!(
            r.message.as_deref(),
            Some("this tightens the terms; somebody has to approve it")
        );
    }

    #[test]
    fn a_reply_we_cannot_read_never_claims_the_contract_is_in_force() {
        let r = parse(&serde_json::json!({}));
        assert!(!r.is_active(), "an empty reply must not read as active");
    }
}
