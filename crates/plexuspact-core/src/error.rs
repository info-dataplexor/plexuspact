//! Orchestration error type.

/// Errors from running a validation or profiling a source.
///
/// Contract *parsing* and *linting* are deliberately not here: the CLI performs
/// those first (rendering miette diagnostics) and passes an already-valid
/// [`plexuspact_contract::Contract`] into [`crate::run`]. This keeps `core`
/// free of presentation concerns (doc 03 §12: `core` is library-safe).
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// A check-engine failure (unreadable input, internal data error).
    #[error(transparent)]
    Engine(#[from] plexuspact_engine::EngineError),

    /// An input/output failure while opening or reading the source.
    #[error(transparent)]
    Io(#[from] plexuspact_io::IoError),
}

impl CoreError {
    /// Whether this is a user-facing input error (missing/unreadable/malformed
    /// source → exit code 2) rather than an internal engine failure (exit 3).
    /// Lets the CLI choose an exit code without depending on `plexuspact-engine`.
    #[must_use]
    pub fn is_user_error(&self) -> bool {
        use plexuspact_engine::EngineError;
        match self {
            CoreError::Io(_) => true,
            CoreError::Engine(e) => {
                matches!(e, EngineError::Io(_) | EngineError::NestedColumn { .. })
            }
        }
    }
}
