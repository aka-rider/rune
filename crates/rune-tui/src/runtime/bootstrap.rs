use std::io;
use std::sync::mpsc;
use std::thread;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_md::element::doc::DocMachine;

use super::effects::{RedrawLatch, Sink};
use super::pool::Pool;
use super::transmit_queue::TransmitQueue;
use crate::app::App;
use crate::document::DocumentId;
use crate::term::Guard;

use super::{Cmd, Effects, Msg};

const LARGE_DOC_BOOTSTRAP_BYTES: usize = 1_048_576;

pub(crate) struct Bootstrap {
    pub sink: Sink,
    pub tx: mpsc::Sender<Msg>,
    pub rx: mpsc::Receiver<Msg>,
    pub save_handles: Vec<thread::JoinHandle<()>>,
}

pub(crate) fn bootstrap(app: &mut App) -> io::Result<Bootstrap> {
    let (tx, rx) = mpsc::channel::<Msg>();

    let mut sink = Sink {
        guard: Guard::new()?,
        transmits: TransmitQueue::default(),
        redraw: RedrawLatch::default(),
        pool: Pool::new(super::pool::size(), tx.clone()),
    };

    app.theme = crate::theme::Theme::catppuccin_mocha(!crate::theme::probe::supports_truecolor());

    crate::graphics::redetect(app, &mut sink.guard);

    app.icon_tier = crate::theme::icons::choose(
        std::env::var("RUNE_ICONS").ok().as_deref(),
        std::env::var("TERM_PROGRAM").ok().as_deref(),
        std::env::var("TERM").ok().as_deref(),
    );

    super::spawn_input_reader(sink.guard.event_reader(), tx.clone());

    if let Some(db) = &app.db {
        db.bridge.attach(tx.clone());
    }

    app.timers.attach(tx.clone());
    let mut save_handles: Vec<thread::JoinHandle<()>> = Vec::new();

    let (width, height) = sink.guard.size()?;
    super::apply(
        app,
        Msg::Resize(width, height),
        &mut sink,
        &tx,
        &mut save_handles,
    )?;

    crate::highlight::first_paint_highlight(app);

    {
        let mut effects = Effects::default();
        crate::highlight::schedule_highlight(app, app.active, &mut effects);
        crate::graphics::schedule_image_decode(app, app.active, &mut effects);
        crate::explorer::ensure_loaded(app, &mut effects);
        super::discharge(&mut effects, &mut sink, &tx, &mut save_handles)?;
    }

    if app.active_doc().buffer.content().len() < LARGE_DOC_BOOTSTRAP_BYTES {
        app.sync_view();

        let mut effects = Effects::default();
        crate::graphics::sync_embeds(app, app.active, &mut effects);
        super::discharge(&mut effects, &mut sink, &tx, &mut save_handles)?;
    } else {
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
        super::spawn_cmd(cmd, tx.clone(), &mut save_handles, &sink.pool);
    }

    sink.redraw_before_draw();
    sink.guard.draw(|frame| crate::render::draw(app, frame))?;

    Ok(Bootstrap {
        sink,
        tx,
        rx,
        save_handles,
    })
}

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
