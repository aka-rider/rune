//! `runtime`'s tree-sitter highlight `Cmd` constructor and the region pass
//! behind it — split out of `runtime.rs` itself (500-line budget). Everything
//! here reaches `rune_ts::parse`; nothing else in `rune-tui` does, with the
//! one sanctioned exception of the pre-first-draw pass, which calls
//! [`run_regions`] directly rather than through a `Cmd`.

use std::time::{Duration, Instant};

use crate::document::DocumentId;
use crate::highlight::{
    HighlightReply, PassOutcome, RegionJob, RegionLang, RegionOutcome, RegionPayload, RegionResult,
    RegionWork,
};

use super::md_fence::markdown_fence_spans;
use super::{Cmd, Msg};

/// The wall-clock budget ONE region's parse is allowed before it aborts and
/// reports nothing for that region — one attempt, no retry: a region
/// exceeding this stays plain but *surfaced* via the status message
/// `dispatch::handle_highlighted` sets, rather than silently uncoloured
/// forever.
///
/// Per region, undivided: a single large region is exactly what a budget in
/// seconds exists for, and dividing by the region count is how the old fence
/// pipeline made a four-fence document render flat at 62ms apiece. What
/// keeps the undivided cap affordable is that it is not the only cap —
/// [`PASS_BUDGET`] bounds the whole pass — plus tree retention: a region is
/// reparsed when its own text changed, not whenever anything in the document
/// did. Unmeasured against a real large region (see `TODO.md`).
pub const PARSE_BUDGET: Duration = Duration::from_secs(5);

/// The wall-clock budget ONE region pass — every region of one document —
/// may spend parsing in total, whatever the region count.
///
/// Equal to [`PARSE_BUDGET`] deliberately: the pathological case a
/// seconds-long per-region cap exists for is ONE region so large it needs
/// seconds, and that region must still get the whole cap when it is the only
/// one with work (the ordinary steady-state case, since trees are retained).
/// A document holding two such regions colours the first and leaves the
/// second on its stale colours until the next version change schedules
/// another pass — the same degradation a per-region timeout already produces,
/// and the price of the pass being bounded by a constant rather than by
/// `region count × PARSE_BUDGET`.
pub const PASS_BUDGET: Duration = Duration::from_secs(5);

/// The wall-clock budget the single synchronous, pre-first-draw pass gives
/// its attempt at the startup document's regions — small on purpose: it runs
/// on the main thread before anything is drawn, so even a miss is invisible
/// (the background `Cmd` picks the document up on the very next frame via
/// the ordinary `schedule_highlight` bootstrap kick) while a hit means frame
/// 1 is already highlighted. It serves as that pass's per-region cap AND its
/// total: what matters before the first draw is how long the process may
/// block before showing anything, and no per-region share of it would make a
/// region that needs longer than the whole ceiling succeed anyway.
pub(crate) const FIRST_PAINT_BUDGET: Duration = Duration::from_millis(20);

/// The time [`run_regions`] may spend, as a pair of caps that cannot be
/// separated: what one region gets, and what the whole pass gets.
///
/// A pass cannot be given a per-region cap without also being given a total —
/// there is no constructor that omits one — which is what keeps
/// `region count × per-region` off the `Cmd` thread by construction rather
/// than by a check someone has to remember to write. Each region is handed
/// `min(per_region, whatever is left of total)`, so the one-region document
/// still gets the full per-region cap while an N-region document cannot
/// outrun the total.
///
/// Holds the pass's start rather than its deadline: `Instant + Duration`
/// panics on overflow, and subtracting an elapsed span cannot. The clock is
/// a seam — production reads `Instant::now`, tests substitute a
/// deterministic one so budget logic is asserted without measuring wall
/// time under load.
#[derive(Clone, Copy)]
pub(crate) struct PassBudget {
    per_region: Duration,
    total: Duration,
    start: Instant,
    now: fn() -> Instant,
}

impl PassBudget {
    /// A pass giving each region `per_region` and all of them `total`
    /// between them, starting now.
    pub(crate) fn new(per_region: Duration, total: Duration) -> Self {
        Self::with_clock(per_region, total, Instant::now)
    }

    pub(crate) fn with_clock(per_region: Duration, total: Duration, now: fn() -> Instant) -> Self {
        Self {
            per_region,
            total,
            start: now(),
            now,
        }
    }

    /// The budget the next region may parse for, or `None` once the pass is
    /// spent — the point past which regions stop being attempted at all.
    fn next_region(&self) -> Option<Duration> {
        let elapsed = (self.now)().saturating_duration_since(self.start);
        let left = self.total.checked_sub(elapsed)?;
        (!left.is_zero()).then(|| self.per_region.min(left))
    }
}

