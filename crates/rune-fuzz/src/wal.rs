//! A write-ahead log for the one case a `TestRunner::run` case closure
//! can never survive to report on its own: a process-level signal (a
//! linked tree-sitter grammar's C-level assert, SIGABRT) killing the
//! process mid-case. `catch_unwind` cannot trap a signal, and both of
//! this crate's own persistence points — proptest's regression file and
//! `report::write` — run only after `TestRunner::run` returns, so a
//! signal death leaves nothing behind unless the about-to-run case was
//! already durable before it started.
//!
//! `arm` writes that case to disk before the driver touches it and
//! returns a guard; the guard's `Drop` removes the file again. Drop runs
//! on every ordinary return AND on a Rust unwind (proptest itself catches
//! each case's panic, so the guard still drops on the way out) — only a
//! signal, which skips unwinding entirely, can leave the file behind.
//! `sweep`, called once before the next run starts, treats a leftover
//! file as proof the previous process died that way and promotes it to
//! an ordinary replayable artifact.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::action::Action;
use crate::hash::fnv1a32;
use crate::script;

/// The write-ahead file's on-disk name, under whatever `dir_root` the
/// caller passes to `arm`/`sweep` — this is what `sweep` looks for.
pub const INFLIGHT_NAME: &str = "inflight.rune";

/// Held for the lifetime of one fuzz case. Its `Drop` removes the
/// write-ahead file; only a process-level death skips that and leaves it
/// behind for the next run's `sweep` to find.
pub struct WalGuard {
    path: PathBuf,
}

impl Drop for WalGuard {
    fn drop(&mut self) {
        // Best-effort: the file may legitimately already be gone, and
        // Drop must never panic.
        let _ = fs::remove_file(&self.path);
    }
}

/// Writes the about-to-run case to `dir_root/inflight.rune` and returns a
/// guard that removes it again once the case has settled.
pub fn arm(dir_root: &Path, path: &str, content: &str, actions: &[Action]) -> io::Result<WalGuard> {
    fs::create_dir_all(dir_root)?;
    let inflight = dir_root.join(INFLIGHT_NAME);
    fs::write(&inflight, script::encode(path, content, actions))?;
    Ok(WalGuard { path: inflight })
}

/// Looks for a leftover `inflight.rune` under `dir_root` from a process
/// that died before it could clean up after itself. If found, promotes it
/// to a `proc-abort-<hash>` artifact directory (matching the
/// `<id>-<hash>` naming `report::write` uses) with a `report.txt`
/// explaining what happened, removes the write-ahead file, and returns
/// the promoted directory. Returns `Ok(None)` when there is nothing to
/// promote.
pub fn sweep(dir_root: &Path) -> io::Result<Option<PathBuf>> {
    let inflight = dir_root.join(INFLIGHT_NAME);
    let encoded = match fs::read_to_string(&inflight) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };

    let hash = fnv1a32(encoded.as_bytes());
    let dir = dir_root.join(format!("proc-abort-{hash:08x}"));
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("script.rune"), &encoded)?;
    fs::write(dir.join("report.txt"), render_report())?;
    fs::remove_file(&inflight)?;
    Ok(Some(dir))
}

fn render_report() -> String {
    "the previous fuzz process died to a process-level signal in the \
     middle of a case (for example a C-level assert in a linked \
     tree-sitter grammar) — invisible to catch_unwind, so it never \
     reached this crate's own persistence points.\n\n\
     this script was written to the write-ahead log before the case ran, \
     so it survived the process death. it decodes with the same script \
     codec as any other artifact and replays the same way.\n"
        .to_string()
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
    use crate::action::Action;
    use crate::test_support::{ScratchDir, must};

    #[test]
    fn arm_writes_the_encoded_script_and_it_decodes_back() {
        let scratch = ScratchDir::new("wal-test-arm-writes");
        let dir = scratch.path();
        let path = "/fuzz/doc.md";
        let content = "hello";
        let actions = vec![Action::Type("world".to_string())];

        let guard = must(arm(dir, path, content, &actions), "arm");
        let on_disk = must(fs::read_to_string(&guard.path), "read inflight");
        assert_eq!(on_disk, script::encode(path, content, &actions));

        let (decoded_path, decoded_content, decoded_actions) =
            must(script::decode(&on_disk), "decode");
        assert_eq!(decoded_path, path);
        assert_eq!(decoded_content, content);
        assert_eq!(decoded_actions, actions);

        drop(guard);
    }

    #[test]
    fn dropping_the_guard_removes_the_file_and_sweep_then_finds_nothing() {
        let scratch = ScratchDir::new("wal-test-drop-removes");
        let dir = scratch.path();
        let guard = must(arm(dir, "/fuzz/doc.md", "hi", &[]), "arm");
        let inflight_path = dir.join(INFLIGHT_NAME);
        assert!(inflight_path.is_file());

        drop(guard);
        assert!(!inflight_path.exists());
        assert_eq!(must(sweep(dir), "sweep"), None);
    }

    #[test]
    fn sweep_promotes_a_leftover_inflight_file() {
        let scratch = ScratchDir::new("wal-test-sweep-promotes");
        let dir = scratch.path();
        must(fs::create_dir_all(dir), "create_dir_all");
        let path = "/fuzz/doc.md";
        let content = "leftover";
        let actions = vec![Action::Type("x".to_string())];
        let encoded = script::encode(path, content, &actions);
        must(
            fs::write(dir.join(INFLIGHT_NAME), &encoded),
            "write inflight",
        );

        let promoted = must(sweep(dir), "sweep").expect("expected a promoted dir");
        let expected_hash = fnv1a32(encoded.as_bytes());
        assert_eq!(
            promoted.file_name().and_then(|n| n.to_str()),
            Some(format!("proc-abort-{expected_hash:08x}").as_str())
        );

        assert!(!dir.join(INFLIGHT_NAME).exists());
        let promoted_script = must(fs::read_to_string(promoted.join("script.rune")), "read");
        assert_eq!(promoted_script, encoded);
        let report_text = must(
            fs::read_to_string(promoted.join("report.txt")),
            "read report",
        );
        assert!(!report_text.is_empty());
    }

    #[test]
    fn sweep_on_a_directory_with_no_inflight_file_returns_none() {
        let scratch = ScratchDir::new("wal-test-sweep-empty");
        let dir = scratch.path();
        must(fs::create_dir_all(dir), "create_dir_all");
        assert_eq!(must(sweep(dir), "sweep"), None);
    }

    #[test]
    fn sweep_on_a_nonexistent_dir_root_returns_none() {
        let scratch = ScratchDir::new("wal-test-sweep-nonexistent");
        let dir = scratch.path();
        assert!(!dir.exists());
        assert_eq!(must(sweep(dir), "sweep"), None);
    }
}
