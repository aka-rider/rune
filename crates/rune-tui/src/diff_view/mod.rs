use std::ops::Range;
use std::time::Duration;

use rune_core::assert_invariant;
use rune_core::buffer::Buffer;
use rune_core::coords::DisplayRow;
use rune_merge::{AlignmentMap, RegionKind};

use crate::app::App;
use crate::document::{Document, DocumentId, ReadOnly};

pub mod keys;
pub mod rows;

const INTRALINE_BUDGET: Duration = Duration::from_millis(4);

#[derive(Debug, PartialEq, Eq)]
pub enum DiffInstallError {
    InvalidUtf8,
}

pub struct DiffView {
    pub left: Document,
    pub left_name: String,
    pub right: DocumentId,
    pub hunk_cur: usize,
    pub alignment: AlignmentMap,
    pub intraline_left: Vec<Range<usize>>,
    pub intraline_right: Vec<Range<usize>>,
    right_version: u64,
}

pub fn install(
    app: &mut App,
    left_bytes: Vec<u8>,
    left_name: String,
) -> Result<(), DiffInstallError> {
    let text = String::from_utf8(left_bytes).map_err(|_| DiffInstallError::InvalidUtf8)?;
    install_text(app, app.active, text, left_name);
    Ok(())
}

pub(crate) fn install_text(app: &mut App, right: DocumentId, text: String, left_name: String) {
    let Some(right_doc) = app.doc(right) else {
        return;
    };
    let right_content = right_doc.buffer.content().to_string();
    let right_version = right_doc.buffer.version();
    let mut left = Document::new(Buffer::new(&text));
    left.read_only = ReadOnly::Always;
    left.focused = false;
    left.display_name = Some(left_name.clone());
    let alignment = rune_merge::align(left.buffer.content(), &right_content);
    app.diff = Some(DiffView {
        left,
        left_name,
        right,
        hunk_cur: 0,
        alignment,
        intraline_left: Vec::new(),
        intraline_right: Vec::new(),
        right_version,
    });
}

pub(crate) fn teardown(app: &mut App, right: DocumentId) {
    if app.diff.as_ref().is_some_and(|d| d.right == right) {
        app.diff = None;
    }
}

fn row_range_intersects(scroll: usize, height: usize, start: usize, len: usize) -> bool {
    len > 0 && start < scroll + height && scroll < start + len
}

fn append_global_ranges(
    out: &mut Vec<Range<usize>>,
    text: &str,
    base: usize,
    lines: &[rune_merge::LineSpans],
) {
    let line_starts = rune_merge::line_starts(text);
    for span in lines {
        let Some(&line_start) = line_starts.get(span.line) else {
            assert_invariant!(false, || {
                format!("intraline line {} out of range for region text", span.line)
            });
            continue;
        };
        for range in &span.ranges {
            out.push(base + line_start + range.start..base + line_start + range.end);
        }
    }
}

pub fn sync(app: &mut App) {
    let Some(right_id) = app.diff.as_ref().map(|d| d.right) else {
        return;
    };
    if app.active != right_id {
        return;
    }
    let Some(right_doc) = app.doc(right_id) else {
        return;
    };
    let right_scroll = right_doc.viewport.scroll_row;
    let right_height = right_doc.viewport.height as usize;
    let right_version = right_doc.buffer.version();
    let right_content = right_doc.buffer.content().to_string();
    let right_heights = right_doc
        .view
        .as_ref()
        .map(|v| rows::line_heights(&v.wrap))
        .unwrap_or_default();
    let deadline = Some(app.clock.now() + INTRALINE_BUDGET);
    let area = ratatui::layout::Rect::new(0, 0, app.frame_width, app.frame_height);
    let folded = crate::layout::geometry(area, app).diff_left.is_none();

    let Some(diff) = app.diff.as_mut() else {
        return;
    };
    let view = diff.left.sync();
    diff.left.view = Some(view);

    if diff.right_version != right_version {
        diff.alignment = rune_merge::align(diff.left.buffer.content(), &right_content);
        diff.right_version = right_version;
    }

    let left_heights = diff
        .left
        .view
        .as_ref()
        .map(|v| rows::line_heights(&v.wrap))
        .unwrap_or_default();
    let layout = rows::layout_rows(&diff.alignment, &left_heights, &right_heights);
    diff.left.viewport.scroll_row = if folded {
        let plan = rows::plan_fold(&layout, right_scroll.0, right_height);
        let left_scroll = plan
            .iter()
            .find_map(|slot| match slot {
                rows::FoldSlot::LeftVirtual(idx) => Some(*idx),
                rows::FoldSlot::Right(_) => None,
            })
            .unwrap_or(0);
        DisplayRow(left_scroll)
    } else {
        DisplayRow(rows::left_row_for_right_row(&layout, right_scroll.0))
    };

    let total_rows = diff
        .left
        .view
        .as_ref()
        .map_or(0, |v| v.display.total_rows());
    diff.left.viewport.clamp_to_document(total_rows);

    let left_scroll = diff.left.viewport.scroll_row.0;
    let left_height = diff.left.viewport.height as usize;
    let left_content = diff.left.buffer.content().to_string();

    diff.intraline_left.clear();
    diff.intraline_right.clear();
    for (region, region_rows) in diff.alignment.regions.iter().zip(layout.regions.iter()) {
        if region.kind != RegionKind::Changed {
            continue;
        }
        let visible = row_range_intersects(
            left_scroll,
            left_height,
            region_rows.left_start,
            region_rows.left_rows,
        ) || row_range_intersects(
            right_scroll.0,
            right_height,
            region_rows.right_start,
            region_rows.right_rows,
        );
        if !visible {
            continue;
        }
        let (left_base, left_text) = rows::region_text(&left_content, region.left_lines.clone());
        let (right_base, right_text) =
            rows::region_text(&right_content, region.right_lines.clone());
        let spans = rune_merge::intraline(&left_text, &right_text, deadline);
        append_global_ranges(&mut diff.intraline_left, &left_text, left_base, &spans.left);
        append_global_ranges(
            &mut diff.intraline_right,
            &right_text,
            right_base,
            &spans.right,
        );
    }
}
