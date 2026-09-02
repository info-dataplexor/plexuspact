//! Semantic contract diff (architecture doc 03 §7).
//!
//! [`diff`] is a pure function classifying every difference between two
//! contracts as [`Impact::Breaking`], [`Impact::NonBreaking`], or
//! [`Impact::Cosmetic`]. The classification table:
//!
//! | change | impact |
//! |---|---|
//! | column removed, type changed, `required` added, required column added | Breaking |
//! | check added at `error` severity, check tightened, severity warn→error | Breaking |
//! | `columns_exact` enabled, `allow_extra_columns` disabled, `on_type_mismatch` warn→error | Breaking |
//! | dataset renamed | Breaking |
//! | a column's or the dataset's written definition redefined or withdrawn | Semantic |
//! | a column's `stability` weakened, a `sunset` date set | Semantic |
//! | a `sunset` or migration window brought forward | Breaking |
//! | a personal-data tag added, changed or withdrawn; a handling level changed or withdrawn | Semantic |
//! | optional column added, check removed/loosened, severity error→warn, new warn check, `required` removed | NonBreaking |
//! | owner/version/consumers edits, a definition, handling level or `pii: none` written down for the first time | Cosmetic |
//!
//! The semantic tier exists because the classification above used to have a
//! hole in it. "`amount` now means net of refunds" changes no check, fails no
//! delivery and moves no data — and silently breaks every dashboard built on
//! the old meaning. Filed as cosmetic it reached nobody; filed as breaking it
//! would have stopped pipelines that are, mechanically, fine. It is its own
//! kind of change: nothing executes differently, and the agreement changed.

use std::fmt;

use crate::model::{ColumnCheck, ColumnDef, Contract, DatasetCheck, PiiKind, Severity, Stability};

/// How a change affects downstream consumers of the data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Impact {
    /// Data that passed the old contract may fail the new one (or consumers
    /// may lose a column/type they relied on).
    Breaking,
    /// The *meaning* changed while the mechanics did not.
    ///
    /// Ranked second, above `NonBreaking`, and deliberately: a redefinition is
    /// the one change that never announces itself. A tightened check fails a
    /// delivery and somebody investigates within the hour. "`amount` now means
    /// net of refunds" passes every check ever written and quietly falsifies
    /// every number built on the old meaning, for as long as nobody notices.
    Semantic,
    /// The contract got looser or gained warn-level reporting; passing data
    /// keeps passing.
    NonBreaking,
    /// Metadata only; neither validation behavior nor meaning is changed.
    Cosmetic,
}

impl fmt::Display for Impact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Impact::Breaking => f.write_str("breaking"),
            Impact::Semantic => f.write_str("semantic"),
            Impact::NonBreaking => f.write_str("non-breaking"),
            Impact::Cosmetic => f.write_str("cosmetic"),
        }
    }
}

/// One classified difference between two contracts.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    /// Classification of the change.
    pub impact: Impact,
    /// Dotted path to the changed element, e.g. `columns.age.checks[min]`.
    pub path: String,
    /// Human-readable description of what changed.
    pub description: String,
}

impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}: {}", self.impact, self.path, self.description)
    }
}

/// How the *parameters* of a check moved between old and new.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rel {
    /// Parameters identical.
    Same,
    /// Stricter: data that passed before may now fail.
    Tightened,
    /// Looser: everything that passed before still passes.
    Loosened,
    /// Incomparable (regex swapped, enum values both added and removed, …);
    /// treated as tightening because we cannot prove safety.
    Changed,
}

/// Computes the semantic diff between two contracts.
///
/// `diff(x, x)` is always empty. Order matters: `diff(old, new)` answers
/// "what happens to producers currently satisfying `old` when `new` lands?".
pub fn diff(old: &Contract, new: &Contract) -> Vec<Change> {
    let mut out = Vec::new();

    if old.dataset != new.dataset {
        out.push(Change {
            impact: Impact::Breaking,
            path: "dataset".into(),
            description: format!(
                "dataset renamed from `{}` to `{}` (contract identity changes)",
                old.dataset, new.dataset
            ),
        });
    }
    push_cosmetic_opt(&mut out, "owner", &old.owner, &new.owner);
    push_definition(
        &mut out,
        "description",
        "the dataset's written definition",
        &old.description,
        &new.description,
    );
    push_cosmetic_opt(&mut out, "version", &old.version, &new.version);
    if old.consumers != new.consumers {
        out.push(Change {
            impact: Impact::Cosmetic,
            path: "consumers".into(),
            description: "consumers list changed".into(),
        });
    }

    diff_migration(old, new, &mut out);

    diff_columns(old, new, &mut out);
    diff_dataset_checks(old, new, &mut out);
    diff_settings(old, new, &mut out);

    out
}

/// Records a cosmetic change for an optional metadata field.
fn push_cosmetic_opt(
    out: &mut Vec<Change>,
    path: &str,
    old: &Option<String>,
    new: &Option<String>,
) {
    if old != new {
        let description = match (old, new) {
            (None, Some(v)) => format!("{path} set to `{v}`"),
            (Some(v), None) => format!("{path} `{v}` removed"),
            (Some(a), Some(b)) => format!("{path} changed from `{a}` to `{b}`"),
            (None, None) => return,
        };
        out.push(Change {
            impact: Impact::Cosmetic,
            path: path.to_string(),
            description,
        });
    }
}

/// Records a change to a written definition, at the tier it deserves.
///
/// Three cases, and they are genuinely different. Writing a definition down for
/// the first time changes nothing that was ever agreed — the meaning was always
/// whatever it was, and now it is on paper: cosmetic. *Changing* one is a
/// redefinition, which is the whole reason the semantic tier exists. *Removing*
/// one withdraws the shared meaning without replacing it, which leaves every
/// consumer holding an assumption nobody is standing behind any more — so it is
/// filed the same way.
fn push_definition(
    out: &mut Vec<Change>,
    path: &str,
    subject: &str,
    old: &Option<String>,
    new: &Option<String>,
) {
    if old == new {
        return;
    }
    let (impact, description) = match (old, new) {
        (None, Some(_)) => (Impact::Cosmetic, format!("{subject} was written down")),
        (Some(_), None) => (
            Impact::Semantic,
            format!("{subject} was removed — nobody is standing behind the old meaning now"),
        ),
        (Some(_), Some(_)) => (
            Impact::Semantic,
            format!("{subject} changed — the same values now mean something else"),
        ),
        (None, None) => return,
    };
    out.push(Change {
        impact,
        path: path.to_string(),
        description,
    });
}

