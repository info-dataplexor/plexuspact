//! Reporting a run to PlexusPact Cloud.
//!
//! `check` produces a verdict; this sends it somewhere it can be kept. Without
//! it the only way to get a result off the machine that produced it is to pipe
//! the JSON into `curl` by hand — which every user has to write, get wrong in
//! the same way (a failing check aborts the pipeline under `set -e`, so nothing
//! is ever reported for exactly the runs worth reporting), and maintain.
//!
//! Three rules shape everything here.
//!
//! 1. **The verdict is never changed by the reporting.** A push that fails is a
//!    warning on stderr and nothing else. The data was either good or bad
//!    before the network was involved, and the exit code says which.
//! 2. **The token has one home: `PLEXUSPACT_API_KEY`.** There is deliberately
//!    no `--token` flag. A secret on a command line ends up in shell history,
//!    in `ps` output, and in CI logs that echo the command they ran.
//! 3. **The token is never sent in the clear.** Plain `http://` is accepted for
//!    localhost, because that is a person testing against a dev server, and
//!    refused for anything else.
//!
//! And one switch above all three: `--offline`, or `PLEXUSPACT_NO_NETWORK` in
//! the environment, and nothing here opens a socket no matter what else is set.

use std::time::Duration;

use url::Url;

/// Where runs go when nothing says otherwise.
const DEFAULT_API: &str = "https://api.plexuspact.com/api/v1";

/// Environment variable holding the project API key (`ck_live_…`).
pub const TOKEN_VAR: &str = "PLEXUSPACT_API_KEY";

/// Environment variable that forbids all network activity, whatever else is
/// configured. Set it in a CI base image and no key anyone exports below it can
/// open a socket. It is the outermost switch on purpose: the people who need to
/// guarantee an air-gap are not the same people who write the pipelines.
pub const NO_NETWORK_VAR: &str = "PLEXUSPACT_NO_NETWORK";

/// Environment variable overriding the API base URL (self-hosted installs).
/// Read by clap on the `--api` flag; named here only so the messages that
/// suggest fixing it and the flag that reads it cannot drift apart.
pub const API_VAR: &str = "PLEXUSPACT_API";

/// How long the whole request may take, connect to last byte. Long enough for a
/// slow runner on a cold TLS handshake, short enough that a black-holed proxy
/// does not hold a CI job open.
const TIMEOUT: Duration = Duration::from_secs(30);

/// A resolved place to send a run, with the credential to send it under.
#[derive(Debug, Clone)]
pub struct Destination {
    /// Fully-qualified URL of the ingest endpoint.
    pub url: String,
    token: String,
}

impl Destination {
    /// Attach a query string to an already-resolved destination.
    ///
    /// The guards live in [`endpoint_url`] and have already run on the path;
    /// what is added here is a `limit` or a cursor, never a credential and
    /// never anything that changes where the call goes. Personal data does not
    /// belong in a query string, and nothing in this crate puts it there.
    #[must_use]
    pub fn with_query(mut self, query: &str) -> Self {
        let sep = if self.url.contains('?') { '&' } else { '?' };
        self.url.push(sep);
        self.url.push_str(query);
        self
    }
}

/// Something that stopped a run from being reported. Never fatal to a check.
#[derive(Debug)]
pub struct PushError {
    /// What went wrong, as one line.
    pub message: String,
    /// What to do about it, when there is something to do.
    pub hint: Option<String>,
}

impl PushError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: None,
        }
    }

    pub(crate) fn with_hint(message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: Some(hint.into()),
        }
    }
}

impl std::fmt::Display for PushError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for PushError {}

/// A run as the cloud now holds it.
#[derive(Debug)]
pub struct Recorded {
    /// The run's id in the cloud.
    pub id: String,
    /// A URL a person can open to read it.
    pub url: String,
}

