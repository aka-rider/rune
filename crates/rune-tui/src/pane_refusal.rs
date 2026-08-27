//! The registry-availability refusal check shared by every `GlobalCommand`
//! arm in `pane_command.rs` (and every `Command` arm in `commands::
//! editor_exec`) that can be turned off by `registry::availability` — split
//! out to keep `pane_command.rs` under the 500-line budget.

use crate::app::App;
use crate::registry::{self, Availability, CommandId};

pub(crate) fn registry_refusal(app: &App, id: CommandId) -> Option<String> {
    match registry::availability(app, id) {
        Availability::Available => None,
        Availability::Unavailable(reason) => Some(reason.into_owned()),
    }
}
