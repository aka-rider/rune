use unicode_segmentation::UnicodeSegmentation;

use crate::app::App;
use crate::binding::{Binding, KeyPattern, resolve_in};
use crate::clipboard::pbpaste_cmd;
use crate::keymap::{self, Command, KeyCode, KeyInput, KeyOutcome, Mods};
use crate::listnav::ListCommand;
use crate::queryline;
use crate::registry::{self, ArgKind, Availability, CommandId, ExecOutcome};
use crate::runtime::{Effects, PasteTarget};

use super::{PaletteMode, close, recompute, row_capacity};

const SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: false,
    sup: false,
};

pub(crate) type PaletteKeyCommand = ListCommand;

pub(crate) const PALETTE_BINDINGS: &[Binding<PaletteKeyCommand>] = &[
    Binding {
        key: KeyPattern::printable(Mods::NONE),
        cmd: PaletteKeyCommand::Type,
        help: "type to filter",
        secondary: false,
    },
    Binding {
        key: KeyPattern::printable(SHIFT),
        cmd: PaletteKeyCommand::Type,
        help: "type to filter",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Backspace, Mods::NONE),
        cmd: PaletteKeyCommand::Erase,
        help: "erase",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Up, Mods::NONE),
        cmd: PaletteKeyCommand::Up,
        help: "up",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Down, Mods::NONE),
        cmd: PaletteKeyCommand::Down,
        help: "down",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::PageUp, Mods::NONE),
        cmd: PaletteKeyCommand::PageUp,
        help: "page up",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::PageDown, Mods::NONE),
        cmd: PaletteKeyCommand::PageDown,
        help: "page down",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Home, Mods::NONE),
        cmd: PaletteKeyCommand::Top,
        help: "top",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::End, Mods::NONE),
        cmd: PaletteKeyCommand::Bottom,
        help: "bottom",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Enter, Mods::NONE),
        cmd: PaletteKeyCommand::Enter,
        help: "run",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Tab, Mods::NONE),
        cmd: PaletteKeyCommand::Tab,
        help: "accept",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Escape, Mods::NONE),
        cmd: PaletteKeyCommand::Cancel,
        help: "cancel",
        secondary: false,
    },
];

pub(crate) fn handle_key(app: &mut App, key: KeyInput, effects: &mut Effects) -> KeyOutcome {
    if keymap::resolve(key) == Some(Command::Paste) {
        effects.cmds.push(pbpaste_cmd(PasteTarget::Palette));
        return KeyOutcome::Consumed;
    }
    if let Some(cmd) = resolve_in(PALETTE_BINDINGS, key) {
        apply(app, cmd, key, effects);
    }
    KeyOutcome::Consumed
}

fn apply(app: &mut App, cmd: PaletteKeyCommand, key: KeyInput, effects: &mut Effects) {
    match cmd {
        PaletteKeyCommand::Type => {
            if let KeyCode::Char(c) = key.code {
                type_char(app, c);
            }
        }
        PaletteKeyCommand::Erase => erase(app),
        PaletteKeyCommand::Up => nav_move(app, -1),
        PaletteKeyCommand::Down => nav_move(app, 1),
        PaletteKeyCommand::PageUp => nav_move(app, -page_amount(app)),
        PaletteKeyCommand::PageDown => nav_move(app, page_amount(app)),
        PaletteKeyCommand::Top => nav_edge(app, true),
        PaletteKeyCommand::Bottom => nav_edge(app, false),
        PaletteKeyCommand::Tab => tab(app),
        PaletteKeyCommand::Enter => enter(app, effects),
        PaletteKeyCommand::Cancel => close(app),
    }
}

fn type_char(app: &mut App, c: char) {
    let Some(state) = app.palette_mut() else {
        return;
    };
    let window = 0..state.field.len();
    let mut buf = [0u8; 4];
    let inserted = c.encode_utf8(&mut buf);
    let _ = state.field.insert(inserted, window);
    recompute(app);
}

fn erase(app: &mut App) {
    let Some(state) = app.palette_mut() else {
        return;
    };
    if let Some((byte_idx, _)) = state.field.text().grapheme_indices(true).next_back() {
        let end = state.field.len();
        let _ = state.field.delete_range(byte_idx..end);
    }
    if let PaletteMode::Param { cmd_end, .. } = state.mode
        && state.field.len() < cmd_end
    {
        state.mode = PaletteMode::Name;
    }
    recompute(app);
}

fn set_refusal(app: &mut App, reason: String) {
    if let Some(state) = app.palette_mut() {
        state.refusal = Some(reason);
    }
}

fn tab(app: &mut App) {
    let Some(state) = app.palette() else { return };
    match state.mode {
        PaletteMode::Name => {
            let Some(row) = state.rows.get(state.nav.cursor) else {
                set_refusal(app, "no matching command".to_string());
                return;
            };
            let id = row.id;
            if let Availability::Unavailable(reason) = &row.availability {
                let reason = reason.to_string();
                set_refusal(app, reason);
                return;
            }
            complete_name(app, id);
        }
        PaletteMode::Param { cmd, cmd_end } => {
            if let Availability::Unavailable(reason) = registry::availability(app, cmd) {
                set_refusal(app, reason.to_string());
                return;
            }
            let Some(row) = state.arg_rows.get(state.nav.cursor) else {
                set_refusal(app, "no matching argument".to_string());
                return;
            };
            let label = row.label.clone();
            complete_arg(app, cmd_end, &label);
        }
    }
}

