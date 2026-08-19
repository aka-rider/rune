use std::ops::Range;

use rune_db::ObsId;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Conflict {
    pub ours: String,
    pub theirs: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Resolution {
    Unresolved,
    TookTheirs,
    KeptOurs,
    HandEdited,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BlockOrigin {
    #[default]
    Conflict,
    AutoApplied,
}

impl Resolution {
    pub fn is_resolved(self) -> bool {
        !matches!(self, Resolution::Unresolved)
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Block {
    #[serde(flatten)]
    pub range: Range<usize>,
    pub resolution: Resolution,
    pub origin: BlockOrigin,
}

impl<'de> serde::Deserialize<'de> for Block {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Raw {
            start: usize,
            end: usize,
            #[serde(default)]
            resolved: bool,
            #[serde(default)]
            resolution: Option<Resolution>,
            #[serde(default)]
            origin: Option<BlockOrigin>,
        }
        let raw = Raw::deserialize(deserializer)?;
        let resolution = raw.resolution.unwrap_or(if raw.resolved {
            Resolution::HandEdited
        } else {
            Resolution::Unresolved
        });
        Ok(Block {
            range: raw.start..raw.end,
            resolution,
            origin: raw.origin.unwrap_or_default(),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConflictBlock {
    pub conflict: Conflict,
    pub block: Block,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MergeSession {
    pub conflicts: Vec<ConflictBlock>,
    pub cur: usize,
    pub saved_display_name: Option<String>,
    pub theirs_obs: ObsId,
    pub install_pos: usize,
}

impl MergeSession {
    pub fn unresolved_count(&self) -> usize {
        self.conflicts
            .iter()
            .filter(|c| !c.block.resolution.is_resolved())
            .count()
    }

    pub fn resolve(&mut self, idx: usize, resolution: Resolution) {
        if let Some(c) = self.conflicts.get_mut(idx) {
            c.block.resolution = resolution;
        }
    }

    pub fn next_unresolved(&self, dir: isize) -> Option<usize> {
        next_unresolved(&self.conflicts, self.cur, dir)
    }
}

fn next_unresolved(conflicts: &[ConflictBlock], from: usize, dir: isize) -> Option<usize> {
    let n = conflicts.len();
    if n == 0 {
        return None;
    }
    (1..=n)
        .map(|i| {
            let signed = from as isize + dir * i as isize;
            signed.rem_euclid(n as isize) as usize
        })
        .find(|idx| {
            conflicts
                .get(*idx)
                .is_some_and(|c| !c.block.resolution.is_resolved())
        })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn conflict_block(resolution: Resolution) -> ConflictBlock {
        ConflictBlock {
            conflict: Conflict {
                ours: "ours".to_string(),
                theirs: "theirs".to_string(),
            },
            block: Block {
                range: 0..1,
                resolution,
                origin: BlockOrigin::Conflict,
            },
        }
    }

    #[test]
    fn unresolved_count_counts_only_unresolved() {
        let session = MergeSession {
            conflicts: vec![
                conflict_block(Resolution::Unresolved),
                conflict_block(Resolution::TookTheirs),
                conflict_block(Resolution::KeptOurs),
                conflict_block(Resolution::HandEdited),
            ],
            cur: 0,
            saved_display_name: None,
            theirs_obs: ObsId::new(1).expect("nonzero"),
            install_pos: 0,
        };
        assert_eq!(session.unresolved_count(), 1);
    }

    #[test]
    fn resolve_sets_the_targeted_conflicts_resolution() {
        let mut session = MergeSession {
            conflicts: vec![
                conflict_block(Resolution::Unresolved),
                conflict_block(Resolution::Unresolved),
            ],
            cur: 0,
            saved_display_name: None,
            theirs_obs: ObsId::new(1).expect("nonzero"),
            install_pos: 0,
        };
        session.resolve(1, Resolution::TookTheirs);
        assert_eq!(
            session.conflicts[0].block.resolution,
            Resolution::Unresolved
        );
        assert_eq!(
            session.conflicts[1].block.resolution,
            Resolution::TookTheirs
        );
    }

    #[test]
    fn next_unresolved_wraps_and_skips_resolved() {
        let session = MergeSession {
            conflicts: vec![
                conflict_block(Resolution::TookTheirs),
                conflict_block(Resolution::Unresolved),
                conflict_block(Resolution::KeptOurs),
            ],
            cur: 0,
            saved_display_name: None,
            theirs_obs: ObsId::new(1).expect("nonzero"),
            install_pos: 0,
        };
        assert_eq!(session.next_unresolved(1), Some(1));
        assert_eq!(session.next_unresolved(-1), Some(1));
    }

    #[test]
    fn next_unresolved_is_none_when_every_conflict_is_resolved() {
        let session = MergeSession {
            conflicts: vec![
                conflict_block(Resolution::TookTheirs),
                conflict_block(Resolution::KeptOurs),
            ],
            cur: 0,
            saved_display_name: None,
            theirs_obs: ObsId::new(1).expect("nonzero"),
            install_pos: 0,
        };
        assert_eq!(session.next_unresolved(1), None);
    }
}
