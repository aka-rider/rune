//! The first-ever-load anchor/adopt/bridge machinery for `load::load`'s
//! `!has_history` branch — split out of `load.rs` purely to keep
//! that module under the line budget; this has no callers outside it.
//!
//! [`Inherited::Disk`] and [`Inherited::Bridged`] both anchor the recovery
//! snapshot and the adoption observation on disk content — the ordinary
//! "first sighting IS the adoption" case. [`Inherited::Diverged`] instead
//! re-anchors on the dead session's OWN baseline (`H0`): the honest
//! reconstruction at `load_seq` really is `H0`, so post-restart undo steps
//! from the bridged draft back to `H0`, not to disk's current content
//! (`H1`). A bare, uncorrelated sighting of `H1` is then recorded LAST, so
//! `newest_observation` ("theirs") reports `H1` and `sync()` at the end of
//! `load` sees ours=draft / theirs=H1 / ancestor=H0 → `Diverged` — the
//! existing in-session `DiskConflict` guard and merge machinery take it
//! from there without needing to know any of this happened.
//!
//! In every anchor sequence below, the blob read (when there is one) and
//! every write it produces commit as ONE `retry::with_retry` transaction,
//! never as separate back-to-back transactions. A window between a
//! committed anchor and its not-yet-committed bridge would let a concurrent
//! fresh session on the same path observe — and double-bridge — a
//! still-half-anchored dead draft.

use std::time::SystemTime;

use rusqlite::{Connection, Transaction};

use rune_core::buffer::AppliedEdit;

use crate::Error;
use crate::adopt;
use crate::confirmation::Confirmation;
use crate::ids::{DocId, Seq, SessionId};
use crate::inherit::Inherited;
use crate::obs_origin::ObsOrigin;
use crate::observation::{self, StatFacts};
use crate::retry;

/// What the `!has_history` branch of `load` needs back: the content the
/// caller's buffer should adopt, and the durable journal seq of the bridge
/// edit that produced it (`None` when nothing was bridged).
pub(crate) struct AnchorOutcome {
    pub(crate) recovered: String,
    pub(crate) bridge_seq: Option<Seq>,
}

/// The load-time facts every anchor/bridge step below needs, bundled so
/// `anchor_first_load`/`anchor_diverged` stay under clippy's
/// too-many-arguments lint without an `#[allow]` (repo rule: none outside
/// test code) — the same "bundle instead of allow" shape `observation.rs`'s
/// `StatFacts`/`ObservationMeta` split already uses.
pub(crate) struct LoadContext<'a> {
    pub(crate) session_id: SessionId,
    pub(crate) doc_id: DocId,
    pub(crate) load_seq: Seq,
    pub(crate) disk_hash: &'a str,
    /// Whether `load`'s own bracketed read of `disk_hash`'s content was
    /// confirmed — carried into every observation this module records FROM
    /// that same read, never re-derived or assumed.
    pub(crate) disk_confirmed: bool,
    pub(crate) live_stat: &'a StatFacts,
    pub(crate) now: SystemTime,
}

/// Anchors a snapshot + adoption on `content` (disk content), tagged with
/// `hash` — the ordinary "first sighting is the adoption" shape shared by
/// every branch that isn't re-anchoring on a dead session's older baseline.
/// Runs entirely inside the caller's already-open `tx` — never opens
/// its own transaction.
fn anchor_on_disk_tx(
    tx: &Transaction<'_>,
    ctx: &LoadContext<'_>,
    content: &str,
    hash: &str,
) -> Result<(), Error> {
    crate::snapshot::create_snapshot(
        tx,
        ctx.session_id,
        ctx.now,
        ctx.doc_id,
        content,
        ctx.load_seq,
    )?;
    adopt::record_adoption_tx(
        tx,
        ctx.doc_id,
        ctx.session_id,
        observation::ObservationMeta {
            blob_hash: hash,
            seq: Some(ctx.load_seq.0),
            origin: ObsOrigin::Load,
            confirmed: Confirmation::from_bracket(ctx.disk_confirmed),
        },
        ctx.live_stat,
        &crate::session::format_rfc3339_nanos(ctx.now),
        None,
    )?;
    Ok(())
}

