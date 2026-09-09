//! Read options shared by all readers.
//!
//! Options come from two places and layer in this order: the contract's
//! `settings.input` block (see [`ReadOptions::apply_input_settings`]), then
//! anything the caller sets explicitly on top (command-line flags). The
//! contract is the durable place; the flags are for the one-off.

use plexuspact_contract::{FixedWidthField, InputSettings};

use crate::error::IoError;
use crate::format::InputFormat;

/// Default number of rows per streamed batch.
pub const DEFAULT_BATCH_ROWS: usize = 50_000;

/// Character encoding handling for CSV input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CsvEncoding {
    /// Strict UTF-8; invalid bytes are an error.
    #[default]
    Utf8,
    /// Invalid UTF-8 bytes are replaced with `�`.
    Utf8Lossy,
}

/// CSV/TSV parsing options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CsvOptions {
    /// Field delimiter. `None` = `,` for CSV, `\t` for TSV.
    pub delimiter: Option<u8>,
    /// Quote character (default `"`). `None` disables quoting.
    pub quote_char: Option<u8>,
    /// Whether the first record is a header row (default `true`).
    pub has_header: bool,
    /// Lines to skip before the header (title lines a report writer puts at
    /// the top). Default 0.
    pub skip_rows: usize,
    /// Encoding handling.
    pub encoding: CsvEncoding,
}

impl Default for CsvOptions {
    fn default() -> Self {
        CsvOptions {
            delimiter: None,
            quote_char: Some(b'"'),
            has_header: true,
            skip_rows: 0,
            encoding: CsvEncoding::Utf8,
        }
    }
}

impl CsvOptions {
    /// Effective delimiter for the given format.
    pub fn effective_delimiter(&self, format: InputFormat) -> u8 {
        self.delimiter.unwrap_or(match format {
            InputFormat::Tsv => b'\t',
            _ => b',',
        })
    }
}

/// Excel / OpenDocument workbook options.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExcelOptions {
    /// The sheet holding the data: a name (matched case-insensitively) or a
    /// 1-based position. `None` = the first sheet.
    pub sheet: Option<String>,
    /// Rows to skip before the header row (default 0).
    pub skip_rows: usize,
    /// Whether the first row (after `skip_rows`) names the columns. When
    /// `false` the columns are `column_1`, `column_2`, … Default `true`.
    pub has_header: Option<bool>,
}

impl ExcelOptions {
    /// Whether a header row is expected (default `true`).
    pub fn header(&self) -> bool {
        self.has_header.unwrap_or(true)
    }
}

/// XML options.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct XmlOptions {
    /// The element that is one record: a local name (`order`, matched at any
    /// depth) or a slash path from the root (`orders/order`). `None` = the
    /// first child element of the root, and its same-named siblings.
    pub record: Option<String>,
}

/// One resolved fixed-width span: 0-based, end-exclusive character positions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedWidthSpan {
    /// Column name.
    pub name: String,
    /// First character, 0-based.
    pub start: usize,
    /// One past the last character.
    pub end: usize,
}

/// Fixed-width text options.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FixedWidthOptions {
    /// The spans, in line order.
    pub fields: Vec<FixedWidthSpan>,
    /// Whether the first line is a header to skip (default `false`: the
    /// layout names the columns).
    pub has_header: bool,
    /// Lines to skip before the data (default 0).
    pub skip_rows: usize,
}

impl FixedWidthOptions {
    /// Parses the command-line form: comma-separated `name=start-end`
    /// (1-based, inclusive) or `name=width` (laid right after the previous
    /// field). `id=1-8,name=9-40,amount=12` means `amount` is columns 41–52.
    pub fn parse(spec: &str) -> Result<Self, IoError> {
        Self::from_fields(&Self::parse_fields(spec)?, "--fixed-width")
    }