/// Parses every region of one document that needs it, off-thread, and
/// replies with `Msg::Highlighted`.
///
/// Owns `jobs` (moved into the closure) since the `Cmd` closure is
/// `FnOnce() -> Option<Msg> + Send + 'static` and cannot borrow the
/// document's buffer across the thread boundary — which is why each job
/// carries its own already-reconstructed source text. Always replies with
/// `Some(Msg::Highlighted { .. })`, even when nothing parsed, so `in_flight`
/// is guaranteed to clear on the UI thread; `rune_ts::parse` itself never
/// panics (it surfaces a failed language load or query compile as
/// `None`, never `ts_assert`'s `SIGABRT`, since every parse is a full parse
/// — no incremental-reparse edit is ever fed back into it).
pub(crate) fn highlight_cmd(doc: DocumentId, version: u64, jobs: Vec<RegionJob>) -> Cmd {
    Cmd::highlight(move || {
        let result = run_regions(jobs, PassBudget::new(PARSE_BUDGET, PASS_BUDGET));
        Some(Msg::Highlighted {
            doc,
            version,
            result,
        })
    })
}

/// One parse job's outcome: what the region's slot in the reply carries,
/// plus whether a parse actually succeeded (the signal `run_regions`
/// aggregates into its pass-level verdict).
fn parse_region(
    map: &crate::linemap::LineMap,
    lang: RegionLang,
    source: String,
    left: Duration,
    truncated: &mut bool,
) -> (RegionOutcome, bool) {
    match lang {
        RegionLang::Ts(lang) => match rune_ts::parse(lang, &source, left) {
            Some(tree) => (RegionOutcome::Replace(RegionPayload::Tree(tree)), true),
            None => (RegionOutcome::CarryForward { source }, false),
        },
        RegionLang::Markdown => {
            let fence = markdown_fence_spans(&source);
            *truncated |= fence.truncated;
            let spans = fence
                .spans
                .into_iter()
                .flat_map(|(range, scope)| {
                    map.to_buffer(range)
                        .into_iter()
                        .map(move |piece| (piece.range(), scope))
                })
                .collect();
            (
                RegionOutcome::Replace(RegionPayload::Spans { source, spans }),
                true,
            )
        }
    }
}

/// The region pass itself, shared by the background `Cmd` above and the
/// pre-first-draw pass — the same work at two budgets, so the two can never
/// drift apart on what a region's highlight means.
///
/// Every job contributes one `RegionResult`, in order, carrying its map
/// whether or not it produced a payload: the reply describes the document's
/// whole region LAYOUT, and a region whose parse overran its budget — or
/// never got one, because the pass's total was already spent — still needs
/// its offsets refreshed. A `Retain` job carries its retained tree's slot
/// forward untouched; every carry names its source, so the install side can
/// verify the kept colours were produced from these exact bytes.
///
/// `CarryForward` iff a parse was ATTEMPTED and not one succeeded `[R2]` —
/// a document where every parse overran must not flash to unstyled. A pass
/// with nothing to attempt still succeeds: every region's tree was still
/// valid, and the refreshed maps it carries are exactly the result.
///
/// A ```markdown fence has no grammar; it reuses the comrak emitter
/// instead. That is a bounded parse of the fence's own (typically short)
/// text, so it never needs a timeout of its own — only the pass's total
/// gates it — but it produces spans rather than a tree, which is why the
/// span channel exists at all. They are mapped back to buffer coordinates
/// while the region's map is still at hand, so everything downstream sees
/// one coordinate space.
pub(crate) fn run_regions(jobs: Vec<RegionJob>, budget: PassBudget) -> PassOutcome {
    let mut regions = Vec::with_capacity(jobs.len());
    let mut attempted = false;
    let mut any_parsed = false;
    // One region hitting the span cap truncates the whole reply: the
    // document's colours are incomplete either way, and the flag says so.
    let mut truncated = false;
    for job in jobs {
        let outcome = match job.work {
            RegionWork::Retain { source } => RegionOutcome::CarryForward { source },
            RegionWork::Parse { lang, source } => {
                attempted = true;
                match budget.next_region() {
                    // The pass's total is spent. Every region from here on
                    // keeps the colours it already had and takes only its
                    // refreshed map, and the next version change schedules a
                    // fresh pass that starts with them.
                    None => RegionOutcome::CarryForward { source },
                    Some(left) => {
                        let (outcome, parsed) =
                            parse_region(&job.map, lang, source, left, &mut truncated);
                        any_parsed |= parsed;
                        outcome
                    }
                }
            }
        };
        regions.push(RegionResult {
            map: job.map,
            outcome,
        });
    }
    if attempted && !any_parsed {
        PassOutcome::CarryForward
    } else {
        PassOutcome::Replace(HighlightReply { regions, truncated })
    }
}

/// Deterministic clocks for [`PassBudget::with_clock`]: budget logic is
/// asserted against a clock the test owns, never against wall time a loaded
/// machine can stretch. `fn() -> Instant` cannot capture state, so the
/// ticking clock advances a thread-local counter — each `#[test]` runs on
/// its own thread, so tests never see each other's ticks.
#[cfg(test)]
pub(crate) mod test_clock {
    use std::cell::Cell;
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};

    fn base() -> Instant {
        static BASE: OnceLock<Instant> = OnceLock::new();
        *BASE.get_or_init(Instant::now)
    }

    pub(crate) fn frozen() -> Instant {
        base()
    }

    thread_local! {
        static TICKS: Cell<u32> = const { Cell::new(0) };
    }

    pub(crate) fn hundred_ms_per_call() -> Instant {
        let ticks = TICKS.with(|c| {
            let ticks = c.get();
            c.set(ticks + 1);
            ticks
        });
        base() + Duration::from_millis(100) * ticks
    }
}

