//! Process identity and liveness. Darwin-only (`CLAUDE.md`: no portability
//! shims).
//!
//! A `sessions` row is this process's own identity for every journaled edit
//! and recorded observation it will ever produce: two
//! `Store` handles sharing one database (two rune windows on the same file)
//! tell their own history apart from each other instead of racing a single
//! shared journal. `proc_started_at` — the OS-reported start time of `pid`
//! — is what lets a LATER session tell "pid still running MY writer" apart
//! from "pid recycled to an unrelated process since".

use std::ffi::c_int;
use std::time::{Duration, SystemTime};

use rusqlite::Connection;

use crate::Error;

/// Extracts a `struct timeval` (`i64` seconds + `i32` microseconds, 4 bytes
/// padding, 12 bytes total) from the front of a `sysctl` result buffer.
/// Refuses a buffer too short to hold one, and a negative `sec` or `usec` —
/// a negative timeval is never a real timestamp, so it is treated the same
/// as any other malformed read: `None`, never a positive claim.
fn parse_timeval(buf: &[u8]) -> Option<(i64, i32)> {
    let sec = i64::from_ne_bytes(buf.get(0..8)?.try_into().ok()?);
    let usec = i32::from_ne_bytes(buf.get(8..12)?.try_into().ok()?);
    if sec < 0 || usec < 0 {
        return None;
    }
    Some((sec, usec))
}

/// Reads `pid`'s start time via `sysctl(CTL_KERN, KERN_PROC, KERN_PROC_PID)`
/// — the Darwin equivalent of Linux's `/proc/<pid>/stat` starttime field.
/// Returns `None` on any failure, including "no such
/// process": Darwin's `sysctl` does not cleanly distinguish that from other
/// I/O failures, so existence is decided separately and portably by
/// [`process_exists`] (`kill(pid, 0)`); this function is only ever consulted
/// once existence is already established, purely to detect pid reuse.
///
/// The kernel's `struct kinfo_proc` is `{ struct extern_proc kp_proc;
/// struct eproc kp_eproc; }` (`SizeofKinfoProc` = 0x288 on arm64/Darwin,
/// cross-checked against the kernel's own struct layout for that size),
/// and `p_starttime` (a `struct timeval`: `i64`
/// seconds + `i32` microseconds, 4 bytes padding) is `extern_proc`'s very
/// first field — offset 0 of the buffer `sysctl` fills. Reading just those
/// 12 bytes avoids having to replicate the rest of the struct's layout
/// (dozens of fields this crate never needs).
fn proc_started_at(pid: i32) -> Option<String> {
    let mut mib: [c_int; 4] = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PID, pid];
    let mut len: usize = 0;

    // First call with a null buffer: sysctl fills `len` with the required
    // buffer size for this pid's kinfo_proc.
    let probe = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            std::ptr::null_mut(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if probe != 0 || len < 12 {
        return None;
    }

    let mut buf = vec![0u8; len];
    let fetch = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    // A pid that has since exited between the probe and the fetch reports
    // back a short (often zero-length) buffer — treated the same as any
    // other failure to read start time: `None`, never a positive claim.
    if fetch != 0 || len < 12 {
        return None;
    }

    let (sec, usec) = parse_timeval(&buf)?;
    Some(format!("{sec}.{usec:06}"))
}

/// Reads the system boot time via `sysctl(CTL_KERN, KERN_BOOTTIME)` — the
/// same 12-byte `timeval` read as [`proc_started_at`], but for a fixed
/// `mib` whose buffer size is known up front (no probe/fetch dance). Returns
/// `None` on any failure; callers that compare against it must fail toward
/// treating the compared fact as unresolved, never as a positive claim.
pub(crate) fn boot_time() -> Option<SystemTime> {
    let mut mib: [c_int; 2] = [libc::CTL_KERN, libc::KERN_BOOTTIME];
    let mut buf = [0u8; std::mem::size_of::<libc::timeval>()];
    let mut len = buf.len();
    let ret = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if ret != 0 || len < 12 {
        return None;
    }
    let (sec, usec) = parse_timeval(&buf)?;
    Some(SystemTime::UNIX_EPOCH + Duration::new(sec as u64, usec as u32 * 1_000))
}

