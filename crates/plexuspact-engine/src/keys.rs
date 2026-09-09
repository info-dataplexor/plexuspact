//! Key sets: the hashed primary keys of one delivery, kept so that another
//! dataset's `references` check can ask "does this value exist over there?"
//! without ever holding the other dataset's rows.
//!
//! Hashes are stable — the same key tuple hashes the same way on every
//! machine and in every version — because the cloud stores a passing run's
//! key set and compares it against a run made later, somewhere else.

use std::collections::BTreeMap;
use std::sync::Arc;

use ahash::AHashSet;
use plexuspact_io::BatchSource;
use sha2::{Digest, Sha256};

use crate::EngineError;

/// Separator written between the parts of a composite key before hashing, so
/// `("ab", "c")` and `("a", "bc")` hash differently.
const PART_SEPARATOR: &[u8] = &[0x1f];

/// Stable 64-bit hash of one key tuple. Every part is length-prefixed and
/// separated, so tuples of different shape never collide by construction.
pub fn hash_key<'a>(parts: impl IntoIterator<Item = &'a str>) -> u64 {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
        hasher.update(PART_SEPARATOR);
    }
    let digest = hasher.finalize();
    let mut first = [0u8; 8];
    first.copy_from_slice(&digest[..8]);
    u64::from_le_bytes(first)
}

/// The distinct key tuples one delivery carried, as stable hashes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeySet {
    /// The key columns, in the order the parts were hashed.
    pub columns: Vec<String>,
    /// Distinct hashes of every complete (no-null) key tuple.
    hashes: AHashSet<u64>,
}

impl KeySet {
    /// An empty set over the given columns.
    pub fn new(columns: Vec<String>) -> Self {
        KeySet {
            columns,
            hashes: AHashSet::new(),
        }
    }

    /// Rebuilds a set from hashes stored earlier (any order, duplicates fine).
    pub fn from_hashes(columns: Vec<String>, hashes: impl IntoIterator<Item = u64>) -> Self {
        KeySet {
            columns,
            hashes: hashes.into_iter().collect(),
        }
    }

    /// Records one key tuple; `true` when it was not seen before.
    pub fn insert(&mut self, hash: u64) -> bool {
        self.hashes.insert(hash)
    }

    /// Whether a key tuple with this hash is present.
    pub fn contains(&self, hash: u64) -> bool {
        self.hashes.contains(&hash)
    }

    /// Number of distinct key tuples.
    pub fn len(&self) -> usize {
        self.hashes.len()
    }

    /// Whether no key tuple was recorded.
    pub fn is_empty(&self) -> bool {
        self.hashes.is_empty()
    }

    /// The hashes in ascending order — the form to store.
    pub fn sorted_hashes(&self) -> Vec<u64> {
        let mut out: Vec<u64> = self.hashes.iter().copied().collect();
        out.sort_unstable();
        out
    }
}

/// The key sets a run may consult, by the name of the dataset each belongs to.
#[derive(Debug, Clone, Default)]
pub struct ReferenceSets {
    sets: BTreeMap<String, Arc<KeySet>>,
}

impl ReferenceSets {
    /// No key sets at all — every `references` check reports it could not run.
    pub fn none() -> Self {
        Self::default()
    }

    /// Makes `set` answer for `dataset`.
    pub fn insert(&mut self, dataset: impl Into<String>, set: KeySet) {
        self.sets.insert(dataset.into(), Arc::new(set));
    }

    /// The key set registered for `dataset`, if any.
    pub fn get(&self, dataset: &str) -> Option<&Arc<KeySet>> {
        self.sets.get(dataset)
    }

    /// Names of the datasets a set is registered for.
    pub fn datasets(&self) -> impl Iterator<Item = &str> {
        self.sets.keys().map(String::as_str)
    }

    /// Whether any set is registered.
    pub fn is_empty(&self) -> bool {
        self.sets.is_empty()
    }
}

