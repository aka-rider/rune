//! The deterministic engine: drives the real `rune_tui::app::update` against
//! an in-memory `Vfs`, with no terminal, no clock, and no subprocess — a
//! drain loop scoped to the seam this crate actually has: `App::new` +
//! `app::update` + `Cmd` (WP2's tagged struct) + `Mem`.
//!
//! This driver never delivers `Msg::Error` or `Msg::Quit`: neither is ever
//! constructed by an `Action` this crate generates, and production itself
//! only ever sends them from paths this driver doesn't exercise (a real
//! terminal input stream ending, or a spawned `Cmd`'s caught panic — see
//! `runtime.rs`). Every `Msg` the driver DOES deliver is tagged with an
//! owned `MsgTag` at the point it's constructed (`crate::step`), so there is
//! no need for a totalizing `Msg -> MsgTag` conversion that would have to
//! account for those two unreachable-here variants.

mod checks;
mod discharge;
mod seed_scope;
mod session;
mod step_exec;
mod store_ops;

pub use session::Session;
pub use store_ops::wait_for_db_op;

use crate::action::Action;
use crate::guard;
use crate::invariant::Violation;
use crate::snapshot::Snapshot;
use crate::step::StepCtx;

/// The default seeded file path (plan WP7.S2): every `SEEDS` entry
/// inherited from before this package pairs with this path, and the script
/// codec's optional `path` line defaults to it when absent — so the
/// checked-in `repros/tripwire-clean.rune` (written before sessions carried
/// a path) still decodes unchanged.
pub const DOC_PATH: &str = "/fuzz/doc.md";

/// The result of driving one whole session. `final_snapshot`/`final_ctx`
/// are frozen at the violating step (`None` on a clean run).
pub struct RunResult {
    pub violation: Option<Violation>,
    pub steps: usize,
    pub final_content: String,
    pub final_snapshot: Option<Snapshot>,
    pub final_ctx: Option<StepCtx>,
    /// True iff `Snapshot::merge_active` was ever true on any step of this
    /// session (non-vacuous merge coverage) — unlike
    /// `final_snapshot`, tracked on EVERY step, not just a violating one,
    /// since a session's own resolver work can legitimately exit `Active`
    /// again (a full resolution, an auto-exit on tab switch) before the
    /// session ends.
    pub merge_activated: bool,
}

/// `run`, with the whole session under the panic guard: a panic anywhere
/// the per-window guards do not reach still comes back as a recorded
/// `NO-PANIC` violation, so the caller can write its artifact bundle
/// instead of unwinding past the one writer that exists.
pub fn run_catching_panic(path: &str, content: &str, actions: &[Action]) -> RunResult {
    match guard::catching_panic(|| run(path, content, actions)) {
        Ok(result) => result,
        Err(violation) => panicked_result(violation, content),
    }
}

fn panicked_result(violation: Violation, content: &str) -> RunResult {
    RunResult {
        violation: Some(violation),
        steps: 0,
        final_content: content.to_string(),
        final_snapshot: None,
        final_ctx: None,
        merge_activated: false,
    }
}

/// Runs `actions` against a fresh session seeded with `content` at `path`
/// (plan WP7.S2 — a session now opens an arbitrary path, so `DocumentKind`
/// producer selection, including a code or plain document, is reachable
/// from this driver, not just markdown). Deterministic: same input, same
/// result, always — zero wall-clock reads, zero threads, zero subprocesses
/// (WP3.S7 rule 7).
pub fn run(path: &str, content: &str, actions: &[Action]) -> RunResult {
    let mut session = Session::open(path, content);
    for action in actions {
        if session.act(action.clone()).is_some() {
            break;
        }
    }
    session.finish()
}
