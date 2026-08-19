//! `navigate::follow` — following the link under the cursor: ⌘Enter/^Enter
//! and a ctrl-click both funnel through this one entry point.
//! Reads the ACTIVE document's `Document::catalogue` (rebuilt on every
//! `Document::view()` call) to find what the cursor sits on, then dispatches
//! on the `rune_nav::Destination` it resolves to — a same-document heading
//! jump, opening (or reactivating) another document and landing on its
//! anchor, or handing an external URL off to the OS opener. A miss under
//! the cursor, or a target that never resolves, is reported through the
//! status line rather than the error Banner — following a link is routine
//! navigation, not a failure worth interrupting the user over.

use std::process::Command as ProcessCommand;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_nav::{Anchor, AnchorRole, DefRole, Destination, Ref, RefKind, Target, UseRole};

use crate::app::App;
use crate::document::DocumentId;
use crate::messages;
use crate::runtime::{Cmd, Effects, Msg};
use crate::viewport::ScrollMode;
use crate::workspace;

/// Finds the first `Ref` in the ACTIVE document's catalogue whose `site`
/// contains the primary cursor's byte offset and whose kind is a followable
/// link, then dispatches on it. `UseRole::Embed` is catalogued but never
/// followed here (it is an image, not a navigable link); a cursor sitting
/// on neither a link nor an embed is a silent
/// no-op.
pub fn follow(app: &mut App, effects: &mut Effects) {
    let offset = app.active_doc().cursors.primary().position;
    let Some(target) = link_target_at(&app.active_doc().catalogue, offset) else {
        return;
    };

    if let Target::SameDoc(anchor) = &target {
        follow_same_doc(app, anchor);
        return;
    }

    let doc_dir = app
        .active_doc()
        .file_path
        .as_deref()
        .and_then(|p| p.parent())
        .map(std::path::Path::to_path_buf);
    let root = app.root.clone().unwrap_or_default();
    let destination = rune_nav::resolve(
        app.vfs.as_ref(),
        &target,
        doc_dir.as_deref(),
        &root,
        rune_md::catalogue::NAME_RESOLUTION_EXTENSION,
    );

    match destination {
        Destination::Url(url) => effects.cmds.push(open_external_cmd(url)),
        Destination::Location { path, anchor } => {
            follow_location(app, &path, anchor, effects);
        }
        Destination::Unresolved => {
            messages::warn(
                app,
                format!("could not follow link to \"{}\"", describe_target(&target)),
            );
        }
    }
}

/// The followable link, if any, sitting under `offset` — a `Use { role:
/// Link, .. }` whose `site` touches it (edge-inclusive: a caret at the
/// site's own end, where reveal already shows the raw markup, must still
/// follow). Never matches a `Def` or an `Embed`.
fn link_target_at(catalogue: &[Ref], offset: usize) -> Option<Target> {
    catalogue.iter().find_map(|r| match &r.kind {
        RefKind::Use {
            role: UseRole::Link,
            target,
        } if r.site.touches(offset) => Some(target.clone()),
        _ => None,
    })
}

/// `Target::SameDoc` never touches `resolve`/the filesystem: the anchor
/// is searched for directly in the ACTIVE document's own
/// catalogue (`Anchor::Named`) or its buffer (`Anchor::Line`).
fn follow_same_doc(app: &mut App, anchor: &Anchor) {
    let doc = app.active_doc();
    let Some(offset) = anchor_offset(&doc.catalogue, &doc.buffer, anchor) else {
        messages::warn(
            app,
            format!("no match for anchor \"{}\"", anchor_label(anchor)),
        );
        return;
    };
    let doc = app.active_doc_mut();
    doc.cursors = CursorSet::new(offset);
    doc.viewport.mode = ScrollMode::EnsureVisible;
}

/// Opens (or reactivates) the document at `path` and, if `anchor` is
/// `Some`, lands the caret on the matching heading once it's open:
/// `workspace::open_path_async` reads the file
/// off-thread when it isn't already open, so landing the anchor can't
/// happen inline here anymore — it moves into the `Msg::FileOpened` ack
/// reaction ([`land_anchor`] below), reached via `workspace::
/// handle_file_opened`. An already-open target still lands its anchor
/// synchronously, right here, since no read is needed.
fn follow_location(
    app: &mut App,
    path: &std::path::Path,
    anchor: Option<Anchor>,
    effects: &mut Effects,
) {
    crate::navhistory::record_departure(app, app.active);
    workspace::open_path_async(app, path, anchor, effects);
}

