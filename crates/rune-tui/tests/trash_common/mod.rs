//! Shared setup helpers for the `⌘⌫`/`^⌫` trash suite, split across
//! `trash.rs` (the guard raise/cancel and every refusal — dirty, pathless,
//! directory) and `trash_reply.rs` (the confirm's `Cmd` enqueue and
//! `Msg::TrashDone`'s close/keep-open/error/stale-generation branches,
//! including the async A4 dirty-at-reply and guard-at-reply cases, and the
//! inherited exact-path-match limitation) — this is the 500-line-budget
//! split of the original `trash.rs`. Each file pulls this in via
//! `mod trash_common;` — integration test files are separate binaries, so
//! this is the one place both draw an identical fixture from, rather than
//! risking the two drifting apart.
#![allow(dead_code)]

use std::sync::Arc;

use rune_fuzz::Session;
use rune_tui::app;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Effects, Msg};

use rune_vfs::Mem;

use crate::explorer_common;

/// [`explorer_common::open_seeded`], with the store stripped back out
/// (`App::db` back to `None`) — the trash flow always takes the no-store
/// `Cmd` route (plan: rune-db gets no purge path), so a document must stay
/// unbound for `Msg::SaveDone`'s no-store fallback (Assumption A1) to stay
/// reachable through this suite.
pub fn app_with(mem: &Arc<Mem>) -> Session {
    let mut session = explorer_common::open_seeded(mem);
    if let Some(db) = session.app_mut().db.take() {
        db.shutdown();
    }
    session
}

pub fn select_row(session: &mut Session, name: &str) {
    let idx = session
        .app()
        .explorer
        .entries
        .iter()
        .position(|e| e.name == name)
        .unwrap_or_else(|| panic!("{name} is listed"));
    session.app_mut().explorer.nav.cursor = idx;
}

pub fn send(session: &mut Session, msg: Msg) -> Effects {
    let mut effects = Effects::default();
    app::update(session.app_mut(), msg, &mut effects);
    effects
}

pub fn key(code: KeyCode, mods: Mods) -> Msg {
    Msg::Key(KeyInput { code, mods })
}

pub fn sup_backspace() -> Msg {
    key(
        KeyCode::Backspace,
        Mods {
            sup: true,
            ..Mods::NONE
        },
    )
}

pub fn escape() -> Msg {
    key(KeyCode::Escape, Mods::NONE)
}

pub fn yes() -> Msg {
    key(KeyCode::Char('y'), Mods::NONE)
}