/// Journals ONE synthetic replace-all edit turning `from` into `to`, under
/// `ctx.session_id`, at the very next journal position — exactly as if the
/// user had just pasted `to` in. Returns the durable seq it landed at. Runs
/// entirely inside the caller's already-open `tx`.
fn bridge_edit_tx(
    tx: &Transaction<'_>,
    ctx: &LoadContext<'_>,
    from: &str,
    to: &str,
) -> Result<Seq, Error> {
    let edit = vec![AppliedEdit {
        start: 0,
        end: from.len(),
        deleted: from.to_string(),
        insert: to.to_string(),
    }];
    crate::journal::append_edit(tx, ctx.session_id, ctx.now, ctx.doc_id, &edit, &[], &[])
}

/// Re-anchors on the dead session's own baseline (`H0`), bridges `H0` to
/// `draft`, then records a bare sighting of disk's current content
/// (`ctx.disk_hash`, `H1`) last — the blob read and all three writes commit
/// as ONE transaction, so a concurrent fresh session on the same path
/// can never observe a partially-anchored bridge. `Ok(None)` when
/// `baseline`'s blob is unusable as an anchor: [`Error::NotFound`] (GC'd or
/// never captured) or [`Error::BlobHashMismatch`] (corrupt) both mean "H0 is
/// gone, fall back to the plain disk-anchor flow" — the caller still
/// preserves the draft via the disk-anchored bridge, it just can't honor
/// H0 as the undo baseline. Every other error from the blob read (a real
/// SQLite/IO failure) propagates instead of silently changing recovery
/// semantics. A `baseline` blob that IS present but not valid UTF-8 also
/// falls back — `load`'s `!has_history` branch has no way to bridge a
/// non-text H0 into a `String`-typed buffer.
fn anchor_diverged(
    conn: &mut Connection,
    ctx: &LoadContext<'_>,
    draft: &str,
    baseline: &observation::Observation,
) -> Result<Option<AnchorOutcome>, Error> {
    retry::with_retry(conn, |tx| {
        let h0_bytes = match crate::blob::get_blob(tx, baseline.blob_hash.as_str()) {
            Ok(bytes) => bytes,
            Err(Error::NotFound(_) | Error::BlobHashMismatch { .. }) => return Ok(None),
            Err(e) => return Err(e),
        };
        let Ok(h0_content) = String::from_utf8(h0_bytes) else {
            return Ok(None);
        };

        crate::snapshot::create_snapshot(
            tx,
            ctx.session_id,
            ctx.now,
            ctx.doc_id,
            &h0_content,
            ctx.load_seq,
        )?;
        adopt::record_adoption_tx(
            tx,
            ctx.doc_id,
            ctx.session_id,
            observation::ObservationMeta {
                blob_hash: baseline.blob_hash.as_str(),
                seq: Some(ctx.load_seq.0),
                origin: ObsOrigin::Load,
                // Copy-forward of a PRIOR observation's own fact, not a
                // fresh read of this call's — carries `baseline`'s own
                // confirmed status forward rather than asserting a new one.
                confirmed: baseline.confirmed,
            },
            &baseline.stat(),
            &crate::session::format_rfc3339_nanos(ctx.now),
            None,
        )?;

        let bridge_seq = bridge_edit_tx(tx, ctx, &h0_content, draft)?;

        // Recorded LAST, uncorrelated (`seq: None`) — `newest_observation`
        // must report H1, never the H0 adoption just recorded above. Stat
        // facts come from the LIVE stat (this session's own fresh
        // sighting), matching every other bare-sighting call site.
        observation::record_observation(
            tx,
            ctx.doc_id,
            ctx.session_id,
            observation::ObservationMeta {
                blob_hash: ctx.disk_hash,
                seq: None,
                origin: ObsOrigin::Load,
                confirmed: Confirmation::from_bracket(ctx.disk_confirmed),
            },
            ctx.live_stat,
            &crate::session::format_rfc3339_nanos(ctx.now),
        )?;

        Ok(Some(AnchorOutcome {
            recovered: draft.to_string(),
            bridge_seq: Some(bridge_seq),
        }))
    })
}