/// What this invocation will do about reporting.
///
/// Three outcomes rather than two, because "nobody asked for this" and "somebody
/// forbade it" are different things and the difference is worth saying out loud
/// when `--push` then cannot be honoured.
#[derive(Debug)]
pub enum Reporting {
    /// No key, so nothing to report to. Not a misconfiguration — it is the tool
    /// being used on its own, which is the whole point of the tool.
    Off,
    /// Deliberately switched off, with the switch named so it can be quoted back.
    Suppressed(&'static str),
    /// Report here.
    To(Destination),
}

/// Work out where to report. `api` is the resolved `--api` value, which clap has
/// already filled from `PLEXUSPACT_API` if the flag was absent; `offline` is the
/// global `--offline` flag.
///
/// Suppression is checked before the key, so an air-gapped image can set
/// `PLEXUSPACT_NO_NETWORK` once and stop caring what any pipeline underneath it
/// exports.
pub fn resolve(api: Option<&str>, offline: bool) -> Result<Reporting, PushError> {
    resolve_endpoint(api, offline, "runs")
}

/// Work out where to call, for any endpoint under the API root.
///
/// Every rule that guards a push guards this too, which is the reason it is one
/// function and not two: the offline switch, the refusal to send a key over
/// plain http, and the single home for the token are not properties of run
/// ingest — they are properties of talking to the cloud at all, and a second
/// caller that re-derived them would eventually re-derive one of them wrongly.
pub fn resolve_endpoint(
    api: Option<&str>,
    offline: bool,
    endpoint: &str,
) -> Result<Reporting, PushError> {
    if offline {
        return Ok(Reporting::Suppressed("--offline was given"));
    }
    if std::env::var(NO_NETWORK_VAR).is_ok_and(|v| !v.trim().is_empty() && v != "0") {
        return Ok(Reporting::Suppressed("PLEXUSPACT_NO_NETWORK is set"));
    }
    let token = match std::env::var(TOKEN_VAR) {
        Ok(t) if !t.trim().is_empty() => t.trim().to_owned(),
        _ => return Ok(Reporting::Off),
    };
    let base = api
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(DEFAULT_API);
    Ok(Reporting::To(Destination {
        url: endpoint_url(base, endpoint)?,
        token,
    }))
}

/// The runs endpoint, which is what the tests below exercise `endpoint_url`
/// through: every rule it enforces — the pasted `…/runs` root, the http refusal,
/// the missing host — was written for the one URL a person actually pastes.
#[cfg(test)]
fn runs_url(base: &str) -> Result<String, PushError> {
    endpoint_url(base, "runs")
}

/// Build the URL for one endpoint under whatever the user gave as the API base.
fn endpoint_url(base: &str, endpoint: &str) -> Result<String, PushError> {
    let mut url = Url::parse(base.trim()).map_err(|_| {
        PushError::with_hint(
            format!("`{base}` is not an http(s) URL"),
            format!("set {API_VAR} to the API root, e.g. {DEFAULT_API}"),
        )
    })?;
    match url.scheme() {
        "https" | "http" => {}
        _ => {
            return Err(PushError::with_hint(
                format!("`{base}` is not an http(s) URL"),
                format!("set {API_VAR} to the API root, e.g. {DEFAULT_API}"),
            ));
        }
    }
    let host = url.host_str().ok_or_else(|| {
        PushError::with_hint(
            format!("`{base}` has no host"),
            format!("set {API_VAR} to the API root, e.g. {DEFAULT_API}"),
        )
    })?;
    if url.query().is_some() || url.fragment().is_some() {
        return Err(PushError::with_hint(
            format!("`{base}` must be an API root without a query or fragment"),
            format!("set {API_VAR} to the API root, e.g. {DEFAULT_API}"),
        ));
    }
    // `host()` is typed: an IPv6 literal comes back as an address, not the
    // bracketed `[::1]` text that `host_str()` returns and no parser accepts.
    let loopback = match url.host() {
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(a)) => a.is_loopback(),
        Some(url::Host::Ipv6(a)) => a.is_loopback(),
        None => false,
    };
    if url.scheme() == "http" && !loopback {
        return Err(PushError::with_hint(
            format!("refusing to send an API key over plain http to `{host}`"),
            "use https, or point at localhost for local testing".to_owned(),
        ));
    }
    // `…/runs` pasted as the root is the one shape we know people paste,
    // because it is the one shape the onboarding screen ever printed. Strip it
    // and append what was actually asked for.
    let path = url.path().trim_end_matches('/');
    let root = path.strip_suffix("/runs").unwrap_or(path);
    url.set_path(&format!("{root}/{endpoint}"));
    Ok(url.into())
}

