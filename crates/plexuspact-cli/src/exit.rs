//! Process exit-code convention (ADR-004 / PRD FR-7).

/// The run passed (no error-severity failures).
pub const CODE_OK: u8 = 0;
/// One or more error-severity checks failed (or, with `--strict`, warns).
pub const CODE_FAILURES: u8 = 1;
/// Usage error or invalid contract.
pub const CODE_USAGE: u8 = 2;
/// Unexpected internal error.
pub const CODE_INTERNAL: u8 = 3;
