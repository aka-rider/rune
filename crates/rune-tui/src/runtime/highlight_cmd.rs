use std::time::{Duration, Instant};

use crate::document::DocumentId;
use crate::highlight::{
    HighlightReply, PassOutcome, RegionJob, RegionLang, RegionOutcome, RegionPayload, RegionResult,
    RegionWork,
};

use super::md_fence::markdown_fence_spans;
use super::{Cmd, Msg};

pub const PARSE_BUDGET: Duration = Duration::from_secs(5);

pub const PASS_BUDGET: Duration = Duration::from_secs(5);

pub(crate) const FIRST_PAINT_BUDGET: Duration = Duration::from_millis(20);

#[derive(Clone, Copy)]
pub(crate) struct PassBudget {
    per_region: Duration,
    total: Duration,
    start: Instant,
    now: fn() -> Instant,
}

impl PassBudget {
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

    fn next_region(&self) -> Option<Duration> {
        let elapsed = (self.now)().saturating_duration_since(self.start);
        let left = self.total.checked_sub(elapsed)?;
        (!left.is_zero()).then(|| self.per_region.min(left))
    }
}

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
        RegionLang::Markdown if markdown_fits_budget(source.len(), left) => {
            let fence = markdown_fence_spans(&source);
            *truncated |= fence.truncated;
            let spans = fence
                .spans
                .into_iter()
                .flat_map(|(range, scope)| {
                    map.to_buffer(
                        crate::linemap::ReconOffset(range.start)
                            ..crate::linemap::ReconOffset(range.end),
                    )
                    .into_iter()
                    .map(move |piece| (piece.range(), scope))
                })
                .collect();
            (
                RegionOutcome::Replace(RegionPayload::Spans { source, spans }),
                true,
            )
        }
        RegionLang::Markdown => (RegionOutcome::CarryForward { source }, false),
    }
}

// comrak has no cooperative deadline hook — a parse that starts cannot be
// stopped partway, so this conservative estimate gates whether to start at all.
const MARKDOWN_BYTES_PER_MS: u128 = 4096;

fn markdown_fits_budget(source_len: usize, left: Duration) -> bool {
    let allowed = left.as_millis().saturating_mul(MARKDOWN_BYTES_PER_MS);
    (source_len as u128) <= allowed
}

pub(crate) fn run_regions(jobs: Vec<RegionJob>, budget: PassBudget) -> PassOutcome {
    let mut regions = Vec::with_capacity(jobs.len());
    let mut attempted = false;
    let mut any_parsed = false;
    let mut truncated = false;
    for job in jobs {
        let outcome = match job.work {
            RegionWork::Retain { source } => RegionOutcome::CarryForward { source },
            RegionWork::Parse { lang, source } => {
                attempted = true;
                match budget.next_region() {
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::linemap::LineMap;

    const LINE: &str = "fn main() { let x = vec![1, 2, 3]; }\n";

    fn job(lines: usize) -> RegionJob {
        RegionJob {
            map: LineMap::new("", Vec::new()),
            work: RegionWork::Parse {
                lang: RegionLang::Ts("rust"),
                source: LINE.repeat(lines),
            },
        }
    }

    fn markdown_job(source_bytes: usize) -> RegionJob {
        RegionJob {
            map: LineMap::new("", Vec::new()),
            work: RegionWork::Parse {
                lang: RegionLang::Markdown,
                source: "#".repeat(source_bytes),
            },
        }
    }

    #[test]
    fn an_oversized_markdown_fence_defers_rather_than_running_unbounded() {
        let jobs = vec![markdown_job(1_000_000)];
        let budget = PassBudget::with_clock(
            Duration::from_millis(1),
            Duration::from_millis(1),
            test_clock::frozen,
        );
        let outcome = run_regions(jobs, budget);
        assert!(
            matches!(outcome, PassOutcome::CarryForward),
            "an oversized fence given almost no budget must defer, never parse"
        );
    }

    #[test]
    fn a_modestly_sized_markdown_fence_still_parses_with_a_fresh_budget() {
        let jobs = vec![markdown_job(200)];
        let outcome = run_regions(jobs, PassBudget::new(PARSE_BUDGET, PASS_BUDGET));
        assert!(
            matches!(outcome, PassOutcome::Replace(_)),
            "an ordinary fence with a full budget must still parse"
        );
    }

    #[test]
    fn a_region_is_offered_the_whole_per_region_cap_never_a_share_of_the_total() {
        let budget = PassBudget::with_clock(PARSE_BUDGET, PASS_BUDGET, test_clock::frozen);
        let offered = budget.next_region().expect("a fresh pass has time left");
        assert_eq!(
            offered, PARSE_BUDGET,
            "a region was offered {offered:?} out of a {PARSE_BUDGET:?} cap"
        );
    }

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
