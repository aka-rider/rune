use super::*;

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
        &(Arc::new(vfs) as Arc<dyn Vfs + Send + Sync>),
        vec![
            OsString::from("/vault/a.md"),
            OsString::from("/vault/b.md"),
            OsString::from("/vault/c.md"),
        ]
        .into_iter(),
        Path::new("/"),
        Some(&home.0),
    )
    .expect("bootstrap should succeed");

    // The first file hydrates synchronously inside `bootstrap_db` and
    // is bound before `App::new` ever runs.
    assert_eq!(app.documents.len(), 3);
    assert!(
        app.doc(app.active)
            .is_some_and(rune_tui::document::Document::is_store_bound)
    );
    // The other two open through `workspace::open_path`'s async path: each
    // one's `Load` must actually be enqueued and tracked, not silently
    // dropped the way a `Sink::Bootstrap`-less bridge used to swallow it —
    // `db_ops` is where `db::load_document` records that at enqueue time,
    // synchronously.
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
        &(Arc::new(vfs) as Arc<dyn Vfs + Send + Sync>),
        vec![
            OsString::from("/vault/notes.md"),
            OsString::from("/vault/sub/../notes.md"),
        ]
        .into_iter(),
        Path::new("/"),
        Some(&home.0),
    )
    .expect("bootstrap should succeed");

    assert_eq!(
        app.documents.len(),
        1,
        "two spellings of the same file must resolve to one document"
    );
}

/// A launch positional with more than one hard link must carry that fact
/// onto the active document and warn that saving will fork it from its
/// other names.
#[test]
fn launch_of_a_hardlinked_positional_carries_the_fact_and_warns() {
    let vfs = Mem::new();
    vfs.save_atomic(Path::new("/vault/notes.md"), b"hi")
        .expect("seed notes.md");
    vfs.set_nlink(Path::new("/vault/notes.md"), 2)
        .expect("set nlink");
    let home = ScratchHome::new("hardlink");

    let app = bootstrap(
        &(Arc::new(vfs) as Arc<dyn Vfs + Send + Sync>),
        vec![OsString::from("/vault/notes.md")].into_iter(),
        Path::new("/"),
        Some(&home.0),
    )
    .expect("bootstrap should succeed");

    assert_eq!(app.doc(app.active).expect("doc exists").nlink, Some(2));
    assert_eq!(
        rune_tui::messages::newest_text(&app),
        Some(
            "this file has 2 hard links \u{2014} saving replaces it atomically, so the other links keep the old content"
        )
    );
}

/// A `.png` first positional bootstraps through the SAME
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
        &(Arc::new(vfs) as Arc<dyn Vfs + Send + Sync>),
        vec![OsString::from("/vault/x.png")].into_iter(),
        Path::new("/"),
        None,
    )
    .expect("bootstrap should succeed for an image first positional");

    assert_eq!(
        app.documents.len(),
        1,
        "the blank untitled anchor must be closed once the image opens"
    );
    assert!(
        app.doc(app.active)
            .is_some_and(rune_tui::document::Document::is_read_only)
    );
    assert!(
        app.doc(app.active)
            .is_some_and(|d| d.file_path.as_deref() == Some(Path::new("/vault/x.png")))
    );
}

/// A missing-path launch is a recovery-backed draft that already knows its
/// name, not a launch with zero crash protection: no banner, a live
/// app-wide `Db`, and the active document bound to a fresh scratch row in
/// the create-only publish mode — the same shape a no-positional launch's
/// default document gets.
#[test]
fn launch_nonexistent_path_is_recovery_backed() {
    let vfs = Mem::new();
    let home = ScratchHome::new("missing-path");

    let app = bootstrap(
        &(Arc::new(vfs) as Arc<dyn Vfs + Send + Sync>),
        vec![OsString::from("/vault/missing.md")].into_iter(),
        Path::new("/"),
        Some(&home.0),
    )
    .expect("bootstrap should succeed for a missing-path launch");

    assert!(
        app.db_banner.is_none(),
        "a missing-path launch is now recovery-backed, not degraded"
    );
    assert!(app.db.is_some(), "a live app-wide store must be bound");
    assert!(
        app.doc(app.active)
            .and_then(|d| d.doc_db())
            .is_some_and(|db| db.publish_mode.is_create_only()),
        "the active document must be bound to a scratch row awaiting its first publish"
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
        &(Arc::new(vfs) as Arc<dyn Vfs + Send + Sync>),
        vec![OsString::from("/vault/missing.md")].into_iter(),
        Path::new("/"),
        None,
    )
    .expect("bootstrap should succeed even with no recovery store");

    assert!(
        app.db_banner.is_some(),
        "a missing-path launch with no usable $HOME must still say so"
    );
}