/// The result of [`process_exists`] — three genuinely distinct outcomes, not
/// two independent booleans (a `(bool, bool)` pair leaves the
/// "inconclusive, but which way does the existence bit lean" question only
/// implicit in the caller's own memory of the old encoding, `(false,
/// false)`). Every caller must fail toward [`Alive`](Existence::Alive) on
/// [`Inconclusive`](Existence::Inconclusive): wrongly refusing to inherit
/// a recoverable draft is tolerable; wrongly inheriting into and
/// corrupting a still-live session's journal is not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Existence {
    /// `kill(pid, 0)` succeeded, or failed with `EPERM` (exists, owned by
    /// another user — still a real process).
    Alive,
    /// Positively confirmed gone (`ESRCH`).
    Dead,
    /// The check itself failed for any other reason — no positive claim
    /// either way.
    Inconclusive,
}

/// Reports whether `pid` currently identifies a running process, via the
/// POSIX `kill(pid, 0)` idiom (sends no signal, only checks addressability)
/// — the one existence check that behaves identically and unambiguously on
/// every unix, unlike `proc_started_at`, which on
/// Darwin cannot cleanly distinguish "no such process" from an unrelated
/// read failure.
fn process_exists(pid: i32) -> Existence {
    let ret = unsafe { libc::kill(pid, 0) };
    if ret == 0 {
        return Existence::Alive; // exists, and we can signal it
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(code) if code == libc::ESRCH => Existence::Dead, // positively confirmed: no such process
        Some(code) if code == libc::EPERM => Existence::Alive, // exists, owned by another user — still a real process
        _ => Existence::Inconclusive,
    }
}

/// Reports whether `pid` is still running the SAME process that was
/// recorded with `started_at`. Fails toward "alive" on any ambiguity:
/// wrongly refusing to auto-inherit a recoverable draft is tolerable; wrongly
/// inheriting into and corrupting a still-live session's journal is not.
/// Only a POSITIVE confirmation of death returns `false`.
pub fn is_process_alive(pid: i64, started_at: &str) -> bool {
    if pid <= 0 {
        return false; // never a valid pid; a zero-value/corrupt row is never a live blocker
    }
    let pid = match i32::try_from(pid) {
        Ok(p) => p,
        Err(_) => return false, // out of pid_t range: not a real, currently-alive pid
    };
    match process_exists(pid) {
        Existence::Inconclusive => return true, // e.g. sandboxed — fail toward alive
        Existence::Dead => return false,        // positively confirmed: no such process
        Existence::Alive => {}
    }
    if started_at.is_empty() {
        return true; // this session never captured a start time to compare — fail toward alive
    }
    match proc_started_at(pid) {
        Some(current) => current == started_at,
        None => true, // can't positively confirm identity right now — fail toward alive
    }
}

/// Inserts a new `sessions` row for the CURRENT process and returns its id
/// — called exactly once per `Store` construction, giving that `Store` its
/// own private session identity. `now` is threaded in rather than reading
/// the system clock directly so an injected clock (tests) is honored even
/// at construction time; `opened_at` is informational bookkeeping only,
/// never consulted by any liveness/inheritance/reaper decision (those use
/// `proc_started_at`).
pub fn establish_session(conn: &Connection, now: SystemTime) -> Result<i64, Error> {
    let pid = std::process::id() as i64;
    let started_at = proc_started_at(pid as i32).ok_or_else(|| {
        Error::SessionEstablish(format!("could not read start time of own pid {pid}"))
    })?;
    let opened_at = format_rfc3339_nanos(now);

    conn.execute(
        "INSERT INTO sessions(pid, proc_started_at, opened_at) VALUES(?1, ?2, ?3)",
        rusqlite::params![pid, started_at, opened_at],
    )
    .map_err(|e| Error::SessionEstablish(e.to_string()))?;
    Ok(conn.last_insert_rowid())
}

