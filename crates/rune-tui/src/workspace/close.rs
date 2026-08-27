use crate::app::App;
use crate::document::{Document, DocumentId};
use crate::guard::{self, GuardKind, GuardPrompt};
use crate::runtime::Effects;

#[must_use]
pub enum CloseOutcome {
    Closed,
    Unknown,
}

pub fn request_close(app: &mut App, id: DocumentId, effects: &mut Effects) {
    if app.doc(id).is_none() {
        return;
    }
    if app.refuse_if_preview(id) {
        return;
    }
    if app.doc(id).is_some_and(Document::is_dirty) {
        let _ = guard::set_guard_or_warn(
            app,
            GuardPrompt {
                doc: id,
                kind: GuardKind::DirtyClose,
            },
            "close confirmation dropped \u{2014} a prompt is already showing",
            effects,
        );
    } else if app
        .doc(id)
        .is_some_and(crate::document::Document::save_in_flight)
    {
        app.pending_close_on_save = Some(id);
        crate::messages::info(app, "save in progress \u{2014} closing once it completes");
    } else {
        let _ = close_now(app, id, effects);
    }
}

pub fn new_untitled_document(app: &mut App) -> DocumentId {
    let name = next_untitled_name(app);
    let id = app.open_document(rune_core::buffer::Buffer::new(""));
    if let Some(doc) = app.doc_mut(id) {
        doc.display_name = Some(name);
    }
    super::switch_to(app, id);
    crate::db_enqueue::create_scratch(app, id);
    id
}

pub fn next_untitled_name(app: &App) -> String {
    format!("Untitled {}", next_untitled_number(app))
}

fn next_untitled_number(app: &App) -> usize {
    app.documents
        .values()
        .filter_map(|doc| doc.display_name.as_deref())
        .filter_map(|name| name.strip_prefix("Untitled ")?.parse::<usize>().ok())
        .max()
        .unwrap_or(0)
        + 1
}

pub fn close_now(app: &mut App, id: DocumentId, effects: &mut Effects) -> CloseOutcome {
    if !app.documents.contains_key(&id) {
        return CloseOutcome::Unknown;
    }
    if app.merge.doc() == Some(id) {
        crate::merge::auto_exit(app);
    }
    crate::diff_view::teardown(app, id);
    let image_info = app
        .doc(id)
        .and_then(|d| d.image())
        .map(|image| (image.id.get(), image.path.to_string_lossy().into_owned()));
    if let Some((kitty_id, key)) = image_info {
        if app.graphics.kitty {
            effects.write(rune_image::encode_delete(kitty_id).into_bytes());
        }
        app.image_ids.free_all_for(&key);
    }
    let mut active_changed = false;
    if app.documents.len() == 1 {
        new_untitled_document(app);
        active_changed = true;
    } else if app.active == id
        && let Some(neighbor) = neighbor_of(app, id)
    {
        app.active = neighbor;
        active_changed = true;
    }
    app.documents.remove(&id);
    app.db_ops.retain(|_, pending| pending.doc != id);
    if app.pending_close_on_save == Some(id) {
        app.pending_close_on_save = None;
    }
    if app.pending_save_confirm.is_some_and(|(cid, _)| cid == id) {
        app.pending_save_confirm = None;
    }
    crate::materialize_ack::retire_quit_wait(app, id);
    crate::rename::forget_document(app, id);

    if active_changed {
        let name = crate::title::name_for(app.active_doc());
        app.title.seed(&name);
        app.documents.touch(app.active);
    }

    app.tabs.nav.cursor = app
        .documents
        .order()
        .iter()
        .position(|&t| t == app.active)
        .unwrap_or(0);
    CloseOutcome::Closed
}