/// Diffs the `columns` maps.
fn diff_columns(old: &Contract, new: &Contract, out: &mut Vec<Change>) {
    for (name, old_col) in &old.columns {
        match new.columns.get(name) {
            // Still breaking — the data is gone either way. But a removal that
            // was announced months ago and one that arrived this morning are
            // not the same event for the person reading the diff, and only one
            // of them is a surprise.
            None => out.push(Change {
                impact: Impact::Breaking,
                path: format!("columns.{name}"),
                description: match (old_col.stability, old_col.sunset.as_deref()) {
                    (Stability::Deprecated, Some(date)) => format!(
                        "column `{name}` removed — deprecated, and announced for removal after {date}"
                    ),
                    (Stability::Deprecated, None) => {
                        format!("column `{name}` removed — deprecated, but no removal date was ever given")
                    }
                    _ => format!("column `{name}` removed"),
                },
            }),
            Some(new_col) => diff_column(name, old_col, new_col, out),
        }
    }
    for (name, new_col) in &new.columns {
        if !old.columns.contains_key(name) {
            if new_col.required {
                out.push(Change {
                    impact: Impact::Breaking,
                    path: format!("columns.{name}"),
                    description: format!("required column `{name}` added"),
                });
            } else {
                out.push(Change {
                    impact: Impact::NonBreaking,
                    path: format!("columns.{name}"),
                    description: format!("optional column `{name}` added"),
                });
            }
        }
    }
}

/// Diffs one column present in both contracts.
fn diff_column(name: &str, old: &ColumnDef, new: &ColumnDef, out: &mut Vec<Change>) {
    if old.r#type != new.r#type {
        out.push(Change {
            impact: Impact::Breaking,
            path: format!("columns.{name}.type"),
            description: format!(
                "`{name}` changed type from `{}` to `{}`",
                old.r#type, new.r#type
            ),
        });
    }
    match (old.required, new.required) {
        (false, true) => out.push(Change {
            impact: Impact::Breaking,
            path: format!("columns.{name}.required"),
            description: format!("`{name}` is now required — nulls become failures"),
        }),
        (true, false) => out.push(Change {
            impact: Impact::NonBreaking,
            path: format!("columns.{name}.required"),
            description: format!("`{name}` is no longer required"),
        }),
        _ => {}
    }
    push_definition(
        out,
        &format!("columns.{name}.description"),
        &format!("the definition of `{name}`"),
        &old.description,
        &new.description,
    );
    // No check runs on either of these, which is exactly why they are not
    // cosmetic: they are the fields that say what a consumer is *allowed to do*
    // with the column. A column that quietly stops being tagged as personal
    // data has not changed one byte and has changed everything.
    if old.pii != new.pii {
        // `pii: none` is a statement, not silence: it says somebody looked at
        // this column and it holds nothing personal. Reading it as "some kind
        // of personal data" — which comparing the options alone does — turns
        // that assurance into an alarm, and turns withdrawing the assurance
        // into a clean bill of health.
        let personal = |p: Option<PiiKind>| matches!(p, Some(k) if k != PiiKind::None);
        let (impact, description) = match (personal(old.pii), personal(new.pii)) {
            (false, true) => (
                Impact::Semantic,
                format!("`{name}` is now marked as personal data"),
            ),
            (true, true) => (
                Impact::Semantic,
                format!("the kind of personal data `{name}` holds changed"),
            ),
            (true, false) if new.pii.is_some() => (
                Impact::Semantic,
                format!(
                    "`{name}` is no longer marked as personal data — it is now declared not to be"
                ),
            ),
            (true, false) => (
                Impact::Semantic,
                format!("`{name}` no longer says whether it holds personal data"),
            ),
            // Neither side is personal, so the two are `none` and silence in
            // one order or the other. Saying so is new information; taking it
            // back leaves consumers holding an assurance nobody stands behind.
            (false, false) if new.pii.is_some() => (
                Impact::Cosmetic,
                format!("`{name}` was written down as holding no personal data"),
            ),
            (false, false) => (
                Impact::Semantic,
                format!("`{name}` no longer says whether it holds personal data"),
            ),
        };
        out.push(Change {
            impact,
            path: format!("columns.{name}.pii"),
            description,
        });
    }
    if old.classification != new.classification {
        // How a column must be handled is an instruction, and an instruction
        // that does not say what it changed to is not one.
        let change = match (old.classification, new.classification) {
            (None, Some(c)) => Some((
                Impact::Cosmetic,
                format!("`{name}` was written down as {c}"),
            )),
            (Some(a), Some(b)) => Some((
                Impact::Semantic,
                format!("`{name}` must now be handled as {b}, not {a}"),
            )),
            (Some(a), None) => Some((
                Impact::Semantic,
                format!("`{name}` no longer says how it must be handled — it was {a}"),
            )),
            (None, None) => None,
        };
        if let Some((impact, description)) = change {
            out.push(Change {
                impact,
                path: format!("columns.{name}.classification"),
                description,
            });
        }
    }
    diff_stability(name, old, new, out);
    diff_checks(name, &old.checks, &new.checks, out);
}

/// Diffs the promise attached to a column: how stable it is, and until when.
///
/// The direction is the whole story. Weakening a promise is a change every
/// consumer has to read — nothing fails, nothing moves, and what they were
/// entitled to rely on got smaller. Strengthening one needs no ceremony.
///
/// The exception is a `sunset` date that moves *closer*. That is not a weaker
/// promise, it is a shorter deadline: work that was scheduled against the old
/// date is now late. Which is what breaking means, so that is what it is filed
/// as — the one place in this file where a metadata field can break something.
fn diff_stability(name: &str, old: &ColumnDef, new: &ColumnDef, out: &mut Vec<Change>) {
    use std::cmp::Ordering;
    match new.stability.promises_more_than(old.stability) {
        Ordering::Less => out.push(Change {
            impact: Impact::Semantic,
            path: format!("columns.{name}.stability"),
            description: format!(
                "`{name}` is now {} — it was {}, and you may rely on it less than you could",
                new.stability, old.stability
            ),
        }),
        Ordering::Greater => out.push(Change {
            impact: Impact::NonBreaking,
            path: format!("columns.{name}.stability"),
            description: format!(
                "`{name}` is now {} — a stronger promise than {}",
                new.stability, old.stability
            ),
        }),
        Ordering::Equal => {}
    }

    // ISO-8601 sorts lexicographically, which is the entire reason the date is
    // held as a string: no clock is needed to tell nearer from further.
    let change = match (old.sunset.as_deref(), new.sunset.as_deref()) {
        (None, None) => None,
        (None, Some(d)) => Some((
            Impact::Semantic,
            format!("`{name}` will be removed after {d}"),
        )),
        (Some(d), None) => Some((
            Impact::NonBreaking,
            format!("`{name}` is no longer scheduled for removal (it was {d})"),
        )),
        (Some(a), Some(b)) if a == b => None,
        (Some(a), Some(b)) if b < a => Some((
            Impact::Breaking,
            format!("`{name}` will now be removed after {b}, not {a} — anyone who planned against the old date is late"),
        )),
        (Some(a), Some(b)) => Some((
            Impact::NonBreaking,
            format!("`{name}` now survives until {b}, not {a}"),
        )),
    };
    if let Some((impact, description)) = change {
        out.push(Change {
            impact,
            path: format!("columns.{name}.sunset"),
            description,
        });
    }
}

