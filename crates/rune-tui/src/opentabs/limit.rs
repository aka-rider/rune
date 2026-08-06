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

/// Whether the strip has room for one more interactive tab: occupied slots
/// (`tabs.order`'s length, minus one when a live Explorer preview still
/// holds a place) under [`MAX_TABS`]. A preview is displaced by the very
/// switch that lands whatever open is about to proceed, so it never holds a
/// lasting slot and must not count toward the cap.
fn room_available(app: &App) -> bool {
    let occupied = app.tabs.order.len();
    let occupied = if app.explorer.preview.is_some_and(|id| app.doc(id).is_some()) {
        occupied.saturating_sub(1)
    } else {
        occupied
    };
    occupied < MAX_TABS
}

/// Makes room for one more interactive tab, evicting if necessary.
/// Returns `true` when the strip already has room (accounting for a
/// displaceable live preview) or an eviction just freed a slot; `false`
/// when the open must be refused — either because nothing was eligible,
/// because a foreign guard already occupies the one prompt slot an
/// eviction would need, or because the eviction candidate turned out to
/// be dirty and now has its own close prompt in the way.
pub fn ensure_room(app: &mut App, effects: &mut Effects) -> bool {
    if room_available(app) {
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
            if had_guard {
                // The single guard slot is already taken, so the close
                // below is guaranteed to refuse arming its own — switching
                // the active document over for an eviction that cannot
                // happen would only hijack focus (and auto-exit a live
                // merge session) for an open that gets refused anyway.
                crate::messages::warn(app, "Tab limit reached — close or unpin a tab");
                return false;
            }
            // The DirtyClose prompt must cover the document the user can
            // see; arming it for a background tab invites a [D]iscard
            // aimed at the wrong buffer.
            crate::workspace::switch_to(app, victim);
        }
        crate::workspace::request_close(app, victim, effects);
        // `request_close` discards `set_guard`'s own return, and a
        // pre-existing foreign guard would also leave `app.guard` `Some`
        // here — the `had_guard` snapshot plus a victim match is the only
        // way to tell "this eviction just armed its own prompt" apart from
        // "something else already had the guard". Checked BEFORE the room
        // re-check below: a guard now covering `victim` must refuse the
        // open even if `victim`'s own removal would otherwise have freed a
        // slot, because the prompt still needs to land on a visible tab.
        let armed_for_victim = !had_guard
            && matches!(&app.guard, Some(GuardPrompt { doc, kind: GuardKind::DirtyClose }) if *doc == victim);
        if armed_for_victim {
            return false;
        }
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
    use crate::messages;
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
        let mut app = app();
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
        let (mut app, ids) = filled_app(10);
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
        let (mut app, ids) = filled_app(10);
        for &id in &ids[1..] {
            app.doc_mut(id).unwrap().pinned = true;
        }

        let mut effects = Effects::default();
        assert!(!ensure_room(&mut app, &mut effects));
        assert_eq!(app.tabs.order.len(), 10);
        assert_eq!(
            messages::newest_text(&app),
            Some("Tab limit reached — close or unpin a tab")
        );
    }

    #[test]
    fn a_dirty_lra_victim_arms_the_dirty_close_guard() {
        let (mut app, ids) = filled_app(10);
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
        assert_eq!(app.tabs.order.len(), 10);
    }

    #[test]
    fn a_pinned_tab_is_never_the_eviction_victim() {
        let (mut app, ids) = filled_app(10);
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
        for i in 0..10 {
            mem.save_atomic(
                std::path::Path::new(&format!("/root/doc{i}.md")),
                b"content",
            )
            .unwrap();
        }
        mem.save_atomic(std::path::Path::new("/root/eleventh.md"), b"content")
            .unwrap();
        let vfs: Arc<dyn rune_vfs::Vfs + Send + Sync> =
            Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
        let mut app = App::new(Buffer::new("hello"), None, vfs, None);
        app.doc_mut(app.active)
            .unwrap()
            .bind_path(std::path::PathBuf::from("/root/doc0.md"));
        for i in 1..10 {
            let id = app.open_document(Buffer::new("hello"));
            app.doc_mut(id)
                .unwrap()
                .bind_path(std::path::PathBuf::from(format!("/root/doc{i}.md")));
        }
        let entries: Vec<rune_vfs::DirEntry> = (0..10)
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
        app.explorer.nav.cursor = 10; // the "eleventh.md" row, never opened yet
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
    fn a_futile_dirty_eviction_under_a_foreign_guard_does_not_switch() {
        let (mut app, ids) = filled_app(10);
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
        let (mut app, ids) = filled_app(10);
        let preview = ids[1];
        app.doc_mut(preview).unwrap().read_only = ReadOnly::Preview;
        app.explorer.preview = Some(preview);

        let mut effects = Effects::default();
        assert!(ensure_room(&mut app, &mut effects));

        assert_eq!(app.tabs.order.len(), 10, "no tab was evicted");
        assert!(
            app.tabs.order.contains(&preview),
            "the preview itself is still open — it is displaced by the switch \
             the incoming document performs, not by ensure_room"
        );
        assert!(app.guard.is_none());
        assert_eq!(messages::newest_text(&app), None);
    }
}
