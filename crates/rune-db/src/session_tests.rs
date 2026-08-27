#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use rusqlite::params;

use super::*;

#[test]
fn is_process_alive_confirms_own_pid_with_no_started_at() {
    let pid = std::process::id() as i64;
    assert!(is_process_alive(pid, ""));
}

#[test]
fn is_process_alive_rejects_a_nonexistent_pid() {
    // pid 0 is never a real, addressable process for kill(2)'s purposes
    // on Darwin (it's the kernel's own reserved value); our guard
    // rejects it outright before ever calling kill.
    assert!(!is_process_alive(0, "anything"));
    assert!(!is_process_alive(-1, "anything"));
}

#[test]
fn is_process_alive_confirms_own_pid_with_matching_started_at() {
    let pid = std::process::id() as i32;
    let started_at = proc_started_at(pid).expect("read own start time");
    assert!(is_process_alive(pid as i64, &started_at));
}

#[test]
fn is_process_alive_detects_pid_reuse_via_started_at_mismatch() {
    // A pid that DOES exist (ours) but with a bogus started_at must
    // still resolve to false — the whole point of the recorded start
    // time is to catch pid reuse.
    let pid = std::process::id() as i64;
    assert!(!is_process_alive(pid, "0.000000"));
}

#[test]
fn proc_started_at_returns_a_parseable_clock_tick_count() {
    let pid = std::process::id() as i32;
    let started_at = proc_started_at(pid).expect("read own start time");
    assert!(
        started_at.parse::<u64>().is_ok(),
        "must be a numeric clock-tick count, got {started_at:?}"
    );
}

#[test]
fn boot_time_reads_a_plausible_past_instant() {
    let bt = boot_time().expect("read boot time");
    assert!(bt < SystemTime::now(), "boot time must be in the past");
}

#[test]
fn boot_time_matches_proc_stat_btime_line() {
    let stat = std::fs::read_to_string("/proc/stat").expect("read /proc/stat");
    let secs: u64 = stat
        .lines()
        .find(|line| line.starts_with("btime "))
        .expect("btime line present")
        .split_whitespace()
        .nth(1)
        .expect("btime value present")
        .parse()
        .expect("btime value parses");
    let expected = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
    assert_eq!(boot_time(), Some(expected));
}

#[test]
fn process_exists_reports_alive_for_a_pid_we_cannot_signal() {
    // kill(2) with signal 0 against pid 1 (init/systemd, owned by root)
    // returns EPERM for any non-root caller: the process exists but we
    // may not signal it — the one reliable, environment-independent way
    // to reach the EPERM arm without a second real user account. Skipped
    // when this test itself runs as root, where pid 1 becomes signalable
    // and the EPERM arm is never reached.
    if unsafe { libc::geteuid() } == 0 {
        eprintln!(
            "process_exists_reports_alive_for_a_pid_we_cannot_signal: running as root, skipping"
        );
        return;
    }
    assert_eq!(process_exists(1), Existence::Alive);
}

#[test]
fn format_rfc3339_nanos_round_trips_a_known_instant() {
    // 2024-01-02T03:04:05.123456789Z
    let t = SystemTime::UNIX_EPOCH + std::time::Duration::new(1_704_164_645, 123_456_789);
    assert_eq!(format_rfc3339_nanos(t), "2024-01-02T03:04:05.123456789Z");
}

#[test]
fn format_rfc3339_nanos_handles_the_epoch() {
    assert_eq!(
        format_rfc3339_nanos(SystemTime::UNIX_EPOCH),
        "1970-01-01T00:00:00.000000000Z"
    );
}

#[test]
fn format_rfc3339_nanos_handles_a_pre_epoch_instant_on_the_second_boundary() {
    let t = SystemTime::UNIX_EPOCH - Duration::from_secs(100);
    assert_eq!(format_rfc3339_nanos(t), "1969-12-31T23:58:20.000000000Z");
}

#[test]
fn format_rfc3339_nanos_handles_a_pre_epoch_instant_with_a_fractional_second() {
    let t = SystemTime::UNIX_EPOCH - Duration::new(100, 500_000_000);
    assert_eq!(format_rfc3339_nanos(t), "1969-12-31T23:58:19.500000000Z");
}

#[test]
fn establish_session_inserts_exactly_one_row_and_returns_its_id() {
    let conn = crate::conn::open_recovery_store(crate::conn::RecoveryTarget::Memory(
        &crate::conn::memory_uri(),
    ))
    .expect("open");
    let id = establish_session(&conn, SystemTime::now()).expect("establish session");
    assert_eq!(id, SessionId(1));

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
        .expect("count sessions");
    assert_eq!(count, 1);

    let started_at: String = conn
        .query_row(
            "SELECT proc_started_at FROM sessions WHERE id=?1",
            params![id],
            |r| r.get(0),
        )
        .expect("read proc_started_at");
    assert!(
        !started_at.is_empty(),
        "a live process must always record a real start time, never the empty liveness hole"
    );
}

