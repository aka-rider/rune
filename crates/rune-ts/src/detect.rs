//! Compile-free path+content -> language decision. Given an optional
//! filesystem path and a document's text, `detect` walks a fixed priority
//! ladder — modeline, then whole filename, then extension, then shebang —
//! and returns what the document should open as. It constructs no
//! tree-sitter parser or query anywhere in this module, so it is
//! UI-thread-safe like `lang::resolve`, and it reaches language identity
//! only through that same function — never a language name of its own
//! invention.

use std::path::Path;

use rune_syntax::LangId;

use crate::lang;

/// What a path plus its content identify a document as. `Markdown` is a
/// distinct variant rather than a `lang::resolve` result because markdown
/// is comrak's, never a tree-sitter grammar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detected {
    Markdown,
    Lang(LangId),
}

/// Whole-file-name matches, compared case-insensitively against
/// `Path::file_name()` — the leading dot IS part of the key, because
/// `Path::extension()` is `None` for a dotfile. Every value here must be a
/// spelling `lang::resolve` already accepts.
pub static FILENAMES: &[(&str, &str)] = &[
    // shell
    (".bashrc", "bash"),
    (".bash_profile", "bash"),
    (".bash_aliases", "bash"),
    (".bash_logout", "bash"),
    (".profile", "bash"),
    (".zshrc", "bash"),
    (".zshenv", "bash"),
    (".zprofile", "bash"),
    (".zlogin", "bash"),
    (".zlogout", "bash"),
    (".env", "bash"),
    (".envrc", "bash"),
    // ruby
    ("gemfile", "ruby"),
    ("rakefile", "ruby"),
    ("podfile", "ruby"),
    ("brewfile", "ruby"),
    ("guardfile", "ruby"),
    ("vagrantfile", "ruby"),
    ("fastfile", "ruby"),
    ("appfile", "ruby"),
    // json
    (".babelrc", "json"),
    (".eslintrc", "json"),
    (".prettierrc", "json"),
    (".swcrc", "json"),
    (".jshintrc", "json"),
    // toml
    ("cargo.lock", "toml"),
    ("poetry.lock", "toml"),
    ("pipfile", "toml"),
    // yaml
    (".clang-format", "yaml"),
];

/// Whole file names that open as markdown — the extension-less
/// documentation files a markdown editor sees most often.
static MARKDOWN_FILENAMES: &[&str] = &[
    "readme",
    "changelog",
    "changes",
    "contributing",
    "notes",
    "todo",
];

/// Shebang interpreter basenames that are NOT already a spelling
/// `lang::resolve` accepts. Everything else (`bash`, `sh`, `zsh`,
/// `python`, `ruby`, `php`, `swift`) already resolves, so the shebang
/// step falls through to `resolve`. Every value here must be a spelling
/// `lang::resolve` already accepts.
pub static INTERPRETERS: &[(&str, &str)] = &[
    ("node", "javascript"),
    ("bun", "javascript"),
    ("deno", "typescript"),
    ("ksh", "bash"),
    ("dash", "bash"),
    ("ash", "bash"),
];

const HEAD_LINES: usize = 5;
const TAIL_BYTES: usize = 4096;
const MAX_LINE: usize = 256;

/// Truncates `line` to at most `MAX_LINE` bytes, cutting on a char
/// boundary, then lowercases it.
fn cap_and_lower(line: &str) -> String {
    let end = line.floor_char_boundary(line.len().min(MAX_LINE));
    line.get(..end).unwrap_or("").to_lowercase()
}

/// Maps an extracted modeline value to a `Detected`, falling through to
/// `None` when it names neither markdown nor a known grammar.
fn map_modeline_value(value: &str) -> Option<Detected> {
    let value = value.trim();
    if value == "markdown" || value == "md" {
        return Some(Detected::Markdown);
    }
    lang::resolve(value).map(Detected::Lang)
}

/// Tries the vim/vi/ex `ft=`/`filetype=` modeline form on one lowercased,
/// already-capped line.
fn try_vim_form(line: &str) -> Option<Detected> {
    let bytes = line.as_bytes();
    for marker in ["vim:", "vi:", "ex:"] {
        let mut search_from = 0usize;
        while let Some(rel) = line.get(search_from..).and_then(|s| s.find(marker)) {
            let idx = search_from + rel;
            let preceded_ok = idx == 0
                || bytes
                    .get(idx.wrapping_sub(1))
                    .is_some_and(u8::is_ascii_whitespace);
            if preceded_ok {
                let remainder = line.get(idx + marker.len()..).unwrap_or("");
                let token = remainder
                    .split([':', ' ', '\t'])
                    .find(|tok| tok.starts_with("ft=") || tok.starts_with("filetype="));
                if let Some(tok) = token {
                    let value = tok.split_once('=').map_or("", |(_, v)| v);
                    if let Some(detected) = map_modeline_value(value) {
                        return Some(detected);
                    }
                }
            }
            search_from = idx + marker.len();
        }
    }
    None
}