pub(crate) fn neighbor_of(app: &App, id: DocumentId) -> Option<DocumentId> {
    let order = app.documents.order();
    if let Some(idx) = order.iter().position(|&t| t == id) {
        if let Some(&next) = order.get(idx + 1) {
            return Some(next);
        }
        if idx > 0 {
            return order.get(idx - 1).copied();
        }
    }
    app.documents.keys().find(|&&k| k != id).copied()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use rune_core::buffer::Buffer;
    use rune_vfs::{Mem, Vfs, VfsTestExt};

    use crate::document::ReadOnly;

    use super::*;
    use crate::app::App;

    const X_PNG: &[u8] = include_bytes!("../../../../testdata/assets/x.png");

    #[test]
    fn closing_an_image_document_emits_encode_delete_when_kitty_is_on() {
        let mem = Arc::new(Mem::new());
        mem.save_atomic(Path::new("/vault/x.png"), X_PNG)
            .expect("seed x.png");
        let vfs: Arc<dyn Vfs + Send + Sync> = mem;
        let mut app = App::new(Buffer::new(""), None, vfs, None);
        app.graphics.kitty = true;
        let image_id =
            crate::workspace::open_path(&mut app, Path::new("/vault/x.png")).expect("open");
        let expected_id = app.doc(image_id).unwrap().image().unwrap().id;

        let mut effects = Effects::default();
        let _ = close_now(&mut app, image_id, &mut effects);

        assert_eq!(effects.raw_bytes().len(), 1);
        assert_eq!(
            effects.raw_bytes()[0],
            rune_image::encode_delete(expected_id.get()).into_bytes()
        );
    }

    #[test]
    fn closing_an_image_document_emits_nothing_when_kitty_is_off() {
        let mem = Arc::new(Mem::new());
        mem.save_atomic(Path::new("/vault/x.png"), X_PNG)
            .expect("seed x.png");
        let vfs: Arc<dyn Vfs + Send + Sync> = mem;
        let mut app = App::new(Buffer::new(""), None, vfs, None);
        app.graphics.kitty = false;
        let image_id =
            crate::workspace::open_path(&mut app, Path::new("/vault/x.png")).expect("open");

        let mut effects = Effects::default();
        let _ = close_now(&mut app, image_id, &mut effects);

        assert!(effects.raw_bytes().is_empty());
    }

    #[test]
    fn closing_a_non_image_document_emits_nothing() {
        let mem = Arc::new(Mem::new());
        let vfs: Arc<dyn Vfs + Send + Sync> = mem;
        let mut app = App::new(Buffer::new("hello"), None, vfs, None);
        app.graphics.kitty = true;
        let extra = app.open_document(Buffer::new("second"));

        let mut effects = Effects::default();
        let _ = close_now(&mut app, extra, &mut effects);

        assert!(effects.raw_bytes().is_empty());
    }

    #[test]
    fn closing_the_only_document_mints_a_fresh_untitled_instead_of_refusing() {
        let mem = Arc::new(Mem::new());
        let vfs: Arc<dyn Vfs + Send + Sync> = mem;
        let mut app = App::new(Buffer::new("hello"), None, vfs, None);
        let only = app.active;

        let mut effects = Effects::default();
        let outcome = close_now(&mut app, only, &mut effects);

        assert!(matches!(outcome, CloseOutcome::Closed));
        assert_eq!(app.documents.len(), 1);
        assert!(!app.documents.contains_key(&only));
        assert_eq!(app.active_doc().display_name.as_deref(), Some("Untitled 1"));
        assert!(crate::messages::newest_text(&app).is_none());
    }

    #[test]
    fn closing_a_clean_document_with_a_save_in_flight_defers_until_the_ack() {
        let mem = Arc::new(Mem::new());
        let vfs: Arc<dyn Vfs + Send + Sync> = mem;
        let mut app = App::new(Buffer::new("hello"), None, vfs, None);
        let id = app.active;
        let extra = app.open_document(Buffer::new("second"));
        let (version, content) = {
            let doc = app.doc(id).unwrap();
            (doc.buffer.version(), Arc::from(doc.buffer.content()))
        };
        let ticket = app.doc_mut(id).unwrap().begin_save(version, content);

        let mut effects = Effects::default();
        request_close(&mut app, id, &mut effects);

        assert!(
            app.doc(id).is_some(),
            "the close must wait for the save's ack"
        );
        assert_eq!(app.pending_close_on_save, Some(id));
        assert_eq!(
            crate::messages::newest_text(&app),
            Some("save in progress \u{2014} closing once it completes")
        );

        crate::app::update(
            &mut app,
            crate::runtime::Msg::SaveDone {
                id,
                ticket,
                version,
                result: Ok(()),
                detail: crate::runtime::SaveOutcomeDetail {
                    durable: true,
                    stray_temp: None,
                    race: None,
                },
            },
            &mut effects,
        );

        assert!(app.doc(id).is_none(), "the ack completes the close");
        assert!(app.pending_close_on_save.is_none());
        assert!(app.doc(extra).is_some());
    }

    #[test]
    fn request_close_refuses_a_preview_document() {
        let mem = Arc::new(Mem::new());
        let vfs: Arc<dyn Vfs + Send + Sync> = mem;
        let mut app = App::new(Buffer::new("hello"), None, vfs, None);
        let id = app.active;
        app.doc_mut(id).unwrap().read_only = ReadOnly::Preview;

        let mut effects = Effects::default();
        request_close(&mut app, id, &mut effects);

        assert!(
            app.documents.contains_key(&id),
            "a preview document must not be closed"
        );
        assert_eq!(app.active, id, "active must stay on the refused document");
        assert_eq!(
            crate::messages::newest_text(&app),
            ReadOnly::Preview.refusal_message()
        );
    }
}