    /// Parses the command-line form into contract-shaped fields (1-based,
    /// `start`/`end` or `width`), so a caller can record the layout as the
    /// user wrote it.
    pub fn parse_fields(spec: &str) -> Result<Vec<FixedWidthField>, IoError> {
        let bad = |detail: String| IoError::InvalidOption {
            key: "--fixed-width".to_owned(),
            detail,
        };
        let mut fields = Vec::new();
        for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (name, span) = part.split_once('=').ok_or_else(|| {
                bad(format!(
                    "`{part}` is not `name=start-end` or `name=width`; \
                     example: `id=1-8,name=9-40,amount=12`"
                ))
            })?;
            let name = name.trim();
            if name.is_empty() {
                return Err(bad(format!("`{part}` has no field name")));
            }
            let num = |s: &str| -> Result<u32, IoError> {
                s.trim()
                    .parse::<u32>()
                    .map_err(|_| bad(format!("`{s}` in `{part}` is not a number")))
            };
            let field = match span.split_once('-') {
                Some((a, b)) => FixedWidthField {
                    name: name.to_owned(),
                    start: Some(num(a)?),
                    end: Some(num(b)?),
                    width: None,
                },
                None => FixedWidthField {
                    name: name.to_owned(),
                    start: None,
                    end: None,
                    width: Some(num(span)?),
                },
            };
            fields.push(field);
        }
        if fields.is_empty() {
            return Err(bad(
                "no fields given; example: `id=1-8,name=9-40,amount=12`".to_owned(),
            ));
        }
        Ok(fields)
    }

    /// Resolves contract-style fields (1-based inclusive, `start` optional,
    /// `end` or `width`) into spans. `key` names the source for error messages.
    pub fn from_fields(fields: &[FixedWidthField], key: &str) -> Result<Self, IoError> {
        let bad = |detail: String| IoError::InvalidOption {
            key: key.to_owned(),
            detail,
        };
        let mut out = Vec::with_capacity(fields.len());
        let mut cursor: u32 = 1;
        for f in fields {
            if f.name.trim().is_empty() {
                return Err(bad("a field has no name".to_owned()));
            }
            let start = f.start.unwrap_or(cursor);
            if start == 0 {
                return Err(bad(format!(
                    "`{}` starts at 0; positions are 1-based",
                    f.name
                )));
            }
            let end = match (f.end, f.width) {
                (Some(e), _) if e < start => {
                    return Err(bad(format!(
                        "`{}` ends at {e}, before it starts at {start}",
                        f.name
                    )))
                }
                (Some(e), _) => e,
                (None, Some(0)) => return Err(bad(format!("`{}` has width 0", f.name))),
                (None, Some(w)) => start + w - 1,
                (None, None) => {
                    return Err(bad(format!("`{}` has neither `end` nor `width`", f.name)))
                }
            };
            if out.iter().any(|s: &FixedWidthSpan| s.name == f.name.trim()) {
                return Err(bad(format!("`{}` is listed twice", f.name)));
            }
            out.push(FixedWidthSpan {
                name: f.name.trim().to_owned(),
                start: (start - 1) as usize,
                end: end as usize,
            });
            cursor = end + 1;
        }
        Ok(FixedWidthOptions {
            fields: out,
            has_header: false,
            skip_rows: 0,
        })
    }
}

/// How columns are typed in the yielded batches (see crate docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TypingMode {
    /// Text formats deliver every column as `String`; the engine casts and
    /// counts per-row type mismatches (check path). Parquet ignores this and
    /// always delivers native dtypes.
    #[default]
    Stringly,
    /// Deliver inferred/native dtypes (profiling path for `init`).
    Inferred,
}

/// Options for opening a [`crate::BatchSource`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadOptions {
    /// Explicit format (`--input-format`); `None` = detect from extension.
    pub format: Option<InputFormat>,
    /// CSV/TSV options.
    pub csv: CsvOptions,
    /// Target rows per batch (readers may deliver slightly different sizes).
    pub batch_rows: usize,
    /// Stringly (check path) or inferred (profiling) typing.
    pub typing: TypingMode,
    /// For JSON input only: a dotted path to the record array nested inside a
    /// wrapping object (e.g. `results`, `data.items`). `None` = the document is
    /// itself the array. Leading `.`/`$.` are tolerated. See `--json-path`.
    pub json_path: Option<String>,
    /// Excel options.
    pub excel: ExcelOptions,
    /// XML options.
    pub xml: XmlOptions,
    /// Fixed-width layout; required to read fixed-width input.
    pub fixed_width: Option<FixedWidthOptions>,
}

impl Default for ReadOptions {
    fn default() -> Self {
        ReadOptions {
            format: None,
            csv: CsvOptions::default(),
            batch_rows: DEFAULT_BATCH_ROWS,
            typing: TypingMode::Stringly,
            json_path: None,
            excel: ExcelOptions::default(),
            xml: XmlOptions::default(),
            fixed_width: None,
        }
    }
}

