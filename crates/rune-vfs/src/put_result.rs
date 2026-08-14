use std::path::PathBuf;

use crate::Etag;
use crate::publish::PutOutcome;
use crate::sighting::{Sighted, Sighting};

#[derive(Debug)]
pub struct Published {
    pub etag: Etag,
    pub sighted: Sighted,
    pub durable: bool,
    pub stray_temp: Option<PathBuf>,
}

impl From<Published> for PutOutcome {
    fn from(published: Published) -> PutOutcome {
        PutOutcome::Committed {
            etag: published.etag,
            sighted: published.sighted,
            durable: published.durable,
            stray_temp: published.stray_temp,
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
                stray_temp: published.stray_temp,
                displaced,
            },
        }
    }
}

#[derive(Debug)]
pub enum IfAbsentOutcome {
    Committed(Published),
    Conflict {
        current: Sighting,
        stray_temp: Option<PathBuf>,
    },
}

impl From<IfAbsentOutcome> for PutOutcome {
    fn from(outcome: IfAbsentOutcome) -> PutOutcome {
        match outcome {
            IfAbsentOutcome::Committed(published) => published.into(),
            IfAbsentOutcome::Conflict {
                current,
                stray_temp,
            } => PutOutcome::Conflict {
                current,
                stray_temp,
            },
        }
    }
}
