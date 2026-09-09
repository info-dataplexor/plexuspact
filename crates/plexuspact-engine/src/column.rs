//! Per-batch typed column extraction.
//!
//! Turns one Polars column into a [`ColumnData`]: the raw string view (for
//! string checks and failure samples) plus a parsed typed view (for numeric and
//! temporal checks) with an explicit per-row cast-failure flag.
//!
//! Works for both input typings (crate `plexuspact-io` docs): in
//! [`InputTyping::Stringly`] every column arrives as `String` and is parsed
//! here; in [`InputTyping::Native`] (Parquet) the column already carries a
//! native dtype and is read directly, with a declared-vs-actual mismatch
//! surfaced as a structural (whole-column) failure.

use plexuspact_contract::ColType;
use plexuspact_io::InputTyping;
use polars::prelude::*;

use crate::error::EngineError;

/// Parsed typed values for a column, aligned 1:1 with the raw rows.
#[derive(Debug, Clone)]
pub enum Parsed {
    /// Integer column (`int`).
    Int(Vec<Option<i64>>),
    /// Float column (`float`).
    Float(Vec<Option<f64>>),
    /// Boolean column (`bool`). Parsed values are reserved for future
    /// bool-specific checks; today only the cast-failure flag is consumed.
    Bool(#[allow(dead_code)] Vec<Option<bool>>),
    /// Date column, days since the Unix epoch (`date`).
    Date(Vec<Option<i32>>),
    /// Datetime column, microseconds since the Unix epoch, UTC (`datetime`).
    Datetime(Vec<Option<i64>>),
    /// String column (`string`) — identical to the raw view.
    Str,
}

/// One column of a batch, extracted for checking.
#[derive(Debug, Clone)]
pub struct ColumnData {
    /// Raw string values; `None` = null/missing in the source.
    pub raw: Vec<Option<String>>,
    /// Parsed typed values (same length as `raw`).
    pub parsed: Parsed,
    /// `true` where the raw value is non-null but could not be parsed as the
    /// declared type (a type mismatch). Empty for string columns and for the
    /// structural-mismatch case.
    pub cast_failed: Vec<bool>,
    /// Set when a Native (Parquet) column's dtype does not match the declared
    /// type: the whole column is a structural mismatch (no per-row capture).
    pub structural_mismatch: bool,
}

impl ColumnData {
    /// Number of rows.
    pub fn len(&self) -> usize {
        self.raw.len()
    }

    /// Count of non-null raw values.
    pub fn non_null(&self) -> u64 {
        self.raw.iter().filter(|v| v.is_some()).count() as u64
    }

    /// Count of null raw values.
    pub fn null_count(&self) -> u64 {
        self.raw.iter().filter(|v| v.is_none()).count() as u64
    }
}

/// Extracts a declared column from a batch.
pub fn extract(
    df: &DataFrame,
    name: &str,
    declared: ColType,
    typing: InputTyping,
) -> Result<ColumnData, EngineError> {
    let column = df.column(name)?;
    let series = column.as_materialized_series();

    // Nested data (JSON arrays → List, objects → Struct) can't be validated as a
    // flat column. Surface a clear, actionable error rather than an opaque cast
    // failure (common when pointing the tool at a raw REST-API response).
    if let Some(kind) = nested_kind(series.dtype()) {
        return Err(EngineError::NestedColumn {
            column: name.to_owned(),
            kind,
        });
    }

    // Raw string view: cast to String (a no-op in stringly mode).
    let as_string = series.cast(&DataType::String)?;
    let raw = string_series_to_vec(&as_string)?;

    match typing {
        InputTyping::Stringly => parse_stringly(raw, declared),
        InputTyping::Native => extract_native(series, raw, declared),
    }
}

/// Classifies a nested Polars dtype: JSON arrays parse to `List`/`Array`,
/// objects to `Struct`. Scalars return `None`.
pub(crate) fn nested_kind(dtype: &DataType) -> Option<&'static str> {
    match dtype {
        DataType::List(_) => Some("array"),
        DataType::Struct(_) => Some("object"),
        _ => None,
    }
}

