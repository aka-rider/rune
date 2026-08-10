//! Tests for `bootstrap`/`launch` — split out to keep the parent under the
//! file-size ceiling, the same shape `decode_cmd_tests.rs` (rune-tui)
//! already uses elsewhere in the workspace.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use rune_vfs::Mem;
use std::sync::atomic::{AtomicU32, Ordering};

/// Counts every [`Vfs::read`] call made against ANY path, wrapping a real
/// [`Mem`] for everything else — the TOCTOU pin for issue #77: a launch's
/// first positional must be read off disk exactly once, never once for the
/// buffer and again for the recovery store's own CAS baseline.
struct CountingReadVfs {
    inner: Mem,
    reads: AtomicU32,
}

impl CountingReadVfs {
    fn new(inner: Mem) -> CountingReadVfs {
        CountingReadVfs {
            inner,
            reads: AtomicU32::new(0),
        }
    }
}

impl Vfs for CountingReadVfs {
    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.read(path)
    }
    fn write_durable(&self, path: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
        self.inner.write_durable(path, bytes)
    }
    fn exchange(&self, a: &Path, b: &Path) -> std::io::Result<()> {
        self.inner.exchange(a, b)
    }
    fn rename_excl(&self, old: &Path, new: &Path) -> std::io::Result<()> {
        self.inner.rename_excl(old, new)
    }
    fn remove(&self, path: &Path) -> std::io::Result<()> {
        self.inner.remove(path)
    }
    fn trash(&self, path: &Path) -> std::io::Result<()> {
        self.inner.trash(path)
    }
    fn stat(&self, path: &Path) -> std::io::Result<rune_vfs::Stat> {
        self.inner.stat(path)
    }
    fn resolve(&self, path: &Path) -> std::io::Result<PathBuf> {
        self.inner.resolve(path)
    }
    fn mkdir_all(&self, path: &Path) -> std::io::Result<()> {
        self.inner.mkdir_all(path)
    }
    fn read_dir(&self, path: &Path) -> std::io::Result<Vec<rune_vfs::DirEntry>> {
        self.inner.read_dir(path)
    }
}

/// A real, throwaway `$HOME` under the OS temp dir — `Store::open`
/// talks to the sqlite file directly via `rusqlite`, bypassing the
/// injected `vfs` entirely (same pattern `rune-db`'s own multiprocess
/// tests use), so the recovery-store half of these launch tests needs
/// a real directory even though every document byte lives in `Mem`.
struct ScratchHome(PathBuf);

