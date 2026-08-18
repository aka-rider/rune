//! Unit coverage for the scheduler's own gates. The pipeline's behaviour —
//! a fence and a file colouring identically, budgets, tree reuse — is
//! specified end-to-end through the crate's public surface in
//! `tests/highlight_*.rs` instead.

use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_vfs::Mem;

use super::*;
use crate::app::App;

fn app_for(content: &str, path: &str) -> App {
    App::new(
        Buffer::new(content),
        Some(PathBuf::from(path)),
        Arc::new(Mem::new()),
        None,
    )
}

#[test]
fn schedule_highlight_skips_resolving_sources_while_one_is_already_in_flight() {
    // `schedule_highlight` used to resolve its source (reconstructing every
    // region's whole text) before checking whether a highlight was already
    // in flight — so every version-changing message paid that even on the
    // overwhelmingly common path where the gate immediately discards it.
    let content = "fn main() {}\n";
    let mut app = app_for(content, "/x/main.rs");
    let id = app.active;

    let version = app.doc(id).expect("doc").buffer.version();
    app.doc_mut(id).expect("doc").highlight.in_flight = Some(version);

    let mut effects = Effects::default();
    super::schedule_highlight(&mut app, id, &mut effects);

    let doc = app.doc(id).expect("doc");
    assert!(
        doc.highlight.pending,
        "a call while in_flight is set must arm pending"
    );
    assert!(
        effects.cmds.is_empty(),
        "a call while in_flight is set must not dispatch a second cmd"
    );
    assert_eq!(
        test_support::resolve_call_count(),
        0,
        "the source reconstruction must not run while a highlight is already \
         in flight — the in-flight gate must be checked BEFORE resolving"
    );
}

#[test]
fn schedule_highlight_resolves_and_dispatches_when_no_highlight_is_in_flight() {
    // The converse of the case above: with no highlight running and no
    // stored version yet, the gates must fall through and the source must
    // actually be resolved once, dispatching exactly one cmd.
    let content = "fn main() {}\n";
    let mut app = app_for(content, "/x/main.rs");
    let id = app.active;

    let mut effects = Effects::default();
    super::schedule_highlight(&mut app, id, &mut effects);

    assert_eq!(effects.cmds.len(), 1, "expected exactly one dispatched cmd");
    assert_eq!(
        test_support::resolve_call_count(),
        1,
        "sources must be resolved exactly once for the call that dispatches"
    );
}

/// A small `.rs` startup document parses inside `FIRST_PAINT_BUDGET`
/// synchronously, populating its one region's tree before any `Cmd` ever
/// runs — and, since the first-paint pass stamps `highlight.version` on
/// success exactly like a completed background reply would, a subsequent
/// `schedule_highlight` finds the document already current and pushes no
/// `Cmd` at all.
#[test]
fn first_paint_highlights_small_file_synchronously() {
    let mut app = app_for("fn main() {}\n", "/x/main.rs");
    let id = app.active;

    first_paint_highlight(&mut app);

    let doc = app.doc(id).expect("doc");
    assert_eq!(doc.highlight.regions.len(), 1, "a code file is one region");
    assert!(
        doc.highlight
            .regions
            .first()
            .is_some_and(|r| r.tree.is_some()),
        "a trivial rust source must parse within the generous first-paint budget"
    );
    assert_eq!(doc.highlight.version, doc.buffer.version());

    let mut effects = Effects::default();
    schedule_highlight(&mut app, id, &mut effects);
    assert!(
        effects.cmds.is_empty(),
        "the already-current guard must suppress the bootstrap Cmd once the \
         first-paint pass already populated this document's regions"
    );
}

/// A markdown document with no code region at all has nothing to parse —
/// the first-paint pass must be a clean no-op, never inventing a region.
#[test]
fn first_paint_highlight_is_a_no_op_for_a_document_with_no_code_region() {
    let mut app = App::new(Buffer::new("# hello\n"), None, Arc::new(Mem::new()), None);
    let id = app.active;

    first_paint_highlight(&mut app);

    assert!(app.doc(id).expect("doc").highlight.regions.is_empty());
}

