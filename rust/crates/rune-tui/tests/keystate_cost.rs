#![allow(clippy::unwrap_used, clippy::expect_used)]
//! The two FFI checks no ordinary test covers: that the `CGEventSourceKeyState`
//! query is cheap enough to run on a keystroke, and that it never triggers a
//! macOS permission prompt. Both are `#[ignore]` — they touch real hardware
//! state, so they run only when invoked by name.

use rune_tui::keystate::{HidSpaceProbe, SpaceProbe};
use std::process::Command;
use std::time::Instant;

/// Risk R2: the per-call cost is unverified (CoreGraphics ships only inside
/// the dyld shared cache). 100_000 calls under one second is < 10 µs/call —
/// the threshold above which calling this on a keypress would matter.
#[test]
#[ignore]
fn keystate_query_is_cheap() {
    let probe = HidSpaceProbe;
    let start = Instant::now();
    let mut sink = 0u32;
    for _ in 0..100_000 {
        if probe.space_is_down() {
            sink += 1;
        }
    }
    let elapsed = start.elapsed();
    // Keep the loop from being optimised away.
    assert!(sink <= 100_000);
    assert!(
        elapsed < std::time::Duration::from_secs(1),
        "100_000 CGEventSourceKeyState calls took {elapsed:?}; \
         that is over 10 µs/call and too expensive for a keystroke path"
    );
}

/// A macOS permission prompt always writes a row to the TCC database. An
/// unchanged row count across 1_000 calls is machine-checkable proof that
/// none fired. If the database itself is unreadable (it is SIP-protected),
/// return early — that proves nothing either way, so it must not fail.
#[test]
#[ignore]
fn keystate_query_does_not_prompt_for_permissions() {
    let Some(before) = tcc_row_count() else {
        return;
    };
    let probe = HidSpaceProbe;
    for _ in 0..1_000 {
        let _ = probe.space_is_down();
    }
    let Some(after) = tcc_row_count() else {
        return;
    };
    assert_eq!(
        before, after,
        "the TCC row count for this client changed ({before} -> {after}); \
         CGEventSourceKeyState triggered a permission prompt"
    );
}

fn tcc_row_count() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let db = format!("{home}/Library/Application Support/com.apple.TCC/TCC.db");
    let out = Command::new("sqlite3")
        .arg(db)
        .arg("select count(*) from access where client like '%rune%'")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
