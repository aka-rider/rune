//! The conflict-lifecycle comparison. Pure SQLite: never touches disk (`sync`/
//! `sync_with_theirs` are Update-safe); `probe::probe` is the disk-touching
//! counterpart that records a fresh observation first, making ITS OWN new
//! observation the newest by construction, then calls `sync_with_theirs`
//! with it.

use rusqlite::{Transaction, params};

use crate::Error;
use crate::observation::{self, ObsId};

/// A comparable fact for the Sync/Probe three-way comparison: a content
/// hash, optionally correlated to the [`crate::observation::Observation`]
/// it came from. An out-of-band validity bit is instead modeled as
/// `Option<Version>` at the call site (this crate's own "Options for
/// absent facts" rule).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub hash: String,
    pub obs: Option<ObsId>,
}

/// Discriminates the outcome of comparing buffer/saved/ancestor state for a
/// document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncKind {
    /// The buffer matches what we believe is on disk (or there is no disk
    /// fact yet — an untitled document with an empty buffer).
    Clean,
    /// Only the buffer has changed since the ancestor — an ordinary unsaved
    /// edit; disk has not moved.
    BufferAhead,
    /// Only disk has changed since the ancestor — an external edit landed
    /// while the buffer stayed untouched; safe to adopt.
    DiskAhead,
    /// Both the buffer and disk changed since the ancestor (or there is no
    /// ancestor to reason from at all) — a real conflict.
    Diverged,
}

impl SyncKind {
    /// The disk holds changes the buffer doesn't (`DiskAhead`/`Diverged`) —
    /// the one predicate behind every "disk changed" affordance: the merge
    /// invitation, the footer/tab divergence markers, and merge entry's own
    /// pre-check. `BufferAhead` is deliberately excluded: an ordinary unsaved
    /// edit is the dirty flag's job, not a divergence.
    pub fn is_disk_divergent(self) -> bool {
        matches!(self, SyncKind::DiskAhead | SyncKind::Diverged)
    }
}

/// The result of comparing three hashes for a document: the buffer head
/// (`ours`), the freshest disk knowledge (`theirs`), and the derived
/// 3-way-merge ancestor.
#[derive(Clone, Debug, PartialEq)]
pub struct SyncState {
    pub kind: SyncKind,
    pub ancestor: Option<Version>,
    pub ours: Version,
    pub theirs: Option<Version>,
}

/// The SHA-256 of the empty string — the "nothing to save yet" baseline for
/// a document with no disk fact at all.
fn empty_hash() -> &'static str {
    use std::sync::OnceLock;
    static EMPTY: OnceLock<String> = OnceLock::new();
    EMPTY.get_or_init(|| observation::hash_bytes(b""))
}

/// The Conflict lifecycle comparison.
pub fn classify_sync(
    ancestor: Option<&Version>,
    ours: &Version,
    theirs: Option<&Version>,
) -> SyncKind {
    let Some(theirs) = theirs else {
        return if ours.hash == empty_hash() {
            SyncKind::Clean
        } else {
            SyncKind::BufferAhead
        };
    };
    if ours.hash == theirs.hash {
        return SyncKind::Clean;
    }
    let Some(ancestor) = ancestor else {
        return SyncKind::Diverged;
    };
    if theirs.hash == ancestor.hash {
        SyncKind::BufferAhead
    } else if ours.hash == ancestor.hash {
        SyncKind::DiskAhead
    } else {
        SyncKind::Diverged
    }
}

