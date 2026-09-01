use std::fmt;

mod lineindex;
mod trailing;

use lineindex::LineStarts;
pub use lineindex::line_starts;
pub use trailing::trailing_whitespace_edits;

pub(crate) fn check_char_boundary(content: &str, offset: usize) -> Result<(), BufferError> {
    if content.is_char_boundary(offset) {
        Ok(())
    } else {
        Err(BufferError::SplitsRune { offset })
    }
}

pub(crate) fn snap_char_boundary(content: &str, offset: usize) -> usize {
    content.floor_char_boundary(offset)
}

pub fn clamp_to_char_boundary(content: &str, offset: usize) -> usize {
    snap_char_boundary(content, offset.min(content.len()))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub insert: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppliedEdit {
    pub start: usize,
    pub end: usize,
    pub deleted: String,
    pub insert: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BufferError {
    InvalidUtf8,
    EditsNotSortedOrOverlapping,
    OutOfBounds {
        start: usize,
        end: usize,
        len: usize,
    },
    SplitsRune {
        offset: usize,
    },
    DuplicateEditStart {
        start: usize,
    },
}

impl fmt::Display for BufferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BufferError::InvalidUtf8 => write!(f, "invalid UTF-8 sequence"),
            BufferError::EditsNotSortedOrOverlapping => {
                write!(f, "edits must be non-overlapping and sorted descending")
            }
            BufferError::OutOfBounds { start, end, len } => {
                write!(f, "edit out of bounds: [{start},{end}) len={len}")
            }
            BufferError::SplitsRune { offset } => {
                write!(f, "edit splits a rune at byte offset {offset}")
            }
            BufferError::DuplicateEditStart { start } => {
                write!(f, "two edits collide on post-edit start {start}")
            }
        }
    }
}

impl std::error::Error for BufferError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Buffer {
    content: String,
    line_starts: LineStarts,
    version: u64,
}

impl Default for Buffer {
    fn default() -> Buffer {
        Buffer::new("")
    }
}

