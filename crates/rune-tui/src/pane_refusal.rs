use crate::app::App;
use crate::messages;
use crate::registry::{self, Availability, CommandId};

pub(crate) fn registry_refusal(app: &App, id: CommandId) -> Option<String> {
    match registry::availability(app, id) {
        Availability::Available => None,
        Availability::Unavailable(reason) => Some(reason.into_owned()),
    }
}

pub(crate) fn report_refusal(app: &mut App, reason: impl Into<String>) {
    messages::warn(app, reason);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::Arc;

    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;

    use crate::app::App;
    use crate::keymap::{KeyCode, KeyInput, Mods};
    use crate::messages::{self, Severity};
    use crate::read_only::ReadOnly;
    use crate::runtime::Effects;

    fn app() -> App {
        let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
        app.frame = Some(crate::app::FrameSize::new(100, 30));
        app
    }

    fn press(app: &mut App, code: KeyCode, mods: Mods) {
        crate::dispatch::handle_key(app, KeyInput { code, mods }, &mut Effects::default());
    }

    #[test]
    fn a_command_refused_from_the_palette_and_one_from_a_global_key_both_post_a_warning() {
        let ctrl = Mods {
            ctrl: true,
            ..Mods::NONE
        };

        let mut from_palette = app();
        press(&mut from_palette, KeyCode::Char('p'), ctrl);
        for c in "uppercase".chars() {
            press(&mut from_palette, KeyCode::Char(c), Mods::NONE);
        }
        let top = from_palette
            .palette()
            .and_then(|state| state.rows.first())
            .expect("test setup: the palette lists rows")
            .id;
        assert_eq!(
            crate::registry::spec(top).map(|s| s.name),
            Some("uppercase"),
            "test setup: the query must put the uppercase row under the cursor"
        );
        from_palette.active_doc_mut().read_only = ReadOnly::Always;
        press(&mut from_palette, KeyCode::Enter, Mods::NONE);

        let mut from_key = app();
        press(&mut from_key, KeyCode::Char('m'), ctrl);

        for (route, app) in [("palette", &from_palette), ("global key", &from_key)] {
            assert_eq!(
                messages::newest(app).map(|m| m.severity),
                Some(Severity::Warn),
                "a refusal from the {route} must be a warning"
            );
        }
    }
}