/// The same-session counterpart to [`anchor_first_load`]'s cross-session
/// bridge: `load`'s `has_history` branch calls this when this session's own
/// journal reconstruction (`from`) already agrees with its last-adopted
/// baseline (nothing of its own left unsaved) but disk content (`to`) has
/// moved on since — some other tool rewrote the file. Bridges the journal
/// from `from` to `to` and adopts `to` as the new baseline, correlated to
/// the bridge edit's own seq (the only journal position at which the
/// reconstruction actually equals `to`), both inside ONE transaction, so the
/// returned buffer and the journal that reconstructs it never disagree.
pub(crate) fn reanchor_clean_reload_tx(
    conn: &mut Connection,
    ctx: &LoadContext<'_>,
    from: &str,
    to: &str,
) -> Result<AnchorOutcome, Error> {
    retry::with_retry(conn, |tx| {
        let bridge_seq = bridge_edit_tx(tx, ctx, from, to)?;
        adopt::record_adoption_tx(
            tx,
            ctx.doc_id,
            ctx.session_id,
            observation::ObservationMeta {
                blob_hash: ctx.disk_hash,
                seq: Some(bridge_seq.0),
                origin: ObsOrigin::Load,
                confirmed: Confirmation::from_bracket(ctx.disk_confirmed),
            },
            ctx.live_stat,
            &crate::session::format_rfc3339_nanos(ctx.now),
            None,
        )?;
        Ok(AnchorOutcome {
            recovered: to.to_string(),
            bridge_seq: Some(bridge_seq),
        })
    })
}

