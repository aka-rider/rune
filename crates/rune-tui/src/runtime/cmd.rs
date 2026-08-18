use super::Msg;

/// What kind of off-thread work a `Cmd` performs. Exists so a consumer that
/// must NOT execute certain effects (a headless driver: `QuitTimeout` sleeps
/// 2 real seconds, `ClipboardRead` forks `/usr/bin/pbpaste`) can decide by
/// inspection instead of inferring it from `App` field diffs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmdKind {
    /// `vfs.save_atomic` — the durable publish.
    Save,
    /// `/usr/bin/pbpaste`. Spawns a subprocess and reads the live OS
    /// clipboard; never run it inline.
    ClipboardRead,
    /// `vfs.rename_excl` (a rename) or `write_durable` + `rename_excl` (a
    /// draft create) for the no-store route — the no-clobber atomic
    /// publish. Off-thread, never inline in `update`.
    Rename,
    /// `vfs.read_dir` for the Explorer pane. Not a sleeping/
    /// forking `Cmd` like the three above, but still off-thread so
    /// a slow or degraded filesystem (an NFS mount, a huge directory) never
    /// blocks the main loop.
    ReadDir,
    /// `vfs.read` for a single FILE —
    /// `workspace::open_path_async`'s own off-thread read, the `ReadDir`
    /// sibling for opening (rather than listing) a path: a slow or
    /// degraded filesystem must never block the main loop just because the
    /// target happens to be a file instead of a directory.
    ReadFile,
    /// `/usr/bin/open` on an external link's URL. Spawns a
    /// subprocess; never run it inline. The session fuzzer's driver keeps
    /// only `CmdKind::Save` and drops every other `Cmd`, so this can never
    /// be spawned from a fuzz run.
    OpenExternal,
    /// A tree-sitter parse (`rune_ts::parse`) of every code region of one
    /// document that needs one, each bounded by [`super::PARSE_BUDGET`] and
    /// all of them together by [`super::PASS_BUDGET`].
    /// Off-thread: a large region's parse must never
    /// block the main loop, and grammar crashes (`ts_assert`) are
    /// architecturally avoided rather than caught —
    /// every parse is a full parse, never an incremental reparse fed a
    /// prior edit's location. The ONE sanctioned exception is a single
    /// bounded synchronous attempt at the startup document, made from
    /// `runtime::run`'s bootstrap strictly before the first draw — see
    /// `highlight::first_paint_highlight` — where nothing is on screen yet
    /// to block.
    Highlight,
    /// `vfs.read` + `rune_image::decode_still` for an image document.
    /// Off-thread: decode is CPU work and must never
    /// block the main loop.
    ImageDecode,
    /// `vfs.trash` — a blocking `NSFileManager` call, off-thread, never
    /// inline in `update`.
    Trash,
    /// `ReaderQuery::query(RecentSearches)` for the search bar's history
    /// list. Off-thread for the same reason `ReadDir` is: the
    /// reader thread's reply time is out of `update`'s control, and this
    /// must never block the main loop waiting on it.
    SearchHistory,
    /// The one-shot display-pipeline compute (`sync_content`, `set_width`,
    /// `sync_cursors`, `snapshot`) `bootstrap` defers off-thread for a
    /// document at/over its large-document threshold, so the first draw
    /// never blocks on it. Never spawned outside `bootstrap` — every later
    /// edit runs this same pipeline synchronously through the ordinary
    /// `App::sync_view` path.
    BootstrapView,
}

/// Off-thread work `update` asks the runtime to perform, spawned one
/// `std::thread` each. Returns the `Msg` to feed back once the work
/// completes, or `None` to produce nothing.
pub struct Cmd {
    kind: CmdKind,
    run: Box<dyn FnOnce() -> Option<Msg> + Send + 'static>,
}

impl Cmd {
    pub fn kind(&self) -> CmdKind {
        self.kind
    }

    pub fn run(self) -> Option<Msg> {
        (self.run)()
    }

    fn of(kind: CmdKind, run: impl FnOnce() -> Option<Msg> + Send + 'static) -> Cmd {
        Cmd {
            kind,
            run: Box::new(run),
        }
    }

    pub fn save(run: impl FnOnce() -> Option<Msg> + Send + 'static) -> Cmd {
        Self::of(CmdKind::Save, run)
    }

    pub fn clipboard_read(run: impl FnOnce() -> Option<Msg> + Send + 'static) -> Cmd {
        Self::of(CmdKind::ClipboardRead, run)
    }

    pub fn rename(run: impl FnOnce() -> Option<Msg> + Send + 'static) -> Cmd {
        Self::of(CmdKind::Rename, run)
    }

    pub fn read_dir(run: impl FnOnce() -> Option<Msg> + Send + 'static) -> Cmd {
        Self::of(CmdKind::ReadDir, run)
    }

    pub fn read_file(run: impl FnOnce() -> Option<Msg> + Send + 'static) -> Cmd {
        Self::of(CmdKind::ReadFile, run)
    }

    pub fn open_external(run: impl FnOnce() -> Option<Msg> + Send + 'static) -> Cmd {
        Self::of(CmdKind::OpenExternal, run)
    }

    pub fn highlight(run: impl FnOnce() -> Option<Msg> + Send + 'static) -> Cmd {
        Self::of(CmdKind::Highlight, run)
    }

    pub fn image_decode(run: impl FnOnce() -> Option<Msg> + Send + 'static) -> Cmd {
        Self::of(CmdKind::ImageDecode, run)
    }

    pub fn trash(run: impl FnOnce() -> Option<Msg> + Send + 'static) -> Cmd {
        Self::of(CmdKind::Trash, run)
    }

    pub fn search_history(run: impl FnOnce() -> Option<Msg> + Send + 'static) -> Cmd {
        Self::of(CmdKind::SearchHistory, run)
    }

    pub fn bootstrap_view(run: impl FnOnce() -> Option<Msg> + Send + 'static) -> Cmd {
        Self::of(CmdKind::BootstrapView, run)
    }
}