/// What the two caps mean: a pass cannot cost `region count × PARSE_BUDGET`,
/// and the region that genuinely needs seconds still gets them.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::linemap::LineMap;

    /// One line of real Rust, repeated: parse cost scales with source
    /// length, so this is how a test names a region expensive enough to
    /// matter without depending on any grammar pathology.
    const LINE: &str = "fn main() { let x = vec![1, 2, 3]; }\n";

    /// A job whose text is `lines` copies of [`LINE`] — roughly 36 bytes and,
    /// at the debug-profile parse rates this crate's tests run at, tens of
    /// microseconds each.
    fn job(lines: usize) -> RegionJob {
        RegionJob {
            map: LineMap::new("", Vec::new()),
            work: RegionWork::Parse {
                lang: RegionLang::Ts("rust"),
                source: LINE.repeat(lines),
            },
        }
    }

    /// The allocation rule itself: the first region of a pass is offered the
    /// whole per-region cap, and the offer is not a function of how many
    /// regions the pass holds. `total / region count` is the division that
    /// made the old fence pipeline render a many-fence document flat. The
    /// frozen clock makes the offer exact: no time has passed, so nothing is
    /// deducted.
    #[test]
    fn a_region_is_offered_the_whole_per_region_cap_never_a_share_of_the_total() {
        let budget = PassBudget::with_clock(PARSE_BUDGET, PASS_BUDGET, test_clock::frozen);
        let offered = budget.next_region().expect("a fresh pass has time left");
        assert_eq!(
            offered, PARSE_BUDGET,
            "a region was offered {offered:?} out of a {PARSE_BUDGET:?} cap"
        );
    }

    /// A spent total stops the pass at the very first region — no job is
    /// ever handed a parse budget the pass cannot afford, so an exhausted
    /// pass attempts nothing and reports nothing (every region keeps the
    /// colours it had).
    #[test]
    fn a_pass_with_no_total_left_attempts_no_region_at_all() {
        let jobs = vec![job(1), job(1), job(1)];
        let outcome = run_regions(jobs, PassBudget::new(PARSE_BUDGET, Duration::ZERO));
        assert!(
            matches!(outcome, PassOutcome::CarryForward),
            "with the pass's total already spent, no region may be parsed — a \
             reply here would mean one was"
        );
    }

    /// The bound this pass exists for: MANY regions that all need parsing
    /// cost the pass's total, not the region count times the per-region cap.
    ///
    /// The clock charges every budget consultation 100ms, so a 400ms total
    /// affords exactly three offers before the pass is spent — whatever the
    /// machine is doing. Sixteen jobs then split deterministically: the
    /// affordable prefix parses, the starved tail carries forward, and every
    /// job still owes a slot.
    #[test]
    fn a_many_region_pass_costs_its_total_not_its_region_count() {
        const REGIONS: usize = 16;
        const AFFORDABLE: usize = 3;
        const TOTAL: Duration = Duration::from_millis(400);

        let jobs: Vec<RegionJob> = (0..REGIONS).map(|_| job(1)).collect();
        let outcome = run_regions(
            jobs,
            PassBudget::with_clock(PARSE_BUDGET, TOTAL, test_clock::hundred_ms_per_call),
        );

        let PassOutcome::Replace(reply) = outcome else {
            panic!("a pass whose prefix parsed must produce a reply");
        };
        assert_eq!(
            reply.regions.len(),
            REGIONS,
            "every job owes a slot even when the total ran out before it — \
             the reply describes the document's whole region layout"
        );
        let parsed: Vec<bool> = reply
            .regions
            .iter()
            .map(|region| matches!(region.outcome, RegionOutcome::Replace(_)))
            .collect();
        assert_eq!(
            parsed,
            (0..REGIONS).map(|i| i < AFFORDABLE).collect::<Vec<_>>(),
            "a {TOTAL:?} total at 100ms per offer affords exactly the first \
             {AFFORDABLE} regions — the pass is bounded by its total, never by \
             region count × PARSE_BUDGET"
        );
    }

    /// The property the total must not cost: ONE region that genuinely needs
    /// hundreds of milliseconds still parses, even sitting last behind a
    /// crowd of cheap ones. A budget divided by region count would have
    /// starved it.
    #[test]
    fn a_single_expensive_region_still_parses_among_many_cheap_ones() {
        let mut jobs: Vec<RegionJob> = (0..20).map(|_| job(1)).collect();
        jobs.push(job(30_000));

        let PassOutcome::Replace(reply) =
            run_regions(jobs, PassBudget::new(PARSE_BUDGET, PASS_BUDGET))
        else {
            panic!("a pass of parseable rust must produce a reply");
        };

        assert!(
            reply
                .regions
                .last()
                .is_some_and(|region| matches!(region.outcome, RegionOutcome::Replace(_))),
            "the expensive region must get a real budget, not the crowd's share \
             of the pass total"
        );
    }
}