/// Known days-since-epoch/calendar-date pairs, independently computed via
/// `date -u -d '<date>' +%s` (GNU coreutils) rather than by this module's
/// own algorithm — spans both 400-year leap-eligible era boundaries
/// (1600, 2000, 2400) and merely-century (non-leap) boundaries (1900,
/// 2100), plus every month at the mp=0/9/10/11 branch edges.
const KNOWN_CIVIL_DAYS: &[(i64, i64, u32, u32)] = &[
    (0, 1970, 1, 1),
    (-1, 1969, 12, 31),
    (10_957, 2000, 1, 1),
    (11_016, 2000, 2, 29),
    (11_017, 2000, 3, 1),
    (-25_508, 1900, 3, 1),
    (-25_568, 1899, 12, 31),
    (47_541, 2100, 3, 1),
    (-135_081, 1600, 2, 29),
    (-135_080, 1600, 3, 1),
    (157_113, 2400, 2, 29),
    (19_722, 2023, 12, 31),
    (19_782, 2024, 2, 29),
    (19_783, 2024, 3, 1),
];

#[test]
fn civil_from_days_matches_known_calendar_dates() {
    for &(days, year, month, day) in KNOWN_CIVIL_DAYS {
        assert_eq!(civil_from_days(days), (year, month, day), "days={days}");
    }
}

#[test]
fn days_from_civil_matches_known_calendar_dates() {
    for &(days, year, month, day) in KNOWN_CIVIL_DAYS {
        assert_eq!(
            days_from_civil(year, month, day),
            days,
            "date={year:04}-{month:02}-{day:02}"
        );
    }
}

#[test]
fn days_from_civil_matches_known_pre_common_era_dates() {
    // Year 0 (astronomical numbering, i.e. 1 BC) with month <= 2: the
    // internal `y - 1` shift lands on a negative year, exercising
    // `days_from_civil`'s own `y < 0` era branch — a value none of the
    // AD dates above ever reach. Independently computed the same way
    // as `KNOWN_CIVIL_DAYS`: `date -u -d '0000-01-01' +%s`.
    assert_eq!(days_from_civil(0, 1, 1), -719_528);
    assert_eq!(days_from_civil(0, 2, 29), -719_469);
}

#[test]
fn days_in_month_handles_the_december_year_rollover() {
    assert_eq!(days_in_month(2023, 12), 31);
}

#[test]
fn parse_rfc3339_nanos_round_trips_format_rfc3339_nanos() {
    let t = SystemTime::UNIX_EPOCH + Duration::new(1_704_164_645, 123_456_789);
    let s = format_rfc3339_nanos(t);
    assert_eq!(parse_rfc3339_nanos(&s), Some(t));
}

#[test]
fn parse_rfc3339_nanos_rejects_garbage() {
    assert_eq!(parse_rfc3339_nanos(""), None);
    assert_eq!(parse_rfc3339_nanos("not a timestamp"), None);
    assert_eq!(parse_rfc3339_nanos("2024-13-40T99:99:99.000000000Z"), None);
}

#[test]
fn parse_rfc3339_nanos_rejects_impossible_calendar_days() {
    assert_eq!(parse_rfc3339_nanos("2024-02-30T00:00:00.000000000Z"), None);
    assert_eq!(parse_rfc3339_nanos("2025-04-31T00:00:00.000000000Z"), None);
    assert_eq!(
        parse_rfc3339_nanos("2025-02-29T00:00:00.000000000Z"),
        None,
        "2025 is not a leap year"
    );
}

#[test]
fn parse_rfc3339_nanos_accepts_a_leap_day() {
    assert!(parse_rfc3339_nanos("2024-02-29T00:00:00.000000000Z").is_some());
}

#[test]
fn parse_rfc3339_nanos_rejects_trailing_date_component() {
    assert_eq!(
        parse_rfc3339_nanos("2024-01-15-05T00:00:00.000000000Z"),
        None
    );
}

#[test]
fn parse_rfc3339_nanos_rejects_a_trailing_time_component() {
    assert_eq!(
        parse_rfc3339_nanos("2024-01-01T00:00:00:00.000000000Z"),
        None
    );
}

#[test]
fn parse_rfc3339_nanos_rejects_an_out_of_range_hour() {
    assert_eq!(parse_rfc3339_nanos("2024-01-01T25:00:00.000000000Z"), None);
}

#[test]
fn parse_rfc3339_nanos_rejects_an_out_of_range_minute() {
    assert_eq!(parse_rfc3339_nanos("2024-01-01T00:60:00.000000000Z"), None);
}

#[test]
fn parse_rfc3339_nanos_accepts_the_epoch_exactly() {
    assert_eq!(
        parse_rfc3339_nanos("1970-01-01T00:00:00.000000000Z"),
        Some(SystemTime::UNIX_EPOCH)
    );
}

#[test]
fn parse_rfc3339_nanos_rejects_a_pre_epoch_date() {
    assert_eq!(parse_rfc3339_nanos("1969-12-31T00:00:00.000000000Z"), None);
}