/// Tries the emacs `-*- mode: NAME -*-` (or bare `-*- NAME -*-`) modeline
/// form on one lowercased, already-capped line.
fn try_emacs_form(line: &str) -> Option<Detected> {
    let first = line.find("-*-")?;
    let after_first = first + 3;
    let rest = line.get(after_first..)?;
    let second_rel = rest.find("-*-")?;
    let span = rest.get(..second_rel)?;
    if let Some(value) = span.split(';').find_map(|segment| {
        let (key, value) = segment.split_once(':')?;
        if key.trim() == "mode" {
            Some(value.trim())
        } else {
            None
        }
    }) {
        return map_modeline_value(value);
    }
    if !span.contains(':') {
        return map_modeline_value(span.trim());
    }
    None
}

/// Extracts a UTF-8-safe tail slice of at most `TAIL_BYTES` bytes from the
/// end of `content`.
fn safe_tail(content: &str) -> &str {
    let start = content.floor_char_boundary(content.len().saturating_sub(TAIL_BYTES));
    content.get(start..).unwrap_or("")
}

/// Looks for an explicit vim- or emacs-style modeline in the first
/// [`HEAD_LINES`] lines of `content`, then in the last [`HEAD_LINES`] lines
/// of a bounded tail slice — an author's explicit filetype declaration
/// outranks everything else this module infers.
fn from_modeline(content: &str) -> Option<Detected> {
    let head = content.lines().take(HEAD_LINES);
    let tail_slice = safe_tail(content);
    let tail_lines: Vec<&str> = tail_slice.lines().collect();
    let tail_start = tail_lines.len().saturating_sub(HEAD_LINES);
    let tail = tail_lines.get(tail_start..).into_iter().flatten().copied();

    for line in head.chain(tail) {
        let capped = cap_and_lower(line);
        if let Some(detected) = try_vim_form(&capped).or_else(|| try_emacs_form(&capped)) {
            return Some(detected);
        }
    }
    None
}

/// Matches `path`'s whole file name, case-insensitively, against
/// [`MARKDOWN_FILENAMES`] and then [`FILENAMES`].
fn from_filename(path: &Path) -> Option<Detected> {
    let name = path.file_name()?.to_str()?.to_lowercase();
    if MARKDOWN_FILENAMES.contains(&name.as_str()) {
        return Some(Detected::Markdown);
    }
    FILENAMES
        .iter()
        .find(|(key, _)| *key == name)
        .and_then(|(_, value)| lang::resolve(value))
        .map(Detected::Lang)
}

/// Resolves `path`'s extension through `lang::resolve`, which already
/// lowercases and strips a leading dot.
fn from_extension(path: &Path) -> Option<Detected> {
    lang::resolve(path.extension()?.to_str()?).map(Detected::Lang)
}

/// Reduces a shebang interpreter path to a bare, lowercased, unversioned
/// basename — `/usr/bin/python3.11` and `python3.11` alike become
/// `python`.
fn interpreter_basename(token: &str) -> Option<String> {
    let base = token.rsplit('/').next().unwrap_or(token).to_lowercase();
    let trimmed = base.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Reads the interpreter named on a `#!` line, resolving an `env`
/// indirection (including `env -S` and `env FOO=bar` forms) to the real
/// interpreter token.
fn shebang_interpreter(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("#!")?;
    let mut tokens = rest.split_ascii_whitespace();
    let first = tokens.next()?;
    let first_base = first.rsplit('/').next().unwrap_or(first);
    if first_base != "env" {
        return Some(first);
    }
    tokens.find(|tok| !tok.starts_with('-') && !tok.contains('='))
}

/// Looks for a `#!` shebang on the first line of `content` and maps its
/// interpreter to a language, first through [`INTERPRETERS`] and then
/// through `lang::resolve` for spellings it already understands.
fn from_shebang(content: &str) -> Option<Detected> {
    if !content.starts_with("#!") {
        return None;
    }
    let first_line = content.lines().next().unwrap_or("");
    let end = first_line.floor_char_boundary(first_line.len().min(MAX_LINE));
    let capped = first_line.get(..end).unwrap_or("");
    let interpreter = shebang_interpreter(capped)?;
    let name = interpreter_basename(interpreter)?;
    let canonical = INTERPRETERS
        .iter()
        .find(|(key, _)| *key == name)
        .map_or(name.as_str(), |(_, value)| *value);
    lang::resolve(canonical).map(Detected::Lang)
}

/// The one language decision for a file: a modeline (explicit author
/// intent) beats a well-known whole file name, which beats the
/// extension, which beats a shebang. Pure — no tree-sitter call, no
/// filesystem access — so it is safe on the UI thread and testable
/// without a buffer. `content` may be empty; only a bounded head and
/// tail are ever scanned.
pub fn detect(path: Option<&Path>, content: &str) -> Option<Detected> {
    from_modeline(content)
        .or_else(|| path.and_then(from_filename))
        .or_else(|| path.and_then(from_extension))
        .or_else(|| from_shebang(content))
}
