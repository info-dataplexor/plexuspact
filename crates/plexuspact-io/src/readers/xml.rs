//! Streaming XML reader.
//!
//! An XML feed is a sequence of record elements — `<order>` inside
//! `<orders>`, `<Employee>` inside `<Employees>` — and everything else in the
//! document is envelope. This reader walks the document once with a pull
//! parser, keeps only the record it is inside, and yields batches, so memory
//! is bounded by the batch size however large the file (US-6 applies to XML
//! as it does to NDJSON).
//!
//! ## Which element is a record
//!
//! `settings.input.xml_record` (or `--xml-record`) names it: a local name
//! (`order`, matched at any depth and case-insensitively) or a slash path
//! (`orders/order`, matched against the end of the element's path, so the
//! root may be left out). With neither, the first child element of
//! the root is the record and its same-named siblings are the rest — right
//! for the plain list every exporter writes, wrong for a document with a
//! `<header>` first, which the error message for zero records points at.
//!
//! ## How a record becomes a row
//!
//! Attributes of the record element become columns named after them. Child
//! elements with text become columns named after them; nested children get
//! dotted names (`address.city`), and attributes on children too
//! (`amount.currency`). An empty element, or one carrying `xsi:nil="true"`,
//! is a null. Namespace prefixes are dropped from names. When a child repeats
//! (`<item>` three times), the first one is the column's value — a record's
//! line items are a different dataset and belong in their own contract.
//!
//! Columns are pinned by the first batch in order of first appearance; a
//! column that only shows up later is appended to the batch it appears in,
//! and every batch carries every column seen so far.

use std::collections::HashMap;
use std::io::BufRead;

use polars::prelude::*;
use quick_xml::events::attributes::Attribute;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use crate::batch::{BatchSource, InputTyping};
use crate::error::IoError;
use crate::readers::frame_from_text_columns;

/// How record elements are recognized.
#[derive(Debug)]
enum RecordMatcher {
    /// The root's first child element (name learned on first sight).
    Auto(Option<String>),
    /// A local name at any depth.
    Name(String),
    /// A path of local names, matched against the end of the element path.
    Path(Vec<String>),
}

impl RecordMatcher {
    fn parse(spec: Option<&str>) -> Self {
        match spec.map(str::trim).filter(|s| !s.is_empty()) {
            None => RecordMatcher::Auto(None),
            Some(s) if s.contains('/') => RecordMatcher::Path(
                s.split('/')
                    .map(local_part)
                    .filter(|p| !p.is_empty())
                    .map(str::to_owned)
                    .collect(),
            ),
            Some(s) => RecordMatcher::Name(local_part(s).to_owned()),
        }
    }

    /// Whether the element just opened (already pushed onto `stack`) is a
    /// record. `Auto` learns its name from the first depth-2 element.
    fn matches(&mut self, stack: &[String]) -> bool {
        let Some(name) = stack.last() else {
            return false;
        };
        match self {
            RecordMatcher::Auto(learned) => {
                if stack.len() != 2 {
                    return false;
                }
                match learned {
                    Some(n) => n.eq_ignore_ascii_case(name),
                    None => {
                        *learned = Some(name.clone());
                        true
                    }
                }
            }
            RecordMatcher::Name(n) => n.eq_ignore_ascii_case(name),
            RecordMatcher::Path(parts) => {
                parts.len() <= stack.len()
                    && parts
                        .iter()
                        .rev()
                        .zip(stack.iter().rev())
                        .all(|(p, s)| p.eq_ignore_ascii_case(s))
            }
        }
    }

    fn describe(&self) -> String {
        match self {
            RecordMatcher::Auto(Some(n)) => n.clone(),
            RecordMatcher::Auto(None) => "<auto>".to_owned(),
            RecordMatcher::Name(n) => n.clone(),
            RecordMatcher::Path(p) => p.join("/"),
        }
    }
}

/// `ns:name` → `name`.
fn local_part(s: &str) -> &str {
    s.rsplit(':').next().unwrap_or(s).trim()
}

