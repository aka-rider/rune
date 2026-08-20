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

pub(crate) fn sync_embeds(app: &mut App, id: DocumentId, effects: &mut Effects) {
    if !app.graphics.kitty {
        return;
    }
    let Some(doc) = app.doc(id) else { return };
    if doc.kind != DocumentKind::Markdown {
        return;
    }

    let content = doc.buffer.content().to_string();
    let starts = rune_md::parse::line_starts(&content);
    let mut anchors: HashMap<usize, &ImageM> = HashMap::new();
    rune_md::snapshot::collect_standalone_images(doc.doc.blocks(), &content, &starts, &mut anchors);
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
            if existing.mtime == mtime || existing.in_flight.is_some() {
                continue;
            }
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
            effects.write(rune_image::encode_delete(state.id.get()).into_bytes());
        }
    }
}

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

fn file_mtime(vfs: &dyn Vfs, abs_path: &Path) -> Option<SystemTime> {
    vfs.stat(abs_path).ok().map(|s| s.mtime)
}
