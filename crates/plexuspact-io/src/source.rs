//! `Source` — where the data comes from.

use std::path::PathBuf;

/// A resolved data source: a file on disk or the standard input stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A file path.
    File(PathBuf),
    /// Standard input (`-` on the command line).
    Stdin,
}

impl Source {
    /// Display name used in errors and `RunResult.source.path`.
    pub fn display_name(&self) -> String {
        match self {
            Source::File(p) => p.display().to_string(),
            Source::Stdin => "-".to_string(),
        }
    }
}

/// Resolves a CLI path argument: `-` means stdin, anything else is a file path.
pub fn resolve(path_or_dash: &str) -> Source {
    if path_or_dash == "-" {
        Source::Stdin
    } else {
        Source::File(PathBuf::from(path_or_dash))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dash_is_stdin() {
        assert_eq!(resolve("-"), Source::Stdin);
        assert_eq!(resolve("-").display_name(), "-");
    }

    #[test]
    fn path_is_file() {
        assert_eq!(resolve("data.csv"), Source::File(PathBuf::from("data.csv")));
    }
}