/// POST a `RunResult` document and return where it landed.
///
/// `body` must be the exact JSON `check --format json` emits; the cloud stores
/// it verbatim, so anything rewritten here is what the supplier will be shown a
/// year from now.
pub fn send(dest: &Destination, body: &str) -> Result<Recorded, PushError> {
    let text = post_json(dest, body)?;
    let parsed: serde_json::Value = serde_json::from_str(&text).map_err(|_| {
        PushError::new("the run was accepted but the reply could not be read".to_owned())
    })?;
    let field = |name: &str| {
        parsed
            .get(name)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    Ok(Recorded {
        id: field("id"),
        url: field("url"),
    })
}

/// POST JSON to a resolved destination and hand back the reply body.
///
/// Shared by run ingest and by preflight, so a 402 or a revoked key reads the
/// same way whichever call hit it.
pub fn post_json(dest: &Destination, body: &str) -> Result<String, PushError> {
    send_json(dest, "POST", Some(body))
}

/// GET a resolved destination and hand back the reply body.
///
/// Reading is the one thing this client did not do until the MCP server needed
/// it: every other call here changes something. It goes through the same hop as
/// the rest so that a revoked key, an exhausted plan and a self-hosted install
/// behind a slow proxy all read the same way whichever verb hit them.
pub fn get_json(dest: &Destination) -> Result<String, PushError> {
    send_json(dest, "GET", None)
}

/// PUT JSON to a resolved destination and hand back the reply body.
///
/// Registering a contract is idempotent on its content — the same bytes pushed
/// twice are the same version, not two — so it is a PUT, and the server may
/// answer `202` when the project requires somebody to approve a tightening.
/// That is a successful call with an unwelcome answer, not a failure, and the
/// caller is the one who has to say so.
pub fn put_json(dest: &Destination, body: &str) -> Result<String, PushError> {
    send_json(dest, "PUT", Some(body))
}

/// The one HTTP hop. Every guard the CLI has — the timeout, the user agent, the
/// bearer header, the RFC 7807 reading of a rejection — lives here once so no
/// endpoint can quietly acquire different manners.
fn send_json(dest: &Destination, method: &str, body: Option<&str>) -> Result<String, PushError> {
    // `http_status_as_error(false)`: a 4xx carries a `problem+json` body saying
    // precisely what was wrong, and turning it into a transport error throws
    // that away.
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(TIMEOUT))
        .user_agent(format!("plexuspact/{}", env!("CARGO_PKG_VERSION")))
        .build();
    let agent = config.new_agent();

    let bearer = format!("Bearer {}", dest.token);
    // The three verbs are three types in `ureq`, so they cannot share a builder
    // variable — but they share every header and every guard above.
    let sent = match method {
        "GET" => agent.get(&dest.url).header("authorization", &bearer).call(),
        "PUT" => agent
            .put(&dest.url)
            .header("authorization", &bearer)
            .header("content-type", "application/json")
            .send(body.unwrap_or_default()),
        _ => agent
            .post(&dest.url)
            .header("authorization", &bearer)
            .header("content-type", "application/json")
            .send(body.unwrap_or_default()),
    };
    // A read that never arrived changed nothing, so telling its caller that a
    // run went unreported would be a sentence about something that never
    // happened.
    let hint = if method == "GET" {
        "nothing was read and nothing was changed"
    } else {
        "the run was not reported; the check result above still stands"
    };
    let mut response = sent.map_err(|e| {
        PushError::with_hint(
            format!("could not reach {}: {e}", dest.url),
            hint.to_owned(),
        )
    })?;

    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string().unwrap_or_default();

    // 202 is the review gate answering: the version was recorded and is waiting
    // for a person. The call worked; whether that is good news is the caller's
    // to judge.
    if status == 200 || status == 201 || status == 202 {
        return Ok(text);
    }

    Err(rejected(status, &text))
}

