use std::io;
use std::path::PathBuf;

use rune_vfs::DirEntry;

use crate::document::DocumentId;
use crate::highlight::PassOutcome;
use crate::keymap::KeyInput;
use crate::pointer::MouseInput;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteTarget {
    Document(DocumentId),
    Title(DocumentId),
    Search,
    Palette,
}

#[derive(Debug)]
pub enum CmdError {
    Io(io::Error),
    Get(rune_vfs::GetRefusal),
    Db(rune_db::Error),
    Image(rune_image::ImageError),
    Refused(String),
}

impl std::fmt::Display for CmdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CmdError::Io(e) => write!(f, "{e}"),
            CmdError::Get(e) => write!(f, "{e}"),
            CmdError::Db(e) => write!(f, "{e}"),
            CmdError::Image(e) => write!(f, "{e}"),
            CmdError::Refused(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for CmdError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CmdError::Io(e) => Some(e),
            CmdError::Get(e) => Some(e),
            CmdError::Db(e) => Some(e),
            CmdError::Image(e) => Some(e),
            CmdError::Refused(_) => None,
        }
    }
}

impl From<io::Error> for CmdError {
    fn from(e: io::Error) -> Self {
        CmdError::Io(e)
    }
}

impl From<rune_vfs::GetRefusal> for CmdError {
    fn from(e: rune_vfs::GetRefusal) -> Self {
        CmdError::Get(e)
    }
}

impl From<rune_db::Error> for CmdError {
    fn from(e: rune_db::Error) -> Self {
        CmdError::Db(e)
    }
}

impl From<rune_image::ImageError> for CmdError {
    fn from(e: rune_image::ImageError) -> Self {
        CmdError::Image(e)
    }
}

// `durable` matters only when `Msg::SaveDone`'s `result` is `Ok`;
// `stray_temp`/`race` are independent, optional facts about that same
// successful publish.
#[derive(Debug)]
pub struct SaveOutcomeDetail {
    pub durable: bool,
    pub stray_temp: Option<PathBuf>,
    pub race: Option<crate::materialize_ack::SaveRace>,
}

#[derive(Debug)]
pub enum Msg {
    Key(KeyInput),
    PumpGraphics,
    Paste(String),
    Resize(u16, u16),
    KeyboardFlagsReport(termina::escape::csi::KittyKeyboardFlags),
    Mouse(MouseInput),
    ClipboardRead {
        text: String,
        target: PasteTarget,
    },
    SaveDone {
        id: DocumentId,
        ticket: crate::document::SaveTicket,
        version: u64,
        result: Result<(), CmdError>,
        detail: SaveOutcomeDetail,
    },
    Timer {
        key: crate::runtime::TimerMsgKey,
        generation: u64,
    },
    SnapshotDue {
        id: DocumentId,
        generation: u32,
    },
    Db(rune_db::DbEvent),
    MaterializeVfsDone {
        id: DocumentId,
        ticket: crate::document::SaveTicket,
        db_id: i64,
        seq: i64,
        content: std::sync::Arc<str>,
        outcome: crate::materialize_ack::MaterializeVfsOutcome,
    },
    DirLoaded {
        root: PathBuf,
        entries: Vec<DirEntry>,
        cause: DirCause,
        generation: crate::generation::DirLoadGen,
    },
    RenameDone {
        generation: crate::generation::RenameGen,
        result: Result<rune_db::RenameOutcome, CmdError>,
    },
    TrashDone {
        generation: crate::generation::TrashGen,
        path: PathBuf,
        result: Result<(), CmdError>,
    },
    FileOpened {
        path: PathBuf,
        result: Result<Vec<u8>, CmdError>,
        anchor: Option<rune_nav::Anchor>,
        preview_generation: Option<crate::generation::PreviewGen>,
    },
    Highlighted {
        doc: DocumentId,
        version: u64,
        result: PassOutcome,
    },
    BootstrapViewReady {
        id: DocumentId,
        version: u64,
        machine: Box<rune_md::element::doc::DocMachine>,
        view: rune_md::element::doc::ViewSnapshots,
    },
    ImageDecoded {
        doc: DocumentId,
        generation: crate::generation::ImageDecodeGen,
        result: Result<rune_image::decode::Decoded, CmdError>,
    },
    EmbedDecoded {
        doc: DocumentId,
        generation: u64,
        result: Result<rune_image::decode::Decoded, CmdError>,
    },
    ImageEncoded {
        doc: DocumentId,
        generation: crate::generation::ImageDecodeGen,
        was_live: bool,
        result: Result<rune_image::Transmit, CmdError>,
    },
    EmbedEncoded {
        doc: DocumentId,
        generation: u64,
        result: Result<rune_image::Transmit, CmdError>,
    },
    Posted {
        severity: crate::messages::Severity,
        text: String,
    },
    RecentsLoaded {
        generation: u64,
        result: RecentsResult,
    },
    FileSearchScanned {
        generation: crate::generation::FileSearchGen,
        result: Result<crate::filesearch::walk::ScanResult, String>,
    },
    ProjectIndexScanned {
        generation: crate::generation::ProjectIndexGen,
        result: Result<crate::filesearch::walk::ScanResult, String>,
    },
    ProjectIndexBatch {
        generation: crate::generation::ProjectIndexGen,
        outcomes: Vec<crate::projectsearch::index::ReadOutcome>,
    },
    Quit,
}

#[derive(Debug)]
pub enum RecentsResult {
    Search(Result<Vec<String>, CmdError>),
    FileSearch(Result<Vec<crate::filesearch::Candidate>, CmdError>),
    Palette(Result<Vec<String>, CmdError>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirCause {
    Nav,
    Refresh,
}

mod cmd;
pub use cmd::{Cmd, CmdKind};

mod effects;
pub use effects::{Effects, Outbound};
use effects::{Sink, discharge};

mod run_loop;
pub use run_loop::{MAX_TURN_BATCH, drain_batch, run};
use run_loop::{apply, spawn_cmd, spawn_input_reader};

mod io_cmd;
pub use io_cmd::{load_dir_cmd, load_search_history_cmd, read_file_cmd};

mod preview_cmd;
pub use preview_cmd::{MAX_PREVIEW_BYTES, read_preview_cmd};

mod bootstrap;
mod exit_settle;
mod pool;

mod transmit_queue;
pub use transmit_queue::{Pumped, TransmitQueue};

mod highlight_cmd;
#[cfg(test)]
pub(crate) use highlight_cmd::test_clock;
pub(crate) use highlight_cmd::{FIRST_PAINT_BUDGET, PassBudget, highlight_cmd, run_regions};
pub use highlight_cmd::{PARSE_BUDGET, PASS_BUDGET};

mod md_fence;

mod timer;
pub use timer::{TimerKey, TimerMsgKey, TimerService};

mod filesearch_recents_cmd;
pub use filesearch_recents_cmd::load_filesearch_recents_cmd;
mod filesearch_cmd;
pub(crate) use filesearch_cmd::filesearch_scan_cmd;
mod projectsearch_cmd;
pub(crate) use projectsearch_cmd::{project_read_batch_cmd, project_scan_cmd};

mod command_history_cmd;
pub use command_history_cmd::load_command_history_cmd;