/// Formats `t` as UTC RFC3339 with nanosecond precision (`opened_at`'s
/// on-disk shape), without pulling in a
/// date/time crate — civil-calendar conversion via Howard Hinnant's
/// `civil_from_days` algorithm (public domain, widely used; the same
/// algorithm chrono itself is built on).
pub(crate) fn format_rfc3339_nanos(t: SystemTime) -> String {
    let (secs, nanos) = match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => (d.as_secs() as i64, d.subsec_nanos()),
        Err(e) => {
            let d = e.duration();
            let secs = d.as_secs() as i64;
            let nanos = d.subsec_nanos();
            if nanos == 0 {
                (-secs, 0)
            } else {
                (-secs - 1, 1_000_000_000 - nanos)
            }
        }
    };

    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{nanos:09}Z")
}

/// Days-since-epoch (1970-01-01) to a proleptic Gregorian (year, month,
/// day). `z` may be negative (dates before the epoch).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// The inverse of [`civil_from_days`]: a proleptic Gregorian (year, month,
/// day) to days-since-epoch. Same Howard Hinnant algorithm, run backwards.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 }.div_euclid(400);
    let yoe = y - era * 400; // [0, 399]
    let mp = if m > 2 { m as i64 - 3 } else { m as i64 + 9 }; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// The number of days in Gregorian `(year, month)`, leap years included —
/// derived from [`days_from_civil`] itself (the gap between this month's
/// first day and next month's first day) rather than a duplicate leap-year
/// rule, so the two can never disagree.
fn days_in_month(year: i64, month: u32) -> u32 {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    (days_from_civil(next_year, next_month, 1) - days_from_civil(year, month, 1)) as u32
}

/// The inverse of [`format_rfc3339_nanos`], scoped to that function's own
/// fixed output shape (`YYYY-MM-DDTHH:MM:SS.NNNNNNNNNZ`) rather than general
/// RFC3339 — any deviation, or a pre-epoch instant, is refused as `None`
/// rather than guessed at, so a corrupt `opened_at` column always fails
/// toward the caller's own "can't confirm" handling.
pub(crate) fn parse_rfc3339_nanos(s: &str) -> Option<SystemTime> {
    let body = s.strip_suffix('Z')?;
    let (date, time) = body.split_once('T')?;

    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() || !(1..=12).contains(&month) {
        return None;
    }
    if !(1..=days_in_month(year, month)).contains(&day) {
        return None;
    }

    let (hms, nanos_str) = time.split_once('.')?;
    let mut hms_parts = hms.split(':');
    let hour: i64 = hms_parts.next()?.parse().ok()?;
    let minute: i64 = hms_parts.next()?.parse().ok()?;
    let second: i64 = hms_parts.next()?.parse().ok()?;
    if hms_parts.next().is_some() || hour >= 24 || minute >= 60 || second >= 60 {
        return None;
    }
    if nanos_str.len() != 9 {
        return None;
    }
    let nanos: u32 = nanos_str.parse().ok()?;

    let days = days_from_civil(year, month, day);
    let secs = days
        .checked_mul(86_400)?
        .checked_add(hour * 3600 + minute * 60 + second)?;
    if secs < 0 {
        return None;
    }
    Some(SystemTime::UNIX_EPOCH + Duration::new(secs as u64, nanos))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
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
    fn establish_session_inserts_exactly_one_row_and_returns_its_id() {
        let conn = Connection::open_in_memory().expect("open in-memory connection");
        crate::schema::apply(&conn).expect("apply schema");
        let id = establish_session(&conn, SystemTime::now()).expect("establish session");
        assert_eq!(id, 1);

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

    #[test]
    fn boot_time_reads_a_plausible_past_instant() {
        let bt = boot_time().expect("read boot time");
        assert!(bt < SystemTime::now(), "boot time must be in the past");
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
}