/// Turn a non-2xx reply into something worth reading.
///
/// The server sends RFC 7807 `problem+json`; its `detail` is written for a
/// person and is almost always the whole answer. The status codes worth naming
/// are the ones where the detail alone would leave somebody guessing what to
/// change.
fn rejected(status: u16, body: &str) -> PushError {
    let detail = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("detail")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .filter(|d| !d.is_empty());

    let describe = |fallback: &str| detail.clone().unwrap_or_else(|| fallback.to_owned());

    match status {
        401 => PushError::with_hint(
            format!("the API key was rejected: {}", describe("not authorized")),
            format!("check {TOKEN_VAR}; keys are per-project, expire, and can be revoked"),
        ),
        // 403 is a live key that is not allowed to do this — the scope it was
        // minted with does not cover this call. The detail names the scope.
        403 => PushError::with_hint(
            format!(
                "the API key is not allowed to do this: {}",
                describe("missing scope")
            ),
            "mint a key with the right scope under Settings → API keys and update it in CI"
                .to_owned(),
        ),
        402 => PushError::with_hint(
            describe("this plan has no runs left this month"),
            "the run was not recorded; the check result above still stands".to_owned(),
        ),
        404 => PushError::with_hint(
            format!("no ingest endpoint at {}", describe("that address")),
            format!("{API_VAR} should be the API root, e.g. {DEFAULT_API}"),
        ),
        422 => PushError::with_hint(
            format!("the result was rejected: {}", describe("invalid document")),
            "this usually means the CLI and the cloud are different versions".to_owned(),
        ),
        429 => PushError::new(describe("too many requests; the run was not recorded")),
        _ => PushError::new(format!(
            "the run was not recorded (HTTP {status}): {}",
            describe(body.chars().take(200).collect::<String>().trim())
        )),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn appends_runs_to_an_api_root() {
        assert_eq!(
            runs_url("https://api.plexuspact.com/api/v1").unwrap(),
            "https://api.plexuspact.com/api/v1/runs"
        );
        assert_eq!(
            runs_url("https://api.plexuspact.com/api/v1/").unwrap(),
            "https://api.plexuspact.com/api/v1/runs"
        );
    }

    #[test]
    fn other_endpoints_hang_off_the_same_root() {
        assert_eq!(
            endpoint_url("https://api.plexuspact.com/api/v1", "contracts/preflight").unwrap(),
            "https://api.plexuspact.com/api/v1/contracts/preflight"
        );
        // Including when the root somebody pasted was really the ingest URL.
        assert_eq!(
            endpoint_url(
                "https://api.plexuspact.com/api/v1/runs",
                "contracts/preflight"
            )
            .unwrap(),
            "https://api.plexuspact.com/api/v1/contracts/preflight"
        );
    }

    #[test]
    fn a_key_is_never_sent_over_plain_http_off_this_machine() {
        assert!(endpoint_url("http://example.com/api/v1", "contracts/preflight").is_err());
        assert!(endpoint_url("http://127.0.0.1:8080/api/v1", "contracts/preflight").is_ok());
    }

    #[test]
    fn accepts_a_pasted_runs_url_unchanged() {
        assert_eq!(
            runs_url("https://api.plexuspact.com/api/v1/runs").unwrap(),
            "https://api.plexuspact.com/api/v1/runs"
        );
    }

    #[test]
    fn allows_plain_http_only_for_localhost() {
        assert_eq!(
            runs_url("http://127.0.0.1:8080/api/v1").unwrap(),
            "http://127.0.0.1:8080/api/v1/runs"
        );
        assert!(runs_url("http://localhost:8080/api/v1").is_ok());
        assert!(runs_url("http://[::1]:8080/api/v1").is_ok());
        assert!(runs_url("http://127.0.0.2:8080/api/v1").is_ok());
        let err = runs_url("http://example.com/api/v1").unwrap_err();
        assert!(err.message.contains("plain http"));
    }

    #[test]
    fn rejects_nonsense_bases() {
        assert!(runs_url("api.plexuspact.com").is_err());
        assert!(runs_url("ftp://example.com").is_err());
        assert!(runs_url("https://").is_err());
    }

    #[test]
    fn a_missing_token_is_not_an_error() {
        // Only meaningful when the variable is genuinely absent; on a machine
        // that has it set this asserts the other branch instead.
        match std::env::var(TOKEN_VAR) {
            Ok(t) if !t.trim().is_empty() => assert!(matches!(
                resolve(Some("https://example.com/api/v1"), false).unwrap(),
                Reporting::To(_)
            )),
            _ => assert!(matches!(resolve(None, false).unwrap(), Reporting::Off)),
        }
    }

    #[test]
    fn offline_wins_over_everything_else() {
        // Not even a bad URL is looked at: nothing is going anywhere, so there
        // is nothing to be wrong about.
        assert!(matches!(
            resolve(Some("nonsense"), true).unwrap(),
            Reporting::Suppressed(_)
        ));
    }

    #[test]
    fn rejection_prefers_the_servers_own_words() {
        let err = rejected(
            422,
            r#"{"title":"Unprocessable","status":422,"detail":"dataset is required"}"#,
        );
        assert!(err.message.contains("dataset is required"));

        let err = rejected(500, "<html>gateway</html>");
        assert!(err.message.contains("500"));
    }
}
