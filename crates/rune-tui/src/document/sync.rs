use rune_core::coords::{BufferOffset, DisplayRow, WrapPoint, WrapRow};
use rune_core::cursor::Cursor;
use rune_md::element::doc::ViewSnapshots;

use super::Document;
use crate::viewport::ScrollMode;

impl Document {
    pub fn view(&mut self) -> ViewSnapshots {
        self.doc.set_reveal_mode(self.reveals_under_cursor().into());
        self.sync_catalogue();
        self.doc.set_icons(self.icons.clone());
        self.doc.set_width(self.viewport.width);
        if let Some(image) = self.image() {
            let (width, rows) = match &image.status {
                crate::graphics::ImageStatus::Live { cells, .. } => (cells.cols, cells.rows),
                _ => (0, crate::render::image::INFO_CARD_ROWS),
            };
            self.doc.set_image_document_dims(width, rows);
        }
        let embed_dims = self
            .embeds()
            .map(crate::graphics::EmbedSet::to_image_dims)
            .unwrap_or_default();
        self.doc.set_embed_dims(embed_dims);
        let reveal_offsets = self.reveal_probe_offsets();
        self.doc
            .sync_cursors(&self.buffer, &self.cursors, &reveal_offsets);
        self.doc.snapshot(&self.buffer)
    }

    fn reveal_probe_offsets(&self) -> Vec<usize> {
        let mut offsets = self.search_reveal_offsets.clone();
        if self.has_insertion_point() {
            let content = self.buffer.content();
            for (open, close) in rune_core::bracket::cursor_bracket_pairs(content, &self.cursors) {
                offsets.push(open);
                offsets.push(close);
            }
        }
        offsets
    }

    pub fn sync_catalogue(&mut self) {
        let built_before = self.doc.built_version();
        self.doc.sync_content(&self.buffer);
        if self.doc.built_version() != built_before {
            self.catalogue =
                rune_md::catalogue::catalogue(self.buffer.content(), self.doc.blocks());
        }
    }

    pub fn scroll_to_cursor(&mut self, view: &ViewSnapshots) {
        if self.is_read_only() {
            if self.viewport.mode != ScrollMode::EnsureVisible {
                self.viewport.clamp_to_document(view.display.total_rows());
                return;
            }
            self.viewport.mode = ScrollMode::FollowCursor;
        }
        let display_row = self.cursor_display_row(view);
        if let Some(target_row) = self
            .viewport
            .reconcile(display_row, view.display.total_rows())
        {
            let wrap_row = view.display.display_to_wrap(target_row);
            self.snap_cursor_to_row(view, wrap_row.0);
        }
    }

    fn cursor_display_row(&self, view: &ViewSnapshots) -> DisplayRow {
        let primary = self.cursors.primary();
        let buffer_point = self.buffer.offset_to_line_col(primary.position.get());
        let syntax_point = view.syntax.buffer_to_syntax(buffer_point);
        let wrap_point = view.wrap.syntax_to_wrap(syntax_point);
        view.display.wrap_to_display(WrapRow(wrap_point.row))
    }

    fn snap_cursor_to_row(&mut self, view: &ViewSnapshots, row: usize) {
        let primary = self.cursors.primary();
        let col = view
            .wrap
            .byte_col_from_visual(self.buffer.content(), row, primary.desired_col.0);
        let syntax_point = view
            .wrap
            .wrap_to_syntax(self.buffer.content(), WrapPoint { row, col });
        let buffer_point = view.syntax.syntax_to_buffer(syntax_point);
        let offset = self.buffer.line_col_to_offset(buffer_point);
        let snapped = Cursor {
            position: BufferOffset(offset),
            anchor: BufferOffset(offset),
            desired_col: primary.desired_col,
            id: primary.id,
        };
        self.cursors = self.cursors.collapse_to(snapped);
    }

    pub fn sync(&mut self) -> ViewSnapshots {
        let view = self.view();
        self.scroll_to_cursor(&view);
        let settled = self.view();
        self.scroll_to_cursor(&settled);
        self.view()
    }
}