/// A record's fields in order of first appearance; a repeat is ignored.
#[derive(Default)]
struct Record {
    fields: Vec<(String, Option<String>)>,
}

impl Record {
    fn insert(&mut self, key: String, value: Option<String>) {
        if !self.fields.iter().any(|(k, _)| *k == key) {
            self.fields.push((key, value));
        }
    }
}

/// Streaming XML batch source.
pub struct XmlBatchSource {
    reader: Reader<Box<dyn BufRead + Send>>,
    buf: Vec<u8>,
    matcher: RecordMatcher,
    /// Local names of the open elements outside any record.
    stack: Vec<String>,
    /// Column order, pinned by first appearance and only ever extended.
    columns: Vec<String>,
    schema: Schema,
    batch_rows: usize,
    pending: Option<DataFrame>,
    records_read: u64,
    finished: bool,
    /// Element paths seen at the top of the document, for the "no records"
    /// message.
    seen: Vec<String>,
    path_display: String,
    codec: Option<&'static str>,
    size_bytes: Option<u64>,
}

impl std::fmt::Debug for XmlBatchSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XmlBatchSource")
            .field("records_read", &self.records_read)
            .field("columns", &self.columns)
            .finish_non_exhaustive()
    }
}

impl XmlBatchSource {
    /// Opens an XML source from any buffered reader (file, stdin, or a
    /// decompression stream — pass `codec` for better error messages then).
    pub fn from_reader(
        reader: Box<dyn BufRead + Send>,
        record: Option<&str>,
        batch_rows: usize,
        path_display: &str,
        codec: Option<&'static str>,
        size_bytes: Option<u64>,
    ) -> Result<Self, IoError> {
        let mut xml = Reader::from_reader(reader);
        {
            // Text is trimmed once a value is assembled (entity references
            // split it into several events), never per event.
            let cfg = xml.config_mut();
            cfg.expand_empty_elements = true;
            cfg.check_end_names = false;
        }
        let mut source = XmlBatchSource {
            reader: xml,
            buf: Vec::new(),
            matcher: RecordMatcher::parse(record),
            stack: Vec::new(),
            columns: Vec::new(),
            schema: Schema::default(),
            batch_rows: batch_rows.max(1),
            pending: None,
            records_read: 0,
            finished: false,
            seen: Vec::new(),
            path_display: path_display.to_string(),
            codec,
            size_bytes,
        };
        match source.read_batch()? {
            Some(df) => {
                source.schema = df.schema().as_ref().clone();
                source.pending = Some(df);
                Ok(source)
            }
            None if source.seen.is_empty() => Err(IoError::EmptyInput {
                path: path_display.to_string(),
            }),
            None => Err(IoError::XmlRecordNotFound {
                path: path_display.to_string(),
                record: source.matcher.describe(),
                seen: source.seen.join(", "),
            }),
        }
    }

    fn error(&self, e: quick_xml::Error) -> IoError {
        if let (quick_xml::Error::Io(io), Some(codec)) = (&e, self.codec) {
            return IoError::Decompress {
                path: self.path_display.clone(),
                codec,
                source: std::io::Error::new(io.kind(), io.to_string()),
            };
        }
        IoError::Malformed {
            path: self.path_display.clone(),
            format: "xml",
            position: format!(
                " (around record {}, byte {})",
                self.records_read + 1,
                self.reader.buffer_position()
            ),
            detail: e.to_string(),
        }
    }