/// Diffs the migration window.
///
/// Written to be quiet on purpose. The block describes the version it is
/// attached to, and is expected to be dropped once its window closes, so a
/// later version losing it is bookkeeping rather than news. Two things are not:
/// a deadline that moved closer, which shortens time somebody had already
/// planned against, and the note, which is the instructions consumers are
/// following.
fn diff_migration(old: &Contract, new: &Contract, out: &mut Vec<Change>) {
    match (old.migration.as_ref(), new.migration.as_ref()) {
        (None, None) => {}
        (None, Some(m)) => out.push(Change {
            impact: Impact::NonBreaking,
            path: "migration".into(),
            description: format!("a migration window was declared, ending {}", m.window_ends),
        }),
        (Some(m), None) => out.push(Change {
            impact: Impact::Cosmetic,
            path: "migration".into(),
            description: format!(
                "the migration window that ended {} was removed",
                m.window_ends
            ),
        }),
        (Some(a), Some(b)) => {
            if b.window_ends < a.window_ends {
                out.push(Change {
                    impact: Impact::Breaking,
                    path: "migration.window_ends".into(),
                    description: format!(
                        "the migration deadline moved forward from {} to {} — work planned against the old date is late",
                        a.window_ends, b.window_ends
                    ),
                });
            } else if b.window_ends > a.window_ends {
                out.push(Change {
                    impact: Impact::NonBreaking,
                    path: "migration.window_ends".into(),
                    description: format!(
                        "the migration deadline moved out from {} to {}",
                        a.window_ends, b.window_ends
                    ),
                });
            }
            if a.note != b.note {
                out.push(Change {
                    impact: Impact::Cosmetic,
                    path: "migration.note".into(),
                    description: "the migration instructions changed".into(),
                });
            }
        }
    }
}

/// Diffs the check lists of one column, kind by kind.
fn diff_checks(column: &str, old: &[ColumnCheck], new: &[ColumnCheck], out: &mut Vec<Change>) {
    let mut kinds: Vec<&'static str> = Vec::new();
    for c in old.iter().chain(new) {
        if !kinds.contains(&c.kind_name()) {
            kinds.push(c.kind_name());
        }
    }
    for kind in kinds {
        let path = format!("columns.{column}.checks[{kind}]");
        let olds: Vec<&ColumnCheck> = old.iter().filter(|c| c.kind_name() == kind).collect();
        let news: Vec<&ColumnCheck> = new.iter().filter(|c| c.kind_name() == kind).collect();
        if let ([o], [n]) = (olds.as_slice(), news.as_slice()) {
            if let Some(change) = compare_check_pair(&path, o, n, Some(column)) {
                out.push(change);
            }
            continue;
        }
        // Zero-or-many on either side: classify as removals and additions.
        for o in &olds {
            if !news.contains(o) {
                out.push(Change {
                    impact: Impact::NonBreaking,
                    path: path.clone(),
                    description: format!("`{kind}` check removed from `{column}`"),
                });
            }
        }
        for n in &news {
            if !olds.contains(n) {
                out.push(added_check(&path, kind, n.severity(), Some(column)));
            }
        }
    }
}

/// A `Change` for a newly added check, classified by its severity.
fn added_check(path: &str, kind: &str, severity: Severity, on: Option<&str>) -> Change {
    let onto = on.map(|c| format!(" to `{c}`")).unwrap_or_default();
    match severity {
        Severity::Error => Change {
            impact: Impact::Breaking,
            path: path.to_string(),
            description: format!("`{kind}` check added{onto} at error severity"),
        },
        Severity::Warn => Change {
            impact: Impact::NonBreaking,
            path: path.to_string(),
            description: format!("`{kind}` check added{onto} at warn severity"),
        },
    }
}

/// Compares a single old/new check of the same kind.
fn compare_check_pair(
    path: &str,
    old: &ColumnCheck,
    new: &ColumnCheck,
    on: Option<&str>,
) -> Option<Change> {
    if old == new {
        return None;
    }
    let (rel, detail) = column_check_relation(old, new);
    Some(classify_pair(
        path,
        old.kind_name(),
        rel,
        detail,
        old.severity(),
        new.severity(),
        on,
    ))
}

/// Final impact classification for a changed check, combining the parameter
/// relation with the severity transition.
fn classify_pair(
    path: &str,
    kind: &str,
    rel: Rel,
    detail: String,
    old_sev: Severity,
    new_sev: Severity,
    on: Option<&str>,
) -> Change {
    let severity_note = match (old_sev, new_sev) {
        (Severity::Error, Severity::Warn) => "; severity relaxed error → warn",
        (Severity::Warn, Severity::Error) => "; severity escalated warn → error",
        _ => "",
    };
    // A parameter that moved is a statement about the column it guards, and
    // "min raised from 5 to 10" is a fact about nothing until it says whose
    // minimum moved. At dataset scope there is no column to name, and the
    // sentence is already about the whole table.
    let description = match (detail.is_empty(), on) {
        (true, Some(c)) => format!("`{kind}` check severity changed on `{c}`{severity_note}"),
        (true, None) => format!("`{kind}` check severity changed{severity_note}"),
        (false, Some(c)) => format!("`{c}`: {detail}{severity_note}"),
        (false, None) => format!("{detail}{severity_note}"),
    };
    // Escalating warn → error is breaking regardless of parameters: failures
    // that used to be warnings now fail the run.
    let impact = if old_sev == Severity::Warn && new_sev == Severity::Error {
        Impact::Breaking
    } else {
        match rel {
            Rel::Tightened | Rel::Changed if new_sev == Severity::Error => Impact::Breaking,
            _ => Impact::NonBreaking,
        }
    };
    Change {
        impact,
        path: path.to_string(),
        description,
    }
}

