//! Issue #87: `Hydration::Refused` must take the same detach exit
//! `handle_load_ack`'s `saved_obs == None` arm already took — a document
//! whose recovered content this session just rejected may never keep
//! journaling against that row, and a direct-vfs save must still work for
//! it afterward.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod rename_common;

use std::path::Path;

use rune_tui::db_ack::handle_load_ack;
use rune_tui::runtime::CmdKind;
use rune_vfs::Vfs;

use rename_common::{app_with_store, seeded_vfs, sup, type_text};

/// A `recovered` far shorter than `disk_content` trips `rune_core::
/// is_suspicious_shrink` inside `Document::hydrate`, refusing the
/// adoption outright.
const REFUSING_DISK: &str = "this is the genuine content sitting on disk, plenty long";
const REFUSING_RECOVERED: &str = "x";

#[test]
fn refused_hydration_detaches() {
    let mem = seeded_vfs();
    let (mut app, _bridge) = app_with_store(&mem);
    let id = app.active;
    let db_id = app.doc(id).unwrap().doc_db().unwrap().db_id;

    // Dirties the buffer BEFORE the refusal, so the later save actually has
    // something to publish rather than short-circuiting on "not dirty".
    type_text(&mut app, "!");
    let issued_version = app.doc(id).unwrap().buffer.version();

    let load_result = rune_db::LoadResult {
        doc_id: db_id,
        renamed_from: None,
        disk_content: REFUSING_DISK.to_string(),
        recovered: REFUSING_RECOVERED.to_string(),
        has_history: true,
        sync: rune_db::SyncState {
            kind: rune_db::SyncKind::Clean,
            ancestor: None,
            ours: rune_db::Version {
                hash: String::new(),
                obs: None,
            },
            theirs: None,
        },
        nlink: 1,
        saved_obs: Some(99),
        bridge_seq: Some(1),
        resumable_merge: None,
    };

    handle_load_ack(&mut app, id, load_result, Some(issued_version), false);

    assert!(
        !app.doc(id).unwrap().is_store_bound(),
        "a refused hydration must drop the document's own db binding"
    );
    assert!(
        app.file_binding(db_id).is_none(),
        "a refused hydration must prune the now-unreferenced shared file binding"
    );
    assert!(
        rune_tui::messages::log_text(&app).contains("crash recovery:"),
        "the refusal must be surfaced to the user"
    );
    assert_eq!(
        app.doc(id).unwrap().buffer.content(),
        "!a content",
        "hydrate never mutates the buffer before refusing"
    );

    // A subsequent ⌘S must still work — the direct-vfs fallback
    // (`save::trigger_save`'s `has_binding` check) rather than the now-gone
    // store binding.
    let effects = rename_common::send(&mut app, sup('s'));
    let save_cmd = effects
        .cmds
        .into_iter()
        .find(|c| c.kind() == CmdKind::Save)
        .expect("no store binding must fall back to the direct-vfs Save Cmd");
    let done_msg = save_cmd.run().expect("the vfs Cmd must reply");
    let mut effects = rune_tui::runtime::Effects::default();
    rune_tui::app::update(&mut app, done_msg, &mut effects);

    assert!(
        !app.doc(id).unwrap().is_dirty(),
        "the direct-vfs save must succeed and clear dirty"
    );
    assert_eq!(
        mem.read(Path::new("/root/a.md"))
            .expect("file still present"),
        b"!a content",
        "the direct-vfs save must publish the buffer's own bytes"
    );
}