/// Lands the caret on `anchor` in the just-opened (or just-reactivated)
/// document `id` — the shared reaction `workspace::open_path_async`'s
/// synchronous reactivation branch AND `workspace::handle_file_opened`'s
/// async ack both call, so the two routes can't drift apart on how an
/// anchor is resolved. Forces `id`'s catalogue to exist NOW through
/// `Document::sync_catalogue` rather than waiting for
/// `App::sync_view`'s lazy per-active-document parse, since
/// `id` isn't necessarily the active (or even the only) document yet.
pub(crate) fn land_anchor(app: &mut App, id: DocumentId, anchor: &Anchor) {
    let Some(doc) = app.doc_mut(id) else {
        return;
    };
    doc.sync_catalogue();
    let Some(offset) = anchor_offset(&doc.catalogue, &doc.buffer, anchor) else {
        messages::warn(
            app,
            format!("no match for anchor \"{}\"", anchor_label(anchor)),
        );
        return;
    };
    let Some(doc) = app.doc_mut(id) else {
        return;
    };
    doc.cursors = CursorSet::new(offset);
    doc.viewport.mode = ScrollMode::EnsureVisible;
}

/// The byte offset `anchor` refers to: a `Named` anchor is name-based and
/// is searched for against `catalogue`'s heading `Def`s via
/// `rune_nav::anchor_matches`; a `Line` anchor is positional — its
/// (1-based) number converts directly to the start of that line in
/// `buffer` and never touches the catalogue at all, so its number can
/// never be silently discarded the way a name-only lookup would force.
fn anchor_offset(catalogue: &[Ref], buffer: &Buffer, anchor: &Anchor) -> Option<usize> {
    match anchor {
        Anchor::Named {
            role: AnchorRole::Heading,
            name,
        } => catalogue.iter().find_map(|r| match &r.kind {
            RefKind::Def {
                role: DefRole::Heading(_),
                name: def_name,
            } if rune_nav::anchor_matches(name, def_name) => Some(r.site.start),
            _ => None,
        }),
        Anchor::Line(n) => buffer.line_start(n.saturating_sub(1) as usize),
    }
}

fn anchor_label(anchor: &Anchor) -> String {
    match anchor {
        Anchor::Named { name, .. } => name.clone(),
        Anchor::Line(n) => format!("line {n}"),
    }
}

/// A human-readable label for a target that failed to resolve — the raw
/// string the user actually wrote, so the status message names something
/// they recognize.
fn describe_target(target: &Target) -> String {
    match target {
        Target::Url(u) => u.clone(),
        Target::Path { path, .. } => path.clone(),
        Target::Name { name, .. } => name.clone(),
        Target::SameDoc(anchor) => anchor_label(anchor),
    }
}

/// The external-URL opener `Cmd`, the exact shape of
/// `clipboard::pbpaste_cmd`: runs off-thread, never touches the terminal.
/// `url` is passed to `/usr/bin/open` as a SEPARATE argv element, never
/// interpolated into a shell string, so a crafted link can never inject a
/// command. The scheme allowlist is enforced inside `rune_nav::resolve`,
/// which is the only thing that can produce the `Destination::Url` this
/// function consumes — so no producer of navigation targets, present or
/// future, can route an arbitrary scheme to this spawn.
fn open_external_cmd(url: String) -> Cmd {
    Cmd::open_external(
        move || match ProcessCommand::new("/usr/bin/open").arg(&url).status() {
            Ok(status) if status.success() => None,
            Ok(status) => Some(Msg::Posted {
                severity: crate::messages::Severity::Warn,
                text: format!("open exited with status {status}"),
            }),
            Err(e) => Some(Msg::Posted {
                severity: crate::messages::Severity::Warn,
                text: format!("could not open {url}: {e}"),
            }),
        },
    )
}
