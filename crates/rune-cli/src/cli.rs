//! Strict command-line parsing (plan WP7.S1-S3) — no dependency added for
//! this; `rune-cli` stays free of `clap`/`lexopt`. Supports a flag set
//! (`-w`, `--version`) plus any number of positional files, but rejects
//! any unrecognised `-`-prefixed argument outright rather than silently
//! treating it as a filename.
//!
//! `-w`'s value and every positional are absolutized the same way
//! (`crate::to_abs_path`, reused rather than duplicated) against the ONE
//! `cwd` the caller reads (plan WP4.S6/[rune-cli 8]: `main` reads it
//! exactly once and hands it down here, rather than this module or
//! `to_abs_path` re-reading `env::current_dir()` per argument) — but
//! `-w`'s existence/directory-ness is NOT checked here: that needs the
//! injected `Vfs`, which this module never touches, so it happens in
//! `main` right after parsing (WP7.S4).
//!
//! Every argument arrives as an `OsString` (`env::args_os`, plan WP4.S2):
//! a non-Unicode argument is a [`CliError::NonUnicodeArg`], not a panic.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// A successfully parsed launch: an optional `-w` working directory
/// (already absolutized, not yet validated as an existing directory — see
/// the module doc) and the positional files, in command-line order.
#[derive(Debug, PartialEq, Eq)]
pub struct Cli {
    pub work_dir: Option<PathBuf>,
    pub files: Vec<PathBuf>,
    pub diff: Option<(PathBuf, PathBuf)>,
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
/// exits `exit_code::USAGE` — except the `-w` causes below, which report
/// their own distinct reason (plan WP4.S6/[rune-cli 9]: a wildcard "not a
/// directory" used to collapse "doesn't exist", "permission denied", and
/// "genuinely a file" into one indistinguishable message).
#[derive(Debug, PartialEq, Eq)]
pub enum CliError {
    UnknownFlag(String),
    MissingValue(&'static str),
    /// An argument's raw bytes don't round-trip through UTF-8 (plan
    /// WP4.S2/[rune-cli 4]) — reproducible from ordinary shell input since
    /// macOS filenames are byte strings, not `env::args()`'s panic.
    NonUnicodeArg(OsString),
    /// A file path argument (a positional, or `-w`'s value) was the empty
    /// string (plan WP4.S6/[rune-cli 10]) — rejected at parse rather than
    /// silently absolutizing to `cwd` itself and opening that as a file.
    EmptyPath,
    /// `-w`'s value exists but isn't a directory.
    NotADirectory(PathBuf),
    /// `-w`'s value doesn't exist at all.
    WorkDirNotFound(PathBuf),
    /// `-w`'s value couldn't be `stat`ed for some OTHER reason (permission
    /// denied, an I/O error) — `e` is that error's `Display` text.
    WorkDirUnreadable(PathBuf, String),
    DiffMissingArgument,
    DiffExtraArguments,
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::UnknownFlag(flag) => write!(f, "unknown flag: {flag}"),
            CliError::MissingValue(flag) => write!(f, "{flag} requires a value"),
            CliError::NonUnicodeArg(arg) => {
                write!(f, "argument is not valid UTF-8: {}", arg.to_string_lossy())
            }
            CliError::EmptyPath => write!(f, "a file path argument must not be empty"),
            CliError::NotADirectory(path) => write!(f, "not a directory: {}", path.display()),
            CliError::WorkDirNotFound(path) => {
                write!(f, "no such directory: {}", path.display())
            }
            CliError::WorkDirUnreadable(path, e) => {
                write!(f, "cannot access {}: {e}", path.display())
            }
            CliError::DiffMissingArgument => {
                write!(f, "--diff requires exactly two file arguments")
            }
            CliError::DiffExtraArguments => {
                write!(f, "--diff accepts no other file arguments")
            }
        }
    }
}

pub const USAGE_TEXT: &str = "usage: rune [-w <dir>] [--version] [--help] [--diff A B] [file...]";

/// Parses `args` (already excluding argv[0]) into a [`CliAction`], resolving
/// every relative path against `cwd` (read exactly once by the caller). `-w`
/// consumes the very next argument as its value unconditionally — a value
/// starting with `-` is still accepted as the directory, since only
/// arguments OUTSIDE a flag's own value are checked against the
/// unknown-flag rule.
fn next_diff_arg<I: Iterator<Item = OsString>>(args: &mut I) -> Result<String, CliError> {
    let value = args
        .next()
        .ok_or(CliError::DiffMissingArgument)?
        .into_string()
        .map_err(CliError::NonUnicodeArg)?;
    if value.is_empty() {
        return Err(CliError::EmptyPath);
    }
    if value == "-" || value.starts_with('-') {
        return Err(CliError::DiffMissingArgument);
    }
    Ok(value)
}

