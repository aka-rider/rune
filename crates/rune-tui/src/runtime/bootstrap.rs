//! The startup sequence `runtime::run` executes exactly once before it ever
//! enters its main `recv` loop (split out of `runtime/mod.rs`, 500-line budget):
//! acquiring the terminal, probing the theme, wiring up every background
//! thread (input reader, `rune-db` bridge, snapshot timer), seeding the
//! initial size, the one bounded synchronous first-paint parse, and the
//! very first draw. Nothing about the sequence itself changes — this is
//! still the same steps `run` used to run inline, now reached through
//! [`bootstrap`].

use std::io;
use std::sync::mpsc;
use std::thread;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_md::element::doc::DocMachine;

use crate::app::App;
use crate::document::DocumentId;
use crate::term::Guard;

use super::{Cmd, Effects, Msg};

/// The buffer size at/over which `bootstrap` defers the first display-
/// pipeline compute to a background `Cmd` instead of running it
/// synchronously ahead of the first draw (issue #11). Chosen from measured
/// post-fix pipeline cost: every non-pathological shape measured completes
/// `sync_content` + `snapshot` comfortably under 40ms at 1 MiB (worst case
/// observed: `many_short_lines`, 31ms sync_content + 38ms snapshot on the
/// reference machine, release profile) — too fast to justify an extra frame
/// or a visible flash — while a 5 MiB document in that same shape already
/// costs several hundred ms, the regime a "still preparing" indicator earns
/// its keep in. 1 MiB is comfortably above an ordinary note or README and
/// comfortably below the sizes the issue's "multi-megabyte" title names.
const LARGE_DOC_BOOTSTRAP_BYTES: usize = 1_048_576;

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

    // Plan WP3.S5: populate `app.graphics` in this same startup block,
    // before `spawn_input_reader` — same ordering reason as the theme probe
    // immediately above (and the icon tier immediately below): decided once
    // here, never per frame, from real environment/window state that only
    // exists once a real `Guard` does. `Msg::Resize` (`runtime::apply`)
    // re-derives this on every later resize, since the reported pixel
    // dimensions can change even when the Kitty/truecolor decision itself
    // cannot.
    crate::graphics::redetect(app, &mut guard);

    // Plan WP5.S2: the icon tier is decided once, right beside the theme
    // probe above — same "one-shot, at startup, never per frame" reasoning,
    // and the pure selector itself takes these three as plain values (see
    // `theme::icons::choose`'s doc comment) so this is the one place that
    // actually reads the real process environment.
    app.icon_tier = crate::theme::icons::choose(
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
    //
    // Plan WP5.S1: the same gap applies to an image document opened before
    // this runtime ever starts (`rune-cli`'s first-positional bootstrap,
    // WP4.S8) — `app::update`'s own active-change hook never ran for it
    // either, so its decode would otherwise never be scheduled at all. A
    // no-op for a non-image document (`schedule_image_decode`'s own guard).
    {
        let mut effects = Effects::default();
        crate::highlight::schedule_highlight(app, app.active, &mut effects);
        crate::graphics::schedule_image_decode(app, app.active, &mut effects);
        crate::explorer::ensure_loaded(app, &mut effects);
        super::discharge(&mut effects, &mut guard, &tx, &mut save_handles)?;
    }

    if app.active_doc().buffer.content().len() < LARGE_DOC_BOOTSTRAP_BYTES {
        app.sync_view();

        // Inline embeds are reconciled from the post-dispatch chokepoint,
        // which only runs once a message arrives — and a user who opens a
        // file and simply looks at it generates none, so every embed stayed
        // unspawned and rendered as a blank gap forever. The same startup
        // gap the block above closes for the highlight and for an image
        // document's own decode.
        //
        // It cannot join that block: `sync_embeds` reads the parsed block
        // tree, which is empty until `sync_view` above has run. Discovery
        // therefore belongs here, after the first parse and before the loop
        // parks on `recv`. Decode replies arrive asynchronously and redraw
        // on their own.
        let mut effects = Effects::default();
        crate::graphics::sync_embeds(app, app.active, &mut effects);
        super::discharge(&mut effects, &mut guard, &tx, &mut save_handles)?;
    } else {
        // Over the threshold: the synchronous pipeline above `sync_view`
        // would run is deferred to a background `Cmd` instead, so the first
        // draw below never blocks on it. `relayout` alone (not `sync_view`)
        // sizes the viewport — needed both for the frame this draws and for
        // the wrap width the deferred compute below runs against — without
        // touching `doc.view`, which stays `None` until the reply lands;
        // `render::draw` falls back to unstyled raw buffer lines while it
        // does. Every other `sync_view` concern (focus, reveal, the search
        // bar, `doc.icons`) is a no-op until a document has a view to paint
        // a caret or a match onto, and is caught up for free by the
        // ordinary `App::sync_view` the main loop already runs after every
        // message — including the reply itself once it arrives.
        app.relayout();
        crate::messages::info(app, "Preparing a large document for display…");
        let doc = app.active_doc();
        let cmd = bootstrap_view_cmd(
            app.active,
            doc.buffer.version(),
            doc.buffer.content().to_string(),
            doc.viewport.width,
            app.icons(),
            doc.kind,
        );
        super::spawn_cmd(cmd, tx.clone(), &mut save_handles);
    }

    guard.draw(|frame| crate::render::draw(app, frame))?;

    Ok(Bootstrap {
        guard,
        tx,
        rx,
        save_handles,
    })
}

/// The large-document bootstrap compute: exactly the pipeline `Document::
/// view` runs synchronously for every ordinary message (`sync_content` ->
/// `set_width` -> `sync_cursors` -> `snapshot`), against an owned scratch
/// `Buffer`/`DocMachine` built from a snapshot of the live document's
/// content and configuration rather than a live borrow — no other shape
/// crosses the thread boundary an `FnOnce() -> Option<Msg> + Send + 'static`
/// `Cmd` requires. Replies with the finished `DocMachine` itself (already
/// self-consistent: `dirty` cleared, its cache warm) so installing the
/// reply is a plain replacement rather than a second, redundant rebuild on
/// the main thread — `dispatch::handle_bootstrap_view_ready` swaps it in
/// wholesale when the reply is still current.
fn bootstrap_view_cmd(
    id: DocumentId,
    version: u64,
    content: String,
    width: u16,
    icons: rune_md::icons::IconSet,
    kind: rune_syntax::DocumentKind,
) -> Cmd {
    Cmd::bootstrap_view(move || {
        let buf = Buffer::new(content);
        let mut machine = DocMachine::new();
        machine.set_kind(kind);
        machine.set_width(width);
        machine.set_icons(icons);
        machine.sync_content(&buf);
        machine.sync_cursors(&buf, &CursorSet::new(0));
        let view = machine.snapshot(&buf);
        Some(Msg::BootstrapViewReady {
            id,
            version,
            machine: Box::new(machine),
            view,
        })
    })
}
