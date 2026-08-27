//! `predates_boot`/reboot-death boundary tests for `reaper.rs` — split out
//! of `reaper_tests.rs` to keep it under the file-size ceiling, the same
//! shape `load_reopen_tests.rs` already uses alongside `load_tests.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;

/// `predates_boot` pins its own boundary: a session opened exactly AT boot
/// is not "before" it, and a session opened just after boot is likewise not
/// a reboot-death candidate — both stay governed by ordinary `is_alive`
/// liveness, which here reports alive, so both must be spared.
#[test]
fn predates_boot_is_strict_not_inclusive_of_the_boot_instant() {
    let boot = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(24 * 3600);

    assert!(
        !predates_boot(&crate::session::format_rfc3339_nanos(boot), Some(boot)),
        "a session opened exactly at boot does not predate it"
    );
    assert!(
        !predates_boot(
            &crate::session::format_rfc3339_nanos(boot + std::time::Duration::from_secs(1)),
            Some(boot)
        ),
        "a session opened after boot does not predate it"
    );
    assert!(
        predates_boot(
            &crate::session::format_rfc3339_nanos(boot - std::time::Duration::from_secs(1)),
            Some(boot)
        ),
        "a session opened before boot does predate it"
    );
}

/// `predates_boot` degrades to `false` (never reboot-death) whenever either
/// input is unusable: an unparseable `opened_at`, or no injected `boot` at
/// all — never `true` by default.
#[test]
fn predates_boot_defaults_to_false_when_either_input_is_unusable() {
    let boot = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(24 * 3600);
    assert!(!predates_boot("not a timestamp", Some(boot)));
    assert!(!predates_boot(
        &crate::session::format_rfc3339_nanos(SystemTime::UNIX_EPOCH),
        None
    ));
}

/// The reboot-death OR is not an AND: a legacy `proc_started_at=''` row
/// whose `opened_at` does NOT predate boot must stay governed by ordinary
/// `is_alive` liveness alone, not be treated as reboot-dead just because
/// `started_at.is_empty()` alone is true.
#[test]
fn empty_started_at_alone_without_predating_boot_is_not_reboot_death() {
    let mut conn = open();
    let own_pid = std::process::id() as i64;
    let boot = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(24 * 3600);
    let after_boot = crate::session::format_rfc3339_nanos(
        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(2 * 24 * 3600),
    );
    let session_old = seed_session_at(&conn, own_pid, "", &after_boot);
    let doc_id = seed_doc(&conn);
    journal_one_edit(&mut conn, session_old, doc_id);
    materialize_footprint(&mut conn, session_old, doc_id);

    // is_alive reports this pid alive: with a correct `&&`, `dead_since_reboot`
    // is false (empty started_at, but opened after boot), so `!dead_since_reboot
    // && is_alive(...)` spares it before the reapable/footprint check is ever
    // reached. A loosened `||` would instead treat the bare-empty
    // `started_at` alone as reboot-dead, skip the `is_alive` short-circuit,
    // and reap this fully-materialized footprint regardless.
    reap_dead_sessions(&mut conn, &|_pid, _started_at| true, Some(boot)).expect("reap");

    let old_events: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE session_id=?1",
            params![session_old],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(
        old_events, 1,
        "an empty started_at that does not predate boot must not alone trigger reboot-death"
    );
}
