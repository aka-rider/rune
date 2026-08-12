use crate::Etag;
use crate::publish::PutOutcome;
use crate::sighting::{Sighted, Sighting};

#[derive(Debug)]
pub struct Published {
    pub etag: Etag,
    pub sighted: Sighted,
    pub durable: bool,
}

impl From<Published> for PutOutcome {
    fn from(published: Published) -> PutOutcome {
        PutOutcome::Committed {
            etag: published.etag,
            sighted: published.sighted,
            durable: published.durable,
        }
    }
}

#[derive(Debug)]
pub enum ForceOutcome {
    Committed(Published),
    Raced {
        published: Published,
        displaced: Sighting,
    },
}

impl From<ForceOutcome> for PutOutcome {
    fn from(outcome: ForceOutcome) -> PutOutcome {
        match outcome {
            ForceOutcome::Committed(published) => published.into(),
            ForceOutcome::Raced {
                published,
                displaced,
            } => PutOutcome::Raced {
                etag: published.etag,
                sighted: published.sighted,
                durable: published.durable,
                displaced,
            },
        }
    }
}

#[derive(Debug)]
pub enum IfAbsentOutcome {
    Committed(Published),
    Conflict { current: Sighting },
}

impl From<IfAbsentOutcome> for PutOutcome {
    fn from(outcome: IfAbsentOutcome) -> PutOutcome {
        match outcome {
            IfAbsentOutcome::Committed(published) => published.into(),
            IfAbsentOutcome::Conflict { current } => PutOutcome::Conflict { current },
        }
    }
}
