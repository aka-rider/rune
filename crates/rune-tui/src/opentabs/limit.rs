//! Pinning marks a tab the upcoming tab-cap eviction must never pick. The
//! toggle refuses previews because a preview is a transient the user never
//! opened.
//!
//! [`ensure_room`] keeps every INTERACTIVE open within the digit-chord
//! range (`^1`-`^0` only ever names ten tabs): once the strip is full it
//! evicts the least-recently-active eligible tab through the ordinary
//! close chokepoint, or refuses outright when nothing is eligible.
//! Bootstrap and recovery adoption never call this — recovered or launch-
//! requested work must never be turned away for a tab.

use crate::app::App;
use crate::document::DocumentId;
use crate::guard::{GuardKind, GuardPrompt};
use crate::runtime::Effects;

/// Flips the active document's pin, refusing (with its own warn message) on
/// a preview tab.
pub fn toggle_pin(app: &mut App, id: DocumentId) {
    if app.refuse_if_preview(id) {
        return;
    }
    if let Some(doc) = app.doc_mut(id) {
        doc.pinned = !doc.pinned;
    }
}

/// The interactive-open ceiling — matches the ten digit-chord slots
/// `^1`-`^0` can ever address.
pub const MAX_TABS: usize = 10;

/// Makes room for one more interactive tab, evicting if necessary.
/// Returns `true` when the strip already has room or an eviction just
/// freed a slot; `false` when the open must be refused — either because
/// nothing was eligible, or because the eviction candidate turned out to
/// be dirty and now has its own close prompt in the way.
pub fn ensure_room(app: &mut App, effects: &mut Effects) -> bool {
    if app.tabs.order.len() < MAX_TABS {
        return true;
    }
    let eligible: Vec<DocumentId> = app
        .tabs
        .mru
        .iter()
        .copied()
        .filter(|&id| {
            id != app.active
                && app
                    .doc(id)
                    .is_some_and(|doc| !doc.pinned && !doc.is_preview() && doc.file_path.is_some())
        })
        .collect();
    let clean = eligible
        .iter()
        .copied()
        .find(|&id| !crate::materialize_ack::is_dirty_now(app, id));
    let had_guard = app.guard.is_some();
    if let Some(victim) = clean.or_else(|| eligible.first().copied()) {
        if clean.is_none() {
            // The DirtyClose prompt must cover the document the user can
            // see; arming it for a background tab invites a [D]iscard
            // aimed at the wrong buffer.
            crate::workspace::switch_to(app, victim);
        }
        crate::workspace::request_close(app, victim, effects);
        if app.tabs.order.len() < MAX_TABS {
            return true;
        }
        // `request_close` discards `set_guard`'s own return, and a
        // pre-existing foreign guard would also leave `app.guard` `Some`
        // here — the `had_guard` snapshot plus a victim match is the only
        // way to tell "this eviction just armed its own prompt" apart from
        // "something else already had the guard".
        let armed_for_victim = !had_guard
            && matches!(&app.guard, Some(GuardPrompt { doc, kind: GuardKind::DirtyClose }) if *doc == victim);
        if armed_for_victim {
            return false;
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
    use crate::messages;
    use crate::workspace;
    use rune_core::buffer::Buffer;
    use rune_vfs::{Mem, Vfs};
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

    /// Builds an `App` with `n` file-bound tabs, none pinned, dirty, or
    /// preview — the initial document (index 0, `app.active`) plus `n - 1`
    /// more opened straight through `App::open_document`, matching how
    /// bootstrap/recovery adoption bind a path without ever touching
    /// `ensure_room`. Returns the ids in open (= mru, at this point) order.
    fn filled_app(n: usize) -> (App, Vec<DocumentId>) {
        filled_app_with(Arc::new(Mem::new()), n)
    }

    /// Same as [`filled_app`], but binds the documents' paths inside a
    /// caller-supplied `Mem` rather than a private, unseeded one — needed
    /// by any test that also seeds file content for a later real `Vfs`
    /// read (an already-open reactivation never reads; a fresh open —
    /// `open_path_checked`, an Explorer preview — does).
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

        assert!(app.tabs.order.len() <= MAX_TABS);
        assert!(
            !app.tabs.order.contains(&expected_victim),
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
        assert_eq!(app.tabs.order.len(), MAX_TABS);
        assert_eq!(
            messages::newest_text(&app),
            Some("Tab limit reached — close or unpin a tab")
        );
    }

    #[test]
    fn a_dirty_lra_victim_arms_the_dirty_close_guard() {
        let (mut app, ids) = filled_app(MAX_TABS);
        for &id in &ids[1..] {
            crate::commands::edit::insert_char(&mut app, id, '!');
        }
        let expected_victim = ids[1];

        let mut effects = Effects::default();
        assert!(!ensure_room(&mut app, &mut effects));

        assert!(matches!(
            &app.guard,
            Some(GuardPrompt { doc, kind: GuardKind::DirtyClose }) if *doc == expected_victim
        ));
        assert_eq!(app.active, expected_victim, "the victim was switched to");
        assert_eq!(app.tabs.order.len(), MAX_TABS);
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
            app.tabs.order.contains(&pinned),
            "the pinned tab must survive"
        );
        assert!(
            !app.tabs.order.contains(&next_lra),
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
                is_dir: false,
            })
            .chain(std::iter::once(rune_vfs::DirEntry {
                name: "eleventh.md".to_string(),
                path: std::path::PathBuf::from("/root/eleventh.md"),
                is_dir: false,
            }))
            .collect();
        crate::explorer::handle_dir_loaded(
            &mut app,
            std::path::PathBuf::from("/root"),
            entries,
            crate::runtime::DirCause::Nav,
            0,
        );
        let tabs_before = app.tabs.order.len();
        let newest_before = messages::newest_text(&app).map(str::to_string);

        let mut effects = Effects::default();
        // The Explorer entry list is prefixed with a synthetic ".."
        // parent row (`with_parent_entry`), so the never-opened
        // "eleventh.md" row sits one past `MAX_TABS`, not at it.
        app.explorer.nav.cursor = MAX_TABS + 1;
        crate::explorer_preview::after_cursor_move(&mut app, &mut effects);
        let cmds = std::mem::take(&mut effects.cmds);
        for cmd in cmds {
            if let Some(crate::runtime::Msg::FileOpened {
                path,
                result,
                anchor,
            }) = cmd.run()
            {
                crate::workspace::handle_file_opened(&mut app, path, result, anchor, &mut effects);
            }
        }

        assert_eq!(app.tabs.order.len(), tabs_before, "no new tab at cap");
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
        assert!(crate::guard::set_guard(
            &mut app,
            GuardPrompt {
                doc,
                kind: GuardKind::DirtyQuit,
            }
        ));

        let mut effects = Effects::default();
        assert!(!ensure_room(&mut app, &mut effects));

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
        assert_eq!(app.tabs.order.len(), MAX_TABS);
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
        assert!(app.tabs.order.len() <= MAX_TABS);
        assert!(
            !app.tabs.order.contains(&expected_victim),
            "the least-recently-active clean tab must have been evicted"
        );
    }
}
