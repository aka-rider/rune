//! `sync_embeds` (plan WP9.S4/S5/S7): the spawn/despawn reconciler for the
//! ACTIVE document's inline embeds, scoped to Kitty-only placement (decision
//! 2, no iTerm2 half) with no animation frame ids yet (WP10's own scope —
//! none exist yet for an embed).
//!
//! Spawn and despawn are DELIBERATELY asymmetric (plan gotcha 2): spawn
//! only ever considers a RENDERED-only standalone image line
//! (`rune_md::snapshot::collect_standalone_images`, which already refuses
//! anything else — see its own docs); despawn considers ANY embed target
//! anywhere in the parse tree, rendered or revealed
//! (`rune_md::catalogue::embed_targets`). Otherwise moving the caret onto
//! an embed line — which reveals its raw `![alt](path)` source and so
//! drops out of the standalone-only spawn set — would immediately despawn
//! a live image the instant the caret arrived, then respawn it the instant
//! the caret left.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use rune_md::element::inline::ImageM;
use rune_nav::Destination;
use rune_syntax::DocumentKind;
use rune_vfs::Vfs;

use super::EmbedState;
use super::decode::schedule_embed_decode;
use crate::app::App;
use crate::document::DocumentId;
use crate::graphics::ImageStatus;
use crate::runtime::Effects;

/// Reconciles the embed set of the document named by `id` against its
/// current content (plan WP9.S4) — called from `dispatch::after_update`
/// (the same post-dispatch chokepoint `schedule_highlight`/
/// `schedule_image_decode` already funnel through) after every message, so
/// no future edit path can forget to keep the embed set current. A no-op
/// when Kitty isn't available (nothing to spawn INTO) or the document isn't
/// a markdown one (an image document has no embeds of its own; every other
/// kind has no images at all — plan Context).
pub(crate) fn sync_embeds(app: &mut App, id: DocumentId, effects: &mut Effects) {
    if !app.graphics.kitty {
        return;
    }
    let Some(doc) = app.doc(id) else { return };
    if doc.kind != DocumentKind::Markdown {
        return;
    }

    // Every field pulled out below is OWNED (never a borrow of `doc`
    // itself): the spawn/despawn passes further down need `App::doc_mut`,
    // which cannot coexist with an immutable borrow still rooted in `doc`.
    let content = doc.buffer.content().to_string();
    let starts = rune_md::parse::line_starts(&content);
    let mut anchors: HashMap<usize, &ImageM> = HashMap::new();
    rune_md::snapshot::collect_standalone_images(doc.doc.blocks(), &content, &starts, &mut anchors);
    // `HashMap` iteration order is arbitrary, so when the same target
    // appears on more than one line, which line's `ImageM` survives the
    // dedupe below must not depend on it — sort by line first so the
    // earliest line deterministically wins, run over run.
    let mut by_line: Vec<(usize, &ImageM)> = anchors.into_iter().collect();
    by_line.sort_by_key(|(line, _)| *line);
    let mut seen = HashSet::new();
    let mut standalone: Vec<ImageM> = Vec::new();
    for (_, m) in by_line {
        if seen.insert(m.target_text.clone()) {
            standalone.push(m.clone());
        }
    }
    let present: HashSet<String> = rune_md::catalogue::embed_targets(doc.doc.blocks())
        .into_iter()
        .collect();

    let doc_dir = doc
        .file_path
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf);
    let root = app.root.clone().unwrap_or_default();
    let vfs = Arc::clone(&app.vfs);

    spawn_or_respawn(
        app,
        id,
        &standalone,
        doc_dir.as_deref(),
        &root,
        &vfs,
        effects,
    );
    despawn_gone(app, id, &present, effects);
}

fn spawn_or_respawn(
    app: &mut App,
    id: DocumentId,
    standalone: &[ImageM],
    doc_dir: Option<&Path>,
    root: &Path,
    vfs: &Arc<dyn Vfs + Send + Sync>,
    effects: &mut Effects,
) {
    for m in standalone {
        let target = m.target_text.as_str();
        let Some(abs_path) = resolve_embed_path(vfs.as_ref(), m, doc_dir, root) else {
            continue;
        };
        let mtime = file_mtime(vfs.as_ref(), &abs_path);

        let Some(doc) = app.doc_mut(id) else { return };
        let Some(embeds) = doc.ensure_embeds() else {
            continue;
        };
        if let Some(existing) = embeds.images.get(target) {
            // Retry rule (plan WP9.S5): an unchanged mtime never respawns
            // (Failed is sticky per (path, mtime)); an in-flight decode
            // never respawns either — it must run to completion first.
            if existing.mtime == mtime || existing.in_flight.is_some() {
                continue;
            }
            // Plan gotcha 3: on an mtime respawn, delete FRAME ids only,
            // never the base id — the respawn reuses the base id and its
            // own retransmit would race a base-id delete. The allocator
            // entry for that id is therefore left exactly as it is:
            // allocator entries stay until despawn's FreeAllForPath —
            // conservative, no reuse, no collision.
            // WP9 tracks no frame ids at all (animation is WP10's scope),
            // so there is nothing else to delete here.
            let Some(state) = embeds.images.get_mut(target) else {
                continue;
            };
            state.abs_path = abs_path;
            state.mtime = mtime;
            state.status = ImageStatus::Pending;
            state.dims = None;
            state.in_flight = None;
            schedule_embed_decode(app, id, target, effects);
            continue;
        }

        let key = abs_path.to_string_lossy().into_owned();
        let embed_id = embeds.alloc.alloc_free_id(&key);
        embeds.images.insert(
            target.to_string(),
            EmbedState {
                abs_path,
                id: embed_id,
                mtime,
                dims: None,
                status: ImageStatus::Pending,
                in_flight: None,
            },
        );
        schedule_embed_decode(app, id, target, effects);
    }
}