/// Collects a String series into `Vec<Option<String>>`.
fn string_series_to_vec(series: &Series) -> Result<Vec<Option<String>>, EngineError> {
    let ca = series.str()?;
    Ok(ca
        .into_iter()
        .map(|opt| opt.map(|s| s.to_owned()))
        .collect())
}

/// Parses raw strings into the declared type (check path).
fn parse_stringly(raw: Vec<Option<String>>, declared: ColType) -> Result<ColumnData, EngineError> {
    let mut cast_failed = vec![false; raw.len()];

    let parsed = match declared {
        ColType::String => Parsed::Str,
        ColType::Int => {
            let mut out = Vec::with_capacity(raw.len());
            for (i, cell) in raw.iter().enumerate() {
                match cell {
                    None => out.push(None),
                    Some(s) => match s.trim().parse::<i64>() {
                        Ok(v) => out.push(Some(v)),
                        Err(_) => {
                            out.push(None);
                            cast_failed[i] = true;
                        }
                    },
                }
            }
            Parsed::Int(out)
        }
        ColType::Float => {
            let mut out = Vec::with_capacity(raw.len());
            for (i, cell) in raw.iter().enumerate() {
                match cell {
                    None => out.push(None),
                    Some(s) => match s.trim().parse::<f64>() {
                        Ok(v) if v.is_finite() => out.push(Some(v)),
                        _ => {
                            out.push(None);
                            cast_failed[i] = true;
                        }
                    },
                }
            }
            Parsed::Float(out)
        }
        ColType::Bool => {
            let mut out = Vec::with_capacity(raw.len());
            for (i, cell) in raw.iter().enumerate() {
                match cell {
                    None => out.push(None),
                    Some(s) => match parse_bool(s.trim()) {
                        Some(v) => out.push(Some(v)),
                        None => {
                            out.push(None);
                            cast_failed[i] = true;
                        }
                    },
                }
            }
            Parsed::Bool(out)
        }
        ColType::Date => {
            let mut out = Vec::with_capacity(raw.len());
            for (i, cell) in raw.iter().enumerate() {
                match cell {
                    None => out.push(None),
                    Some(s) => match parse_date_days(s.trim()) {
                        Some(v) => out.push(Some(v)),
                        None => {
                            out.push(None);
                            cast_failed[i] = true;
                        }
                    },
                }
            }
            Parsed::Date(out)
        }
        ColType::Datetime => {
            let mut out = Vec::with_capacity(raw.len());
            for (i, cell) in raw.iter().enumerate() {
                match cell {
                    None => out.push(None),
                    Some(s) => match parse_datetime_micros(s.trim()) {
                        Some(v) => out.push(Some(v)),
                        None => {
                            out.push(None);
                            cast_failed[i] = true;
                        }
                    },
                }
            }
            Parsed::Datetime(out)
        }
    };

    Ok(ColumnData {
        raw,
        parsed,
        cast_failed,
        structural_mismatch: false,
    })
}

/// Reads a natively-typed (Parquet) column, checking declared-vs-actual dtype.
fn extract_native(
    series: &Series,
    raw: Vec<Option<String>>,
    declared: ColType,
) -> Result<ColumnData, EngineError> {
    let actual = series.dtype();
    if !dtype_matches(actual, declared) {
        // Structural mismatch: the whole column is the wrong type.
        return Ok(ColumnData {
            raw,
            parsed: Parsed::Str,
            cast_failed: Vec::new(),
            structural_mismatch: true,
        });
    }

    let parsed = match declared {
        ColType::String => Parsed::Str,
        ColType::Int => Parsed::Int(int_series_to_vec(series)?),
        ColType::Float => Parsed::Float(float_series_to_vec(series)?),
        ColType::Bool => Parsed::Bool(bool_series_to_vec(series)?),
        ColType::Date => Parsed::Date(date_series_to_vec(series)?),
        ColType::Datetime => Parsed::Datetime(datetime_series_to_micros(series)?),
    };
    let cast_failed = vec![false; raw.len()];
    Ok(ColumnData {
        raw,
        parsed,
        cast_failed,
        structural_mismatch: false,
    })
}

