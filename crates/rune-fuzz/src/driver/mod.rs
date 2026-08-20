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

pub const DOC_PATH: &str = "/fuzz/doc.md";

pub struct RunResult {
    pub violation: Option<Violation>,
    pub steps: usize,
    pub final_content: String,
    pub final_snapshot: Option<Snapshot>,
    pub final_ctx: Option<StepCtx>,
    pub merge_activated: bool,
}

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

pub fn run(path: &str, content: &str, actions: &[Action]) -> RunResult {
    let mut session = Session::open(path, content);
    for action in actions {
        if session.act(action.clone()).is_some() {
            break;
        }
    }
    session.finish()
}
