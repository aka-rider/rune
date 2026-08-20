use std::collections::HashSet;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OpKind {
    Read,
    WriteDurable,
    Exchange,
    RenameExcl,
    Remove,
    Trash,
    Stat,
    Resolve,
    MkdirAll,
    ReadDir,
    ReadLink,
}

pub(crate) struct Faults {
    pub fail_next: Mutex<Option<(OpKind, io::Error)>>,
    pub fail_after: Mutex<Option<(OpKind, io::Error)>>,
    pub mutate_after_stat: Mutex<Option<(PathBuf, Vec<u8>)>>,
    pub churning: Mutex<HashSet<PathBuf>>,
    pub resolve_failures: Mutex<HashSet<PathBuf>>,
}

impl Faults {
    pub(crate) fn new() -> Self {
        Faults {
            fail_next: Mutex::new(None),
            fail_after: Mutex::new(None),
            mutate_after_stat: Mutex::new(None),
            churning: Mutex::new(HashSet::new()),
            resolve_failures: Mutex::new(HashSet::new()),
        }
    }
}
