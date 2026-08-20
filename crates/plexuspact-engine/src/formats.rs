//! Built-in `format:` validators (PRD FR-3).
//!
//! Each validator is a pure `fn(&str) -> bool` over a single non-null string
//! value. They are deliberately curated (no external validation crates beyond
//! `url`) so behavior is stable and testable. The table-driven tests at the
//! bottom pin the accept/reject boundary for every format.

use std::sync::OnceLock;

use plexuspact_contract::KnownFormat;
use regex::Regex;

/// Returns the validator for a known format.
pub fn validator(format: KnownFormat) -> fn(&str) -> bool {
    match format {
        KnownFormat::Email => is_email,
        KnownFormat::Uuid => is_uuid,
        KnownFormat::IsoDate => is_iso_date,
        KnownFormat::IsoDatetime => is_iso_datetime,
        KnownFormat::Url => is_url,
        KnownFormat::CountryCodeIso2 => is_country_code_iso2,
    }
}

/// A short, human-readable name for the format (for check IDs and params).
pub fn format_name(format: KnownFormat) -> &'static str {
    match format {
        KnownFormat::Email => "email",
        KnownFormat::Uuid => "uuid",
        KnownFormat::IsoDate => "iso_date",
        KnownFormat::IsoDatetime => "iso_datetime",
        KnownFormat::Url => "url",
        KnownFormat::CountryCodeIso2 => "country_code_iso2",
    }
}

fn email_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Pragmatic RFC-5322 subset: local@domain.tld, no spaces, single `@`,
        // dotted domain with a 2+ char TLD.
        #[allow(clippy::expect_used)]
        Regex::new(r"^[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}$")
            .expect("valid email regex")
    })
}

fn uuid_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        #[allow(clippy::expect_used)]
        Regex::new(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")
            .expect("valid uuid regex")
    })
}

fn is_email(s: &str) -> bool {
    email_re().is_match(s)
}

fn is_uuid(s: &str) -> bool {
    uuid_re().is_match(s)
}

