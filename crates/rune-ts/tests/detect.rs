//! Compile-free `detect` coverage: modeline, whole filename, extension and
//! shebang detection, in priority order, plus the multi-byte-safety and
//! table-integrity guards. Touches no registry (query/parser compilation).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::Path;

use rune_syntax::LangId;
use rune_ts::detect::{Detected, FILENAMES, INTERPRETERS, detect};
use rune_ts::lang;

fn lang_id(name: &str) -> LangId {
    LangId::from_name(name).unwrap()
}

/// A dotfile whole-filename match resolves through the shell alias.
#[test]
fn dotfile_whole_name_resolves_to_bash() {
    assert_eq!(
        detect(Some(Path::new("/x/.zshrc")), ""),
        Some(Detected::Lang(lang_id("bash")))
    );
}

/// A capitalized Ruby convention file matches case-insensitively.
#[test]
fn gemfile_resolves_to_ruby() {
    assert_eq!(
        detect(Some(Path::new("/x/Gemfile")), ""),
        Some(Detected::Lang(lang_id("ruby")))
    );
}

/// An extension-less README opens as markdown.
#[test]
fn readme_resolves_to_markdown() {
    assert_eq!(
        detect(Some(Path::new("/x/README")), ""),
        Some(Detected::Markdown)
    );
}

/// The markdown-filename match is case-insensitive.
#[test]
fn lowercase_readme_resolves_to_markdown() {
    assert_eq!(
        detect(Some(Path::new("/x/readme")), ""),
        Some(Detected::Markdown)
    );
}

/// Cargo.lock is TOML despite its non-`.toml` extension.
#[test]
fn cargo_lock_resolves_to_toml() {
    assert_eq!(
        detect(Some(Path::new("/x/Cargo.lock")), ""),
        Some(Detected::Lang(lang_id("toml")))
    );
}

/// A `#!/bin/sh` shebang on an extension-less file resolves to bash.
#[test]
fn sh_shebang_resolves_to_bash() {
    assert_eq!(
        detect(Some(Path::new("/x/deploy")), "#!/bin/sh\necho hi\n"),
        Some(Detected::Lang(lang_id("bash")))
    );
}

/// `env -S python3 -u` unwraps the `env` indirection and its flags to find
/// the real interpreter.
#[test]
fn env_dash_s_shebang_resolves_to_python() {
    assert_eq!(
        detect(
            Some(Path::new("/x/deploy")),
            "#!/usr/bin/env -S python3 -u\n"
        ),
        Some(Detected::Lang(lang_id("python")))
    );
}

/// `env node` maps through the `INTERPRETERS` table to javascript.
#[test]
fn env_node_shebang_resolves_to_javascript() {
    assert_eq!(
        detect(Some(Path::new("/x/serve")), "#!/usr/bin/env node\n"),
        Some(Detected::Lang(lang_id("javascript")))
    );
}

/// A vim modeline on a `.txt` file overrides the (nonexistent) extension
/// verdict.
#[test]
fn vim_modeline_resolves_to_python() {
    assert_eq!(
        detect(Some(Path::new("/x/a.txt")), "# vim: ft=python\n"),
        Some(Detected::Lang(lang_id("python")))
    );
}

/// A vim modeline beats the file's own `.rs` extension.
#[test]
fn vim_modeline_beats_extension() {
    assert_eq!(
        detect(Some(Path::new("/x/a.rs")), "// vim: ft=bash\n"),
        Some(Detected::Lang(lang_id("bash")))
    );
}

/// The emacs `-*- mode: NAME -*-` form is recognized.
#[test]
fn emacs_modeline_resolves_to_ruby() {
    assert_eq!(
        detect(Some(Path::new("/x/a.rb")), "# -*- mode: ruby -*-\n"),
        Some(Detected::Lang(lang_id("ruby")))
    );
}

/// A modeline in the tail of a longer file (past `HEAD_LINES`) is still
/// found via the trailing-lines scan.
#[test]
fn trailing_modeline_is_found() {
    assert_eq!(
        detect(
            Some(Path::new("/x/notes")),
            "body\nbody\nbody\nbody\nbody\nbody\nbody\n# vim: ft=yaml\n"
        ),
        Some(Detected::Lang(lang_id("yaml")))
    );
}

/// With no modeline, the extension still resolves.
#[test]
fn extension_still_works_without_modeline() {
    assert_eq!(
        detect(Some(Path::new("/x/a.rs")), ""),
        Some(Detected::Lang(lang_id("rust")))
    );
}