impl ScratchHome {
    fn new(label: &str) -> ScratchHome {
        let dir = env::temp_dir().join(format!(
            "rune-cli-launch-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).expect("create scratch home");
        ScratchHome(dir)
    }
}

impl Drop for ScratchHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn launch_multi_file_enqueues_a_load_for_every_extra_tab() {
    let vfs = Mem::new();
    vfs.save_atomic(Path::new("/vault/a.md"), b"a")
        .expect("seed a.md");
    vfs.save_atomic(Path::new("/vault/b.md"), b"b")
        .expect("seed b.md");
    vfs.save_atomic(Path::new("/vault/c.md"), b"c")
        .expect("seed c.md");
    let home = ScratchHome::new("multi-file");

    let app = bootstrap(
        Arc::new(vfs),
        vec![
            OsString::from("/vault/a.md"),
            OsString::from("/vault/b.md"),
            OsString::from("/vault/c.md"),
        ]
        .into_iter(),
        PathBuf::from("/"),
        Some(home.0.clone()),
    )
    .expect("bootstrap should succeed");

    // The first file hydrates synchronously inside `bootstrap_db` and
    // is bound before `App::new` ever runs.
    assert_eq!(app.documents.len(), 3);
    assert!(app.doc(app.active).is_some_and(|d| d.db.is_some()));
    // The other two open through `workspace::open_path`'s async path
    // (plan [rune-cli 1]/WP3.S1): each one's `Load` must actually be
    // enqueued and tracked, not silently dropped the way a `Sink::
    // Bootstrap`-less bridge used to swallow it — `db_ops` is where
    // `db::load_document` records that at enqueue time, synchronously.
    assert_eq!(
        app.db_ops.len(),
        2,
        "every extra tab's Load must be tracked"
    );
}

#[test]
fn launch_same_file_two_spellings_opens_one_document() {
    let vfs = Mem::new();
    vfs.save_atomic(Path::new("/vault/notes.md"), b"hi")
        .expect("seed notes.md");
    let home = ScratchHome::new("dedup");

    let app = bootstrap(
        Arc::new(vfs),
        vec![
            OsString::from("/vault/notes.md"),
            OsString::from("/vault/sub/../notes.md"),
        ]
        .into_iter(),
        PathBuf::from("/"),
        Some(home.0.clone()),
    )
    .expect("bootstrap should succeed");

    assert_eq!(
        app.documents.len(),
        1,
        "two spellings of the same file must resolve to one document"
    );
}

/// Plan WP4.S8: a `.png` first positional bootstraps through the SAME
/// `workspace::open_path` dispatch every extra positional uses (built
/// via the untitled `App` constructor as an anchor), rather than
/// `load_buffer`'s text-only path — which would reject the PNG's bytes
/// outright as invalid UTF-8, exactly the failure this restructuring
/// exists to route around. Exactly one document is left open (the
/// blank untitled anchor is closed once the image opens), it is the
/// active one, and it is read-only.
#[test]
fn launch_first_positional_png_bootstraps_as_a_read_only_image_document() {
    let vfs = Mem::new();
    vfs.save_atomic(
        Path::new("/vault/x.png"),
        &[0x89, b'P', b'N', b'G', 0, 0, 0, 0],
    )
    .expect("seed a (fake) png");

    let app = bootstrap(
        Arc::new(vfs),
        vec![OsString::from("/vault/x.png")].into_iter(),
        PathBuf::from("/"),
        None,
    )
    .expect("bootstrap should succeed for an image first positional");

    assert_eq!(
        app.documents.len(),
        1,
        "the blank untitled anchor must be closed once the image opens"
    );
    assert!(app.doc(app.active).is_some_and(|d| d.is_read_only()));
    assert!(
        app.doc(app.active)
            .is_some_and(|d| d.file_path.as_deref() == Some(Path::new("/vault/x.png")))
    );
}

/// A missing-path launch is a recovery-backed draft that already knows its
/// name, not a launch with zero crash protection: no banner, a live
/// app-wide `Db`, and the active document bound to a fresh scratch row with
/// `bind_new == true` — the same shape a no-positional launch's default
/// document gets.
#[test]
fn launch_nonexistent_path_is_recovery_backed() {
    let vfs = Mem::new();
    let home = ScratchHome::new("missing-path");

    let app = bootstrap(
        Arc::new(vfs),
        vec![OsString::from("/vault/missing.md")].into_iter(),
        PathBuf::from("/"),
        Some(home.0.clone()),
    )
    .expect("bootstrap should succeed for a missing-path launch");

    assert!(
        app.db_banner.is_none(),
        "a missing-path launch is now recovery-backed, not degraded"
    );
    assert!(app.db.is_some(), "a live app-wide store must be bound");
    assert!(
        app.doc(app.active)
            .and_then(|d| d.db.as_ref())
            .is_some_and(|db| db.bind_new),
        "the active document must be bound to a scratch row awaiting its first publish"
    );
}

/// The property that separates a missing-path launch from an untitled
/// draft: `file_path` stays `Some(the path)`, not `None`. Multi-positional
/// on purpose (a real, existing second file) so this also pins that the
/// `DocDb` lands on the FIRST positional's document — the one bootstrap
/// actually binds a scratch row to — and not on the extra tab that opens
/// through the ordinary async `Load` path.
#[test]
fn launch_missing_first_positional_pins_file_path_and_only_the_first_docs_db() {
    let vfs = Mem::new();
    vfs.save_atomic(Path::new("/vault/other.md"), b"other")
        .expect("seed other.md");
    let home = ScratchHome::new("missing-multi");

    let app = bootstrap(
        Arc::new(vfs),
        vec![
            OsString::from("/vault/missing.md"),
            OsString::from("/vault/other.md"),
        ]
        .into_iter(),
        PathBuf::from("/"),
        Some(home.0.clone()),
    )
    .expect("bootstrap should succeed");

    assert_eq!(app.documents.len(), 2);
    let active = app.doc(app.active).expect("active doc exists");
    assert_eq!(
        active.file_path.as_deref(),
        Some(Path::new("/vault/missing.md")),
        "the recovery-backed missing-path document must keep its intended name, \
         not fall back to an untitled draft"
    );
    assert!(
        active.db.as_ref().is_some_and(|db| db.bind_new),
        "the first positional's document must bind the fresh scratch row"
    );

    let other = app
        .documents
        .values()
        .find(|d| d.file_path.as_deref() == Some(Path::new("/vault/other.md")))
        .expect("the second positional opened its own tab");
    assert!(
        other.db.is_none(),
        "the DocDb from bootstrap_new_file must land on the first positional's \
         document, never on an extra tab"
    );
}

/// The plan's deliberate rejection of a `gc_empty_scratch` sweep inside
/// `bootstrap_new_file`: a second concurrent launch sharing this `$HOME`
/// must never sweep away another (still-running) session's freshly minted,
/// not-yet-journaled scratch row. Seeds exactly that row directly, without
/// going through `bootstrap` at all, then confirms a missing-path launch
/// leaves it standing.
#[test]
fn launch_missing_first_positional_never_sweeps_another_sessions_empty_scratch_row() {
    let home = ScratchHome::new("missing-path-no-gc");
    let db_path = home
        .0
        .join("Library")
        .join("Application Support")
        .join("rune")
        .join(rune_db::db_file_name(rune_db::SCHEMA_VERSION));

    let other_sessions_row_id = {
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
        let bridge = rune_tui::db::DbBridge::bootstrap();
        let (store, _warning) = rune_db::Store::open(&db_path, Arc::clone(&vfs), bridge.on_event())
            .expect("open store");

        let create_op = store.create_scratch().expect("enqueue create_scratch");
        let db_id = match bridge.wait_for_bootstrap_event(|evt| match evt {
            rune_db::DbEvent::Ok { id, .. } | rune_db::DbEvent::Err { id, .. } => *id == create_op,
            rune_db::DbEvent::Fatal { .. } => true,
        }) {
            rune_db::DbEvent::Ok {
                result: rune_db::OpOutcome::RowId(id),
                ..
            } => id,
            other => panic!("expected a CreateScratch ack, got {other:?}"),
        };

        store.shutdown();
        db_id
    };

    let vfs = Mem::new();
    let _app = bootstrap(
        Arc::new(vfs),
        vec![OsString::from("/vault/missing.md")].into_iter(),
        PathBuf::from("/"),
        Some(home.0.clone()),
    )
    .expect("bootstrap should succeed for a missing-path launch");

    let raw = rusqlite::Connection::open(&db_path).expect("open db file directly");
    let still_present: bool = raw
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM documents WHERE id=?1)",
            [other_sessions_row_id],
            |r| r.get(0),
        )
        .expect("check the other session's row");
    assert!(
        still_present,
        "bootstrap_new_file must never sweep another session's empty scratch row"
    );
}

