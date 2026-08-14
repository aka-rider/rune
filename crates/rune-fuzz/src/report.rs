//! Writes one failure bundle for the MINIMAL failing case: `script.rune`
//! (directly loadable by a later work package's `tests/replay.rs`) plus
//! `report.txt` (the frozen `Snapshot`/`StepCtx`, human-readable).
//!
//! The bundle directory is named `<id-lowercased>-<hash>`, where `<hash>`
//! is a deterministic 8-hex-digit FNV-1a of the encoded script — never a
//! timestamp — so re-running the same catch overwrites in place instead of
//! accumulating one directory per run.
//!
//! No `unwrap`/`expect`/`panic!`/unchecked indexing in this file (G17):
//! every fallible step is a plain `io::Result`.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rune_tui::render::Cell;

use crate::action::Action;
use crate::driver::{self, RunResult};
use crate::hash::fnv1a32;
use crate::invariant::Violation;
use crate::script;

/// Writes the failure bundle under `dir_root` and returns its directory.
/// `path` (plan WP7.S2) is the document path the session that produced
/// `result` was opened at — carried into the encoded `script.rune` so a
/// non-default-path catch (a code document, a fenced-language document)
/// replays against the SAME `DocumentKind`, not silently against the
/// markdown default.
pub fn write(
    dir_root: &Path,
    v: &Violation,
    path: &str,
    content: &str,
    actions: &[Action],
    result: &RunResult,
) -> io::Result<PathBuf> {
    let encoded = script::encode(path, content, actions);
    let hash = fnv1a32(encoded.as_bytes());
    let dir = dir_root.join(format!("{}-{hash:08x}", v.id.to_lowercase()));

    fs::create_dir_all(&dir)?;
    fs::write(dir.join("script.rune"), &encoded)?;
    fs::write(dir.join("report.txt"), render_report(v, content, result))?;
    Ok(dir)
}

/// Re-runs the failing case under the panic guard and writes its bundle —
/// the one path from "this input fails" to an artifact on disk, so a panic
/// during the re-run still produces the bundle instead of unwinding past
/// the write. `fallback` names the violation to record when the re-run
/// itself reports none.
pub fn capture(
    dir_root: &Path,
    path: &str,
    content: &str,
    actions: &[Action],
    fallback: Violation,
) -> io::Result<(Violation, PathBuf)> {
    let result = driver::run_catching_panic(path, content, actions);
    let violation = result.violation.clone().unwrap_or(fallback);
    let dir = write(dir_root, &violation, path, content, actions, &result)?;
    Ok((violation, dir))
}

fn render_report(v: &Violation, content: &str, result: &RunResult) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "invariant: {}", v.id);
    let _ = writeln!(out, "message: {}", v.message);
    let _ = writeln!(out, "steps: {}", result.steps);
    let _ = writeln!(out, "seed content: {content:?}");
    if let Some(site) = &v.site {
        let _ = writeln!(out, "panic location: {}", site.location);
        let _ = writeln!(out, "panic backtrace:");
        let _ = writeln!(out, "{}", site.backtrace);
    }
    let _ = writeln!(out);

    match &result.final_snapshot {
        Some(snap) => {
            let _ = writeln!(out, "snapshot.content: {:?}", snap.content);
            let _ = writeln!(out, "snapshot.version: {}", snap.version);
            let _ = writeln!(out, "snapshot.saved_version: {}", snap.saved_version);
            let _ = writeln!(out, "snapshot.is_dirty: {}", snap.is_dirty);
            let _ = writeln!(out, "snapshot.cursors: {:?}", snap.cursors);
            let _ = writeln!(out, "snapshot.journal_pos: {}", snap.journal_pos);
            let _ = writeln!(out, "snapshot.journal_len: {}", snap.journal_len);
            let _ = writeln!(out, "snapshot.save_in_flight: {}", snap.save_in_flight);
            let _ = writeln!(out, "snapshot.pending_quit: {:?}", snap.pending_quit);
            let _ = writeln!(out, "snapshot.should_quit: {}", snap.should_quit);
            let _ = writeln!(out, "snapshot.status: {:?}", snap.status);
            let _ = writeln!(out, "snapshot.focus: {:?}", snap.focus);
            let _ = writeln!(out, "snapshot.modal_open: {}", snap.modal_open);
            let _ = writeln!(out, "snapshot.active: {:?}", snap.active);
            let _ = writeln!(out, "snapshot.title_text: {:?}", snap.title_text);
            let _ = writeln!(out, "snapshot.guard: {:?}", snap.guard);
            let _ = writeln!(
                out,
                "snapshot.quit_intent_pending: {:?}",
                snap.quit_intent_pending
            );
            let _ = writeln!(out, "snapshot.dirty_by_doc: {:?}", snap.dirty_by_doc);
            let _ = writeln!(
                out,
                "snapshot.save_in_flight_by_doc: {:?}",
                snap.save_in_flight_by_doc
            );
            let _ = writeln!(out);
            let _ = writeln!(out, "rendered frame:");
            out.push_str(&render_frame(&snap.cells));
        }
        None => {
            let _ = writeln!(out, "<no snapshot captured>");
        }
    }
    let _ = writeln!(out);

    match &result.final_ctx {
        Some(ctx) => {
            let _ = writeln!(out, "ctx.msg: {:?}", ctx.msg);
            let _ = writeln!(out, "ctx.raw: {}", render_raw(&ctx.raw));
            let _ = writeln!(out, "ctx.disk: {}", render_disk(ctx.disk.as_deref()));
            let _ = writeln!(out, "ctx.saves_delivered_ok: {}", ctx.saves_delivered_ok);
        }
        None => {
            let _ = writeln!(out, "<no step context captured>");
        }
    }
    out
}

