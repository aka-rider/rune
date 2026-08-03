//! `runtime`'s tree-sitter highlight `Cmd` constructor and the region pass
//! behind it — split out of `runtime.rs` itself (§1.6 budget). Everything
//! here reaches `rune_ts::parse`; nothing else in `rune-tui` does, with the
//! one sanctioned exception of the pre-first-draw pass, which calls
//! [`run_regions`] directly rather than through a `Cmd`.

use std::time::Duration;

use crate::document::DocumentId;
use crate::highlight::{HighlightReply, RegionJob, RegionLang, RegionPayload, RegionResult};

use super::md_fence::markdown_fence_spans;
use super::{Cmd, CmdKind, Msg};

/// The wall-clock budget ONE region's parse is allowed before it aborts and
/// reports nothing for that region — one attempt, no retry: a region
/// exceeding this stays plain but *surfaced* via the status message
/// `dispatch::handle_highlighted` sets, rather than silently uncoloured
/// forever.
///
/// Per region, undivided. It used to be split two ways: a whole document got
/// this budget while a fence got a quarter-second divided by the number of
/// fences in the document, so a four-fence document gave each 62ms and a
/// large fence rendered flat. One budget per region is affordable only
/// because trees are retained and reused — a region is reparsed when its own
/// text changed, not whenever anything in the document did. Unmeasured
/// against a real large region (see `TODO.md`).
pub const PARSE_BUDGET: Duration = Duration::from_secs(5);

/// The wall-clock budget the single synchronous, pre-first-draw pass gives
/// its attempt at the startup document's regions — small on purpose: it runs
/// on the main thread before anything is drawn, so even a miss is invisible
/// (the background `Cmd` picks the document up on the very next frame via
/// the ordinary `schedule_highlight` bootstrap kick) while a hit means frame
/// 1 is already highlighted.
pub(crate) const FIRST_PAINT_BUDGET: Duration = Duration::from_millis(20);

/// Parses every region of one document that needs it, off-thread, and
/// replies with `Msg::Highlighted`.
///
/// Owns `jobs` (moved into the closure) since the `Cmd` closure is
/// `FnOnce() -> Option<Msg> + Send + 'static` and cannot borrow the
/// document's buffer across the thread boundary — which is why each job
/// carries its own already-reconstructed source text. Always replies with
/// `Some(Msg::Highlighted { .. })`, even when nothing parsed, so `in_flight`
/// is guaranteed to clear on the UI thread; `rune_ts::parse` itself never
/// panics (§1.3: it surfaces a failed language load or query compile as
/// `None`, never `ts_assert`'s `SIGABRT`, since every parse is a full parse
/// — no incremental-reparse edit is ever fed back into it).
pub(crate) fn highlight_cmd(doc: DocumentId, version: u64, jobs: Vec<RegionJob>) -> Cmd {
    Cmd::new(CmdKind::Highlight, move || {
        let result = run_regions(jobs, PARSE_BUDGET);
        Some(Msg::Highlighted {
            doc,
            version,
            result,
        })
    })
}

/// The region pass itself, shared by the background `Cmd` above and the
/// pre-first-draw pass — the same work at two budgets, so the two can never
/// drift apart on what a region's highlight means.
///
/// Every job contributes one `RegionResult`, in order, carrying its map
/// whether or not it produced a payload: the reply describes the document's
/// whole region LAYOUT, and a region whose parse overran `budget` still
/// needs its offsets refreshed. A job with no work carries its retained
/// tree's slot forward untouched.
///
/// `None` iff not one region produced anything `[R2]` — a document where
/// every parse overran must not flash to unstyled.
pub(crate) fn run_regions(jobs: Vec<RegionJob>, budget: Duration) -> Option<HighlightReply> {
    let mut regions = Vec::with_capacity(jobs.len());
    let mut any_parsed = false;
    // One region hitting the span cap truncates the whole reply: the
    // document's colours are incomplete either way, and the flag says so.
    let mut truncated = false;
    for job in jobs {
        let payload = match job.work {
            None => None,
            Some((RegionLang::Ts(lang), text)) => rune_ts::parse(lang, &text, budget).map(|tree| {
                any_parsed = true;
                RegionPayload::Tree(tree)
            }),
            // A ```markdown fence has no grammar; it reuses the comrak
            // emitter instead. That is a bounded parse of the fence's own
            // (typically short) text, so it never needs the timeout — but it
            // produces spans rather than a tree, which is why the span
            // channel exists at all. Mapped back to buffer coordinates here,
            // where the map is still at hand, so everything downstream sees
            // one coordinate space.
            Some((RegionLang::Markdown, text)) => {
                any_parsed = true;
                let fence = markdown_fence_spans(&text);
                truncated |= fence.truncated;
                Some(RegionPayload::Spans(
                    fence
                        .spans
                        .into_iter()
                        .filter_map(|(range, scope)| job.map.to_buffer(range).map(|r| (r, scope)))
                        .collect(),
                ))
            }
        };
        regions.push(RegionResult {
            map: job.map,
            payload,
        });
    }
    any_parsed.then_some(HighlightReply { regions, truncated })
}
