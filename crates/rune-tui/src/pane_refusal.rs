//! The registry-availability refusal check shared by every `GlobalCommand`
//! arm in `pane_command.rs` that can be turned off by `registry::availability`
//! — split out to keep `pane_command.rs` under the 500-line budget.

use crate::app::App;
use crate::keymap::GlobalCommand;
use crate::registry::{self, Availability, CommandId};

pub(crate) fn registry_refusal(app: &App, cmd: GlobalCommand) -> Option<String> {
    match registry::availability(app, CommandId::Global(cmd)) {
        Availability::Available => None,
        Availability::Unavailable(reason) => Some(reason.into_owned()),
    }
}