impl Buffer {
    pub fn new(content: impl Into<String>) -> Buffer {
        let content = content.into();
        let line_starts = LineStarts::from_full(lineindex::line_starts(&content));
        Buffer {
            content,
            line_starts,
            version: 1,
        }
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<Buffer, BufferError> {
        let content = String::from_utf8(bytes).map_err(|_| BufferError::InvalidUtf8)?;
        Ok(Buffer::new(content))
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn len(&self) -> usize {
        self.content.len()
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn slice(&self, start: usize, end: usize) -> Option<&str> {
        self.content.get(start..end)
    }

    pub fn byte(&self, offset: usize) -> Option<u8> {
        self.content.as_bytes().get(offset).copied()
    }

    pub fn rune_at(&self, offset: usize) -> Option<(char, usize)> {
        let c = self.content.get(offset..)?.chars().next()?;
        Some((c, c.len_utf8()))
    }

    pub fn insert(&self, offset: usize, text: &str) -> Result<Buffer, BufferError> {
        self.replace(offset, offset, text)
    }

    pub fn delete(&self, start: usize, end: usize) -> Result<Buffer, BufferError> {
        self.replace(start, end, "")
    }

    pub fn replace(&self, start: usize, end: usize, text: &str) -> Result<Buffer, BufferError> {
        let edit = Edit {
            start,
            end,
            insert: text.to_string(),
        };
        let (new_buf, _) = self.apply_edits(&SortedEdits::single(edit))?;
        Ok(new_buf)
    }

    pub fn apply_edits(
        &self,
        edits: &SortedEdits,
    ) -> Result<(Buffer, Vec<AppliedEdit>), BufferError> {
        let edits = edits.as_slice();
        if edits.is_empty() {
            return Ok((self.clone(), Vec::new()));
        }

        validate_edit_batch(&self.content, edits)?;

        let (new_content, applied) = self.build_edited_content(edits)?;

        if let Some(start) = duplicate_applied_start(&applied) {
            return Err(BufferError::DuplicateEditStart { start });
        }

        let new_line_starts = self.update_line_starts(edits);

        Ok((
            Buffer {
                content: new_content,
                line_starts: new_line_starts,
                version: self.version + 1,
            },
            applied,
        ))
    }

    fn build_edited_content(
        &self,
        edits: &[Edit],
    ) -> Result<(String, Vec<AppliedEdit>), BufferError> {
        let len = self.content.len();

        let net_change: isize = edits
            .iter()
            .map(|e| edit_delta(e.end - e.start, e.insert.len()))
            .sum();
        let cap = len.saturating_add_signed(net_change);
        let mut new_content = String::with_capacity(cap);

        // Precompute each edit's cumulative shift, scanning right-to-left
        // (descending `start` order matches array order here).
        let mut shifts = vec![0isize; edits.len()];
        let mut current_shift: isize = 0;
        for (e, slot) in edits.iter().zip(shifts.iter_mut()).rev() {
            *slot = current_shift;
            current_shift += edit_delta(e.end - e.start, e.insert.len());
        }

        let mut applied: Vec<AppliedEdit> = Vec::with_capacity(edits.len());
        applied.resize_with(edits.len(), AppliedEdit::default);

        // Walk left-to-right (ascending `start`) to build the new content,
        // which is why this loop also runs in reverse over the
        // descending-sorted `edits` array.
        let mut last_end = 0usize;
        for (i, (e, shift)) in edits.iter().zip(&shifts).enumerate().rev() {
            let prefix = self
                .content
                .get(last_end..e.start)
                .ok_or(BufferError::OutOfBounds {
                    start: last_end,
                    end: e.start,
                    len,
                })?;
            new_content.push_str(prefix);
            new_content.push_str(&e.insert);
            last_end = e.end;

            let start = e.start.saturating_add_signed(*shift);
            let deleted = self
                .content
                .get(e.start..e.end)
                .ok_or(BufferError::OutOfBounds {
                    start: e.start,
                    end: e.end,
                    len,
                })?
                .to_string();
            if let Some(slot) = applied.get_mut(i) {
                *slot = AppliedEdit {
                    start,
                    end: start + e.insert.len(),
                    deleted,
                    insert: e.insert.clone(),
                };
            }
        }
        let tail = self
            .content
            .get(last_end..)
            .ok_or(BufferError::OutOfBounds {
                start: last_end,
                end: len,
                len,
            })?;
        new_content.push_str(tail);

        Ok((new_content, applied))
    }
}

fn validate_edit_batch(content: &str, edits: &[Edit]) -> Result<(), BufferError> {
    let len = content.len();
    for e in edits {
        if e.end > len || e.start > e.end {
            return Err(BufferError::OutOfBounds {
                start: e.start,
                end: e.end,
                len,
            });
        }
        check_char_boundary(content, e.start)?;
        check_char_boundary(content, e.end)?;
    }
    Ok(())
}

pub fn edit_delta(deleted_len: usize, insert_len: usize) -> isize {
    insert_len as isize - deleted_len as isize
}

pub(crate) fn duplicate_applied_start(applied: &[AppliedEdit]) -> Option<usize> {
    let mut starts: Vec<usize> = applied.iter().map(|a| a.start).collect();
    starts.sort_unstable();
    starts
        .windows(2)
        .find(|w| w.first() == w.get(1))
        .and_then(|w| w.first().copied())
}

fn is_sorted_descending_non_overlapping(edits: &[Edit]) -> bool {
    edits.windows(2).all(|w| match (w.first(), w.get(1)) {
        (Some(a), Some(b)) => a.start >= b.end,
        _ => true,
    })
}

fn clone_and_sort_edits_descending(edits: &[Edit]) -> Vec<Edit> {
    let mut cloned = edits.to_vec();
    cloned.sort_by(|a, b| b.start.cmp(&a.start).then(b.end.cmp(&a.end)));
    cloned
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SortedEdits(Vec<Edit>);

impl SortedEdits {
    pub fn sort(edits: &[Edit]) -> SortedEdits {
        SortedEdits(clone_and_sort_edits_descending(edits))
    }

    pub fn single(edit: Edit) -> SortedEdits {
        SortedEdits(vec![edit])
    }

    pub fn validate(edits: Vec<Edit>) -> Result<SortedEdits, BufferError> {
        if is_sorted_descending_non_overlapping(&edits) {
            Ok(SortedEdits(edits))
        } else {
            Err(BufferError::EditsNotSortedOrOverlapping)
        }
    }

    pub fn as_slice(&self) -> &[Edit] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

#[cfg(test)]
mod tests;