/// An unknown modeline value falls through to the extension instead of
/// producing `None`.
#[test]
fn unknown_modeline_value_falls_through_to_extension() {
    assert_eq!(
        detect(Some(Path::new("/x/a.rs")), "# vim: ft=cobol\n"),
        Some(Detected::Lang(lang_id("rust")))
    );
}

/// A modeline naming markdown explicitly yields the `Markdown` variant, not
/// a `Lang`.
#[test]
fn modeline_markdown_value_resolves_to_markdown_variant() {
    assert_eq!(
        detect(Some(Path::new("/x/a.txt")), "<!-- vim: ft=markdown -->\n"),
        Some(Detected::Markdown)
    );
}

/// A path/content pair matching nothing yields `None`.
#[test]
fn unmatched_extension_is_none() {
    assert_eq!(detect(Some(Path::new("/x/data.bin")), ""), None);
}

/// No path and no content yields `None`.
#[test]
fn no_path_no_content_is_none() {
    assert_eq!(detect(None, ""), None);
}

/// Plain prose with no path signal and no path is `None`.
#[test]
fn unrecognized_filename_and_content_is_none() {
    assert_eq!(detect(Some(Path::new("/x/mystery")), "random text\n"), None);
}

/// A long multi-byte UTF-8 document (well past `TAIL_BYTES`) must not
/// panic when the tail slice is computed.
#[test]
fn long_multibyte_content_does_not_panic() {
    let content = "\u{4e16}\u{754c}".repeat(4000);
    let _ = detect(Some(Path::new("/x/m")), &content);
}

/// A `vim:` occurrence directly preceded by a non-whitespace character is
/// not a modeline marker and must be rejected, even though a valid `ft=`
/// token sits right after it.
#[test]
fn vim_marker_preceded_by_non_whitespace_is_rejected() {
    assert_eq!(
        detect(Some(Path::new("/x/a.txt")), "Xvim: ft=python\n"),
        None
    );
}

/// A `vim:` marker sitting at the very start of a line (nothing precedes
/// it) is accepted, the same as one preceded by whitespace.
#[test]
fn vim_marker_at_start_of_line_is_accepted() {
    assert_eq!(
        detect(Some(Path::new("/x/a.txt")), "vim: ft=python\n"),
        Some(Detected::Lang(lang_id("python")))
    );
}

/// Rejecting one `vim:` occurrence (bad preceding character) must not
/// corrupt the scan position used to find a later, validly-preceded
/// occurrence on the same line.
#[test]
fn second_vim_marker_is_still_found_after_a_rejected_first_one() {
    assert_eq!(
        detect(Some(Path::new("/x/a.txt")), "01234vim: vim: ft=python\n"),
        Some(Detected::Lang(lang_id("python")))
    );
}

/// The emacs `mode:` form must be read from the modeline itself, not
/// coincidentally reproduced by the file's own extension.
#[test]
fn emacs_mode_form_is_read_without_extension_help() {
    assert_eq!(
        detect(Some(Path::new("/x/a.txt")), "# -*- mode: ruby -*-\n"),
        Some(Detected::Lang(lang_id("ruby")))
    );
}

/// The emacs `-*-` opener at the very start of a line (nothing precedes
/// it) is still recognized.
#[test]
fn emacs_mode_form_at_start_of_line_is_read() {
    assert_eq!(
        detect(Some(Path::new("/x/a.txt")), "-*- mode: ruby -*-\n"),
        Some(Detected::Lang(lang_id("ruby")))
    );
}

/// The bare emacs form `-*- NAME -*-` (no `mode:` key) is read as a
/// language name on its own.
#[test]
fn emacs_bare_name_form_without_mode_key_is_read() {
    assert_eq!(
        detect(Some(Path::new("/x/a.txt")), "# -*- python -*-\n"),
        Some(Detected::Lang(lang_id("python")))
    );
}

/// Every value in `FILENAMES` and `INTERPRETERS` must be a spelling
/// `lang::resolve` already accepts, so neither table can silently name a
/// grammar that does not exist.
#[test]
fn filename_and_interpreter_tables_resolve() {
    for (key, value) in FILENAMES {
        assert!(
            lang::resolve(value).is_some(),
            "FILENAMES entry {key:?} points at unresolvable {value:?}"
        );
    }
    for (key, value) in INTERPRETERS {
        assert!(
            lang::resolve(value).is_some(),
            "INTERPRETERS entry {key:?} points at unresolvable {value:?}"
        );
    }
}