/// Whether a native Polars dtype satisfies the declared contract type.
fn dtype_matches(actual: &DataType, declared: ColType) -> bool {
    match declared {
        ColType::String => matches!(actual, DataType::String),
        ColType::Int => actual.is_integer(),
        ColType::Float => actual.is_float() || actual.is_integer(),
        ColType::Bool => matches!(actual, DataType::Boolean),
        ColType::Date => matches!(actual, DataType::Date),
        ColType::Datetime => matches!(actual, DataType::Datetime(_, _)),
    }
}

fn parse_bool(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "true" | "t" | "1" | "yes" | "y" => Some(true),
        "false" | "f" | "0" | "no" | "n" => Some(false),
        _ => None,
    }
}

/// Parses an ISO date into days since the Unix epoch.
fn parse_date_days(s: &str) -> Option<i32> {
    let d = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1)?;
    Some((d - epoch).num_days() as i32)
}

/// Parses an ISO/RFC-3339 datetime into microseconds since the Unix epoch (UTC).
///
/// A bare date (`2024-01-01`) is accepted as midnight UTC: a workbook column
/// of dates, or a partner feed that drops the time part on days with no
/// events, still fits a `datetime` column instead of failing every row.
fn parse_datetime_micros(s: &str) -> Option<i64> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp_micros());
    }
    const NAIVE: &[&str] = &[
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M:%S%.f",
        // Minute precision: spreadsheet exports and mainframe extracts.
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ];
    for fmt in NAIVE {
        if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(ndt.and_utc().timestamp_micros());
        }
    }
    let day = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    Some(
        day.and_time(chrono::NaiveTime::MIN)
            .and_utc()
            .timestamp_micros(),
    )
}

fn int_series_to_vec(series: &Series) -> Result<Vec<Option<i64>>, EngineError> {
    let s = series.cast(&DataType::Int64)?;
    let ca = s.i64()?;
    Ok(ca.into_iter().collect())
}

fn float_series_to_vec(series: &Series) -> Result<Vec<Option<f64>>, EngineError> {
    let s = series.cast(&DataType::Float64)?;
    let ca = s.f64()?;
    Ok(ca.into_iter().collect())
}

fn bool_series_to_vec(series: &Series) -> Result<Vec<Option<bool>>, EngineError> {
    let ca = series.bool()?;
    Ok(ca.into_iter().collect())
}

fn date_series_to_vec(series: &Series) -> Result<Vec<Option<i32>>, EngineError> {
    let s = series.cast(&DataType::Int32)?;
    let ca = s.i32()?;
    Ok(ca.into_iter().collect())
}

/// Converts any Datetime series to microseconds since epoch, UTC.
fn datetime_series_to_micros(series: &Series) -> Result<Vec<Option<i64>>, EngineError> {
    let micros = series.cast(&DataType::Datetime(TimeUnit::Microseconds, None))?;
    let s = micros.cast(&DataType::Int64)?;
    let ca = s.i64()?;
    Ok(ca.into_iter().collect())
}

#[cfg(test)]
mod parse_tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn datetime_forms() {
        let midnight = parse_datetime_micros("2024-01-01T00:00:00Z").unwrap();
        assert_eq!(parse_datetime_micros("2024-01-01"), Some(midnight));
        assert_eq!(parse_datetime_micros("2024-01-01T00:00:00"), Some(midnight));
        assert_eq!(
            parse_datetime_micros("2024-01-01 00:00:00.000"),
            Some(midnight)
        );
        assert_eq!(
            parse_datetime_micros("2024-01-01T12:30:00+02:00"),
            Some(midnight + (10 * 3600 + 30 * 60) * 1_000_000)
        );
        assert_eq!(parse_datetime_micros("2024-13-01"), None);
        assert_eq!(parse_datetime_micros("yesterday"), None);
        assert_eq!(parse_datetime_micros(""), None);
    }

    #[test]
    fn date_forms() {
        assert_eq!(parse_date_days("1970-01-02"), Some(1));
        assert_eq!(parse_date_days("1970-01-02T00:00:00"), None);
    }
}
