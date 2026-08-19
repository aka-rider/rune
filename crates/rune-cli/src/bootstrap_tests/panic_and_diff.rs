use super::*;

fn caught_panic(f: impl FnOnce() + std::panic::UnwindSafe) -> Box<dyn Any + Send> {
    std::panic::catch_unwind(f).expect_err("the closure must panic")
}

#[test]
fn panic_message_recovers_a_formatted_message() {
    let payload = caught_panic(|| panic!("caught formatted panic {}", 42));
    assert_eq!(panic_message(payload), "caught formatted panic 42");
}

#[test]
fn panic_message_recovers_a_literal_message() {
    let payload = caught_panic(|| panic!("caught literal panic"));
    assert_eq!(panic_message(payload), "caught literal panic");
}

#[test]
fn diff_launch_opens_file_b_normally_and_installs_file_a_read_only() {
    let vfs = Mem::new();
    vfs.save_atomic(Path::new("/vault/a.md"), b"left content")
        .expect("seed a.md");
    vfs.save_atomic(Path::new("/vault/b.md"), b"right content")
        .expect("seed b.md");
    let home = ScratchHome::new("diff-both-exist");

    let app = bootstrap(
        &(Arc::new(vfs) as Arc<dyn Vfs + Send + Sync>),
        vec![
            OsString::from("--diff"),
            OsString::from("/vault/a.md"),
            OsString::from("/vault/b.md"),
        ]
        .into_iter(),
        Path::new("/"),
        Some(&home.0),
    )
    .expect("bootstrap should succeed");

    assert_eq!(app.documents.len(), 1, "fileB is the only ordinary tab");
    assert_eq!(app.active_doc().buffer.content(), "right content");
    assert!(
        app.active_doc()
            .file_path
            .as_deref()
            .is_some_and(|p| p == Path::new("/vault/b.md"))
    );

    let diff = app.diff.as_ref().expect("diff view must be installed");
    assert_eq!(diff.left.buffer.content(), "left content");
    assert_eq!(diff.left.read_only, rune_tui::document::ReadOnly::Always);
    assert_eq!(diff.right, app.active);
}

#[test]
fn diff_launch_with_a_missing_left_file_exits_with_the_io_error_code() {
    let vfs = Mem::new();
    vfs.save_atomic(Path::new("/vault/b.md"), b"right content")
        .expect("seed b.md");
    let home = ScratchHome::new("diff-left-missing");

    let result = bootstrap(
        &(Arc::new(vfs) as Arc<dyn Vfs + Send + Sync>),
        vec![
            OsString::from("--diff"),
            OsString::from("/vault/missing-a.md"),
            OsString::from("/vault/b.md"),
        ]
        .into_iter(),
        Path::new("/"),
        Some(&home.0),
    );
    let Err(code) = result else {
        panic!("a missing fileA must fail the launch");
    };

    assert_eq!(code, ExitCode::from(exit_code::IO_ERR));
}

#[test]
fn diff_launch_with_invalid_utf8_in_the_left_file_exits_with_the_data_error_code() {
    let vfs = Mem::new();
    vfs.save_atomic(Path::new("/vault/a.md"), &[0xff, 0xfe])
        .expect("seed a.md with invalid utf-8");
    vfs.save_atomic(Path::new("/vault/b.md"), b"right content")
        .expect("seed b.md");
    let home = ScratchHome::new("diff-left-bad-utf8");

    let result = bootstrap(
        &(Arc::new(vfs) as Arc<dyn Vfs + Send + Sync>),
        vec![
            OsString::from("--diff"),
            OsString::from("/vault/a.md"),
            OsString::from("/vault/b.md"),
        ]
        .into_iter(),
        Path::new("/"),
        Some(&home.0),
    );
    let Err(code) = result else {
        panic!("invalid UTF-8 in fileA must fail the launch");
    };

    assert_eq!(code, ExitCode::from(exit_code::DATA_ERR));
}

#[test]
fn diff_launch_with_a_missing_right_file_is_recovery_backed_like_an_ordinary_launch() {
    let vfs = Mem::new();
    vfs.save_atomic(Path::new("/vault/a.md"), b"left content")
        .expect("seed a.md");
    let home = ScratchHome::new("diff-right-missing");

    let app = bootstrap(
        &(Arc::new(vfs) as Arc<dyn Vfs + Send + Sync>),
        vec![
            OsString::from("--diff"),
            OsString::from("/vault/a.md"),
            OsString::from("/vault/missing-b.md"),
        ]
        .into_iter(),
        Path::new("/"),
        Some(&home.0),
    )
    .expect("bootstrap should succeed even though fileB does not exist yet");

    assert_eq!(app.active_doc().buffer.content(), "");
    assert!(app.diff.is_some());
}