pub fn parse<I: Iterator<Item = OsString>>(args: I, cwd: &Path) -> Result<CliAction, CliError> {
    let mut work_dir: Option<PathBuf> = None;
    let mut files = Vec::new();
    let mut diff: Option<(PathBuf, PathBuf)> = None;
    let mut args = args;

    while let Some(arg) = args.next() {
        let arg = arg.into_string().map_err(CliError::NonUnicodeArg)?;
        match arg.as_str() {
            "--version" => return Ok(CliAction::Version),
            "--help" => return Ok(CliAction::Help),
            "-w" => {
                let value = args
                    .next()
                    .ok_or(CliError::MissingValue("-w"))?
                    .into_string()
                    .map_err(CliError::NonUnicodeArg)?;
                if value.is_empty() {
                    return Err(CliError::EmptyPath);
                }
                work_dir = Some(crate::to_abs_path(&value, cwd));
            }
            "--diff" => {
                if diff.is_some() {
                    return Err(CliError::DiffExtraArguments);
                }
                let a = next_diff_arg(&mut args)?;
                let b = next_diff_arg(&mut args)?;
                diff = Some((crate::to_abs_path(&a, cwd), crate::to_abs_path(&b, cwd)));
            }
            _ if arg == "-" || arg.starts_with('-') => {
                return Err(CliError::UnknownFlag(arg));
            }
            "" => return Err(CliError::EmptyPath),
            _ => files.push(crate::to_abs_path(&arg, cwd)),
        }
    }

    if diff.is_some() && !files.is_empty() {
        return Err(CliError::DiffExtraArguments);
    }

    Ok(CliAction::Run(Cli {
        work_dir,
        files,
        diff,
    }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> impl Iterator<Item = OsString> {
        items
            .iter()
            .map(OsString::from)
            .collect::<Vec<_>>()
            .into_iter()
    }

    fn cwd() -> PathBuf {
        PathBuf::from("/cwd")
    }

    #[test]
    fn dash_w_with_a_value() {
        let action = parse(args(&["-w", "/some/dir"]), &cwd()).expect("should parse");
        assert_eq!(
            action,
            CliAction::Run(Cli {
                work_dir: Some(PathBuf::from("/some/dir")),
                files: vec![],
                diff: None,
            })
        );
    }

    #[test]
    fn dash_w_with_no_value_is_a_missing_value_error() {
        let err = parse(args(&["-w"]), &cwd()).expect_err("missing value must error");
        assert_eq!(err, CliError::MissingValue("-w"));
    }

    #[test]
    fn dash_w_with_an_empty_value_is_rejected() {
        let err = parse(args(&["-w", ""]), &cwd()).expect_err("empty -w value must error");
        assert_eq!(err, CliError::EmptyPath);
    }

    #[test]
    fn version_flag() {
        let action = parse(args(&["--version"]), &cwd()).expect("should parse");
        assert_eq!(action, CliAction::Version);
    }

    #[test]
    fn help_flag() {
        let action = parse(args(&["--help"]), &cwd()).expect("should parse");
        assert_eq!(action, CliAction::Help);
    }

    #[test]
    fn one_positional_file() {
        let action = parse(args(&["/tmp/a.md"]), &cwd()).expect("should parse");
        assert_eq!(
            action,
            CliAction::Run(Cli {
                work_dir: None,
                files: vec![PathBuf::from("/tmp/a.md")],
                diff: None,
            })
        );
    }

    #[test]
    fn relative_positional_resolves_against_cwd() {
        let action = parse(args(&["a.md"]), &cwd()).expect("should parse");
        assert_eq!(
            action,
            CliAction::Run(Cli {
                work_dir: None,
                files: vec![PathBuf::from("/cwd/a.md")],
                diff: None,
            })
        );
    }

    #[test]
    fn three_positionals_preserve_order() {
        let action =
            parse(args(&["/tmp/a.md", "/tmp/b.md", "/tmp/c.md"]), &cwd()).expect("should parse");
        assert_eq!(
            action,
            CliAction::Run(Cli {
                work_dir: None,
                files: vec![
                    PathBuf::from("/tmp/a.md"),
                    PathBuf::from("/tmp/b.md"),
                    PathBuf::from("/tmp/c.md"),
                ],
                diff: None,
            })
        );
    }

    #[test]
    fn unknown_flag_is_rejected() {
        let err = parse(args(&["--bogus"]), &cwd()).expect_err("unknown flag must error");
        assert_eq!(err, CliError::UnknownFlag("--bogus".to_string()));
    }

    #[test]
    fn bare_dash_is_rejected() {
        let err = parse(args(&["-"]), &cwd()).expect_err("bare dash must error");
        assert_eq!(err, CliError::UnknownFlag("-".to_string()));
    }

    #[test]
    fn empty_positional_is_rejected() {
        let err = parse(args(&[""]), &cwd()).expect_err("empty positional must error");
        assert_eq!(err, CliError::EmptyPath);
    }

    #[test]
    fn non_unicode_argument_is_rejected_not_panicked() {
        use std::os::unix::ffi::OsStringExt;

        let bad = OsString::from_vec(vec![0xff, 0xfe]);
        let err = parse(vec![bad.clone()].into_iter(), &cwd())
            .expect_err("non-UTF-8 argument must error");
        assert_eq!(err, CliError::NonUnicodeArg(bad));
    }

    #[test]
    fn dash_dash_diff_with_two_values() {
        let action =
            parse(args(&["--diff", "/tmp/a.md", "/tmp/b.md"]), &cwd()).expect("should parse");
        assert_eq!(
            action,
            CliAction::Run(Cli {
                work_dir: None,
                files: vec![],
                diff: Some((PathBuf::from("/tmp/a.md"), PathBuf::from("/tmp/b.md"))),
            })
        );
    }

    #[test]
    fn dash_dash_diff_resolves_relative_values_against_cwd() {
        let action = parse(args(&["--diff", "a.md", "b.md"]), &cwd()).expect("should parse");
        assert_eq!(
            action,
            CliAction::Run(Cli {
                work_dir: None,
                files: vec![],
                diff: Some((PathBuf::from("/cwd/a.md"), PathBuf::from("/cwd/b.md"))),
            })
        );
    }

    #[test]
    fn dash_dash_diff_with_one_value_is_a_missing_argument_error() {
        let err = parse(args(&["--diff", "/tmp/a.md"]), &cwd())
            .expect_err("--diff needs two file arguments");
        assert_eq!(err, CliError::DiffMissingArgument);
    }

    #[test]
    fn dash_dash_diff_with_no_values_is_a_missing_argument_error() {
        let err = parse(args(&["--diff"]), &cwd()).expect_err("--diff needs two file arguments");
        assert_eq!(err, CliError::DiffMissingArgument);
    }

    #[test]
    fn dash_dash_diff_second_value_looking_like_a_flag_is_a_missing_argument_error() {
        let err = parse(args(&["--diff", "/tmp/a.md", "--bogus"]), &cwd())
            .expect_err("--diff's second value must be a file, not a flag");
        assert_eq!(err, CliError::DiffMissingArgument);
    }

    #[test]
    fn dash_dash_diff_with_an_extra_positional_is_rejected() {
        let err = parse(
            args(&["--diff", "/tmp/a.md", "/tmp/b.md", "/tmp/c.md"]),
            &cwd(),
        )
        .expect_err("--diff accepts no extra positionals");
        assert_eq!(err, CliError::DiffExtraArguments);
    }

    #[test]
    fn dash_dash_diff_with_a_leading_extra_positional_is_rejected() {
        let err = parse(
            args(&["/tmp/c.md", "--diff", "/tmp/a.md", "/tmp/b.md"]),
            &cwd(),
        )
        .expect_err("--diff accepts no extra positionals");
        assert_eq!(err, CliError::DiffExtraArguments);
    }

    #[test]
    fn dash_dash_diff_twice_is_rejected() {
        let err = parse(
            args(&[
                "--diff",
                "/tmp/a.md",
                "/tmp/b.md",
                "--diff",
                "/tmp/c.md",
                "/tmp/d.md",
            ]),
            &cwd(),
        )
        .expect_err("a second --diff must be rejected");
        assert_eq!(err, CliError::DiffExtraArguments);
    }

    #[test]
    fn dash_dash_diff_combines_with_dash_w() {
        let action = parse(
            args(&["-w", "/work", "--diff", "/tmp/a.md", "/tmp/b.md"]),
            &cwd(),
        )
        .expect("should parse");
        assert_eq!(
            action,
            CliAction::Run(Cli {
                work_dir: Some(PathBuf::from("/work")),
                files: vec![],
                diff: Some((PathBuf::from("/tmp/a.md"), PathBuf::from("/tmp/b.md"))),
            })
        );
    }
}