/// How the parameters of two same-kind column checks relate.
fn column_check_relation(old: &ColumnCheck, new: &ColumnCheck) -> (Rel, String) {
    use ColumnCheck as C;
    match (old, new) {
        (C::Min { min: a, .. }, C::Min { min: b, .. }) => {
            let (a, b) = (a.as_f64(), b.as_f64());
            if b > a {
                (Rel::Tightened, format!("min raised from {a} to {b}"))
            } else if b < a {
                (Rel::Loosened, format!("min lowered from {a} to {b}"))
            } else {
                (Rel::Same, String::new())
            }
        }
        (C::Max { max: a, .. }, C::Max { max: b, .. }) => {
            let (a, b) = (a.as_f64(), b.as_f64());
            if b < a {
                (Rel::Tightened, format!("max lowered from {a} to {b}"))
            } else if b > a {
                (Rel::Loosened, format!("max raised from {a} to {b}"))
            } else {
                (Rel::Same, String::new())
            }
        }
        (C::Regex { regex: a, .. }, C::Regex { regex: b, .. }) => {
            if a == b {
                (Rel::Same, String::new())
            } else {
                (Rel::Changed, format!("regex changed from `{a}` to `{b}`"))
            }
        }
        (C::Enum { values: a, .. }, C::Enum { values: b, .. }) => {
            let removed = a.iter().any(|v| !b.contains(v));
            let added = b.iter().any(|v| !a.contains(v));
            match (removed, added) {
                (false, false) => (Rel::Same, String::new()),
                (true, false) => (
                    Rel::Tightened,
                    format!("enum shrunk from {} to {} values", a.len(), b.len()),
                ),
                (false, true) => (
                    Rel::Loosened,
                    format!("enum grew from {} to {} values", a.len(), b.len()),
                ),
                (true, true) => (Rel::Changed, "enum values replaced".to_string()),
            }
        }
        (C::Length { length: a, .. }, C::Length { length: b, .. }) => {
            let (alo, ahi) = a.bounds();
            let (blo, bhi) = b.bounds();
            let min_tighter = blo > alo;
            let min_looser = blo < alo;
            let max_tighter = match (ahi, bhi) {
                (Some(a), Some(b)) => b < a,
                (None, Some(_)) => true,
                _ => false,
            };
            let max_looser = match (ahi, bhi) {
                (Some(a), Some(b)) => b > a,
                (Some(_), None) => true,
                _ => false,
            };
            match (min_tighter || max_tighter, min_looser || max_looser) {
                (false, false) => (Rel::Same, String::new()),
                (true, false) => (Rel::Tightened, "length constraint narrowed".to_string()),
                (false, true) => (Rel::Loosened, "length constraint widened".to_string()),
                (true, true) => (Rel::Changed, "length constraint moved".to_string()),
            }
        }
        (C::Format { format: a, .. }, C::Format { format: b, .. }) => {
            if a == b {
                (Rel::Same, String::new())
            } else {
                (Rel::Changed, "format changed".to_string())
            }
        }
        (C::NullRatioMax { ratio: a, .. }, C::NullRatioMax { ratio: b, .. }) => {
            if b < a {
                (
                    Rel::Tightened,
                    format!("null_ratio_max lowered from {a} to {b}"),
                )
            } else if b > a {
                (
                    Rel::Loosened,
                    format!("null_ratio_max raised from {a} to {b}"),
                )
            } else {
                (Rel::Same, String::new())
            }
        }
        (C::UniqueRatioMin { ratio: a, .. }, C::UniqueRatioMin { ratio: b, .. }) => {
            if b > a {
                (
                    Rel::Tightened,
                    format!("unique_ratio_min raised from {a} to {b}"),
                )
            } else if b < a {
                (
                    Rel::Loosened,
                    format!("unique_ratio_min lowered from {a} to {b}"),
                )
            } else {
                (Rel::Same, String::new())
            }
        }
        (C::CustomExpr { expr: a, .. }, C::CustomExpr { expr: b, .. }) => {
            if a == b {
                (Rel::Same, String::new())
            } else {
                (Rel::Changed, "custom expression changed".to_string())
            }
        }
        (C::Unique { approx: a, .. }, C::Unique { approx: b, .. }) => match (a, b) {
            (true, false) => (
                Rel::Tightened,
                "unique changed from approximate to exact".to_string(),
            ),
            (false, true) => (
                Rel::Loosened,
                "unique relaxed from exact to approximate".to_string(),
            ),
            _ => (Rel::Same, String::new()),
        },
        (C::NotEmptyString { .. }, C::NotEmptyString { .. }) => (Rel::Same, String::new()),
        // Different kinds never reach here (grouped by kind), but stay safe.
        _ => (Rel::Changed, "check parameters changed".to_string()),
    }
}

/// Identity key for a dataset check: kind plus the column/expression it
/// targets, so `null_ratio_max` on different columns diff independently.
fn dataset_check_key(check: &DatasetCheck) -> String {
    match check {
        DatasetCheck::RowCountMin { .. } => "row_count_min".to_string(),
        DatasetCheck::RowCountMax { .. } => "row_count_max".to_string(),
        DatasetCheck::Freshness { column, .. } => format!("freshness.{column}"),
        DatasetCheck::NullRatioMax { column, .. } => format!("null_ratio_max.{column}"),
        DatasetCheck::UniqueRatioMin { column, .. } => format!("unique_ratio_min.{column}"),
        DatasetCheck::CustomExpr { expr, .. } => format!("custom_expr.{expr}"),
    }
}