/// One line per row, joining each `Cell.text` (a whole grapheme cluster,
/// not necessarily one codepoint) — the rendered frame at the violating
/// step.
fn render_frame(cells: &[Vec<Cell>]) -> String {
    let mut out = String::new();
    for row in cells {
        let line: String = row.iter().map(|c| c.text.as_str()).collect();
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// `None` means never saved (`Mem::read`'s `ErrorKind::NotFound`, G16) —
/// rendered as the literal `<never saved>`, never an empty byte string,
/// so a report can't be misread as "saved empty content".
fn render_disk(disk: Option<&[u8]>) -> String {
    disk.map_or_else(
        || "<never saved>".to_string(),
        |bytes| format!("{:?}", String::from_utf8_lossy(bytes)),
    )
}

fn render_raw(raw: &[Vec<u8>]) -> String {
    if raw.is_empty() {
        return "<none>".to_string();
    }
    raw.iter()
        .map(|chunk| format!("{:?}", String::from_utf8_lossy(chunk)))
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;
    use crate::driver;
    use crate::test_support::{ScratchDir, must};

    #[test]
    fn write_names_the_directory_deterministically_and_is_replayable() {
        let path = "/fuzz/doc.md";
        let content = "hello";
        let actions = vec![Action::Type("world".to_string())];
        let violation = Violation::new(
            "TEST-PROBE",
            "synthetic violation for report::write's own test".to_string(),
        );
        let result = driver::run(path, content, &actions);

        let guard = ScratchDir::new("report-test");
        let scratch = guard.path();

        let dir1 = must(
            write(scratch, &violation, path, content, &actions, &result),
            "write",
        );
        let expected_name = format!(
            "test-probe-{:08x}",
            fnv1a32(script::encode(path, content, &actions).as_bytes())
        );
        assert_eq!(
            dir1.file_name().and_then(|n| n.to_str()),
            Some(expected_name.as_str())
        );
        assert!(dir1.join("script.rune").is_file());
        assert!(dir1.join("report.txt").is_file());

        let script_text = must(
            fs::read_to_string(dir1.join("script.rune")),
            "read script.rune",
        );
        let (decoded_path, decoded_content, decoded_actions) =
            must(script::decode(&script_text), "decode");
        assert_eq!(decoded_path, path);
        assert_eq!(decoded_content, content);
        assert_eq!(decoded_actions, actions);
        assert_eq!(
            driver::run(&decoded_path, &decoded_content, &decoded_actions).final_content,
            result.final_content
        );

        // Re-running the same catch overwrites rather than accumulating.
        let dir2 = must(
            write(scratch, &violation, path, content, &actions, &result),
            "write again",
        );
        assert_eq!(dir1, dir2);
        let siblings: Vec<_> = must(fs::read_dir(scratch), "read_dir").collect();
        assert_eq!(
            siblings.len(),
            1,
            "expected exactly one bundle directory, found {siblings:?}"
        );
    }

    #[test]
    fn disk_none_renders_as_never_saved() {
        assert_eq!(render_disk(None), "<never saved>");
        assert_eq!(render_disk(Some(b"hi")), "\"hi\"");
    }
}
