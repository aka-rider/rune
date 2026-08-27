use crate::app::App;
use crate::document::{Document, DocumentId};
use crate::runtime::Effects;

pub fn toggle_pin(app: &mut App, id: DocumentId) {
    if app.refuse_if_preview(id) {
        return;
    }
    if let Some(doc) = app.doc_mut(id) {
        doc.pinned = !doc.pinned;
    }
}

pub const MAX_TABS: usize = 10;

fn room_available(app: &App) -> bool {
    let occupied = app.documents.order().len();
    let occupied = if app.explorer.preview.is_some_and(|id| app.doc(id).is_some()) {
        occupied.saturating_sub(1)
    } else {
        occupied
    };
    occupied < MAX_TABS
}

pub fn ensure_room(app: &mut App, effects: &mut Effects) -> bool {
    if room_available(app) {
        return true;
    }
    let eligible: Vec<DocumentId> = app
        .documents
        .mru()
        .iter()
        .copied()
        .filter(|&id| {
            id != app.active
                && app.doc(id).is_some_and(|doc| {
                    !doc.pinned
                        && !doc.is_preview()
                        && doc.file_path.is_some()
                        && !doc.save_in_flight()
                })
        })
        .collect();
    let clean = eligible
        .iter()
        .copied()
        .find(|&id| !app.doc(id).is_some_and(Document::is_dirty));
    if let Some(victim) = clean {
        crate::workspace::request_close(app, victim, effects);
        if room_available(app) {
            return true;
        }
    }
    crate::messages::warn(app, "Tab limit reached — close or unpin a tab");
    false
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::document::ReadOnly;
    use crate::guard::{GuardKind, GuardPrompt};
    use crate::messages;
    use crate::workspace;
    use rune_core::buffer::Buffer;
    use rune_vfs::{Mem, VfsTestExt};
    use std::sync::Arc;

    fn app() -> App {
        let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
        app.active_doc_mut().viewport.set_size(80, 23);
        app
    }

    fn active_row(app: &App, width: u16) -> String {
        let buf = crate::testgrid::draw_with(width, 4, |frame| {
            crate::opentabs::draw(app, ratatui::layout::Rect::new(0, 0, width, 4), frame)
        });
        (0..width)
            .filter_map(|x| buf.cell((x, 0)).map(|c| c.symbol().to_string()))
            .collect()
    }

    #[test]
    fn toggle_pin_flips_the_flag_and_draw_shows_the_marker() {
        let mut app = app();
        let active = app.active;

        toggle_pin(&mut app, active);
        assert!(app.doc(active).unwrap().pinned);
        assert_eq!(&active_row(&app, 20)[0..5], "  1:*");

        toggle_pin(&mut app, active);
        assert!(!app.doc(active).unwrap().pinned);
        assert_eq!(&active_row(&app, 20)[0..5], "  1: ");
    }

    #[test]
    fn toggle_pin_refuses_a_preview() {
        let mut app = app();
        let active = app.active;
        app.doc_mut(active).unwrap().read_only = ReadOnly::Preview;

        toggle_pin(&mut app, active);

        assert!(!app.doc(active).unwrap().pinned);
        assert_eq!(
            messages::newest_text(&app),
            ReadOnly::Preview.refusal_message()
        );
    }

    fn filled_app(n: usize) -> (App, Vec<DocumentId>) {
        filled_app_with(Arc::new(Mem::new()), n)
    }

    fn filled_app_with(mem: Arc<Mem>, n: usize) -> (App, Vec<DocumentId>) {
        let mut app = App::new(Buffer::new("hello"), None, mem, None);
        app.active_doc_mut().viewport.set_size(80, 23);
        app.doc_mut(app.active)
            .unwrap()
            .bind_path(std::path::PathBuf::from("/root/doc0.md"));
        let mut ids = vec![app.active];
        for i in 1..n {
            let id = app.open_document(Buffer::new("hello"));
            app.doc_mut(id)
                .unwrap()
                .bind_path(std::path::PathBuf::from(format!("/root/doc{i}.md")));
            ids.push(id);
        }
        (app, ids)
    }

    #[test]
    fn eleventh_open_evicts_the_least_recently_active_clean_tab() {
        let (mut app, ids) = filled_app(MAX_TABS);
        let expected_victim = ids[1];

        let mut effects = Effects::default();
        assert!(ensure_room(&mut app, &mut effects));
        app.open_document(Buffer::new("eleventh"));

        assert!(app.documents.order().len() <= MAX_TABS);
        assert!(
            !app.documents.order().contains(&expected_victim),
            "the oldest non-active, non-pinned, clean tab must have been evicted"
        );
    }

    #[test]
    fn open_is_refused_when_no_tab_is_eligible() {
        let (mut app, ids) = filled_app(MAX_TABS);
        for &id in &ids[1..] {
            app.doc_mut(id).unwrap().pinned = true;
        }

        let mut effects = Effects::default();
        assert!(!ensure_room(&mut app, &mut effects));
        assert_eq!(app.documents.order().len(), MAX_TABS);
        assert_eq!(
            messages::newest_text(&app),
            Some("Tab limit reached — close or unpin a tab")
        );
    }

    #[test]
    fn a_dirty_lra_victim_is_never_hijacked_and_the_open_is_refused_with_feedback() {
        let (mut app, ids) = filled_app(MAX_TABS);
        for &id in &ids[1..] {
            crate::commands::edit::insert_char(&mut app, id, '!');
        }
        let active_before = app.active;

        let mut effects = Effects::default();
        assert!(!ensure_room(&mut app, &mut effects));

        assert!(
            app.guard.is_none(),
            "a dirty-only eligible set must never arm a guard the user never asked for"
        );
        assert_eq!(
            app.active, active_before,
            "the active document must never be switched away to make room"
        );
        assert_eq!(app.documents.order().len(), MAX_TABS);
        assert_eq!(
            messages::newest_text(&app),
            Some("Tab limit reached — close or unpin a tab")
        );
    }

    #[test]
    fn a_tab_with_a_save_in_flight_is_never_the_eviction_victim() {
        let (mut app, ids) = filled_app(MAX_TABS);
        let saving = ids[1];
        let next_lra = ids[2];
        let (version, content) = {
            let doc = app.doc(saving).unwrap();
            (doc.buffer.version(), Arc::from(doc.buffer.content()))
        };
        app.doc_mut(saving).unwrap().begin_save(version, content);

        let mut effects = Effects::default();
        assert!(ensure_room(&mut app, &mut effects));
        app.open_document(Buffer::new("eleventh"));

        assert!(
            app.documents.order().contains(&saving),
            "a tab mid-save must survive eviction"
        );
        assert!(
            !app.documents.order().contains(&next_lra),
            "the next least-recently-active clean tab must have been evicted instead"
        );
    }

    #[test]
    fn a_pinned_tab_is_never_the_eviction_victim() {
        let (mut app, ids) = filled_app(MAX_TABS);
        let pinned = ids[1];
        let next_lra = ids[2];
        app.doc_mut(pinned).unwrap().pinned = true;

        let mut effects = Effects::default();
        assert!(ensure_room(&mut app, &mut effects));
        app.open_document(Buffer::new("eleventh"));

        assert!(
            app.documents.order().contains(&pinned),
            "the pinned tab must survive"
        );
        assert!(
            !app.documents.order().contains(&next_lra),
            "the next least-recently-active clean tab must have been evicted instead"
        );
    }

    #[test]
    fn a_preview_open_at_cap_is_skipped_silently() {
        let mem = Arc::new(Mem::new());
        for i in 0..MAX_TABS {
            mem.save_atomic(
                std::path::Path::new(&format!("/root/doc{i}.md")),
                b"content",
            )
            .unwrap();
        }
        mem.save_atomic(std::path::Path::new("/root/eleventh.md"), b"content")
            .unwrap();
        let (mut app, _ids) = filled_app_with(Arc::clone(&mem), MAX_TABS);

        let entries: Vec<rune_vfs::DirEntry> = (0..MAX_TABS)
            .map(|i| rune_vfs::DirEntry {
                name: format!("doc{i}.md"),
                path: std::path::PathBuf::from(format!("/root/doc{i}.md")),
                kind: rune_vfs::FileKind::File,
                link: rune_vfs::Link::No,
            })
            .chain(std::iter::once(rune_vfs::DirEntry {
                name: "eleventh.md".to_string(),
                path: std::path::PathBuf::from("/root/eleventh.md"),
                kind: rune_vfs::FileKind::File,
                link: rune_vfs::Link::No,
            }))
            .collect();
        crate::explorer::handle_dir_loaded(
            &mut app,
            std::path::PathBuf::from("/root"),
            entries,
            crate::runtime::DirCause::Nav,
            crate::generation::Generation::ZERO,
        );
        let tabs_before = app.documents.order().len();
        let newest_before = messages::newest_text(&app).map(str::to_string);

        let mut effects = Effects::default();
        app.explorer.nav.cursor = MAX_TABS + 1;
        crate::explorer_preview::after_cursor_move(&mut app, &mut effects);
        let cmds = std::mem::take(&mut effects.cmds);
        for cmd in cmds {
            if let Some(crate::runtime::Msg::FileOpened {
                path,
                result,
                anchor,
                preview_generation,
            }) = cmd.run()
            {
                crate::workspace::handle_file_opened(
                    &mut app,
                    &path,
                    result,
                    anchor,
                    preview_generation,
                    &mut effects,
                );
            }
        }

        assert_eq!(
            app.documents.order().len(),
            tabs_before,
            "no new tab at cap"
        );
        assert_eq!(
            messages::newest_text(&app).map(str::to_string),
            newest_before,
            "a passive preview miss at cap must post nothing"
        );
    }

    #[test]
    fn an_armed_foreign_guard_refuses_with_the_message() {
        let (mut app, ids) = filled_app(MAX_TABS);
        for &id in &ids[1..] {
            crate::commands::edit::insert_char(&mut app, id, '!');
        }
        let doc = app.active;
        assert_eq!(
            crate::guard::set_guard(
                &mut app,
                GuardPrompt {
                    doc,
                    kind: GuardKind::DirtyQuit,
                },
                &mut crate::runtime::Effects::default()
            ),
            crate::guard::GuardRaise::Raised
        );

        let mut effects = Effects::default();
        assert!(!ensure_room(&mut app, &mut effects));

        assert_eq!(
            app.active, doc,
            "a foreign guard makes the eviction futile — must not hijack focus for it"
        );
        assert!(matches!(
            &app.guard,
            Some(GuardPrompt { doc: guard_doc, kind: GuardKind::DirtyQuit }) if *guard_doc == doc
        ));
        assert_eq!(
            messages::newest_text(&app),
            Some("Tab limit reached — close or unpin a tab")
        );
    }

    #[test]
    fn a_live_preview_frees_its_slot_for_the_incoming_open() {
        let (mut app, ids) = filled_app(MAX_TABS);
        let preview = ids[1];
        app.doc_mut(preview).unwrap().read_only = ReadOnly::Preview;
        app.explorer.preview = Some(preview);

        let mut effects = Effects::default();
        assert!(ensure_room(&mut app, &mut effects));

        assert_eq!(app.documents.order().len(), MAX_TABS, "no tab was evicted");
        assert!(
            app.documents.order().contains(&preview),
            "the preview itself is still open — it is displaced by the switch \
             the incoming document performs, not by ensure_room"
        );
        assert!(app.guard.is_none());
        assert_eq!(messages::newest_text(&app), None);
    }

    #[test]
    fn reopening_an_open_document_at_cap_reactivates_without_refusal() {
        let (mut app, ids) = filled_app(MAX_TABS);
        for &id in &ids {
            if id != app.active {
                app.doc_mut(id).unwrap().pinned = true;
            }
        }
        let target = ids[3];
        let newest_before = messages::newest_text(&app).map(str::to_string);

        let mut effects = Effects::default();
        let reopened = workspace::open_path_checked(
            &mut app,
            std::path::Path::new("/root/doc3.md"),
            &mut effects,
        );

        assert_eq!(reopened, Some(target));
        assert_eq!(app.active, target);
        assert_eq!(app.documents.order().len(), MAX_TABS);
        assert_eq!(
            messages::newest_text(&app).map(str::to_string),
            newest_before,
            "reactivating an already-open document must post nothing"
        );
    }

    #[test]
    fn open_path_checked_evicts_and_opens_at_cap() {
        let mem = Arc::new(Mem::new());
        mem.save_atomic(std::path::Path::new("/root/eleventh.md"), b"content")
            .unwrap();
        let (mut app, ids) = filled_app_with(mem, MAX_TABS);
        let expected_victim = ids[1];

        let mut effects = Effects::default();
        let opened = workspace::open_path_checked(
            &mut app,
            std::path::Path::new("/root/eleventh.md"),
            &mut effects,
        );

        assert!(opened.is_some());
        assert!(app.documents.order().len() <= MAX_TABS);
        assert!(
            !app.documents.order().contains(&expected_victim),
            "the least-recently-active clean tab must have been evicted"
        );
    }
}