/// The own-history echo check: a `Diverged` verdict is downgraded to
/// `BufferAhead` when `theirs`' hash was INDEPENDENTLY recorded before —
/// some OTHER observation of `doc_id` (any session) already carries the
/// same hash with `origin` `save`/`resolve`, or as a confirmed load/probe
/// sighting — AND `ancestor` is an ancestor of `theirs` in the observations'
/// own parent-edge DAG (`lineage::is_ancestor`). Kills the cross-session
/// echo shape (G7): two sessions (tabs) on the same file each derive
/// `ancestor_at` scoped to their own agreement history, so session A's save
/// is never session B's ancestor — without this check, B rediscovering A's
/// own save via a plain probe, while B's own buffer has since edited
/// further, misclassifies as a foreign conflict against content rune itself
/// produced.
///
/// Deliberately excludes `theirs`' own row from the existence check: a
/// fresh, first-ever sighting of some content is always self-confirmed
/// (bracket.rs's own bracket just settled on it), so without the exclusion
/// EVERY divergence — including a genuine stranger's rewrite — would
/// trivially "match its own history" and vacuously promote. Requiring an
/// INDEPENDENT prior sighting is what makes this a real echo test rather
/// than a no-op.
///
/// Only ever promotes to `BufferAhead`, never `Clean`: the exact-hash-match
/// case (`theirs.hash == ancestor.hash`) is already resolved to
/// `BufferAhead` by `classify_sync` before `Diverged` is ever reached, so no
/// distinct equal-to-baseline case actually arises here.
///
/// A known, accepted trade-off: a parent edge records TEMPORAL succession
/// ("what was newest, or what was reconciled against, right before this
/// sighting"), not true content derivation, so an external tool that
/// restores bytes matching an OLDER hash rune once wrote can also share a
/// lineage with the current ancestor and get promoted here, even though the
/// restore is a genuine external change. This never loses bytes (the blob
/// is retained, and the buffer stays exactly as dirty as it already was) —
/// it only suppresses a merge invitation that would otherwise have been
/// shown; a save afterward still CAS-compares against the disk's real
/// current hash and refuses normally if it has moved again since.
fn own_history_echo(
    tx: &Transaction<'_>,
    doc_id: i64,
    ancestor: Option<&Version>,
    theirs: Option<&Version>,
) -> Result<Option<SyncKind>, Error> {
    let (Some(ancestor), Some(theirs)) = (ancestor, theirs) else {
        return Ok(None);
    };
    let (Some(ancestor_id), Some(theirs_id)) = (ancestor.obs, theirs.obs) else {
        return Ok(None);
    };
    let is_own_write: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM observations WHERE doc_id=?1 AND blob_hash=?2 AND id!=?3 AND (origin IN ('save','resolve') OR confirmed=1))",
        params![doc_id, theirs.hash, theirs_id],
        |r| r.get(0),
    )?;
    if !is_own_write {
        return Ok(None);
    }
    let contained = crate::lineage::is_ancestor(tx, ancestor_id, theirs_id)?;
    Ok(contained.then_some(SyncKind::BufferAhead))
}

/// Compares the journal reconstruction, the newest recorded observation
/// (ANY origin, ANY session — "theirs"), and the derived ancestor for
/// `doc_id`, AS SEEN BY `session_id`.
pub fn sync(tx: &Transaction<'_>, session_id: i64, doc_id: i64) -> Result<SyncState, Error> {
    let newest = observation::newest_observation(tx, doc_id)?;
    let theirs = newest.map(|o| Version {
        hash: o.blob_hash,
        obs: Some(o.id),
    });
    sync_with_theirs(tx, session_id, doc_id, theirs)
}

/// The ours/ancestor reconstruction shared by [`sync`] (theirs = the newest
/// recorded observation) and `probe::probe` (theirs = a just-recorded fresh
/// observation), including the
/// undo-unwind override: a `DiskAhead` classification
/// upgrades to `Diverged` when this session has recorded ANY correlated
/// observation past `pos` — a resolution the buffer has since been undone
/// past.
pub fn sync_with_theirs(
    tx: &Transaction<'_>,
    session_id: i64,
    doc_id: i64,
    theirs: Option<Version>,
) -> Result<SyncState, Error> {
    let pos = crate::journal::current_seq(tx, session_id, doc_id)?;
    let ours_content = crate::snapshot::recover_document(tx, session_id, doc_id)?;
    let ours = Version {
        hash: observation::hash_bytes(ours_content.as_bytes()),
        obs: None,
    };

    let exclude = theirs.as_ref().and_then(|v| v.obs);
    let ancestor_obs = observation::ancestor_at(tx, doc_id, session_id, pos, exclude)?;
    let ancestor = ancestor_obs.map(|o| Version {
        hash: o.blob_hash,
        obs: Some(o.id),
    });

    let mut kind = classify_sync(ancestor.as_ref(), &ours, theirs.as_ref());

    if kind == SyncKind::Diverged
        && let Some(promoted) = own_history_echo(tx, doc_id, ancestor.as_ref(), theirs.as_ref())?
    {
        kind = promoted;
    }

    if kind == SyncKind::DiskAhead {
        let unwound: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM observations WHERE doc_id=?1 AND session_id=?2 AND seq IS NOT NULL AND seq > ?3)",
            params![doc_id, session_id, pos],
            |r| r.get(0),
        )?;
        if unwound {
            kind = SyncKind::Diverged;
        }
    }

    Ok(SyncState {
        kind,
        ancestor,
        ours,
        theirs,
    })
}

