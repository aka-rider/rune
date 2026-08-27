mod graphics;
mod replica;
mod save_state;
mod sync;
#[cfg(test)]
mod tests;

pub use crate::read_only::ReadOnly;
pub(crate) use replica::{Replica, ReplicaStep};
pub(crate) use save_state::PublishParams;
pub use save_state::{SaveCapture, SavePhase, SaveTicket};
use save_state::{SaveState, SaveTicketMint};

use std::num::NonZeroU64;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use rune_core::buffer::{Buffer, Edit, SortedEdits, clamp_to_char_boundary};
use rune_core::coords::{BufferOffset, VisualCol};
use rune_core::cursor::{Cursor, CursorSet};
use rune_core::undo::{EditKind, Journal, Step};
use rune_md::element::doc::{DocMachine, ViewSnapshots};
use rune_md::icons::IconSet;
use rune_syntax::DocumentKind;
use rune_syntax::element::ByteRange;

use crate::db::DocDb;
pub use crate::document_support::Hydration;
use crate::document_support::{is_suspicious_shrink, kind_for};
use crate::highlight::HighlightState;
use crate::undogroup::Direction;
use crate::viewport::Viewport;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DocumentId(pub(crate) NonZeroU64);

pub struct Document {
    pub buffer: Buffer,
    pub cursors: CursorSet,
    pub doc: DocMachine,
    pub viewport: Viewport,
    pub focused: bool,
    pub reveal_engaged: bool,
    pub journal: Journal,
    pub(crate) ladder_presses: usize,
    pub(crate) ladder_pressed_at: Option<Instant>,
    pub(crate) ladder_direction: Option<Direction>,
    pub(crate) ladder_anchor: Option<usize>,
    pub read_only: ReadOnly,
    pub pinned: bool,
    pub file_path: Option<PathBuf>,
    pub saved_version: u64,
    pub(crate) saved_content: Arc<str>,
    save: SaveState,
    next_save_ticket: SaveTicketMint,
    pub view: Option<ViewSnapshots>,
    pub(crate) replica: Replica,
    pub display_name: Option<String>,
    pub catalogue: Vec<rune_nav::Ref>,
    pub reading_link_focus: Option<ByteRange>,
    pub kind: DocumentKind,
    pub kind_pinned: bool,
    pub icons: IconSet,
    pub highlight: HighlightState,
    graphics: crate::graphics::Graphics,
    pub last_sync: Option<rune_db::SyncKind>,
    pub nlink: Option<i64>,
}

impl Document {
    pub fn is_read_only(&self) -> bool {
        !matches!(self.read_only, ReadOnly::No)
    }

    pub fn is_preview(&self) -> bool {
        matches!(self.read_only, ReadOnly::Preview)
    }

    pub fn has_insertion_point(&self) -> bool {
        self.focused && !self.is_read_only()
    }

    pub fn reveals_under_cursor(&self) -> bool {
        self.reveal_engaged && !self.is_read_only()
    }

    pub fn shows_selection(&self) -> bool {
        self.focused
    }

    pub fn has_reloadable_graphics(&self) -> bool {
        self.image().is_some()
            || self
                .embeds()
                .is_some_and(super::graphics::EmbedSet::has_wedged)
    }

    pub fn save_in_flight(&self) -> bool {
        !self.save.is_idle()
    }

    pub fn doc_db(&self) -> Option<&DocDb> {
        self.replica.doc_db()
    }

    pub fn doc_db_mut(&mut self) -> Option<&mut DocDb> {
        self.replica.doc_db_mut()
    }

    pub fn is_store_bound(&self) -> bool {
        self.replica.is_bound()
    }

    #[cfg(any(test, feature = "testgrid"))]
    pub fn set_doc_db_for_test(&mut self, db: DocDb) {
        self.replica = Replica::Bound(db);
    }
}

