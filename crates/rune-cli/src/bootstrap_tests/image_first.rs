use super::*;

#[test]
fn launch_image_first_still_opens_the_session_store() {
    let vfs = Mem::new();
    vfs.save_atomic(
        Path::new("/vault/x.png"),
        &[0x89, b'P', b'N', b'G', 0, 0, 0, 0],
    )
    .expect("seed a (fake) png");
    let home = ScratchHome::new("image-first-store");

    let app = bootstrap(
        &(Arc::new(vfs) as Arc<dyn Vfs + Send + Sync>),
        vec![OsString::from("/vault/x.png")].into_iter(),
        Path::new("/"),
        Some(&home.0),
    )
    .expect("bootstrap should succeed for an image first positional");

    assert!(
        app.db.is_some(),
        "an image-first launch must still open the session-wide store"
    );
    assert!(
        app.db_banner.is_none(),
        "a healthy store open must not post a banner"
    );
}

#[test]
fn launch_image_first_with_an_unopenable_store_banners() {
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
    .expect("bootstrap should succeed even with no recovery store");

    assert!(
        app.db.is_none(),
        "an unopenable store must leave the image-first launch with db: None"
    );
    assert!(
        app.db_banner.is_some(),
        "an unopenable store must still surface the recovery-disabled banner"
    );
}

#[test]
fn launch_image_first_then_opening_markdown_enqueues_journaling() {
    let vfs = Mem::new();
    vfs.save_atomic(
        Path::new("/vault/x.png"),
        &[0x89, b'P', b'N', b'G', 0, 0, 0, 0],
    )
    .expect("seed a (fake) png");
    vfs.save_atomic(Path::new("/vault/notes.md"), b"notes")
        .expect("seed notes.md");
    let home = ScratchHome::new("image-first-then-markdown");

    let mut app = bootstrap(
        &(Arc::new(vfs) as Arc<dyn Vfs + Send + Sync>),
        vec![OsString::from("/vault/x.png")].into_iter(),
        Path::new("/"),
        Some(&home.0),
    )
    .expect("bootstrap should succeed for an image first positional");

    assert!(
        app.db_ops.is_empty(),
        "nothing has opened yet besides the image"
    );
    rune_tui::workspace::open_path(&mut app, Path::new("/vault/notes.md"));
    assert!(
        !app.db_ops.is_empty(),
        "opening a markdown file after an image-first launch must enqueue a Load"
    );
}