/// The full body of `load`'s `!has_history` branch: anchors this session's
/// recovery snapshot/adoption and, when `inherited` carries a dead
/// session's draft, bridges to it — on `H0` for [`Inherited::Diverged`],
/// on disk content for every other case.
pub(crate) fn anchor_first_load(
    conn: &mut Connection,
    ctx: &LoadContext<'_>,
    content: &str,
    inherited: &Inherited,
) -> Result<AnchorOutcome, Error> {
    if let Inherited::Diverged { draft, baseline } = inherited
        && let Some(outcome) = anchor_diverged(conn, ctx, draft, baseline)?
    {
        return Ok(outcome);
    }

    // Disk, Bridged, or a Diverged baseline whose blob turned out to be
    // unusable — anchor on disk content, exactly like today's flow. The
    // anchor write and the bridge edit (when there is one) commit as ONE
    // transaction — the same double-bridge race the `Diverged` path
    // above closes applies equally here.
    retry::with_retry(conn, |tx| {
        anchor_on_disk_tx(tx, ctx, content, ctx.disk_hash)?;
        match inherited {
            Inherited::Bridged { draft } | Inherited::Diverged { draft, .. } => {
                let bridge_seq = bridge_edit_tx(tx, ctx, content, draft)?;
                Ok(AnchorOutcome {
                    recovered: draft.clone(),
                    bridge_seq: Some(bridge_seq),
                })
            }
            Inherited::Disk => Ok(AnchorOutcome {
                recovered: content.to_string(),
                bridge_seq: None,
            }),
        }
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use rusqlite::params;

    use super::*;
    use crate::observation::Observation;
    use crate::test_support::open;

    fn seed_doc(conn: &Connection) -> DocId {
        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/doc.md', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        DocId(conn.last_insert_rowid())
    }

    /// Through `load::load`'s full two-session scenario, A's own H0 blob
    /// can never go missing without ALSO breaking
    /// `inherit::find_inheritable_draft`'s own reconstruction of A's draft
    /// (both read the identical blob by construction), so `load` fails
    /// before ever reaching `anchor_diverged`'s fallback. This test exercises
    /// the fallback directly instead, against `anchor_first_load` itself.
    ///
    /// `baseline` names H0's blob hash, but that blob was never stored —
    /// `anchor_diverged` must fall back to `Ok(None)` (never surface
    /// `Error::NotFound`), letting `anchor_first_load` fall through to the
    /// disk-anchored bridge. The draft still survives; only the undo
    /// baseline degrades from H0 to disk's current content (H1).
    #[test]
    fn diverged_anchor_with_missing_h0_blob_falls_back_to_disk_anchor() {
        let mut conn = open();
        let doc_id = seed_doc(&conn);
        let session_b =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session b");
        let session_a =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session a");

        let draft = "UNSAVED session A's content";
        let disk_content = "disk moved on independently";
        let disk_hash = observation::hash_bytes(disk_content.as_bytes());
        let h0_hash = observation::hash_bytes(b"session A's content");

        let live_stat = StatFacts {
            size: Some(disk_content.len() as i64),
            mtime: Some("t".to_string()),
            ..Default::default()
        };
        let ctx = LoadContext {
            session_id: session_b,
            doc_id,
            load_seq: Seq(0),
            disk_hash: &disk_hash,
            disk_confirmed: true,
            live_stat: &live_stat,
            now: SystemTime::now(),
        };

        // `baseline` claims H0's hash, but no such blob was ever put —
        // simulating corruption/loss distinct from an ordinary GC sweep
        // (`gc::sweep_unreferenced_blobs` never touches a blob a live
        // observation still references, so this can only be reached by a
        // genuinely pathological loss of the row itself).
        let baseline = Observation {
            id: crate::ids::ObsId::new(1).expect("nonzero"),
            doc_id,
            session_id: session_a,
            blob_hash: crate::ids::BlobHash(h0_hash.clone()),
            seq: Some(0),
            size: Some(0),
            mtime: Some("t".to_string()),
            inode: None,
            device: None,
            nlink: None,
            origin: ObsOrigin::Load,
            parent_a: None,
            parent_b: None,
            at: "t".to_string(),
            confirmed: Confirmation::Unclassified,
        };

        let outcome = anchor_first_load(
            &mut conn,
            &ctx,
            disk_content,
            &Inherited::Diverged {
                draft: draft.to_string(),
                baseline: Box::new(baseline),
            },
        )
        .expect("anchor_first_load must succeed via the disk-anchored fallback");

        assert_eq!(
            outcome.recovered, draft,
            "the draft must survive even without H0"
        );
        let bridge_seq = outcome
            .bridge_seq
            .expect("a bridge edit must have been journaled");

        let head = retry::with_retry(&mut conn, |tx| {
            crate::journal::current_seq(tx, session_b, doc_id)
        })
        .expect("current_seq");
        assert_eq!(head, bridge_seq);

        let saved_obs = retry::with_retry(&mut conn, |tx| {
            observation::saved_obs_for(tx, session_b, doc_id)
        })
        .expect("saved_obs_for")
        .expect("session b adopted a baseline");
        assert_eq!(
            saved_obs.blob_hash.as_str(),
            disk_hash,
            "with H0 unusable, the fallback anchors on disk's current content (H1)"
        );

        let sync_state =
            retry::with_retry(&mut conn, |tx| crate::sync::sync(tx, session_b, doc_id))
                .expect("sync");
        assert_eq!(
            sync_state.kind,
            crate::sync::SyncKind::BufferAhead,
            "ours=draft vs theirs=H1=ancestor(seq 0 < bridge_seq) is BufferAhead"
        );

        // No trace of H0 was ever written — the fallback commits only the
        // disk-anchored adoption plus its bridge, never an insert that
        // would have needed the missing blob row.
        let h0_still_absent: bool = conn
            .query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM blobs WHERE hash=?1)",
                params![h0_hash],
                |r| r.get(0),
            )
            .expect("check h0 blob");
        assert!(
            h0_still_absent,
            "the fallback must never attempt to write the missing H0 blob back"
        );
    }
}