/// Reads every batch of `source` and collects the distinct key tuples found
/// in `columns`. Rows with a null in any key column are left out, exactly as
/// the `primary_key` and `references` checks leave them out.
///
/// This is what the CLI's `--reference name=path` does with the file it is
/// handed, and what a cloud does with a delivery it has already validated.
pub fn collect_key_set(
    source: &mut dyn BatchSource,
    columns: &[String],
) -> Result<KeySet, EngineError> {
    let mut set = KeySet::new(columns.to_vec());
    let present: Vec<String> = source
        .schema()
        .iter_names()
        .map(|n| n.to_string())
        .collect();
    if let Some(missing) = columns.iter().find(|c| !present.contains(c)) {
        return Err(EngineError::MissingKeyColumn {
            column: missing.clone(),
        });
    }
    while let Some(df) = source.next_batch()? {
        if df.height() == 0 {
            continue;
        }
        let raw = columns
            .iter()
            .map(|name| crate::column::raw_strings(&df, name))
            .collect::<Result<Vec<_>, _>>()?;
        let raw: Vec<&[Option<String>]> = raw.iter().map(Vec::as_slice).collect();
        for row in 0..df.height() {
            if let Some(hash) = hash_row(&raw, row) {
                set.insert(hash);
            }
        }
    }
    Ok(set)
}

/// Hashes row `row` of the given raw key columns, or `None` when any part is
/// null (a null never forms a key).
pub(crate) fn hash_row(raw: &[&[Option<String>]], row: usize) -> Option<u64> {
    let mut parts: Vec<&str> = Vec::with_capacity(raw.len());
    for column in raw {
        parts.push(column[row].as_deref()?);
    }
    Some(hash_key(parts))
}

/// Renders row `row` of the key columns for a failure sample: the bare value
/// for a single-column key, `a=1, b=2` for a composite one.
pub(crate) fn render_row(columns: &[String], raw: &[&[Option<String>]], row: usize) -> String {
    if raw.len() == 1 {
        return raw[0][row].clone().unwrap_or_default();
    }
    columns
        .iter()
        .zip(raw)
        .map(|(name, values)| format!("{name}={}", values[row].as_deref().unwrap_or("<null>")))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const PINNED_42: u64 = 17_346_295_814_714_295_889;

    #[test]
    fn hash_is_stable_and_shape_sensitive() {
        assert_ne!(hash_key(["ab", "c"]), hash_key(["a", "bc"]));
        assert_ne!(hash_key(["abc"]), hash_key(["ab", "c"]));
        assert_eq!(hash_key(["x", "y"]), hash_key(["x", "y"]));
    }

    /// A pinned value: changing the hashing scheme would silently break every
    /// stored key set, so the number itself is under test.
    #[test]
    fn pinned_hash_value() {
        assert_eq!(hash_key(["42"]), PINNED_42);
    }

    #[test]
    fn hash_row_skips_nulls_and_renders_composites() {
        let owned = [
            vec![Some("1".to_owned()), None, Some("3".to_owned())],
            vec![Some("a".to_owned()), Some("b".to_owned()), None],
        ];
        let raw: Vec<&[Option<String>]> = owned.iter().map(Vec::as_slice).collect();
        assert!(hash_row(&raw, 0).is_some());
        assert!(hash_row(&raw, 1).is_none());
        assert!(hash_row(&raw, 2).is_none());
        let cols = vec!["id".to_owned(), "code".to_owned()];
        assert_eq!(render_row(&cols, &raw, 0), "id=1, code=a");
        assert_eq!(render_row(&cols, &raw, 2), "id=3, code=<null>");
        assert_eq!(render_row(&cols[..1], &raw[..1], 0), "1");
    }

    #[test]
    fn key_set_round_trips_through_sorted_hashes() {
        let mut set = KeySet::new(vec!["id".into()]);
        for v in ["3", "1", "2", "1"] {
            set.insert(hash_key([v]));
        }
        assert_eq!(set.len(), 3);
        let stored = set.sorted_hashes();
        let back = KeySet::from_hashes(vec!["id".into()], stored.clone());
        assert_eq!(back, set);
        assert!(back.contains(hash_key(["2"])));
        assert!(!back.contains(hash_key(["4"])));
        assert!(stored.windows(2).all(|w| w[0] < w[1]));
    }
}