impl Document {
    pub fn new(buffer: Buffer) -> Document {
        let saved_version = buffer.version();
        let saved_content: Arc<str> = Arc::from(buffer.content());
        Document {
            buffer,
            cursors: CursorSet::new(0),
            doc: DocMachine::new(),
            viewport: Viewport::default(),
            focused: true,
            reveal_engaged: true,
            journal: Journal::new(),
            ladder_presses: 0,
            ladder_pressed_at: None,
            ladder_direction: None,
            ladder_anchor: None,
            read_only: ReadOnly::No,
            file_path: None,
            saved_version,
            saved_content,
            save: SaveState::default(),
            next_save_ticket: SaveTicketMint::default(),
            view: None,
            replica: Replica::Detached,
            display_name: None,
            catalogue: Vec::new(),
            reading_link_focus: None,
            kind: DocumentKind::Markdown,
            kind_pinned: false,
            icons: IconSet::unicode(),
            highlight: HighlightState::default(),
            graphics: crate::graphics::Graphics::None,
            last_sync: None,
            nlink: None,
            pinned: false,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.buffer.version() != self.saved_version && self.buffer.content() != &*self.saved_content
    }

    pub fn begin_save(&mut self, version: u64, content: Arc<str>) -> SaveTicket {
        let ticket = self.next_save_ticket.mint();
        self.save.begin_direct(ticket, version, content);
        ticket
    }

    pub fn save_phase(&self) -> SavePhase {
        self.save.phase()
    }

    pub(crate) fn begin_prepare(
        &mut self,
        version: u64,
        content: Arc<str>,
        params: PublishParams,
        prep_op: u64,
    ) -> SaveTicket {
        let ticket = self.next_save_ticket.mint();
        self.save
            .begin_prepare(ticket, version, content, params, prep_op);
        ticket
    }

    pub(crate) fn prep_op(&self) -> Option<u64> {
        self.save.prep_op()
    }

    pub(crate) fn preparing_mode(&self) -> Option<crate::save::SaveMode> {
        self.save.preparing_mode()
    }

    pub fn save_ticket(&self) -> Option<SaveTicket> {
        self.save.ticket()
    }

    pub(crate) fn begin_publishing(&mut self) -> Option<(SaveTicket, Arc<str>, PublishParams)> {
        self.save
            .advance_to_publishing()
            .map(|(ticket, capture, params)| (ticket, capture.content, params))
    }

    pub(crate) fn is_publishing(&self) -> bool {
        self.save.is_publishing()
    }

    #[must_use]
    pub(crate) fn begin_recording(&mut self, record_op: u64, published: bool) -> bool {
        self.save.advance_to_recording(record_op, published)
    }

    pub fn record_op(&self) -> Option<u64> {
        self.save.record_op()
    }

    pub fn bind_target(&self) -> Option<&PathBuf> {
        self.save.bind_target()
    }

    pub(crate) fn take_bind_target(&mut self) -> Option<PathBuf> {
        self.save.take_bind_target()
    }

    pub fn finish_save_ok(&mut self, version: u64) -> bool {
        let Some(capture) = self.save.resolve() else {
            return false;
        };
        if capture.version == version && capture.version > self.saved_version {
            self.saved_version = capture.version;
            self.saved_content = capture.content;
            true
        } else {
            false
        }
    }

    pub fn abandon_save(&mut self) {
        self.save.resolve();
    }

    pub(crate) fn adopt_saved(&mut self, version: u64, content: Arc<str>) {
        self.saved_version = version;
        self.saved_content = content;
    }

    pub fn pending_save_version(&self) -> Option<u64> {
        self.save.pending_version()
    }

    pub fn hydrate(
        &mut self,
        disk_content: &str,
        recovered: &str,
        journaled_cursors: &[Cursor],
    ) -> Hydration {
        if recovered == disk_content {
            return Hydration::NoChange;
        }
        if is_suspicious_shrink(disk_content, recovered) {
            return Hydration::Refused(
                "recovered draft looked truncated relative to the file on disk — kept the on-disk version",
            );
        }
        let edit = SortedEdits::single(Edit {
            start: 0,
            end: self.buffer.len(),
            insert: recovered.to_string(),
        });
        let Ok((new_buffer, applied)) = self.buffer.apply_edits(&edit) else {
            return Hydration::Refused("recovered draft failed to apply to the buffer");
        };
        let cursors_before = self.cursors.all().to_vec();
        let seat = |c: Cursor| {
            let position = clamp_to_char_boundary(recovered, c.position.get());
            let anchor = clamp_to_char_boundary(recovered, c.anchor.get());
            Cursor {
                position: BufferOffset(position),
                anchor: BufferOffset(anchor),
                desired_col: if position == c.position.get() {
                    c.desired_col
                } else {
                    VisualCol(0)
                },
                ..c
            }
        };
        self.cursors = if journaled_cursors.is_empty() {
            self.cursors.map(seat)
        } else {
            let seated: Vec<Cursor> = journaled_cursors.iter().copied().map(seat).collect();
            CursorSet::new_from(&seated)
        };
        self.buffer = new_buffer;
        let cursors_after = self.cursors.all().to_vec();
        self.journal.push(Step {
            edits: applied,
            cursors_before,
            cursors_after,
            kind: EditKind::Other,
        });
        Hydration::Adopted
    }

    pub fn file_name(&self) -> &str {
        if let Some(name) = self.display_name.as_deref() {
            return name;
        }
        self.file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("[No Name]")
    }

    pub fn bind_path(&mut self, path: PathBuf) {
        if !self.kind_pinned {
            let kind = kind_for(Some(&path), self.buffer.content());
            self.kind = kind;
            self.doc.set_kind(self.kind);
        }
        self.file_path = Some(path);
        self.display_name = None;
    }
}