/// Dirty ⟺ ours differs from ancestor (`BufferAhead` or `Diverged`) — NEVER
/// `kind != Clean`, which would also flag `DiskAhead` (a pure external
/// change with nothing of the user's unsaved) as phantom-dirty.
pub fn is_dirty(kind: SyncKind) -> bool {
    matches!(kind, SyncKind::BufferAhead | SyncKind::Diverged)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::time::SystemTime;

    fn open() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        crate::schema::apply(&conn).expect("schema");
        conn
    }

    fn seed_doc(tx: &Transaction<'_>) -> i64 {
        tx.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        tx.last_insert_rowid()
    }

    /// Task WP-C(2), the G7 cross-session echo shape: session A's own save
    /// (`H_shared`) predates this session's own ancestor (`L`); a LATER
    /// confirmed sighting (`M`, unrelated content) chains from `L`, and a
    /// STILL LATER sighting rediscovers `H_shared` again, chaining from
    /// `M`. `theirs` (the rediscovery) is reachable from `L` via that
    /// chain, and `H_shared` was independently recorded before `theirs`
    /// itself existed (session A's row) — the non-vacuous echo shape this
    /// check exists for. Without it this classifies `Diverged`, a foreign
    /// conflict against content rune itself (a different session) wrote.
    #[test]
    fn own_history_echo_promotes_a_cross_session_rediscovery_to_buffer_ahead() {
        let mut conn = open();
        let session_a =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session a");
        let session_b =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session b");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);

        let stat = observation::StatFacts {
            size: 1,
            mtime: "t".to_string(),
            ..Default::default()
        };

        let hash_shared = crate::blob::put_blob(&tx, b"shared content").expect("seed blob");
        let save_id = observation::record_observation(
            &tx,
            doc_id,
            session_a,
            observation::ObservationMeta {
                blob_hash: &hash_shared,
                seq: Some(0),
                origin: "save",
                confirmed: Some(true),
            },
            &stat,
            "t0",
        )
        .expect("seed session a's save");

        let hash_l = crate::blob::put_blob(&tx, b"session b ancestor").expect("seed blob");
        let ancestor_id = observation::record_observation(
            &tx,
            doc_id,
            session_b,
            observation::ObservationMeta {
                blob_hash: &hash_l,
                seq: Some(0),
                origin: "load",
                confirmed: None,
            },
            &stat,
            "t1",
        )
        .expect("seed session b's ancestor");

        let hash_mid = crate::blob::put_blob(&tx, b"unrelated midpoint").expect("seed blob");
        let mid_id = observation::insert_observation_row(
            &tx,
            doc_id,
            session_b,
            observation::ObservationMeta {
                blob_hash: &hash_mid,
                seq: None,
                origin: "probe",
                confirmed: Some(true),
            },
            &stat,
            "t2",
            observation::ParentEdges {
                a: Some(ancestor_id),
                b: None,
            },
        )
        .expect("seed midpoint");

        let theirs_id = observation::insert_observation_row(
            &tx,
            doc_id,
            session_b,
            observation::ObservationMeta {
                blob_hash: &hash_shared,
                seq: None,
                origin: "probe",
                confirmed: Some(true),
            },
            &stat,
            "t3",
            observation::ParentEdges {
                a: Some(mid_id),
                b: None,
            },
        )
        .expect("seed rediscovery");
        let _ = save_id;

        let ancestor = Version {
            hash: hash_l,
            obs: Some(ancestor_id),
        };
        let theirs = Version {
            hash: hash_shared,
            obs: Some(theirs_id),
        };

        let promoted = own_history_echo(&tx, doc_id, Some(&ancestor), Some(&theirs))
            .expect("own_history_echo");
        assert_eq!(promoted, Some(SyncKind::BufferAhead));
        tx.commit().expect("commit");
    }

    /// A fresh, first-ever sighting of some content must never "match its
    /// own history" merely because it is itself confirmed — the exclusion
    /// of `theirs`' own row from the existence check must hold even when
    /// that row is reachable from the ancestor by construction (a genuine
    /// stranger's single rewrite always looks exactly like this).
    #[test]
    fn own_history_echo_does_not_promote_a_first_ever_sighting() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);

        let stat = observation::StatFacts {
            size: 1,
            mtime: "t".to_string(),
            ..Default::default()
        };
        let hash_l = crate::blob::put_blob(&tx, b"ancestor").expect("seed blob");
        let ancestor_id = observation::record_observation(
            &tx,
            doc_id,
            session_id,
            observation::ObservationMeta {
                blob_hash: &hash_l,
                seq: Some(0),
                origin: "load",
                confirmed: None,
            },
            &stat,
            "t1",
        )
        .expect("seed ancestor");

        let hash_stranger = crate::blob::put_blob(&tx, b"a stranger's rewrite").expect("seed blob");
        let theirs_id = observation::insert_observation_row(
            &tx,
            doc_id,
            session_id,
            observation::ObservationMeta {
                blob_hash: &hash_stranger,
                seq: None,
                origin: "probe",
                confirmed: Some(true),
            },
            &stat,
            "t2",
            observation::ParentEdges {
                a: Some(ancestor_id),
                b: None,
            },
        )
        .expect("seed the stranger's own first sighting");

        let ancestor = Version {
            hash: hash_l,
            obs: Some(ancestor_id),
        };
        let theirs = Version {
            hash: hash_stranger,
            obs: Some(theirs_id),
        };

        let promoted = own_history_echo(&tx, doc_id, Some(&ancestor), Some(&theirs))
            .expect("own_history_echo");
        assert_eq!(
            promoted, None,
            "a hash with no independent prior sighting must stay Diverged, even though it is reachable from the ancestor and confirmed"
        );
        tx.commit().expect("commit");
    }

    /// Exercises the CONTAINMENT half specifically: `theirs`' hash IS
    /// independently, trustedly recorded elsewhere (`is_own_write` is
    /// genuinely true, via `independent_id` below — a real, non-vacuous
    /// trust-gate pass, unlike the OLD version of this test, which had no
    /// independent `hash_b` row at all and passed only because the trust
    /// gate had nothing to see), but that independent sighting — like
    /// `theirs` itself — shares NO recorded lineage with the current
    /// ancestor: two disconnected roots for the same document, the shape a
    /// legacy or pre-migration row can leave behind. Must stay `Diverged`.
    /// Mutation-checked: with the containment check deleted (promoting
    /// unconditionally once the trust gate passes), this test goes red.
    #[test]
    fn own_history_echo_does_not_promote_a_disconnected_hash_match() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);

        let hash_a = crate::blob::put_blob(&tx, b"ancestor content").expect("seed blob a");
        let ancestor_id = observation::record_observation(
            &tx,
            doc_id,
            session_id,
            observation::ObservationMeta {
                blob_hash: &hash_a,
                seq: Some(0),
                origin: "load",
                confirmed: None,
            },
            &observation::StatFacts {
                size: 1,
                mtime: "t".to_string(),
                ..Default::default()
            },
            "t",
        )
        .expect("seed ancestor");

        let hash_b = crate::blob::put_blob(&tx, b"disconnected content").expect("seed blob b");
        let theirs_id = observation::record_observation(
            &tx,
            doc_id,
            session_id,
            observation::ObservationMeta {
                blob_hash: &hash_b,
                seq: None,
                origin: "probe",
                confirmed: Some(true),
            },
            &observation::StatFacts {
                size: 1,
                mtime: "t2".to_string(),
                ..Default::default()
            },
            "t2",
        )
        .expect("seed theirs");

        // The independent, TRUSTED prior sighting of `hash_b` — a real root
        // with no lineage to `ancestor_id` at all, distinct from `theirs_id`
        // (excluded by `own_history_echo`'s own `id != theirs_id` guard).
        let _independent_id = observation::record_observation(
            &tx,
            doc_id,
            session_id,
            observation::ObservationMeta {
                blob_hash: &hash_b,
                seq: None,
                origin: "save",
                confirmed: None,
            },
            &observation::StatFacts {
                size: 1,
                mtime: "t3".to_string(),
                ..Default::default()
            },
            "t3",
        )
        .expect("seed independent trusted sighting");

        let ancestor = Version {
            hash: hash_a,
            obs: Some(ancestor_id),
        };
        let theirs = Version {
            hash: hash_b,
            obs: Some(theirs_id),
        };

        let promoted = own_history_echo(&tx, doc_id, Some(&ancestor), Some(&theirs))
            .expect("own_history_echo");
        assert_eq!(
            promoted, None,
            "a hash matching history but sharing no recorded lineage with the ancestor must stay Diverged"
        );
        tx.commit().expect("commit");
    }

    /// Exercises the TRUST GATE half specifically: `theirs` itself chains
    /// from `ancestor` via `parent_a` (so containment alone WOULD promote
    /// it), but the only other sighting of its hash (`independent_id`) is
    /// UNCONFIRMED and not a `save`/`resolve` — an untrusted read decides
    /// nothing, including whether it is an echo of our own history. Must
    /// stay `Diverged`. Mutation-checked: with the trust gate deleted
    /// (`is_own_write` forced `true`), containment alone finds `ancestor_id`
    /// on `theirs_id`'s chain and this test goes red.
    #[test]
    fn own_history_echo_does_not_promote_an_unconfirmed_theirs() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);

        let hash_a = crate::blob::put_blob(&tx, b"ancestor content").expect("seed blob a");
        let ancestor_id = observation::record_observation(
            &tx,
            doc_id,
            session_id,
            observation::ObservationMeta {
                blob_hash: &hash_a,
                seq: Some(0),
                origin: "load",
                confirmed: None,
            },
            &observation::StatFacts {
                size: 1,
                mtime: "t".to_string(),
                ..Default::default()
            },
            "t",
        )
        .expect("seed ancestor");

        let hash_b = crate::blob::put_blob(&tx, b"unconfirmed content").expect("seed blob b");
        // Chains from `ancestor_id` — containment alone (with no trust gate)
        // would find it.
        let theirs_id = observation::insert_observation_row(
            &tx,
            doc_id,
            session_id,
            observation::ObservationMeta {
                blob_hash: &hash_b,
                seq: None,
                origin: "watch",
                confirmed: Some(false),
            },
            &observation::StatFacts {
                size: 1,
                mtime: "t2".to_string(),
                ..Default::default()
            },
            "t2",
            observation::ParentEdges {
                a: Some(ancestor_id),
                b: None,
            },
        )
        .expect("seed theirs");

        // The ONLY other sighting of `hash_b` — untrusted (unconfirmed,
        // never `save`/`resolve`), so the trust gate must refuse it.
        let _independent_id = observation::record_observation(
            &tx,
            doc_id,
            session_id,
            observation::ObservationMeta {
                blob_hash: &hash_b,
                seq: None,
                origin: "watch",
                confirmed: Some(false),
            },
            &observation::StatFacts {
                size: 1,
                mtime: "t3".to_string(),
                ..Default::default()
            },
            "t3",
        )
        .expect("seed independent untrusted sighting");

        let ancestor = Version {
            hash: hash_a,
            obs: Some(ancestor_id),
        };
        let theirs = Version {
            hash: hash_b,
            obs: Some(theirs_id),
        };

        let promoted = own_history_echo(&tx, doc_id, Some(&ancestor), Some(&theirs))
            .expect("own_history_echo");
        assert_eq!(promoted, None);
        tx.commit().expect("commit");
    }

    #[test]
    fn untitled_empty_buffer_with_no_disk_fact_is_clean() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);

        let state = sync(&tx, session_id, doc_id).expect("sync");
        assert_eq!(state.kind, SyncKind::Clean);
        tx.commit().expect("commit");
    }

    #[test]
    fn untitled_nonempty_buffer_with_no_disk_fact_is_buffer_ahead() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);

        crate::journal::append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            &[rune_core::buffer::AppliedEdit {
                start: 0,
                end: 0,
                deleted: String::new(),
                insert: "hi".to_string(),
            }],
            &[],
            &[],
        )
        .expect("append_edit");

        let state = sync(&tx, session_id, doc_id).expect("sync");
        assert_eq!(state.kind, SyncKind::BufferAhead);
        tx.commit().expect("commit");
    }

    #[test]
    fn no_ancestor_with_ours_ne_theirs_is_diverged() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);
        let hash = crate::blob::put_blob(&tx, b"some content").expect("seed blob");

        // A bare (uncorrelated) sighting: never ancestor-eligible.
        crate::observation::record_observation(
            &tx,
            doc_id,
            session_id,
            crate::observation::ObservationMeta {
                blob_hash: &hash,
                seq: None,
                origin: "watch",
                confirmed: None,
            },
            &crate::observation::StatFacts {
                size: 1,
                mtime: "t".to_string(),
                ..Default::default()
            },
            "t",
        )
        .expect("record");

        let state = sync(&tx, session_id, doc_id).expect("sync");
        assert_eq!(state.kind, SyncKind::Diverged);
        tx.commit().expect("commit");
    }

    /// Seeds the completed-reconciliation shape: a 'load' observation at
    /// seq 0 with `load_blob`, one journaled edit producing "merged" at
    /// seq 1, and a 'resolve' observation correlated to that head seq
    /// carrying `resolve_blob` — the record a zero-conflict merge or
    /// Discard leaves behind, where no further edit moves the journal
    /// past the resolve seq.
    fn seed_resolve_at_head(
        tx: &Transaction<'_>,
        session_id: i64,
        doc_id: i64,
        load_blob: &[u8],
        resolve_blob: &[u8],
    ) {
        let load_hash = crate::blob::put_blob(tx, load_blob).expect("seed load blob");
        crate::observation::record_observation(
            tx,
            doc_id,
            session_id,
            crate::observation::ObservationMeta {
                blob_hash: &load_hash,
                seq: Some(0),
                origin: "load",
                confirmed: None,
            },
            &crate::observation::StatFacts {
                size: load_blob.len() as i64,
                mtime: "t".to_string(),
                ..Default::default()
            },
            "t",
        )
        .expect("record load observation");

        crate::journal::append_edit(
            tx,
            session_id,
            SystemTime::now(),
            doc_id,
            &[rune_core::buffer::AppliedEdit {
                start: 0,
                end: 0,
                deleted: String::new(),
                insert: "merged".to_string(),
            }],
            &[],
            &[],
        )
        .expect("append_edit");

        let resolve_hash = crate::blob::put_blob(tx, resolve_blob).expect("seed resolve blob");
        crate::observation::record_observation(
            tx,
            doc_id,
            session_id,
            crate::observation::ObservationMeta {
                blob_hash: &resolve_hash,
                seq: Some(1),
                origin: "resolve",
                confirmed: None,
            },
            &crate::observation::StatFacts {
                size: resolve_blob.len() as i64,
                mtime: "t2".to_string(),
                ..Default::default()
            },
            "t2",
        )
        .expect("record resolve observation");
    }

    /// The zero-conflict-merge / all-both shape: the resolve observation
    /// sits at exactly the journal head, and its blob (the disk bytes) is
    /// NOT what the buffer reconstructs to. The resolve row is both theirs
    /// and the legitimate ancestor — only the buffer moved since the
    /// reconciliation, so this is an ordinary unsaved edit, never a
    /// fabricated conflict.
    #[test]
    fn resolve_at_head_seq_classifies_buffer_ahead_not_diverged() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);
        seed_resolve_at_head(&tx, session_id, doc_id, b"original", b"disk");

        let state = sync(&tx, session_id, doc_id).expect("sync");
        assert_eq!(
            state.kind,
            SyncKind::BufferAhead,
            "a resolve observation at the head seq is a completed reconciliation, not a divergence"
        );
        tx.commit().expect("commit");
    }

    /// The Discard shape: the resolve observation's blob equals the journal
    /// reconstruction — buffer and disk agree byte-for-byte.
    #[test]
    fn resolve_at_head_seq_matching_reconstruction_is_clean() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);
        seed_resolve_at_head(&tx, session_id, doc_id, b"original", b"merged");

        let state = sync(&tx, session_id, doc_id).expect("sync");
        assert_eq!(state.kind, SyncKind::Clean);
        tx.commit().expect("commit");
    }

    /// Pins the already-working case: once an edit moves the journal PAST
    /// the resolve seq, the resolve row is an ordinary older correlation
    /// and classification stays BufferAhead.
    #[test]
    fn edit_after_resolve_still_classifies_buffer_ahead() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);
        seed_resolve_at_head(&tx, session_id, doc_id, b"original", b"disk");

        crate::journal::append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            &[rune_core::buffer::AppliedEdit {
                start: 6,
                end: 6,
                deleted: String::new(),
                insert: " more".to_string(),
            }],
            &[],
            &[],
        )
        .expect("append_edit after resolve");

        let state = sync(&tx, session_id, doc_id).expect("sync");
        assert_eq!(state.kind, SyncKind::BufferAhead);
        tx.commit().expect("commit");
    }

    /// Undo-unwind override: a resolve observation
    /// correlates `theirs` to the edit seq that resolved it. Undoing the
    /// buffer back BELOW that seq makes `ancestor_at` recompute an OLDER
    /// ancestor the wound-back buffer coincidentally matches — plain
    /// `classify_sync` alone would read that as the safe `DiskAhead`
    /// auto-adopt case. It isn't: the resolution's chain to `theirs` no
    /// longer exists at the wound-back position, so this must re-raise as
    /// `Diverged`.
    #[test]
    fn undo_unwind_upgrades_disk_ahead_to_diverged() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);

        // An ancestor-eligible 'load' observation at seq 0, matching the
        // (empty) journal reconstruction at that position.
        let empty_hash = crate::blob::put_blob(&tx, b"").expect("seed empty blob");
        crate::observation::record_observation(
            &tx,
            doc_id,
            session_id,
            crate::observation::ObservationMeta {
                blob_hash: &empty_hash,
                seq: Some(0),
                origin: "load",
                confirmed: None,
            },
            &crate::observation::StatFacts {
                mtime: "t".to_string(),
                ..Default::default()
            },
            "t",
        )
        .expect("record load observation");

        // Journal "hello" at seq 1, then record a 'resolve' observation
        // correlated to seq 1 (matching the reconstruction there) — this is
        // `theirs`, the resolution the undo below will wind back past.
        crate::journal::append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            &[rune_core::buffer::AppliedEdit {
                start: 0,
                end: 0,
                deleted: String::new(),
                insert: "hello".to_string(),
            }],
            &[],
            &[],
        )
        .expect("append_edit");
        let hello_hash = crate::blob::put_blob(&tx, b"hello").expect("seed hello blob");
        crate::observation::record_observation(
            &tx,
            doc_id,
            session_id,
            crate::observation::ObservationMeta {
                blob_hash: &hello_hash,
                seq: Some(1),
                origin: "resolve",
                confirmed: None,
            },
            &crate::observation::StatFacts {
                size: 5,
                mtime: "t".to_string(),
                ..Default::default()
            },
            "t",
        )
        .expect("record resolve observation");

        // Undo back to position 0 — BELOW the resolve observation's seq 1.
        crate::journal::move_undo_pos(&tx, session_id, doc_id, 0).expect("move_undo_pos");

        let state = sync(&tx, session_id, doc_id).expect("sync");
        assert_eq!(
            state.kind,
            SyncKind::Diverged,
            "undo past a resolution must re-raise Diverged, never the plain DiskAhead classify_sync alone would compute"
        );
        tx.commit().expect("commit");
    }
}