    fn next_event(&mut self) -> Result<Event<'static>, IoError> {
        self.buf.clear();
        // `into_owned` frees the borrow on `self.buf` so the caller can keep
        // using `self`; record events are small.
        match self.reader.read_event_into(&mut self.buf) {
            Ok(ev) => Ok(ev.into_owned()),
            Err(e) => Err(self.error(e)),
        }
    }

    /// Reads records until a batch is full or the document ends.
    fn read_batch(&mut self) -> Result<Option<DataFrame>, IoError> {
        if self.finished {
            return Ok(None);
        }
        let mut records: Vec<Record> = Vec::new();
        while records.len() < self.batch_rows {
            match self.next_event()? {
                Event::Start(e) => {
                    let name = element_name(&e);
                    self.stack.push(name);
                    self.note_seen();
                    if self.matcher.matches(&self.stack) {
                        let record = self.read_record(&e)?;
                        self.stack.pop();
                        self.records_read += 1;
                        records.push(record);
                    }
                }
                Event::End(_) => {
                    self.stack.pop();
                }
                Event::Eof => {
                    self.finished = true;
                    break;
                }
                _ => {}
            }
        }
        if records.is_empty() {
            return Ok(None);
        }
        Ok(Some(self.frame(records)))
    }

    /// Remembers the path of an element near the top of the document.
    fn note_seen(&mut self) {
        if self.stack.len() <= 3 && self.seen.len() < 12 {
            let path = self.stack.join("/");
            if !self.seen.contains(&path) {
                self.seen.push(path);
            }
        }
    }

    /// Consumes the events of one record (the start tag has been read) up to
    /// and including its end tag.
    fn read_record(&mut self, start: &BytesStart<'_>) -> Result<Record, IoError> {
        let mut record = Record::default();
        for (key, value) in self.attributes(start)? {
            match value {
                AttrValue::Nil => {}
                AttrValue::Text(v) => record.insert(key, Some(v)),
            }
        }
        // Open child elements: (name, has_children, text so far).
        let mut path: Vec<(String, bool, String)> = Vec::new();
        loop {
            match self.next_event()? {
                Event::Start(e) => {
                    let name = element_name(&e);
                    if let Some(parent) = path.last_mut() {
                        parent.1 = true;
                    }
                    path.push((name, false, String::new()));
                    let key = dotted(&path);
                    for (attr, value) in self.attributes(&e)? {
                        match value {
                            AttrValue::Nil => record.insert(key.clone(), None),
                            AttrValue::Text(v) => record.insert(format!("{key}.{attr}"), Some(v)),
                        }
                    }
                }
                Event::Text(t) => {
                    if let Some(cur) = path.last_mut() {
                        let text = t
                            .xml10_content()
                            .map_err(|e| self.malformed(e.to_string()))?;
                        cur.2.push_str(&text);
                    }
                }
                Event::CData(c) => {
                    if let Some(cur) = path.last_mut() {
                        let text = c.decode().map_err(|e| self.malformed(e.to_string()))?;
                        cur.2.push_str(&text);
                    }
                }
                Event::GeneralRef(r) => {
                    if let Some(cur) = path.last_mut() {
                        if let Ok(Some(ch)) = r.resolve_char_ref() {
                            cur.2.push(ch);
                        } else if let Ok(name) = r.decode() {
                            match name.as_ref() {
                                "amp" => cur.2.push('&'),
                                "lt" => cur.2.push('<'),
                                "gt" => cur.2.push('>'),
                                "quot" => cur.2.push('"'),
                                "apos" => cur.2.push('\''),
                                _ => {}
                            }
                        }
                    }
                }
                Event::End(_) => {
                    let Some((_, had_children, text)) = path.last() else {
                        return Ok(record);
                    };
                    if !had_children {
                        let key = dotted(&path);
                        let value = text.trim();
                        record.insert(
                            key,
                            if value.is_empty() {
                                None
                            } else {
                                Some(value.to_owned())
                            },
                        );
                    }
                    path.pop();
                }
                Event::Eof => {
                    return Err(self.malformed(
                        "the document ended inside a record (missing closing tag)".to_owned(),
                    ));
                }
                _ => {}
            }
        }
    }

    fn malformed(&self, detail: String) -> IoError {
        IoError::Malformed {
            path: self.path_display.clone(),
            format: "xml",
            position: format!(" (around record {})", self.records_read + 1),
            detail,
        }
    }

    /// Decoded attributes of an element, namespace declarations left out.
    fn attributes(&self, e: &BytesStart<'_>) -> Result<Vec<(String, AttrValue)>, IoError> {
        let mut out = Vec::new();
        for attr in e.attributes() {
            let attr: Attribute<'_> = attr.map_err(|err| self.malformed(err.to_string()))?;
            if attr.key.as_ref().starts_with(b"xmlns") {
                continue;
            }
            let name = String::from_utf8_lossy(attr.key.local_name().as_ref()).into_owned();
            let value = attr
                .normalized_value(XmlVersion::Implicit1_0)
                .map_err(|err| self.malformed(err.to_string()))?
                .into_owned();
            let is_nil =
                name == "nil" && attr.key.as_ref() != b"nil" && (value == "true" || value == "1");
            out.push((
                name,
                if is_nil {
                    AttrValue::Nil
                } else {
                    AttrValue::Text(value)
                },
            ));
        }
        Ok(out)
    }

    /// Builds the batch frame, extending the pinned column list with any
    /// column seen for the first time.
    fn frame(&mut self, records: Vec<Record>) -> DataFrame {
        let mut index: HashMap<String, usize> = self
            .columns
            .iter()
            .enumerate()
            .map(|(i, c)| (c.clone(), i))
            .collect();
        for r in &records {
            for (k, _) in &r.fields {
                if !index.contains_key(k) {
                    index.insert(k.clone(), self.columns.len());
                    self.columns.push(k.clone());
                }
            }
        }
        let width = self.columns.len();
        let rows: Vec<Vec<Option<String>>> = records
            .into_iter()
            .map(|r| {
                let mut row = vec![None; width];
                for (k, v) in r.fields {
                    if let Some(&i) = index.get(&k) {
                        row[i] = v;
                    }
                }
                row
            })
            .collect();
        frame_from_text_columns(&self.columns, rows)
    }
}

