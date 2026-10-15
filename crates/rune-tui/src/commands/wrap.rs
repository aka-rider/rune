use rune_syntax::wrap::WrapMode;

use crate::app::App;
use crate::messages;

pub fn toggle(app: &mut App) {
    let message = match app.active_doc_mut().viewport.toggle_wrap() {
        WrapMode::Soft => "Word wrap: on",
        WrapMode::Off => "Word wrap: off",
    };
    messages::info(app, message);
}