/// The first-paint pass is no longer code-document-only: a markdown document
/// whose first screen carries a fence gets that fence coloured on frame 1
/// for the same reason a `.ts` file does.
#[test]
fn first_paint_highlights_a_markdown_fence_too() {
    let mut app = app_for("```rust\nfn main() {}\n```\n", "/x/notes.md");
    let id = app.active;
    app.sync_view();

    first_paint_highlight(&mut app);

    let doc = app.doc(id).expect("doc");
    assert!(
        doc.highlight
            .regions
            .first()
            .is_some_and(|r| r.tree.is_some()),
        "a fence must retain a tree from the first-paint pass, exactly like a file"
    );
}

/// A budget-limited pass that reaches only the first region of a reshaped
/// layout must not let the starved slots inherit trees parsed from some
/// OTHER region's text just because those trees sat at the same index.
///
/// The shape: a two-region layout `[A, B]` is fully highlighted, then a new
/// region lands on top — `[TOP, A, B]` — and the follow-up pass's total
/// (charged 100ms per budget consultation by the deterministic clock)
/// affords only `TOP` before it is spent. The starved slots at indices 1 and
/// 2 sit where `A`'s and `B`'s trees used to; inheriting those positionally
/// would paint `A`'s text with `B`'s tree.
#[test]
fn a_budget_starved_region_never_inherits_a_tree_parsed_from_different_text() {
    const TOP: &str = "let top = 0;\n";
    const A: &str = "fn alpha() {}\n";
    const B: &str = "let beta = 1;\n";
    let parse_job = |text: &str| RegionJob {
        map: LineMap::default(),
        work: RegionWork::Parse {
            lang: RegionLang::Ts("rust"),
            source: text.to_string(),
        },
    };
    let mut app = app_for("x\n", "/x/notes.md");
    let id = app.active;

    let full = runtime::run_regions(
        vec![parse_job(A), parse_job(B)],
        runtime::PassBudget::new(runtime::PARSE_BUDGET, runtime::PASS_BUDGET),
    );
    let PassOutcome::Replace(full) = full else {
        panic!("a full-budget pass of parseable rust must produce a reply");
    };
    apply_reply(app.doc_mut(id).expect("doc"), 1, full);

    let partial = runtime::run_regions(
        vec![parse_job(TOP), parse_job(A), parse_job(B)],
        runtime::PassBudget::with_clock(
            runtime::PARSE_BUDGET,
            std::time::Duration::from_millis(150),
            runtime::test_clock::hundred_ms_per_call,
        ),
    );
    let PassOutcome::Replace(partial) = partial else {
        panic!("a pass whose first region parsed must produce a reply");
    };
    apply_reply(app.doc_mut(id).expect("doc"), 2, partial);

    let doc = app.doc(id).expect("doc");
    assert_eq!(doc.highlight.regions.len(), 3);
    assert_eq!(
        doc.highlight
            .regions
            .first()
            .and_then(|region| region.tree.as_ref())
            .map(rune_ts::ParsedTree::source),
        Some(TOP),
        "the region the budget afforded must hold its own fresh tree"
    );
    for (region, text) in doc.highlight.regions.iter().zip([TOP, A, B]) {
        if let Some(tree) = &region.tree {
            assert_eq!(
                tree.source(),
                text,
                "a region must never hold a tree parsed from different text"
            );
        }
    }
}

/// An info string with no highlighter behind it (unknown tag, or no tag)
/// contributes no region — but is not an error and never blocks the
/// document's other regions.
#[test]
fn region_language_resolves_the_first_token_only() {
    assert_eq!(region_language("rust,ignore"), Some(RegionLang::Ts("rust")));
    assert_eq!(
        region_language("rust title=x"),
        Some(RegionLang::Ts("rust"))
    );
    assert_eq!(region_language("markdown"), Some(RegionLang::Markdown));
    assert_eq!(region_language("MD"), Some(RegionLang::Markdown));
    assert_eq!(region_language("klingon"), None);
    assert_eq!(region_language(""), None);
}