/// How the parameters of two same-key dataset checks relate.
fn dataset_check_relation(old: &DatasetCheck, new: &DatasetCheck) -> (Rel, String) {
    use DatasetCheck as D;
    match (old, new) {
        (D::RowCountMin { count: a, .. }, D::RowCountMin { count: b, .. }) => {
            if b > a {
                (
                    Rel::Tightened,
                    format!("row_count_min raised from {a} to {b}"),
                )
            } else if b < a {
                (
                    Rel::Loosened,
                    format!("row_count_min lowered from {a} to {b}"),
                )
            } else {
                (Rel::Same, String::new())
            }
        }
        (D::RowCountMax { count: a, .. }, D::RowCountMax { count: b, .. }) => {
            if b < a {
                (
                    Rel::Tightened,
                    format!("row_count_max lowered from {a} to {b}"),
                )
            } else if b > a {
                (
                    Rel::Loosened,
                    format!("row_count_max raised from {a} to {b}"),
                )
            } else {
                (Rel::Same, String::new())
            }
        }
        (D::Freshness { max_age: a, .. }, D::Freshness { max_age: b, .. }) => {
            if b < a {
                (
                    Rel::Tightened,
                    format!(
                        "freshness max_age shortened from {} to {}",
                        humantime::format_duration(*a),
                        humantime::format_duration(*b)
                    ),
                )
            } else if b > a {
                (
                    Rel::Loosened,
                    format!(
                        "freshness max_age lengthened from {} to {}",
                        humantime::format_duration(*a),
                        humantime::format_duration(*b)
                    ),
                )
            } else {
                (Rel::Same, String::new())
            }
        }
        (D::NullRatioMax { ratio: a, .. }, D::NullRatioMax { ratio: b, .. }) => {
            if b < a {
                (
                    Rel::Tightened,
                    format!("null_ratio_max lowered from {a} to {b}"),
                )
            } else if b > a {
                (
                    Rel::Loosened,
                    format!("null_ratio_max raised from {a} to {b}"),
                )
            } else {
                (Rel::Same, String::new())
            }
        }
        (D::UniqueRatioMin { ratio: a, .. }, D::UniqueRatioMin { ratio: b, .. }) => {
            if b > a {
                (
                    Rel::Tightened,
                    format!("unique_ratio_min raised from {a} to {b}"),
                )
            } else if b < a {
                (
                    Rel::Loosened,
                    format!("unique_ratio_min lowered from {a} to {b}"),
                )
            } else {
                (Rel::Same, String::new())
            }
        }
        (D::CustomExpr { expr: a, .. }, D::CustomExpr { expr: b, .. }) => {
            if a == b {
                (Rel::Same, String::new())
            } else {
                (Rel::Changed, "custom expression changed".to_string())
            }
        }
        _ => (Rel::Changed, "check parameters changed".to_string()),
    }
}

