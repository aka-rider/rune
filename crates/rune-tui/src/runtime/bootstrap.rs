//! The startup sequence `runtime::run` executes exactly once before it ever
//! enters its main `recv` loop (split out of `runtime/mod.rs`, §1.6 budget):
//! acquiring the terminal, probing the theme, wiring up every background
//! thread (input reader, `rune-db` bridge, snapshot timer), seeding the
//! initial size, the one bounded synchronous first-paint parse, and the
//! very first draw. Nothing about the sequence itself changes — this is
//! still the same steps `run` used to run inline, now reached through
//! [`bootstrap`].

use std::io;
use std::sync::mpsc;
use std::thread;

use crate::app::App;
use crate::term::Guard;

use super::{Effects, Msg};

/// Everything `runtime::run`'s main loop needs once bootstrap has finished:
/// the terminal guard, the message channel's two ends, and the in-flight
/// no-store fallback save handles [`super::spawn_cmd`] has started tracking.
pub(crate) struct Bootstrap {
    pub guard: Guard,
    pub tx: mpsc::Sender<Msg>,
    pub rx: mpsc::Receiver<Msg>,
    pub save_handles: Vec<thread::JoinHandle<()>>,
}

/// Runs the startup sequence `runtime::run` used to run inline at the top of
/// its own body — see this module's doc comment for the full step list.
/// Ends immediately after the very first draw, so `run`'s main loop always
/// starts from an already-rendered frame.
pub(crate) fn bootstrap(app: &mut App) -> io::Result<Bootstrap> {
    let mut guard = Guard::new()?;

    // Plan WP4.S5: probe BEFORE `spawn_input_reader` starts consuming
    // events on its own thread — the probe's own poll/read round trip
    // over the DA1 query would otherwise race that thread for the same
    // input stream (the "typed Csi response" it waits for could be
    // delivered to either reader). One-shot, at startup, never per frame.
    app.theme = crate::theme::Theme::catppuccin_mocha(!guard.probe_truecolor());

    // Plan WP5.S2: the icon tier is decided once, right beside the theme
    // probe above — same "one-shot, at startup, never per frame" reasoning,
    // and the pure selector itself takes these three as plain values (see
    // `theme::icons::choose`'s doc comment) so this is the one place that
    // actually reads the real process environment.
    app.icons = crate::theme::icons::choose(
        std::env::var("RUNE_ICONS").ok().as_deref(),
        std::env::var("TERM_PROGRAM").ok().as_deref(),
        std::env::var("TERM").ok().as_deref(),
    );

    let (tx, rx) = mpsc::channel::<Msg>();
    super::spawn_input_reader(guard.event_reader(), tx.clone());

    // Hand the runtime's own `Sender<Msg>` to the DB bridge (plan WP5.S1's
    // "App-held setter" — `Store::open`, at bootstrap in `rune-cli::main`,
    // ran before this `Sender<Msg>` ever existed, see `db::DbBridge`'s doc
    // comment) so every `DbEvent` from here on is delivered as `Msg::Db`
    // through the ordinary Elm loop below, exactly like the initial
    // `Msg::Resize` seed right after it.
    if let Some(db) = &app.db {
        db.bridge.attach(tx.clone());
    }

    // Same "App-held setter" pattern as `db.bridge.attach` right above:
    // `App::new` constructs `snapshot_timer` with no background thread at
    // all (plan WP16.S5), so every test/fuzz `App` that never reaches this
    // `run` loop never spawns one either. This call starts its one thread.
    app.snapshot_timer.attach(tx.clone());
    // Tracks the join handle of every no-store fallback save `Cmd`
    // (`CmdKind::Save`) currently running, so quitting can join them instead
    // of detaching them mid-write (review fix, [rune-tui A 5]): a
    // store-backed materialize survives process exit via `Store::shutdown`'s
    // own drain, but the no-store fallback is a plain detached thread doing
    // its own `vfs.save_atomic` — `thread::spawn`'s `JoinHandle`, dropped,
    // detaches and keeps running past `main` returning, but the atomic
    // publish it's mid-write on has no other guarantee of finishing before
    // the process actually exits. Pruned opportunistically so this never
    // grows unbounded across a long session of saves.
    let mut save_handles: Vec<thread::JoinHandle<()>> = Vec::new();

    // Seed the initial size through the ordinary `update` path (not a
    // one-off field write) so `Msg::Resize`'s effect on the viewport has
    // exactly one implementation, exercised the same way on every resize.
    let (width, height) = guard.size()?;
    super::apply(
        app,
        Msg::Resize(width, height),
        &mut guard,
        &tx,
        &mut save_handles,
    )?;

    // D4 (syntax-highlighting-latency plan): one bounded synchronous parse
    // attempt at the startup document, strictly before the first draw below
    // — nothing is on screen yet, so even a full-budget miss blocks nothing
    // visible. A hit means frame 1 renders already highlighted; a miss (or
    // a non-code startup document) falls through to the ordinary background
    // kick right after, unchanged.
    crate::highlight::first_paint_highlight(app);

    // Plan WP5.S3, "App::new's bootstrap path": `App::new` itself has no
    // `&mut Effects` to dispatch a highlight `Cmd` with (it runs before this
    // runtime, and before any `Msg` has ever reached `app::update`'s
    // before/after gate), so the document it opened with never gets its
    // first highlight kicked from there. This is the earliest point that
    // both an `App` and an `Effects` sink exist together, so it is the one
    // explicit kick this bootstrap path needs; every later document (an
    // edit, a tab switch, `workspace::open_path`) is already covered by
    // `app::update`'s own before/after gate. When `first_paint_highlight`
    // just succeeded for this document, `schedule_highlight`'s own
    // already-current guard (`highlight.version == version`) makes this a
    // no-op — see that function's doc comment.
    // Same bootstrap-window reasoning as the highlight kick above, for the
    // same reason: a launch with no file to edit shows the left column
    // before any key is pressed, but the constructor had no `Effects` to
    // request the listing with, so without this the pane would render as an
    // empty box until the user pressed the focus chord. A no-op whenever the
    // column starts hidden or the Explorer already has entries.
    {
        let mut effects = Effects::default();
        crate::highlight::schedule_highlight(app, app.active, &mut effects);
        crate::explorer::ensure_loaded(app, &mut effects);
        for cmd in effects.cmds.drain(..) {
            super::spawn_cmd(cmd, tx.clone(), &mut save_handles);
        }
    }

    app.sync_view();
    guard.draw(|frame| crate::render::draw(app, frame))?;

    Ok(Bootstrap {
        guard,
        tx,
        rx,
        save_handles,
    })
}
