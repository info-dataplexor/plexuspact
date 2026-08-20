//! Small edit-distance helper used to produce "did you mean …?" hints in
//! parse errors (unknown top-level keys, unknown check names, unknown formats).

/// Levenshtein edit distance between two strings, in characters.
pub(crate) fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur: Vec<usize> = Vec::with_capacity(b.len() + 1);
        cur.push(i + 1);
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            let val = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
            cur.push(val);
        }
        prev = cur;
    }
    prev.last().copied().unwrap_or(0)
}

/// Returns the closest candidate to `input` if it is plausibly a typo.
///
/// The threshold scales with the input length so that short names only match
/// near-exact candidates while longer names tolerate a couple of edits.
pub(crate) fn did_you_mean<'a, I>(input: &str, candidates: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let input_lower = input.to_ascii_lowercase();
    let mut best: Option<(usize, &'a str)> = None;
    for candidate in candidates {
        let d = levenshtein(&input_lower, &candidate.to_ascii_lowercase());
        if best.map_or(true, |(bd, _)| d < bd) {
            best = Some((d, candidate));
        }
    }
    let (distance, candidate) = best?;
    let threshold = input.chars().count().max(3) / 3 + 1;
    (distance <= threshold).then_some(candidate)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn distance_basics() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("abc", ""), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("type", "type"), 0);
    }

    #[test]
    fn suggests_close_matches() {
        let fields = ["type", "required", "pii", "classification", "checks"];
        assert_eq!(did_you_mean("typ", fields), Some("type"));
        assert_eq!(did_you_mean("requird", fields), Some("required"));
        assert_eq!(
            did_you_mean("uniq", ["unique", "min", "max"]),
            Some("unique")
        );
        assert_eq!(
            did_you_mean("emial", ["email", "uuid", "url"]),
            Some("email")
        );
    }

    #[test]
    fn rejects_distant_matches() {
        let fields = ["type", "required", "checks"];
        assert_eq!(did_you_mean("zzzzzz", fields), None);
        assert_eq!(did_you_mean("owner", ["checks"]), None);
    }
}