/// Diffs the dataset-level check lists.
fn diff_dataset_checks(old: &Contract, new: &Contract, out: &mut Vec<Change>) {
    let mut keys: Vec<String> = Vec::new();
    for c in old.dataset_checks.iter().chain(&new.dataset_checks) {
        let k = dataset_check_key(c);
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    for key in keys {
        let path = format!("dataset_checks[{key}]");
        let olds: Vec<&DatasetCheck> = old
            .dataset_checks
            .iter()
            .filter(|c| dataset_check_key(c) == key)
            .collect();
        let news: Vec<&DatasetCheck> = new
            .dataset_checks
            .iter()
            .filter(|c| dataset_check_key(c) == key)
            .collect();
        if let ([o], [n]) = (olds.as_slice(), news.as_slice()) {
            if o != n {
                let (rel, detail) = dataset_check_relation(o, n);
                out.push(classify_pair(
                    &path,
                    o.kind_name(),
                    rel,
                    detail,
                    o.severity(),
                    n.severity(),
                    None,
                ));
            }
            continue;
        }
        for o in &olds {
            if !news.contains(o) {
                out.push(Change {
                    impact: Impact::NonBreaking,
                    path: path.clone(),
                    description: format!("`{}` check removed", o.kind_name()),
                });
            }
        }
        for n in &news {
            if !olds.contains(n) {
                out.push(added_check(&path, n.kind_name(), n.severity(), None));
            }
        }
    }
}

/// Diffs the `settings` block.
fn diff_settings(old: &Contract, new: &Contract, out: &mut Vec<Change>) {
    let (o, n) = (&old.settings, &new.settings);
    match (o.allow_extra_columns, n.allow_extra_columns) {
        (true, false) => out.push(Change {
            impact: Impact::Breaking,
            path: "settings.allow_extra_columns".into(),
            description: "extra columns are no longer allowed".into(),
        }),
        (false, true) => out.push(Change {
            impact: Impact::NonBreaking,
            path: "settings.allow_extra_columns".into(),
            description: "extra columns are now allowed".into(),
        }),
        _ => {}
    }
    match (o.columns_exact, n.columns_exact) {
        (false, true) => out.push(Change {
            impact: Impact::Breaking,
            path: "settings.columns_exact".into(),
            description: "exact column matching enabled".into(),
        }),
        (true, false) => out.push(Change {
            impact: Impact::NonBreaking,
            path: "settings.columns_exact".into(),
            description: "exact column matching disabled".into(),
        }),
        _ => {}
    }
    match (o.on_type_mismatch, n.on_type_mismatch) {
        (Severity::Warn, Severity::Error) => out.push(Change {
            impact: Impact::Breaking,
            path: "settings.on_type_mismatch".into(),
            description: "type mismatches escalated from warn to error".into(),
        }),
        (Severity::Error, Severity::Warn) => out.push(Change {
            impact: Impact::NonBreaking,
            path: "settings.on_type_mismatch".into(),
            description: "type mismatches relaxed from error to warn".into(),
        }),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::parse::parse_str;

    fn contract(yaml: &str) -> Contract {
        parse_str(yaml, "test.yaml").unwrap()
    }

    /// Builds a contract with the given column checks on an int column `age`.
    fn with_checks(checks: &str) -> Contract {
        contract(&format!(
            "apiVersion: v1\ndataset: t\ncolumns:\n  age: {{ type: int, checks: {checks} }}\n"
        ))
    }

    fn single(old: &Contract, new: &Contract) -> Change {
        let changes = diff(old, new);
        assert_eq!(changes.len(), 1, "expected one change, got {changes:#?}");
        changes.into_iter().next().unwrap()
    }

    const BASE: &str = "apiVersion: v1\ndataset: t\ncolumns:\n  id: { type: string }\n";

    #[test]
    fn identical_contracts_diff_empty() {
        let c = contract(BASE);
        assert_eq!(diff(&c, &c), vec![]);
    }

    /// One row per classification-table entry. Each row is
    /// `(old_checks, new_checks, expected_impact, description_fragment)`.
    #[test]
    fn check_classification_table() {
        let rows: &[(&str, &str, Impact, &str)] = &[
            // Added at error severity ⇒ Breaking; at warn ⇒ NonBreaking.
            (
                "[]",
                "[{ min: 18 }]",
                Impact::Breaking,
                "added to `age` at error severity",
            ),
            (
                "[]",
                "[{ min: 18, severity: warn }]",
                Impact::NonBreaking,
                "added to `age` at warn severity",
            ),
            (
                "[]",
                "[unique]",
                Impact::Breaking,
                "added to `age` at error severity",
            ),
            // Removed ⇒ NonBreaking.
            ("[{ min: 18 }]", "[]", Impact::NonBreaking, "removed"),
            ("[unique]", "[]", Impact::NonBreaking, "removed"),
            // min raised / lowered.
            (
                "[{ min: 18 }]",
                "[{ min: 21 }]",
                Impact::Breaking,
                "min raised",
            ),
            (
                "[{ min: 18 }]",
                "[{ min: 16 }]",
                Impact::NonBreaking,
                "min lowered",
            ),
            // max lowered / raised.
            (
                "[{ max: 120 }]",
                "[{ max: 99 }]",
                Impact::Breaking,
                "max lowered",
            ),
            (
                "[{ max: 120 }]",
                "[{ max: 150 }]",
                Impact::NonBreaking,
                "max raised",
            ),
            // enum shrunk / grown / replaced.
            (
                "[{ enum: [a, b, c] }]",
                "[{ enum: [a, b] }]",
                Impact::Breaking,
                "enum shrunk",
            ),
            (
                "[{ enum: [a, b] }]",
                "[{ enum: [a, b, c] }]",
                Impact::NonBreaking,
                "enum grew",
            ),
            (
                "[{ enum: [a, b] }]",
                "[{ enum: [a, c] }]",
                Impact::Breaking,
                "enum values replaced",
            ),
            // regex changed.
            (
                r#"[{ regex: "^a" }]"#,
                r#"[{ regex: "^b" }]"#,
                Impact::Breaking,
                "regex changed",
            ),
            // length narrowed / widened.
            (
                "[{ length: { min: 1, max: 10 } }]",
                "[{ length: { min: 2, max: 8 } }]",
                Impact::Breaking,
                "narrowed",
            ),
            (
                "[{ length: { min: 2, max: 8 } }]",
                "[{ length: { min: 1, max: 10 } }]",
                Impact::NonBreaking,
                "widened",
            ),
            (
                "[{ length: 2 }]",
                "[{ length: 3 }]",
                Impact::Breaking,
                "moved",
            ),
            (
                "[{ length: { min: 1 } }]",
                "[{ length: { min: 1, max: 5 } }]",
                Impact::Breaking,
                "narrowed",
            ),
            // ratio thresholds tightened / loosened.
            (
                "[{ null_ratio_max: 0.2 }]",
                "[{ null_ratio_max: 0.1 }]",
                Impact::Breaking,
                "null_ratio_max lowered",
            ),
            (
                "[{ null_ratio_max: 0.1 }]",
                "[{ null_ratio_max: 0.2 }]",
                Impact::NonBreaking,
                "null_ratio_max raised",
            ),
            (
                "[{ unique_ratio_min: 0.8 }]",
                "[{ unique_ratio_min: 0.9 }]",
                Impact::Breaking,
                "unique_ratio_min raised",
            ),
            (
                "[{ unique_ratio_min: 0.9 }]",
                "[{ unique_ratio_min: 0.8 }]",
                Impact::NonBreaking,
                "unique_ratio_min lowered",
            ),
            // severity transitions.
            (
                "[{ min: 18 }]",
                "[{ min: 18, severity: warn }]",
                Impact::NonBreaking,
                "relaxed error → warn",
            ),
            (
                "[{ min: 18, severity: warn }]",
                "[{ min: 18 }]",
                Impact::Breaking,
                "escalated warn → error",
            ),
            // tightening a warn-only check stays non-breaking.
            (
                "[{ min: 18, severity: warn }]",
                "[{ min: 21, severity: warn }]",
                Impact::NonBreaking,
                "min raised",
            ),
            // format changed.
            (
                "[{ format: email }]",
                "[{ format: uuid }]",
                Impact::Breaking,
                "format changed",
            ),
            // custom_expr changed.
            (
                r#"[{ custom_expr: "a" }]"#,
                r#"[{ custom_expr: "b" }]"#,
                Impact::Breaking,
                "custom expression changed",
            ),
            // unique approx transitions.
            (
                "[{ unique: { approx: true } }]",
                "[unique]",
                Impact::Breaking,
                "approximate to exact",
            ),
            (
                "[unique]",
                "[{ unique: { approx: true } }]",
                Impact::NonBreaking,
                "exact to approximate",
            ),
        ];
        for (old_checks, new_checks, impact, fragment) in rows {
            let old = with_checks(old_checks);
            let new = with_checks(new_checks);
            let change = single(&old, &new);
            assert_eq!(
                change.impact, *impact,
                "row ({old_checks} -> {new_checks}): got {change:?}"
            );
            assert!(
                change.description.contains(fragment),
                "row ({old_checks} -> {new_checks}): description `{}` missing `{fragment}`",
                change.description
            );
        }
    }

    #[test]
    fn column_removed_is_breaking() {
        let old = contract(BASE);
        let new = contract("apiVersion: v1\ndataset: t\ncolumns: {}\n");
        let change = single(&old, &new);
        assert_eq!(change.impact, Impact::Breaking);
        assert!(change.description.contains("removed"));
    }

    #[test]
    fn optional_column_added_is_non_breaking() {
        let old = contract(BASE);
        let new = contract(&format!("{BASE}  extra: {{ type: int }}\n"));
        let change = single(&old, &new);
        assert_eq!(change.impact, Impact::NonBreaking);
        assert_eq!(change.path, "columns.extra");
    }

    #[test]
    fn required_column_added_is_breaking() {
        let old = contract(BASE);
        let new = contract(&format!("{BASE}  extra: {{ type: int, required: true }}\n"));
        assert_eq!(single(&old, &new).impact, Impact::Breaking);
    }

    #[test]
    fn type_change_is_breaking() {
        let old = contract(BASE);
        let new = contract("apiVersion: v1\ndataset: t\ncolumns:\n  id: { type: int }\n");
        let change = single(&old, &new);
        assert_eq!(change.impact, Impact::Breaking);
        assert_eq!(change.path, "columns.id.type");
    }

    #[test]
    fn required_added_and_removed() {
        let old = contract(BASE);
        let new = contract(
            "apiVersion: v1\ndataset: t\ncolumns:\n  id: { type: string, required: true }\n",
        );
        assert_eq!(single(&old, &new).impact, Impact::Breaking);
        assert_eq!(single(&new, &old).impact, Impact::NonBreaking);
    }

    #[test]
    fn dataset_rename_is_breaking() {
        let old = contract(BASE);
        let new = contract("apiVersion: v1\ndataset: other\ncolumns:\n  id: { type: string }\n");
        assert_eq!(single(&old, &new).impact, Impact::Breaking);
    }

    #[test]
    fn metadata_edits_are_cosmetic() {
        let old = contract(BASE);
        let new = contract(&format!(
            "{BASE}owner: team@x.com\ndescription: hi\nversion: \"2\"\nconsumers:\n  - {{ name: bi }}\n"
        ));
        let changes = diff(&old, &new);
        assert_eq!(changes.len(), 4, "{changes:#?}");
        assert!(changes.iter().all(|c| c.impact == Impact::Cosmetic));
    }

    #[test]
    fn a_handling_tag_is_semantic_not_cosmetic() {
        // No check runs on `pii` or `classification`, which is exactly why they
        // are not cosmetic: they say what a consumer is allowed to do with the
        // column. A column that quietly stops being tagged as personal data has
        // not changed one byte and has changed everything.
        let old = contract(BASE);
        let new = contract(
            "apiVersion: v1
dataset: t
columns:
  id: { type: string, pii: email, classification: internal }
",
        );
        let changes = diff(&old, &new);
        assert_eq!(changes.len(), 2, "{changes:#?}");
        let pii = changes
            .iter()
            .find(|c| c.path.ends_with("pii"))
            .expect("a pii change");
        assert_eq!(pii.impact, Impact::Semantic);
        // Writing a handling level down for the first time takes nothing away
        // and contradicts nothing, for the same reason a definition written
        // down for the first time is cosmetic.
        let class = changes
            .iter()
            .find(|c| c.path.ends_with("classification"))
            .expect("a classification change");
        assert_eq!(class.impact, Impact::Cosmetic);
        assert_eq!(class.description, "`id` was written down as internal");
        // And the directions that matter more.
        let back = diff(&new, &old);
        assert!(back
            .iter()
            .any(|c| c.description == "`id` no longer says whether it holds personal data"));
        assert!(back.iter().any(|c| c.impact == Impact::Semantic
            && c.description == "`id` no longer says how it must be handled — it was internal"));
    }

    #[test]
    fn declaring_a_column_not_personal_is_not_declaring_it_personal() {
        // `pii: none` is the one value that means the opposite of the others.
        // Comparing the options alone reads it as "something was tagged here",
        // which is how an assurance gets announced as a leak.
        let silent = contract(BASE);
        let declared = contract(
            "apiVersion: v1
dataset: t
columns:
  id: { type: string, pii: none }
",
        );
        let written = diff(&silent, &declared);
        assert_eq!(written.len(), 1, "{written:#?}");
        assert_eq!(written[0].impact, Impact::Cosmetic);
        assert_eq!(
            written[0].description,
            "`id` was written down as holding no personal data"
        );

        // Taking the assurance back is not cosmetic: nobody stands behind it
        // any more.
        let withdrawn = diff(&declared, &silent);
        assert_eq!(withdrawn.len(), 1, "{withdrawn:#?}");
        assert_eq!(withdrawn[0].impact, Impact::Semantic);
        assert_eq!(
            withdrawn[0].description,
            "`id` no longer says whether it holds personal data"
        );

        // And a real tag replacing the assurance is still the alarm.
        let personal = contract(
            "apiVersion: v1
dataset: t
columns:
  id: { type: string, pii: email }
",
        );
        let raised = diff(&declared, &personal);
        assert_eq!(raised.len(), 1, "{raised:#?}");
        assert_eq!(raised[0].description, "`id` is now marked as personal data");
    }

    #[test]
    fn writing_a_definition_down_is_cosmetic_but_changing_one_is_not() {
        // Three genuinely different things. Writing a definition down for the
        // first time changes nothing that was ever agreed. Changing it is a
        // redefinition, and removing it withdraws the shared meaning without
        // replacing it, which leaves consumers holding an assumption nobody is
        // standing behind any more.
        let none = contract(BASE);
        let first = contract(
            "apiVersion: v1\ndataset: t\ncolumns:\n  id: { type: string, description: The order line reference. }\n",
        );
        let second = contract(
            "apiVersion: v1\ndataset: t\ncolumns:\n  id: { type: string, description: The shipment reference. }\n",
        );

        let written = single(&none, &first);
        assert_eq!(written.impact, Impact::Cosmetic);
        assert_eq!(written.path, "columns.id.description");

        let redefined = single(&first, &second);
        assert_eq!(redefined.impact, Impact::Semantic);
        assert!(redefined.description.contains("`id`"));

        let withdrawn = single(&first, &none);
        assert_eq!(withdrawn.impact, Impact::Semantic);
    }

    #[test]
    fn the_datasets_own_definition_is_read_the_same_way() {
        let none = contract(BASE);
        let first = contract(&format!("{BASE}description: Orders, as booked.\n"));
        let second = contract(&format!("{BASE}description: Orders, net of refunds.\n"));
        assert_eq!(single(&none, &first).impact, Impact::Cosmetic);
        let redefined = single(&first, &second);
        assert_eq!(redefined.impact, Impact::Semantic);
        assert_eq!(redefined.path, "description");
    }

    #[test]
    fn a_redefinition_outranks_a_loosening() {
        // The ordering is the whole point of the tier. A caller that sorts by
        // impact to decide what to put at the top of a pull request comment has
        // to surface the changed meaning above the relaxed rule, because the
        // relaxed rule announces itself and the changed meaning never does.
        assert!(Impact::Breaking < Impact::Semantic);
        assert!(Impact::Semantic < Impact::NonBreaking);
        assert!(Impact::NonBreaking < Impact::Cosmetic);
        assert_eq!(Impact::Semantic.to_string(), "semantic");
    }

    #[test]
    fn weakening_a_promise_is_semantic_and_strengthening_one_is_not() {
        let stable = contract(BASE);
        let deprecated = contract(
            "apiVersion: v1\ndataset: t\ncolumns:\n  id: { type: string, stability: deprecated }\n",
        );
        let weaker = single(&stable, &deprecated);
        assert_eq!(weaker.impact, Impact::Semantic);
        assert_eq!(weaker.path, "columns.id.stability");
        assert_eq!(single(&deprecated, &stable).impact, Impact::NonBreaking);

        // Beta sits between the two, and the direction still decides.
        let beta = contract(
            "apiVersion: v1\ndataset: t\ncolumns:\n  id: { type: string, stability: beta }\n",
        );
        assert_eq!(single(&stable, &beta).impact, Impact::Semantic);
        assert_eq!(single(&beta, &deprecated).impact, Impact::Semantic);
        assert_eq!(single(&deprecated, &beta).impact, Impact::NonBreaking);
    }

    #[test]
    fn a_removal_date_that_moves_closer_is_breaking() {
        let mk = |date: &str| {
            contract(&format!(
                "apiVersion: v1\ndataset: t\ncolumns:\n  id: {{ type: string, stability: deprecated, sunset: {date} }}\n"
            ))
        };
        let late = mk("2027-06-30");
        let early = mk("2026-06-30");

        // Nothing about the data changed. Work that was scheduled against June
        // 2027 is now a year late, which is the definition of breaking.
        let brought_forward = single(&late, &early);
        assert_eq!(brought_forward.impact, Impact::Breaking);
        assert_eq!(brought_forward.path, "columns.id.sunset");
        assert!(brought_forward.description.contains("late"));

        assert_eq!(single(&early, &late).impact, Impact::NonBreaking);
    }

    #[test]
    fn setting_a_removal_date_is_semantic_and_withdrawing_one_is_not() {
        let none = contract(
            "apiVersion: v1\ndataset: t\ncolumns:\n  id: { type: string, stability: deprecated }\n",
        );
        let dated = contract(
            "apiVersion: v1\ndataset: t\ncolumns:\n  id: { type: string, stability: deprecated, sunset: 2026-12-31 }\n",
        );
        let announced = single(&none, &dated);
        assert_eq!(announced.impact, Impact::Semantic);
        assert!(announced.description.contains("2026-12-31"));
        assert_eq!(single(&dated, &none).impact, Impact::NonBreaking);
    }

    #[test]
    fn removing_an_announced_column_says_it_was_announced() {
        // Breaking either way -- the data is gone. But the reader of the diff
        // needs to know whether this was the plan or a surprise.
        let announced = contract(
            "apiVersion: v1\ndataset: t\ncolumns:\n  id: { type: string }\n  legacy: { type: string, stability: deprecated, sunset: 2026-01-31 }\n",
        );
        let gone = contract(BASE);
        let change = single(&announced, &gone);
        assert_eq!(change.impact, Impact::Breaking);
        assert!(change.description.contains("2026-01-31"), "{change:#?}");

        let unannounced = contract(
            "apiVersion: v1\ndataset: t\ncolumns:\n  id: { type: string }\n  legacy: { type: string }\n",
        );
        let plain = single(&unannounced, &gone);
        assert_eq!(plain.description, "column `legacy` removed");
    }

    #[test]
    fn a_migration_window_is_quiet_until_the_deadline_moves() {
        let none = contract(BASE);
        let far = contract(&format!(
            "{BASE}migration: {{ window_ends: 2026-12-31, note: read amount_minor }}\n"
        ));
        let near = contract(&format!(
            "{BASE}migration: {{ window_ends: 2026-10-01, note: read amount_minor }}\n"
        ));

        assert_eq!(single(&none, &far).impact, Impact::NonBreaking);
        // Dropped once the window closes, which is bookkeeping, not news.
        assert_eq!(single(&far, &none).impact, Impact::Cosmetic);
        assert_eq!(single(&far, &near).impact, Impact::Breaking);
        assert_eq!(single(&near, &far).impact, Impact::NonBreaking);
    }

    #[test]
    fn settings_transitions() {
        let old = contract(&format!(
            "{BASE}settings: {{ allow_extra_columns: true }}\n"
        ));
        let new = contract(&format!(
            "{BASE}settings: {{ allow_extra_columns: false }}\n"
        ));
        assert_eq!(single(&old, &new).impact, Impact::Breaking);
        assert_eq!(single(&new, &old).impact, Impact::NonBreaking);

        let old = contract(BASE);
        let new = contract(&format!(
            "{BASE}settings: {{ columns_exact: true, allow_extra_columns: true }}\n"
        ));
        assert_eq!(single(&old, &new).impact, Impact::Breaking);

        let old = contract(&format!("{BASE}settings: {{ on_type_mismatch: warn }}\n"));
        let new = contract(BASE);
        assert_eq!(single(&old, &new).impact, Impact::Breaking);
        assert_eq!(single(&new, &old).impact, Impact::NonBreaking);
    }

    #[test]
    fn dataset_check_rows() {
        let mk = |checks: &str| {
            contract(&format!(
                "apiVersion: v1\ndataset: t\ncolumns:\n  ts: {{ type: datetime }}\ndataset_checks:\n{checks}"
            ))
        };
        // row_count_min raised ⇒ Breaking; lowered ⇒ NonBreaking.
        let old = mk("  - row_count_min: 10\n");
        let new = mk("  - row_count_min: 100\n");
        assert_eq!(single(&old, &new).impact, Impact::Breaking);
        assert_eq!(single(&new, &old).impact, Impact::NonBreaking);
        // freshness shortened ⇒ Breaking.
        let old = mk("  - freshness: { column: ts, max_age: 48h }\n");
        let new = mk("  - freshness: { column: ts, max_age: 24h }\n");
        assert_eq!(single(&old, &new).impact, Impact::Breaking);
        assert_eq!(single(&new, &old).impact, Impact::NonBreaking);
        // added at error ⇒ Breaking; removed ⇒ NonBreaking.
        let none = mk("  []\n");
        let some = mk("  - row_count_max: 10\n");
        assert_eq!(single(&none, &some).impact, Impact::Breaking);
        assert_eq!(single(&some, &none).impact, Impact::NonBreaking);
        // added at warn ⇒ NonBreaking.
        let warn = mk("  - { row_count_max: 10, severity: warn }\n");
        assert_eq!(single(&none, &warn).impact, Impact::NonBreaking);
        // different columns are independent checks.
        let a = mk("  - null_ratio_max: { column: ts, ratio: 0.1 }\n");
        let b = mk("  - null_ratio_max: { column: other, ratio: 0.1 }\n");
        let changes = diff(&a, &b);
        assert_eq!(changes.len(), 2, "{changes:#?}");
    }

    #[test]
    fn breaking_is_not_cosmetic_in_reverse() {
        let old = with_checks("[{ min: 18 }]");
        let new = with_checks("[{ min: 21 }]");
        let forward = diff(&old, &new);
        assert!(forward.iter().any(|c| c.impact == Impact::Breaking));
        let reverse = diff(&new, &old);
        assert!(!reverse.is_empty());
        assert!(reverse.iter().any(|c| c.impact != Impact::Cosmetic));
    }
}
