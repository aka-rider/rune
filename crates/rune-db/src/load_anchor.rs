//! The first-ever-load anchor/adopt/bridge machinery for `load::load`'s
//! `!has_history` branch — split out of `load.rs` (§1.6) purely to keep
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

use std::time::SystemTime;

use rusqlite::Connection;

use rune_core::buffer::AppliedEdit;

use crate::Error;
use crate::adopt;
use crate::inherit::Inherited;
use crate::observation::{self, StatFacts};
use crate::retry;

/// What the `!has_history` branch of `load` needs back: the content the
/// caller's buffer should adopt, and the durable journal seq of the bridge
/// edit that produced it (`None` when nothing was bridged).
pub(crate) struct AnchorOutcome {
    pub(crate) recovered: String,
    pub(crate) bridge_seq: Option<i64>,
}

/// The load-time facts every anchor/bridge step below needs, bundled so
/// `anchor_first_load`/`anchor_diverged` stay under clippy's
/// too-many-arguments lint without an `#[allow]` (repo rule: none outside
/// test code) — the same "bundle instead of allow" shape `observation.rs`'s
/// `StatFacts`/`ObservationMeta` split already uses.
pub(crate) struct LoadContext<'a> {
    pub(crate) session_id: i64,
    pub(crate) doc_id: i64,
    pub(crate) load_seq: i64,
    pub(crate) disk_hash: &'a str,
    pub(crate) live_stat: &'a StatFacts,
    pub(crate) now: SystemTime,
}

/// Anchors a snapshot + adoption on `content` (disk content), tagged with
/// `hash` — the ordinary "first sighting is the adoption" shape shared by
/// every branch that isn't re-anchoring on a dead session's older baseline.
fn anchor_on_disk(
    conn: &mut Connection,
    ctx: &LoadContext<'_>,
    content: &str,
    hash: &str,
) -> Result<(), Error> {
    retry::with_retry(conn, |tx| {
        crate::snapshot::create_snapshot(
            tx,
            ctx.session_id,
            ctx.now,
            ctx.doc_id,
            content,
            ctx.load_seq,
        )
    })?;
    adopt::record_adoption(
        conn,
        ctx.doc_id,
        ctx.session_id,
        observation::ObservationMeta {
            blob_hash: hash,
            seq: Some(ctx.load_seq),
            origin: "load",
        },
        ctx.live_stat,
        ctx.now,
    )?;
    Ok(())
}

/// Journals ONE synthetic replace-all edit turning `from` into `to`, under
/// `ctx.session_id`, at the very next journal position — exactly as if the
/// user had just pasted `to` in. Returns the durable seq it landed at.
fn bridge_edit(
    conn: &mut Connection,
    ctx: &LoadContext<'_>,
    from: &str,
    to: &str,
) -> Result<i64, Error> {
    let edit = vec![AppliedEdit {
        start: 0,
        end: from.len(),
        deleted: from.to_string(),
        insert: to.to_string(),
    }];
    retry::with_retry(conn, |tx| {
        crate::journal::append_edit(tx, ctx.session_id, ctx.now, ctx.doc_id, &edit, &[], &[])
    })
}

/// Re-anchors on the dead session's own baseline (`H0`), bridges `H0` to
/// `draft`, then records a bare sighting of disk's current content
/// (`ctx.disk_hash`, `H1`) last. `Ok(None)` when `baseline`'s blob is
/// missing/GC'd or not valid UTF-8 (a rare corner — the caller falls back
/// to the plain disk-anchor flow rather than fail the whole load over it).
fn anchor_diverged(
    conn: &mut Connection,
    ctx: &LoadContext<'_>,
    draft: &str,
    baseline: &observation::Observation,
) -> Result<Option<AnchorOutcome>, Error> {
    let Ok(h0_bytes) = retry::with_retry(conn, |tx| crate::blob::get_blob(tx, &baseline.blob_hash))
    else {
        return Ok(None);
    };
    let Ok(h0_content) = String::from_utf8(h0_bytes) else {
        return Ok(None);
    };

    retry::with_retry(conn, |tx| {
        crate::snapshot::create_snapshot(
            tx,
            ctx.session_id,
            ctx.now,
            ctx.doc_id,
            &h0_content,
            ctx.load_seq,
        )
    })?;
    adopt::record_adoption(
        conn,
        ctx.doc_id,
        ctx.session_id,
        observation::ObservationMeta {
            blob_hash: &baseline.blob_hash,
            seq: Some(ctx.load_seq),
            origin: "load",
        },
        &baseline.stat(),
        ctx.now,
    )?;

    let bridge_seq = bridge_edit(conn, ctx, &h0_content, draft)?;

    // Recorded LAST, uncorrelated (`seq: None`) — `newest_observation` must
    // report H1, never the H0 adoption just recorded above. Stat facts come
    // from the LIVE stat (this session's own fresh sighting), matching
    // every other bare-sighting call site.
    let at = crate::session::format_rfc3339_nanos(ctx.now);
    retry::with_retry(conn, |tx| {
        observation::record_observation(
            tx,
            ctx.doc_id,
            ctx.session_id,
            observation::ObservationMeta {
                blob_hash: ctx.disk_hash,
                seq: None,
                origin: "load",
            },
            ctx.live_stat,
            &at,
        )
    })?;

    Ok(Some(AnchorOutcome {
        recovered: draft.to_string(),
        bridge_seq: Some(bridge_seq),
    }))
}

/// The full body of `load`'s `!has_history` branch: anchors this session's
/// recovery snapshot/adoption and, when `inherited` carries a dead
/// session's draft, bridges to it — on `H0` for [`Inherited::Diverged`],
/// on disk content for every other case.
pub(crate) fn anchor_first_load(
    conn: &mut Connection,
    ctx: &LoadContext<'_>,
    content: &str,
    inherited: Inherited,
) -> Result<AnchorOutcome, Error> {
    if let Inherited::Diverged { draft, baseline } = &inherited
        && let Some(outcome) = anchor_diverged(conn, ctx, draft, baseline)?
    {
        return Ok(outcome);
    }

    // Disk, Bridged, or a Diverged baseline whose blob turned out to be
    // unusable — anchor on disk content, exactly like today's flow.
    anchor_on_disk(conn, ctx, content, ctx.disk_hash)?;
    match inherited {
        Inherited::Bridged { draft } | Inherited::Diverged { draft, .. } => {
            let bridge_seq = bridge_edit(conn, ctx, content, &draft)?;
            Ok(AnchorOutcome {
                recovered: draft,
                bridge_seq: Some(bridge_seq),
            })
        }
        Inherited::Disk => Ok(AnchorOutcome {
            recovered: content.to_string(),
            bridge_seq: None,
        }),
    }
}
