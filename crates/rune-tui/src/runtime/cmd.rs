use super::Msg;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmdKind {
    Save,
    ClipboardRead,
    Rename,
    ReadDir,
    ReadFile,
    OpenExternal,
    Highlight,
    ImageDecode,
    ImageEncode,
    Trash,
    SearchHistory,
    BootstrapView,
    ProjectIndex,
    ProjectQuery,
}

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

    pub fn image_encode(run: impl FnOnce() -> Option<Msg> + Send + 'static) -> Cmd {
        Self::of(CmdKind::ImageEncode, run)
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

    pub fn project_index(run: impl FnOnce() -> Option<Msg> + Send + 'static) -> Cmd {
        Self::of(CmdKind::ProjectIndex, run)
    }

    pub fn project_query(run: impl FnOnce() -> Option<Msg> + Send + 'static) -> Cmd {
        Self::of(CmdKind::ProjectQuery, run)
    }
}
