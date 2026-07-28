//! Strict command-line parsing (plan WP7.S1-S3) — no dependency added for
//! this; `rune-cli` stays free of `clap`/`lexopt`. Mirrors the Go
//! reference's flag set (`-w`, `--version`) plus any number of positional
//! files, but rejects any unrecognised `-`-prefixed argument outright
//! rather than silently treating it as a filename (CONSTITUTION §1.3:
//! surface invalid input, no silent fallback).
//!
//! `-w`'s value and every positional are absolutized the same way
//! (`crate::to_abs_path`, reused rather than duplicated) — but `-w`'s
//! existence/directory-ness is NOT checked here: that needs the injected
//! `Vfs`, which this module never touches, so it happens in `main` right
//! after parsing (WP7.S4).

use std::path::PathBuf;

/// A successfully parsed launch: an optional `-w` working directory
/// (already absolutized, not yet validated as an existing directory — see
/// the module doc) and the positional files, in command-line order.
#[derive(Debug, PartialEq, Eq)]
pub struct Cli {
    pub work_dir: Option<PathBuf>,
    pub files: Vec<PathBuf>,
}

/// What `main` should do with a parsed command line.
#[derive(Debug, PartialEq, Eq)]
pub enum CliAction {
    Run(Cli),
    Version,
    Help,
}

/// Why parsing (or the post-parse `-w` validation in `main`) failed. `main`
/// turns every variant into a stderr message plus [`USAGE_TEXT`], then
/// exits `exit_code::USAGE`.
#[derive(Debug, PartialEq, Eq)]
pub enum CliError {
    UnknownFlag(String),
    MissingValue(&'static str),
    NotADirectory(PathBuf),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::UnknownFlag(flag) => write!(f, "unknown flag: {flag}"),
            CliError::MissingValue(flag) => write!(f, "{flag} requires a value"),
            CliError::NotADirectory(path) => write!(f, "not a directory: {}", path.display()),
        }
    }
}

pub const USAGE_TEXT: &str = "usage: rune [-w <dir>] [--version] [--help] [file...]";

/// Parses `args` (already excluding argv[0]) into a [`CliAction`]. `-w`
/// consumes the very next argument as its value unconditionally — matching
/// Go's `flag` package, a value starting with `-` is still accepted as the
/// directory, since only arguments OUTSIDE a flag's own value are checked
/// against the unknown-flag rule.
pub fn parse<I: Iterator<Item = String>>(args: I) -> Result<CliAction, CliError> {
    let mut work_dir: Option<PathBuf> = None;
    let mut files = Vec::new();
    let mut args = args;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--version" => return Ok(CliAction::Version),
            "--help" => return Ok(CliAction::Help),
            "-w" => {
                let value = args.next().ok_or(CliError::MissingValue("-w"))?;
                work_dir = Some(crate::to_abs_path(&value));
            }
            _ if arg == "-" || arg.starts_with('-') => {
                return Err(CliError::UnknownFlag(arg));
            }
            _ => files.push(crate::to_abs_path(&arg)),
        }
    }

    Ok(CliAction::Run(Cli { work_dir, files }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> impl Iterator<Item = String> {
        items
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn dash_w_with_a_value() {
        let action = parse(args(&["-w", "/some/dir"])).expect("should parse");
        assert_eq!(
            action,
            CliAction::Run(Cli {
                work_dir: Some(PathBuf::from("/some/dir")),
                files: vec![],
            })
        );
    }

    #[test]
    fn dash_w_with_no_value_is_a_missing_value_error() {
        let err = parse(args(&["-w"])).expect_err("missing value must error");
        assert_eq!(err, CliError::MissingValue("-w"));
    }

    #[test]
    fn version_flag() {
        let action = parse(args(&["--version"])).expect("should parse");
        assert_eq!(action, CliAction::Version);
    }

    #[test]
    fn help_flag() {
        let action = parse(args(&["--help"])).expect("should parse");
        assert_eq!(action, CliAction::Help);
    }

    #[test]
    fn one_positional_file() {
        let action = parse(args(&["/tmp/a.md"])).expect("should parse");
        assert_eq!(
            action,
            CliAction::Run(Cli {
                work_dir: None,
                files: vec![PathBuf::from("/tmp/a.md")],
            })
        );
    }

    #[test]
    fn three_positionals_preserve_order() {
        let action = parse(args(&["/tmp/a.md", "/tmp/b.md", "/tmp/c.md"])).expect("should parse");
        assert_eq!(
            action,
            CliAction::Run(Cli {
                work_dir: None,
                files: vec![
                    PathBuf::from("/tmp/a.md"),
                    PathBuf::from("/tmp/b.md"),
                    PathBuf::from("/tmp/c.md"),
                ],
            })
        );
    }

    #[test]
    fn unknown_flag_is_rejected() {
        let err = parse(args(&["--bogus"])).expect_err("unknown flag must error");
        assert_eq!(err, CliError::UnknownFlag("--bogus".to_string()));
    }

    #[test]
    fn bare_dash_is_rejected() {
        let err = parse(args(&["-"])).expect_err("bare dash must error");
        assert_eq!(err, CliError::UnknownFlag("-".to_string()));
    }
}
