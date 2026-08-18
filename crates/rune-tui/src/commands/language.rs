use rune_syntax::DocumentKind;

use crate::app::App;
use crate::document::DocumentId;
use crate::document_support::kind_for;
use crate::highlight::{self, HighlightState};
use crate::palette::args::LanguageChoice;
use crate::runtime::Effects;

pub fn set_language(app: &mut App, id: DocumentId, choice: LanguageChoice, effects: &mut Effects) {
    let Some(doc) = app.doc_mut(id) else { return };
    let kind = match choice {
        LanguageChoice::Auto => {
            doc.kind_pinned = false;
            kind_for(doc.file_path.as_deref(), doc.buffer.content())
        }
        LanguageChoice::Markdown => {
            doc.kind_pinned = true;
            DocumentKind::Markdown
        }
        LanguageChoice::Plain => {
            doc.kind_pinned = true;
            DocumentKind::Plain
        }
        LanguageChoice::Lang(lang) => {
            doc.kind_pinned = true;
            DocumentKind::Code(lang)
        }
    };
    doc.kind = kind;
    doc.doc.set_kind(kind);
    doc.doc.invalidate();
    doc.highlight = HighlightState::default();
    highlight::schedule_highlight(app, id, effects);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use rune_core::buffer::Buffer;
    use rune_syntax::LangId;
    use rune_vfs::Mem;

    use super::*;

    fn app_with(content: &str) -> App {
        let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
        let id = app.active;
        app.doc_mut(id)
            .expect("fixture doc must exist")
            .viewport
            .set_size(80, 23);
        app
    }

    #[test]
    fn switching_to_a_language_reparses_at_the_same_buffer_version() {
        let mut app = app_with("fn main() {}\n");
        let id = app.active;
        let before_version = app.doc(id).unwrap().buffer.version();
        let rust = LangId::from_name("rust").expect("rust is a known language");
        let mut effects = Effects::default();

        set_language(&mut app, id, LanguageChoice::Lang(rust), &mut effects);

        let doc = app.doc(id).unwrap();
        assert_eq!(doc.buffer.version(), before_version);
        assert_eq!(doc.kind, DocumentKind::Code(rust));
        assert!(doc.kind_pinned);
        assert_eq!(doc.doc.built_version(), doc.buffer.version());
    }

    #[test]
    fn auto_restores_path_derived_detection() {
        let mut app = app_with("fn main() {}\n");
        let id = app.active;
        app.doc_mut(id).unwrap().bind_path(PathBuf::from("main.rs"));
        let rust = app.doc(id).unwrap().kind;
        assert_eq!(rust, DocumentKind::Code(LangId::from_name("rust").unwrap()));

        let mut effects = Effects::default();
        set_language(&mut app, id, LanguageChoice::Markdown, &mut effects);
        assert_eq!(app.doc(id).unwrap().kind, DocumentKind::Markdown);
        assert!(app.doc(id).unwrap().kind_pinned);

        set_language(&mut app, id, LanguageChoice::Auto, &mut effects);
        let doc = app.doc(id).unwrap();
        assert!(!doc.kind_pinned);
        assert_eq!(doc.kind, rust);
    }

    #[test]
    fn a_pinned_kind_survives_a_rebind() {
        let mut app = app_with("plain text\n");
        let id = app.active;
        let mut effects = Effects::default();
        set_language(&mut app, id, LanguageChoice::Plain, &mut effects);

        app.doc_mut(id)
            .unwrap()
            .bind_path(PathBuf::from("notes.md"));

        assert_eq!(app.doc(id).unwrap().kind, DocumentKind::Plain);
    }
}