fn complete_name(app: &mut App, id: CommandId) {
    let Some(spec) = registry::spec(id) else {
        set_refusal(app, "no matching command".to_string());
        return;
    };
    let name = spec.name.to_string();
    let arg = spec.arg;
    let Some(state) = app.palette_mut() else {
        return;
    };
    if arg == ArgKind::None {
        state.field.set_text(&name);
        recompute(app);
        return;
    }
    let prefix = format!("{name} ");
    let cmd_end = prefix.len();
    state.field.set_text(&prefix);
    state.mode = PaletteMode::Param { cmd: id, cmd_end };
    recompute(app);
}

fn complete_arg(app: &mut App, cmd_end: usize, label: &str) {
    let Some(state) = app.palette_mut() else {
        return;
    };
    let prefix = state.field.text().get(..cmd_end).unwrap_or("").to_string();
    let text = format!("{prefix}{label}");
    state.field.set_text(&text);
    recompute(app);
}

pub(crate) fn paste(app: &mut App, text: &str) {
    let sanitized = queryline::sanitize_pasted_line(text);
    if sanitized.is_empty() {
        return;
    }
    let Some(state) = app.palette_mut() else {
        return;
    };
    let window = 0..state.field.len();
    let _ = state.field.insert(&sanitized, window);
    recompute(app);
}

pub(crate) fn nav_move(app: &mut App, delta: isize) {
    let height = row_capacity(app).max(1);
    let Some(state) = app.palette_mut() else {
        return;
    };
    let len = state.active_len();
    state.nav.move_and_follow(delta, len, height);
}

/// A left-click on a palette row: lands the cursor there and runs it —
/// the mouse equivalent of arrowing to a row and pressing Enter. `absolute`
/// is already resolved against the current scroll window by the caller
/// (`commands::mouse`'s own hit test), so this only has to move the cursor
/// and hand off to the exact same `enter` this module's own Enter binding
/// uses — one execution path for both input devices.
pub(crate) fn click_row(app: &mut App, absolute: usize, effects: &mut Effects) {
    let Some(state) = app.palette_mut() else {
        return;
    };
    state.nav.cursor = absolute;
    enter(app, effects);
}

/// A left-drag over the palette's row list: moves the selection to whatever
/// row is under the pointer, without running it — `absolute` is already
/// resolved by the caller exactly like [`click_row`]'s.
pub(crate) fn drag_hover(app: &mut App, absolute: usize) {
    if let Some(state) = app.palette_mut() {
        state.nav.cursor = absolute;
    }
}

fn nav_edge(app: &mut App, top: bool) {
    let Some(state) = app.palette_mut() else {
        return;
    };
    let len = state.active_len();
    state.nav.jump_to_edge(len, top);
}

fn page_amount(app: &App) -> isize {
    row_capacity(app).max(1) as isize
}

fn enter(app: &mut App, effects: &mut Effects) {
    let Some(state) = app.palette() else {
        return;
    };
    match state.mode {
        PaletteMode::Name => enter_name(app, effects),
        PaletteMode::Param { cmd, .. } => enter_param(app, cmd, effects),
    }
}

fn enter_name(app: &mut App, effects: &mut Effects) {
    let Some(state) = app.palette() else {
        return;
    };
    let Some(row) = state.rows.get(state.nav.cursor) else {
        set_refusal(app, "no matching command".to_string());
        return;
    };
    let id = row.id;
    if let Availability::Unavailable(reason) = &row.availability {
        let reason = reason.to_string();
        set_refusal(app, reason);
        return;
    }
    let Some(spec) = registry::spec(id) else {
        set_refusal(app, "no matching command".to_string());
        return;
    };
    if spec.arg != ArgKind::None {
        complete_name(app, id);
        return;
    }
    let name = spec.name.to_string();
    close(app);
    run_and_persist(app, id, None, &name, effects);
}

fn enter_param(app: &mut App, cmd: CommandId, effects: &mut Effects) {
    let Some(state) = app.palette() else {
        return;
    };
    let Some(row) = state.arg_rows.get(state.nav.cursor) else {
        set_refusal(app, "no matching argument".to_string());
        return;
    };
    let resolved = row.resolved.clone();
    if let Availability::Unavailable(reason) = registry::availability(app, cmd) {
        set_refusal(app, reason.to_string());
        return;
    }
    let Some(spec) = registry::spec(cmd) else {
        set_refusal(app, "no matching command".to_string());
        return;
    };
    let name = spec.name.to_string();
    close(app);
    run_and_persist(app, cmd, Some(resolved), &name, effects);
}

fn run_and_persist(
    app: &mut App,
    id: CommandId,
    arg: Option<registry::ResolvedArg>,
    name: &str,
    effects: &mut Effects,
) {
    match registry::execute(app, id, arg, effects) {
        ExecOutcome::Done => persist_command(app, name),
        ExecOutcome::Refused(reason) => crate::messages::error(app, reason),
    }
}

fn persist_command(app: &mut App, name: &str) {
    let result = app.command_history.touch(app.db.as_ref(), name, |db| {
        db.store.touch_command_name(name)
    });
    if let Some(Err(e)) = result {
        crate::messages::error(app, format!("command history not saved: {e}"));
    }
}