/// A first positional whose resolution fails must never fall back to the
/// caller's unnormalized spelling — `bootstrap` refuses and
/// exits `EX_IOERR`, the same code `open::open_first_positional`'s own
/// unreadable-file arm already returns, rather than launching under a
/// path whose on-disk identity was never actually confirmed.
#[test]
fn launch_resolve_failing_first_positional_exits_with_the_io_error_code() {
    let mem = Arc::new(Mem::new());
    mem.fail_resolve(Path::new("/vault/unresolvable.md"));
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;

    let result = bootstrap(
        &vfs,
        vec![OsString::from("/vault/unresolvable.md")].into_iter(),
        Path::new("/"),
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
        &(Arc::new(vfs) as Arc<dyn Vfs + Send + Sync>),
        vec![OsString::from("")].into_iter(),
        Path::new("/"),
        None,
    );
    assert!(
        result.is_err(),
        "an empty positional must be rejected at parse"
    );
}

/// The untitled draft is really recovery-backed: a no-positional launch
/// against a real (temp) `$HOME` must come up with
/// BOTH a live app-wide `Db` and a bound `DocDb` on the default
/// document — the two facts that together arm the guard's "recovery-
/// backed" exemption. Before this change, this launch mode always had
/// `db: None` (see the now-resolved `crates/rune-tui/TODO.md` entry).
#[test]
fn no_positional_launch_binds_both_the_app_db_and_a_doc_db() {
    let vfs = Mem::new();
    let home = ScratchHome::new("untitled-doc-db");

    let app = bootstrap(
        &(Arc::new(vfs) as Arc<dyn Vfs + Send + Sync>),
        std::iter::empty(),
        Path::new("/"),
        Some(&home.0),
    )
    .expect("bootstrap should succeed with no positional files");

    assert!(
        app.db.is_some(),
        "the default untitled launch must have a live app-wide store"
    );
    assert!(
        app.doc(app.active)
            .is_some_and(rune_tui::document::Document::is_store_bound),
        "the default document must be bound to its own scratch row"
    );
}
/// A full bootstrap of one positional must read that path off disk exactly
/// once — the buffer's bytes and the recovery
/// store's CAS baseline both trace to the SAME [`rune_vfs::Sighting`], never
/// two independent reads racing against an external rewrite in between —
/// AND resolve it exactly once: `open_launch` resolves the positional
/// itself, and everything downstream (`load_sighting` -> `rune_vfs::
/// get_resolved`) must reuse that identity rather than independently
/// resolving it again, which would reopen a symlink-swap TOCTOU window
/// between the two resolves.
#[test]
fn launch_one_positional_reads_and_resolves_the_path_exactly_once() {
    let counting = Arc::new(CountingReadVfs::new(Mem::new()));
    counting
        .inner
        .save_atomic(Path::new("/vault/a.md"), b"hello")
        .expect("seed a.md");
    let home = ScratchHome::new("one-read");
    let vfs: Arc<dyn Vfs + Send + Sync> = counting.clone();

    let app = bootstrap(
        &vfs,
        vec![OsString::from("/vault/a.md")].into_iter(),
        Path::new("/"),
        Some(&home.0),
    )
    .expect("bootstrap should succeed");

    assert_eq!(app.active_doc().buffer.content(), "hello");
    assert_eq!(
        counting.reads.load(Ordering::SeqCst),
        1,
        "one launched positional must read its path exactly once"
    );
    assert_eq!(
        counting.resolves.load(Ordering::SeqCst),
        1,
        "one launched positional must resolve its path exactly once"
    );
}

/// The same single-resolve pin as above, for a positional naming a path
/// that does not exist yet (`bootstrap_new_file`'s scratch-row route,
/// `open_first_text`'s `None` branch) — the TOCTOU window this guards
/// against is not conditioned on the file already existing.
#[test]
fn launch_missing_positional_resolves_the_path_exactly_once() {
    let counting = Arc::new(CountingReadVfs::new(Mem::new()));
    let home = ScratchHome::new("one-resolve-missing");
    let vfs: Arc<dyn Vfs + Send + Sync> = counting.clone();

    let app = bootstrap(
        &vfs,
        vec![OsString::from("/vault/new.md")].into_iter(),
        Path::new("/"),
        Some(&home.0),
    )
    .expect("bootstrap should succeed");

    assert_eq!(app.active_doc().buffer.content(), "");
    assert_eq!(
        counting.resolves.load(Ordering::SeqCst),
        1,
        "a missing positional must still resolve its path exactly once"
    );
}
