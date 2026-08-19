#![allow(dead_code)]

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::{self, App};
use rune_tui::diff_view;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pointer::{MouseInput, MouseKind};
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::Mem;

pub const HEIGHT: u16 = 24;
pub const WIDE_ENOUGH: u16 = 83;
pub const TOO_NARROW: u16 = 82;

pub fn app_with_diff(right_text: &str, left_text: &str, width: u16) -> App {
    let mut app = App::new(Buffer::new(right_text), None, Arc::new(Mem::new()), None);
    diff_view::install(
        &mut app,
        left_text.as_bytes().to_vec(),
        "fileA.md".to_string(),
    )
    .expect("fixture is valid UTF-8");
    app.frame_width = width;
    app.frame_height = HEIGHT;
    app.sync_view();
    app
}

pub fn key(c: char) -> KeyInput {
    KeyInput {
        code: KeyCode::Char(c),
        mods: Mods::NONE,
    }
}

pub fn char_slice_from(row: &str, start: usize, width: usize) -> String {
    row.chars().skip(start).take(width).collect()
}

pub fn col_of(row: &str, needle: &str) -> u16 {
    let byte_idx = row.find(needle).expect("needle present in row");
    row[..byte_idx].chars().count() as u16
}

pub fn col_of_right_pane(row: &str, needle: &str) -> u16 {
    let byte_idx = row.rfind(needle).expect("needle present in row");
    row[..byte_idx].chars().count() as u16
}

pub fn sup_shift(c: char) -> KeyInput {
    KeyInput {
        code: KeyCode::Char(c),
        mods: Mods {
            shift: true,
            alt: false,
            ctrl: false,
            sup: true,
        },
    }
}

pub fn ctrl(c: char) -> KeyInput {
    KeyInput {
        code: KeyCode::Char(c),
        mods: Mods {
            shift: false,
            alt: false,
            ctrl: true,
            sup: false,
        },
    }
}

pub fn send(app: &mut App, kind: MouseKind, col: u16, row: u16) {
    let mut effects = Effects::default();
    app::update(
        app,
        Msg::Mouse(MouseInput {
            kind,
            column: col,
            row,
            shift: false,
            alt: false,
            ctrl: false,
        }),
        &mut effects,
    );
    app.sync_view();
}

pub fn geo(app: &App, width: u16) -> rune_tui::layout::Geometry {
    rune_tui::layout::geometry(ratatui::layout::Rect::new(0, 0, width, HEIGHT), app)
}

pub fn row_strings(buf: &ratatui::buffer::Buffer, w: u16, h: u16) -> Vec<String> {
    (0..h)
        .map(|y| {
            let mut s = String::new();
            for x in 0..w {
                if let Some(cell) = buf.cell((x, y)) {
                    s.push_str(cell.symbol());
                }
            }
            s
        })
        .collect()
}