/// An attribute is text, or the `xsi:nil="true"` marker.
enum AttrValue {
    Text(String),
    Nil,
}

/// Local name of an element, prefix dropped.
fn element_name(e: &BytesStart<'_>) -> String {
    String::from_utf8_lossy(e.local_name().as_ref()).into_owned()
}

/// `a.b.c` for the open child elements.
fn dotted(path: &[(String, bool, String)]) -> String {
    path.iter()
        .map(|(n, _, _)| n.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

impl BatchSource for XmlBatchSource {
    fn schema(&self) -> &Schema {
        &self.schema
    }

    fn typing(&self) -> InputTyping {
        InputTyping::Stringly
    }

    fn next_batch(&mut self) -> Result<Option<DataFrame>, IoError> {
        if let Some(df) = self.pending.take() {
            return Ok(Some(df));
        }
        self.read_batch()
    }

    fn size_bytes(&self) -> Option<u64> {
        self.size_bytes
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use std::io::Cursor;

    fn open(xml: &str, record: Option<&str>, batch: usize) -> Result<XmlBatchSource, IoError> {
        XmlBatchSource::from_reader(
            Box::new(Cursor::new(xml.as_bytes().to_vec())),
            record,
            batch,
            "test.xml",
            None,
            None,
        )
    }

    fn all_rows(src: &mut XmlBatchSource) -> DataFrame {
        let mut frames = Vec::new();
        while let Some(df) = src.next_batch().unwrap() {
            frames.push(df);
        }
        let mut it = frames.into_iter();
        let mut acc = it.next().unwrap();
        for f in it {
            acc = acc.vstack(&f).unwrap();
        }
        acc
    }

    fn col(df: &DataFrame, name: &str) -> Vec<Option<String>> {
        df.column(name)
            .unwrap()
            .as_materialized_series()
            .str()
            .unwrap()
            .into_iter()
            .map(|v| v.map(str::to_owned))
            .collect()
    }

    const ORDERS: &str = r#"<?xml version="1.0"?>
<orders xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
  <order id="1" status="paid">
    <customer>Ada &amp; Co</customer>
    <amount currency="EUR">12.50</amount>
    <address><city>Zürich</city></address>
    <note xsi:nil="true"/>
  </order>
  <order id="2">
    <customer><![CDATA[Bob <b>]]></customer>
    <amount currency="USD">7</amount>
    <address><city></city></address>
    <note>late</note>
    <item>a</item><item>b</item>
  </order>
</orders>"#;

    #[test]
    fn auto_record_reads_attributes_children_and_nesting() {
        let mut src = open(ORDERS, None, 50).unwrap();
        let df = all_rows(&mut src);
        assert_eq!(df.height(), 2);
        assert_eq!(
            src.columns,
            vec![
                "id",
                "status",
                "customer",
                "amount.currency",
                "amount",
                "address.city",
                "note",
                "item"
            ]
        );
        assert_eq!(col(&df, "id"), vec![Some("1".into()), Some("2".into())]);
        assert_eq!(col(&df, "status"), vec![Some("paid".into()), None]);
        assert_eq!(
            col(&df, "customer"),
            vec![Some("Ada & Co".into()), Some("Bob <b>".into())]
        );
        assert_eq!(
            col(&df, "amount"),
            vec![Some("12.50".into()), Some("7".into())]
        );
        assert_eq!(col(&df, "address.city"), vec![Some("Zürich".into()), None]);
        assert_eq!(col(&df, "note"), vec![None, Some("late".into())]);
        assert_eq!(col(&df, "item"), vec![None, Some("a".into())]);
        assert_eq!(src.typing(), InputTyping::Stringly);
    }

    #[test]
    fn batches_keep_order_and_pad_columns() {
        let mut src = open(ORDERS, Some("order"), 1).unwrap();
        let first = src.next_batch().unwrap().unwrap();
        assert_eq!(first.height(), 1);
        assert!(
            first.column("item").is_err(),
            "item only appears in record 2"
        );
        let second = src.next_batch().unwrap().unwrap();
        assert_eq!(second.height(), 1);
        assert_eq!(col(&second, "status"), vec![None]);
        assert_eq!(col(&second, "item"), vec![Some("a".into())]);
        assert!(src.next_batch().unwrap().is_none());
        assert_eq!(src.records_read, 2);
    }

    #[test]
    fn path_and_name_matchers() {
        let xml = r#"<root><meta><generated>x</generated></meta><data><row><v>1</v></row><row><v>2</v></row></data></root>"#;
        // Auto would take <meta>; a path says which element is the record.
        let mut src = open(xml, Some("data/row"), 10).unwrap();
        assert_eq!(
            col(&all_rows(&mut src), "v"),
            vec![Some("1".into()), Some("2".into())]
        );
        let mut src = open(xml, Some("ROW"), 10).unwrap();
        assert_eq!(all_rows(&mut src).height(), 2);
        let mut src = open(xml, None, 10).unwrap();
        assert_eq!(
            col(&all_rows(&mut src), "generated"),
            vec![Some("x".into())]
        );
    }

    #[test]
    fn no_records_is_actionable() {
        let xml = r#"<root><meta><generated>x</generated></meta></root>"#;
        let e = open(xml, Some("order"), 10).unwrap_err().to_string();
        assert!(e.contains("no `order` records"), "{e}");
        assert!(e.contains("root/meta"), "{e}");
        assert!(e.contains("xml_record"), "{e}");
    }

    #[test]
    fn empty_and_malformed() {
        assert!(matches!(
            open("", None, 10).unwrap_err(),
            IoError::EmptyInput { .. }
        ));
        let e = open("<orders><order><id>1</id>", None, 10).unwrap_err();
        assert!(matches!(e, IoError::Malformed { .. }), "{e}");
    }

    #[test]
    fn namespaced_names_are_stripped() {
        let xml = r#"<s:Envelope xmlns:s="urn:x"><s:Body><s:Row s:id="9"><s:Name>n</s:Name></s:Row></s:Body></s:Envelope>"#;
        let mut src = open(xml, Some("Row"), 10).unwrap();
        let df = all_rows(&mut src);
        assert_eq!(col(&df, "id"), vec![Some("9".into())]);
        assert_eq!(col(&df, "Name"), vec![Some("n".into())]);
    }
}