/// Removing `launch_nonexistent_path_sets_a_banner` must not delete the
/// honest degraded signal for the case that actually has no store to bind
/// to: `home: None` short-circuits `open_store` to the `$HOME`-unset arm
/// before any scratch row is ever minted.
#[test]
fn launch_nonexistent_path_without_home_still_banners() {
    let vfs = Mem::new();

    let app = bootstrap(
        Arc::new(vfs),
        vec![OsString::from("/vault/missing.md")].into_iter(),
        PathBuf::from("/"),
        None,
    )
    .expect("bootstrap should succeed even with no recovery store");

    assert!(
        app.db_banner.is_some(),
        "a missing-path launch with no usable $HOME must still say so"
    );
}

/// Issue #80: a first positional whose resolution fails must never fall
/// back to the caller's unnormalized spelling — `bootstrap` refuses and
/// exits `EX_IOERR`, the same code `open::open_first_positional`'s own
/// unreadable-file arm already returns, rather than launching under a
/// path whose on-disk identity was never actually confirmed.
#[test]
fn launch_resolve_failing_first_positional_exits_with_the_io_error_code() {
    let mem = Arc::new(Mem::new());
    mem.fail_resolve(Path::new("/vault/unresolvable.md"));
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;

    let result = bootstrap(
        vfs,
        vec![OsString::from("/vault/unresolvable.md")].into_iter(),
        PathBuf::from("/"),
        None,
    );

    match result {
        Err(code) => assert_eq!(
            format!("{code:?}"),
            format!("{:?}", ExitCode::from(exit_code::IO_ERR)),
            "a resolve failure must exit with the same code load failures use"
        ),
        Ok(_) => panic!("a resolve-failing first positional must not bootstrap"),
    }
}

#[test]
fn launch_empty_positional_is_rejected_before_any_open() {
    let vfs = Mem::new();

    let result = bootstrap(
        Arc::new(vfs),
        vec![OsString::from("")].into_iter(),
        PathBuf::from("/"),
        None,
    );
    assert!(
        result.is_err(),
        "an empty positional must be rejected at parse"
    );
}

/// Plan WP3 ("the untitled draft is really recovery-backed"): a
/// no-positional launch against a real (temp) `$HOME` must come up with
/// BOTH a live app-wide `Db` and a bound `DocDb` on the default
/// document — the two facts that together arm the guard's "recovery-
/// backed" exemption. Before this change, this launch mode always had
/// `db: None` (see the now-resolved `crates/rune-tui/TODO.md` entry).
#[test]
fn no_positional_launch_binds_both_the_app_db_and_a_doc_db() {
    let vfs = Mem::new();
    let home = ScratchHome::new("untitled-doc-db");

    let app = bootstrap(
        Arc::new(vfs),
        std::iter::empty(),
        PathBuf::from("/"),
        Some(home.0.clone()),
    )
    .expect("bootstrap should succeed with no positional files");

    assert!(
        app.db.is_some(),
        "the default untitled launch must have a live app-wide store"
    );
    assert!(
        app.doc(app.active).is_some_and(|d| d.db.is_some()),
        "the default document must be bound to its own scratch row"
    );
}