/// Reload's embed counterpart (plan WP2.S4): an embed whose decode reply
/// was ever lost leaves `in_flight` set forever, and `spawn_or_respawn`'s
/// own retry rule refuses to touch anything already `in_flight` — so
/// without this, a wedged embed had no recovery path at all, unlike a whole
/// image document (`reload_image`, WP2.S2). Abandons every currently in-
/// flight embed for the ACTIVE document (clearing `in_flight` first, so
/// `schedule_embed_decode`'s own in-flight guard doesn't refuse the
/// respawn) and immediately respawns each one. The abandoned reply is then
/// dropped harmlessly by `handle_embed_decoded`'s own generation search: it
/// looks for the `EmbedState` whose `in_flight` still equals the OLD
/// generation, and once the respawn below has overwritten it with a new
/// one, that search simply finds nothing. Called from the same `⌘R`
/// dispatch that already calls `reload_image` — a no-op for a document with
/// no embeds at all, or none currently wedged.
pub(crate) fn reload_embeds(app: &mut App, id: DocumentId, effects: &mut Effects) {
    let Some(doc) = app.doc(id) else { return };
    let Some(embeds) = doc.embeds() else { return };
    let wedged: Vec<String> = embeds
        .images
        .iter()
        .filter(|(_, s)| s.in_flight.is_some())
        .map(|(k, _)| k.clone())
        .collect();
    for target in &wedged {
        if let Some(doc) = app.doc_mut(id)
            && let Some(embeds) = doc.embeds_mut()
            && let Some(state) = embeds.images.get_mut(target.as_str())
        {
            state.in_flight = None;
        }
        schedule_embed_decode(app, id, target, effects);
    }
}

fn despawn_gone(app: &mut App, id: DocumentId, present: &HashSet<String>, effects: &mut Effects) {
    let Some(doc) = app.doc_mut(id) else { return };
    let Some(embeds) = doc.embeds_mut() else {
        return;
    };
    let gone: Vec<String> = embeds
        .images
        .keys()
        .filter(|k| !present.contains(k.as_str()))
        .cloned()
        .collect();
    for key in gone {
        if let Some(state) = embeds.images.remove(&key) {
            embeds.alloc.free_all_for(&state.abs_path.to_string_lossy());
            effects
                .raw
                .push(rune_image::encode_delete(state.id.get()).into_bytes());
        }
    }
}

/// Resolves an embed's target to an absolute path (plan WP9.S7): decode,
/// trim, strip a leading `./`, try the document's own directory then the
/// workspace root — all reused directly from `rune_nav::resolve`
/// (`navigate::follow`'s own resolver), never reimplemented here. The
/// `Target` itself comes from `rune_md::catalogue::image_target`, the same
/// classification a `WikiLink`/`Link` node gets, so an embed and a link
/// sharing the same raw text can never resolve through different policy.
/// `None` when nothing on disk answers to the target — the embed is simply
/// not spawned this pass (plan: "an absolute path resolves only if it
/// exists").
fn resolve_embed_path(
    vfs: &dyn Vfs,
    m: &ImageM,
    doc_dir: Option<&Path>,
    root: &Path,
) -> Option<PathBuf> {
    let target = rune_md::catalogue::image_target(m);
    match rune_nav::resolve(
        vfs,
        &target,
        doc_dir,
        root,
        rune_md::catalogue::NAME_RESOLUTION_EXTENSION,
    ) {
        Destination::Location { path, .. } => Some(path),
        _ => None,
    }
}

/// `abs_path`'s current mtime, or `None` when the stat fails — the
/// "unavailable" sentinel.
fn file_mtime(vfs: &dyn Vfs, abs_path: &Path) -> Option<SystemTime> {
    vfs.stat(abs_path).ok().map(|s| s.mtime)
}