/// ISO-8601 calendar date `YYYY-MM-DD` (zero-padded), validated as a real date.
fn is_iso_date(s: &str) -> bool {
    // chrono's `%m`/`%d` accept single digits, so require zero-padded shape first.
    let bytes = s.as_bytes();
    let padded = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[8..10].iter().all(u8::is_ascii_digit);
    padded && chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

/// ISO-8601 / RFC-3339 date-time. Accepts a trailing `Z` or numeric offset, and
/// a naive `YYYY-MM-DDTHH:MM:SS[.fff]` (assumed UTC).
fn is_iso_datetime(s: &str) -> bool {
    if chrono::DateTime::parse_from_rfc3339(s).is_ok() {
        return true;
    }
    const NAIVE: &[&str] = &[
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
    ];
    NAIVE
        .iter()
        .any(|fmt| chrono::NaiveDateTime::parse_from_str(s, fmt).is_ok())
}

/// Absolute URL with a scheme and host (parsed by the `url` crate).
fn is_url(s: &str) -> bool {
    match url::Url::parse(s) {
        Ok(u) => u.has_host() && !u.scheme().is_empty(),
        Err(_) => false,
    }
}

/// ISO-3166-1 alpha-2 country code (uppercase), binary-searched against the
/// canonical list.
fn is_country_code_iso2(s: &str) -> bool {
    ISO_3166_1_ALPHA2.binary_search(&s).is_ok()
}

/// All 249 assigned ISO-3166-1 alpha-2 codes, sorted for binary search.
pub(crate) const ISO_3166_1_ALPHA2: &[&str] = &[
    "AD", "AE", "AF", "AG", "AI", "AL", "AM", "AO", "AQ", "AR", "AS", "AT", "AU", "AW", "AX", "AZ",
    "BA", "BB", "BD", "BE", "BF", "BG", "BH", "BI", "BJ", "BL", "BM", "BN", "BO", "BQ", "BR", "BS",
    "BT", "BV", "BW", "BY", "BZ", "CA", "CC", "CD", "CF", "CG", "CH", "CI", "CK", "CL", "CM", "CN",
    "CO", "CR", "CU", "CV", "CW", "CX", "CY", "CZ", "DE", "DJ", "DK", "DM", "DO", "DZ", "EC", "EE",
    "EG", "EH", "ER", "ES", "ET", "FI", "FJ", "FK", "FM", "FO", "FR", "GA", "GB", "GD", "GE", "GF",
    "GG", "GH", "GI", "GL", "GM", "GN", "GP", "GQ", "GR", "GS", "GT", "GU", "GW", "GY", "HK", "HM",
    "HN", "HR", "HT", "HU", "ID", "IE", "IL", "IM", "IN", "IO", "IQ", "IR", "IS", "IT", "JE", "JM",
    "JO", "JP", "KE", "KG", "KH", "KI", "KM", "KN", "KP", "KR", "KW", "KY", "KZ", "LA", "LB", "LC",
    "LI", "LK", "LR", "LS", "LT", "LU", "LV", "LY", "MA", "MC", "MD", "ME", "MF", "MG", "MH", "MK",
    "ML", "MM", "MN", "MO", "MP", "MQ", "MR", "MS", "MT", "MU", "MV", "MW", "MX", "MY", "MZ", "NA",
    "NC", "NE", "NF", "NG", "NI", "NL", "NO", "NP", "NR", "NU", "NZ", "OM", "PA", "PE", "PF", "PG",
    "PH", "PK", "PL", "PM", "PN", "PR", "PS", "PT", "PW", "PY", "QA", "RE", "RO", "RS", "RU", "RW",
    "SA", "SB", "SC", "SD", "SE", "SG", "SH", "SI", "SJ", "SK", "SL", "SM", "SN", "SO", "SR", "SS",
    "ST", "SV", "SX", "SY", "SZ", "TC", "TD", "TF", "TG", "TH", "TJ", "TK", "TL", "TM", "TN", "TO",
    "TR", "TT", "TV", "TW", "TZ", "UA", "UG", "UM", "US", "UY", "UZ", "VA", "VC", "VE", "VG", "VI",
    "VN", "VU", "WF", "WS", "YE", "YT", "ZA", "ZM", "ZW",
];

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::*;

    #[test]
    fn country_list_is_sorted_for_binary_search() {
        let mut sorted = ISO_3166_1_ALPHA2.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, ISO_3166_1_ALPHA2, "country list must be sorted");
        assert_eq!(ISO_3166_1_ALPHA2.len(), 249);
    }

    #[test]
    fn email_boundary() {
        for ok in [
            "a@b.co",
            "alice@example.com",
            "x.y+z%1@a.b.c.dev",
            "A_B@sub.domain.io",
        ] {
            assert!(is_email(ok), "{ok} should be valid");
        }
        for bad in [
            "",
            "bob@@example",
            "no-at.com",
            "a@b",
            "a@.com",
            "a b@c.com",
            "a@b.c ",
        ] {
            assert!(!is_email(bad), "{bad} should be invalid");
        }
    }

    #[test]
    fn uuid_boundary() {
        for ok in [
            "550e8400-e29b-41d4-a716-446655440000",
            "00000000-0000-0000-0000-000000000000",
            "FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF",
        ] {
            assert!(is_uuid(ok), "{ok} valid");
        }
        for bad in [
            "",
            "550e8400",
            "550e8400-e29b-41d4-a716",
            "zzzzzzzz-e29b-41d4-a716-446655440000",
            "550e8400e29b41d4a716446655440000",
        ] {
            assert!(!is_uuid(bad), "{bad} invalid");
        }
    }

    #[test]
    fn iso_date_boundary() {
        for ok in ["2026-07-09", "2000-02-29", "1999-12-31"] {
            assert!(is_iso_date(ok), "{ok} valid");
        }
        for bad in [
            "",
            "2026-13-01",
            "2026-02-30",
            "2026/07/09",
            "2026-7-9",
            "2026-07-09T00:00:00Z",
        ] {
            assert!(!is_iso_date(bad), "{bad} invalid");
        }
    }

    #[test]
    fn iso_datetime_boundary() {
        for ok in [
            "2026-07-09T10:11:12Z",
            "2026-07-09T10:11:12+05:30",
            "2026-07-09T10:11:12.500Z",
            "2026-07-09T10:11:12",
            "2026-07-09 10:11:12",
        ] {
            assert!(is_iso_datetime(ok), "{ok} valid");
        }
        for bad in [
            "",
            "2026-07-09",
            "10:11:12",
            "2026-07-09T25:00:00Z",
            "not-a-date",
        ] {
            assert!(!is_iso_datetime(bad), "{bad} invalid");
        }
    }

    #[test]
    fn url_boundary() {
        for ok in [
            "https://example.com",
            "http://a.b/c?d=e",
            "https://sub.host.io:8080/p",
        ] {
            assert!(is_url(ok), "{ok} valid");
        }
        for bad in ["", "example.com", "/relative/path", "http://", "just text"] {
            assert!(!is_url(bad), "{bad} invalid");
        }
    }

    #[test]
    fn country_code_boundary() {
        for ok in ["US", "DE", "IN", "JP", "ZW"] {
            assert!(is_country_code_iso2(ok), "{ok} valid");
        }
        for bad in ["", "usa", "us", "USA", "XX", "U1", "Us"] {
            assert!(!is_country_code_iso2(bad), "{bad} invalid");
        }
    }
}