/// The scratch row a no-positional launch binds to must actually
/// survive: typing into the default document, then relaunching against
/// the SAME `$HOME` with no positional files again, must come back with
/// that text already in the buffer (crash recovery for the untitled
/// draft, not just a live journal nobody ever reads back).
#[test]
fn a_dead_sessions_untitled_draft_is_recovered_on_the_next_launch() {
    let home = ScratchHome::new("untitled-recover");

    let db_path = home
        .0
        .join("Library")
        .join("Application Support")
        .join("rune")
        .join(rune_db::db_file_name(rune_db::SCHEMA_VERSION));

    // First "session": open the store directly (bypassing the whole app)
    // and journal an edit under it, exactly as typing into the default
    // untitled document would — then drop it without ever naming the
    // document, simulating a crash/quit that left the draft unsaved.
    {
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
        let bridge = rune_tui::db::DbBridge::bootstrap();
        let (store, _warning) = rune_db::Store::open(&db_path, Arc::clone(&vfs), bridge.on_event())
            .expect("open store");

        let create_op = store.create_scratch().expect("enqueue create_scratch");
        let doc_id = match bridge.wait_for_bootstrap_event(|evt| match evt {
            rune_db::DbEvent::Ok { id, .. } | rune_db::DbEvent::Err { id, .. } => *id == create_op,
            rune_db::DbEvent::Fatal { .. } => true,
        }) {
            rune_db::DbEvent::Ok {
                result: rune_db::OpOutcome::RowId(id),
                ..
            } => id,
            other => panic!("expected a CreateScratch ack, got {other:?}"),
        };

        // A bare recovery-anchor snapshot is enough on its own: it
        // carries this session's own `session_id`, which is all
        // `most_recent_session_for_doc`/`recover_document` need to find
        // and replay it — no separate journaled edit required.
        let snapshot_op = store
            .create_snapshot(doc_id, "unsaved draft from a dead session")
            .expect("enqueue create_snapshot");
        bridge.wait_for_bootstrap_event(|evt| match evt {
            rune_db::DbEvent::Ok { id, .. } | rune_db::DbEvent::Err { id, .. } => {
                *id == snapshot_op
            }
            rune_db::DbEvent::Fatal { .. } => true,
        });

        store.shutdown();
    }

    // Both "sessions" above and below run in this SAME test process, so
    // the first session's own pid is trivially still alive by the time
    // the second bootstrap runs — stamp its `sessions` row with an
    // unambiguously-dead pid (`is_process_alive`'s own `pid <= 0` guard)
    // directly through a raw connection, the only way to simulate a
    // truly dead prior process without spawning a real second one.
    {
        let raw = rusqlite::Connection::open(&db_path).expect("open db file directly");
        raw.execute("UPDATE sessions SET pid = -1", [])
            .expect("mark every recorded session dead");
    }

    // Second "session": a plain no-positional launch against the SAME
    // `$HOME` — the prior session's process is now unambiguously dead,
    // so `reconstruct_scratch` must find it and recover the draft.
    let vfs = Mem::new();
    let app = bootstrap(
        Arc::new(vfs),
        std::iter::empty(),
        PathBuf::from("/"),
        Some(home.0.clone()),
    )
    .expect("second bootstrap should succeed");

    assert_eq!(
        app.active_doc().buffer.content(),
        "unsaved draft from a dead session",
        "the dead session's own draft must come back on the next launch"
    );
}

/// Issue #77's own regression: a full bootstrap of one positional must read
/// that path off disk exactly once — the buffer's bytes and the recovery
/// store's CAS baseline both trace to the SAME [`rune_vfs::Sighting`], never
/// two independent reads racing against an external rewrite in between.
#[test]
fn launch_one_positional_reads_the_path_exactly_once() {
    let counting = Arc::new(CountingReadVfs::new(Mem::new()));
    counting
        .inner
        .save_atomic(Path::new("/vault/a.md"), b"hello")
        .expect("seed a.md");
    let home = ScratchHome::new("one-read");
    let vfs: Arc<dyn Vfs + Send + Sync> = counting.clone();

    let app = bootstrap(
        vfs,
        vec![OsString::from("/vault/a.md")].into_iter(),
        PathBuf::from("/"),
        Some(home.0.clone()),
    )
    .expect("bootstrap should succeed");

    assert_eq!(app.active_doc().buffer.content(), "hello");
    assert_eq!(
        counting.reads.load(Ordering::SeqCst),
        1,
        "one launched positional must read its path exactly once"
    );
}
