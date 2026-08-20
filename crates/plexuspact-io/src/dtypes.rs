//! Contract-declared column types → Polars dtypes.
//!
//! Conventions (documented, stable):
//!
//! * `string` → `String`
//! * `int` → `Int64`
//! * `float` → `Float64`
//! * `bool` → `Boolean`
//! * `date` → `Date`
//! * `datetime` → `Datetime(µs, UTC)` — datetimes are timezone-aware UTC;
//!   naive datetime strings in the data are assumed to be UTC.

use plexuspact_contract::ColType;
use polars::prelude::{DataType, Schema, TimeUnit, TimeZone};

/// The Polars dtype a declared contract type maps to.
pub fn dtype_for(t: ColType) -> DataType {
    match t {
        ColType::String => DataType::String,
        ColType::Int => DataType::Int64,
        ColType::Float => DataType::Float64,
        ColType::Bool => DataType::Boolean,
        ColType::Date => DataType::Date,
        ColType::Datetime => {
            DataType::Datetime(TimeUnit::Microseconds, Some(TimeZone::from("UTC")))
        }
    }
}

/// Builds a Polars [`Schema`] from the contract's declared columns, in
/// declaration order.
pub fn schema_from_contract<'a>(columns: impl IntoIterator<Item = (&'a str, ColType)>) -> Schema {
    let mut schema = Schema::default();
    for (name, t) in columns {
        schema.with_column(name.into(), dtype_for(t));
    }
    schema
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_is_documented_convention() {
        assert_eq!(dtype_for(ColType::String), DataType::String);
        assert_eq!(dtype_for(ColType::Int), DataType::Int64);
        assert_eq!(dtype_for(ColType::Float), DataType::Float64);
        assert_eq!(dtype_for(ColType::Bool), DataType::Boolean);
        assert_eq!(dtype_for(ColType::Date), DataType::Date);
        assert_eq!(
            dtype_for(ColType::Datetime),
            DataType::Datetime(TimeUnit::Microseconds, Some(TimeZone::from("UTC")))
        );
    }

    #[test]
    fn schema_preserves_order() {
        let s = schema_from_contract([("b", ColType::Int), ("a", ColType::String)]);
        let names: Vec<_> = s.iter_names().map(|n| n.to_string()).collect();
        assert_eq!(names, vec!["b", "a"]);
    }
}