impl ReadOptions {
    /// Layers a contract's `settings.input` onto these options. Only what the
    /// block sets is touched, so caller flags applied afterwards still win.
    pub fn apply_input_settings(&mut self, input: &InputSettings) -> Result<(), IoError> {
        if let Some(f) = input.format {
            self.format = Some(f);
        }
        if let Some(d) = &input.delimiter {
            self.csv.delimiter = Some(delimiter_byte(d, "settings.input.delimiter")?);
        }
        if let Some(h) = input.has_header {
            self.csv.has_header = h;
            self.excel.has_header = Some(h);
        }
        if let Some(n) = input.skip_rows {
            self.csv.skip_rows = n as usize;
            self.excel.skip_rows = n as usize;
        }
        if let Some(p) = &input.json_path {
            self.json_path = Some(p.clone());
        }
        if let Some(s) = &input.sheet {
            self.excel.sheet = Some(s.clone());
        }
        if let Some(r) = &input.xml_record {
            self.xml.record = Some(r.clone());
        }
        if !input.fixed_width.is_empty() {
            let mut fw =
                FixedWidthOptions::from_fields(&input.fixed_width, "settings.input.fixed_width")?;
            fw.has_header = input.has_header.unwrap_or(false);
            fw.skip_rows = input.skip_rows.unwrap_or(0) as usize;
            self.fixed_width = Some(fw);
        }
        Ok(())
    }
}

/// Turns a one-character delimiter (or the escapes `\t`, `tab`, `\|`) into
/// the byte the CSV parser takes.
pub fn delimiter_byte(d: &str, key: &str) -> Result<u8, IoError> {
    let ch = match d {
        "\\t" | "tab" | "TAB" => '\t',
        "\\|" => '|',
        other => {
            let mut chars = other.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => c,
                _ => {
                    return Err(IoError::InvalidOption {
                        key: key.to_owned(),
                        detail: format!(
                            "`{d}` is not a single character; use one character such as `;` or `|`, or `\\t` for tab"
                        ),
                    })
                }
            }
        }
    };
    u8::try_from(ch).map_err(|_| IoError::InvalidOption {
        key: key.to_owned(),
        detail: format!("`{d}` is not a single-byte character; the CSV delimiter must be ASCII"),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn spans(o: &FixedWidthOptions) -> Vec<(&str, usize, usize)> {
        o.fields
            .iter()
            .map(|f| (f.name.as_str(), f.start, f.end))
            .collect()
    }

    #[test]
    fn fixed_width_spec_parses_ranges_and_widths() {
        let o = FixedWidthOptions::parse("id=1-8, name=9-40,amount=12").unwrap();
        assert_eq!(
            spans(&o),
            vec![("id", 0, 8), ("name", 8, 40), ("amount", 40, 52)]
        );
    }

    #[test]
    fn fixed_width_spec_rejects_nonsense() {
        for bad in [
            "id",
            "id=",
            "id=a-b",
            "id=8-1",
            "id=0-4",
            "",
            "id=1-4,id=5-8",
            "x=0",
        ] {
            let e = FixedWidthOptions::parse(bad).unwrap_err().to_string();
            assert!(e.starts_with("--fixed-width:"), "{bad}: {e}");
        }
    }

    #[test]
    fn contract_fields_resolve_defaults() {
        let fields = vec![
            FixedWidthField {
                name: "id".into(),
                start: None,
                end: None,
                width: Some(4),
            },
            FixedWidthField {
                name: "name".into(),
                start: Some(10),
                end: Some(20),
                width: None,
            },
            FixedWidthField {
                name: "flag".into(),
                start: None,
                end: None,
                width: Some(1),
            },
        ];
        let o = FixedWidthOptions::from_fields(&fields, "settings.input.fixed_width").unwrap();
        assert_eq!(
            spans(&o),
            vec![("id", 0, 4), ("name", 9, 20), ("flag", 20, 21)]
        );
    }

    #[test]
    fn input_settings_layer_onto_options() {
        let input = InputSettings {
            format: Some(InputFormat::Excel),
            delimiter: Some(";".into()),
            has_header: Some(false),
            sheet: Some("Data".into()),
            skip_rows: Some(2),
            ..Default::default()
        };
        let mut o = ReadOptions::default();
        o.apply_input_settings(&input).unwrap();
        assert_eq!(o.format, Some(InputFormat::Excel));
        assert_eq!(o.csv.delimiter, Some(b';'));
        assert!(!o.csv.has_header);
        assert_eq!(o.excel.sheet.as_deref(), Some("Data"));
        assert_eq!(o.excel.skip_rows, 2);
        assert_eq!(o.excel.has_header, Some(false));
    }

    #[test]
    fn delimiter_forms() {
        assert_eq!(delimiter_byte(";", "k").unwrap(), b';');
        assert_eq!(delimiter_byte("\\t", "k").unwrap(), b'\t');
        assert_eq!(delimiter_byte("tab", "k").unwrap(), b'\t');
        assert!(delimiter_byte(";;", "k").is_err());
        assert!(delimiter_byte("", "k").is_err());
        assert!(delimiter_byte("→", "k").is_err());
    }
}
