//! The working-form builder (plan WP3.S4): frames one conflict hunk with
//! git-style markers, and lays out a whole `rune_merge::Hunk` sequence into
//! one buffer plus the `Block`/`Conflict` bookkeeping merge mode navigates.

use rune_merge::Hunk;

use super::state::{Block, Conflict, ConflictBlock};

/// The `[B4]` UTF-8 refusal: at least one hunk's bytes are not valid UTF-8.
/// A unit-shaped error — the caller has exactly one thing to do with
/// it: refuse with a fixed status message; no variant to distinguish.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MergeUtf8Error;

/// Frames a block to the EXACT shape the offset math merge navigation and
/// resync depend on: unconditional `\n` after both `ours` and `theirs`,
/// regardless of whether either already ends in one. Decision 5 (editor/disk labels, not
/// ours/theirs) is what `[B]oth` leaves standing verbatim.
pub fn frame_block(ours: &str, theirs: &str) -> String {
    format!("<<<<<<< editor\n{ours}\n=======\n{theirs}\n>>>>>>> disk\n")
}

/// Lays `hunks` out into one buffer: a `Hunk::Clean` region is copied
/// verbatim; a `Hunk::Conflict` is framed via [`frame_block`] and recorded
/// as one `Block` (its byte range in the OUTPUT buffer) paired with one
/// `Conflict` (its original ours/theirs text). `Err` is the `[B4]` UTF-8
/// refusal: `rune-merge` stays byte-typed so BOM/CRLF round-trip on bytes,
/// but the buffer this feeds is a `String` — any hunk byte sequence that
/// isn't valid UTF-8 refuses the WHOLE build rather than silently losing or
/// replacing bytes.
pub fn build_marker_buffer(hunks: &[Hunk]) -> Result<(String, Vec<ConflictBlock>), MergeUtf8Error> {
    let mut buffer = String::new();
    let mut pairs = Vec::new();

    for hunk in hunks {
        match hunk {
            Hunk::Clean(bytes) => {
                let text = String::from_utf8(bytes.clone()).map_err(|_| MergeUtf8Error)?;
                buffer.push_str(&text);
            }
            Hunk::Conflict { ours, theirs } => {
                let ours = String::from_utf8(ours.clone()).map_err(|_| MergeUtf8Error)?;
                let theirs = String::from_utf8(theirs.clone()).map_err(|_| MergeUtf8Error)?;
                let start = buffer.len();
                buffer.push_str(&frame_block(&ours, &theirs));
                let end = buffer.len();
                pairs.push(ConflictBlock {
                    block: Block {
                        range: start..end,
                        resolved: false,
                    },
                    conflict: Conflict { ours, theirs },
                });
            }
        }
    }

    Ok((buffer, pairs))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn frame_block_matches_the_exact_marker_shape() {
        let framed = frame_block("mine", "theirs");
        assert_eq!(
            framed,
            "<<<<<<< editor\nmine\n=======\ntheirs\n>>>>>>> disk\n"
        );
    }

    #[test]
    fn build_marker_buffer_interleaves_clean_regions_and_frames_conflicts() {
        let hunks = vec![
            Hunk::Clean(b"before\n".to_vec()),
            Hunk::Conflict {
                ours: b"mine".to_vec(),
                theirs: b"theirs".to_vec(),
            },
            Hunk::Clean(b"after\n".to_vec()),
        ];
        let (buffer, pairs) = build_marker_buffer(&hunks).expect("valid utf8");
        assert_eq!(
            buffer,
            "before\n<<<<<<< editor\nmine\n=======\ntheirs\n>>>>>>> disk\nafter\n"
        );
        assert_eq!(pairs.len(), 1);
        assert_eq!(
            &buffer[pairs[0].block.range.clone()],
            frame_block("mine", "theirs").as_str()
        );
        assert_eq!(pairs[0].conflict.ours, "mine");
        assert_eq!(pairs[0].conflict.theirs, "theirs");
    }

    #[test]
    fn build_marker_buffer_refuses_non_utf8_hunk_bytes() {
        let hunks = vec![Hunk::Clean(vec![0xff, 0xfe])];
        assert!(build_marker_buffer(&hunks).is_err());
    }
}
