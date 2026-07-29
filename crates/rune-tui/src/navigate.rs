//! `navigate::follow` — following the link under the cursor (plan WP5):
//! ⌘Enter/^Enter and a ctrl-click both funnel through this one entry point.
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

use crate::app::{App, StatusSource};
use crate::runtime::{Cmd, CmdKind, Effects, Msg};
use crate::workspace;

/// Finds the first `Ref` in the ACTIVE document's catalogue whose `site`
/// contains the primary cursor's byte offset and whose kind is a followable
/// link, then dispatches on it. `UseRole::Embed` is catalogued but never
/// followed here (it is an image, not a navigable link — matching the Go
/// reference); a cursor sitting on neither a link nor an embed is a silent
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
    let root = app.root.clone();
    let destination = rune_nav::resolve(
        app.vfs.as_ref(),
        &target,
        doc_dir.as_deref(),
        &root,
        rune_md::catalogue::NAME_RESOLUTION_EXTENSION,
    );

    match destination {
        Destination::Url(url) => effects.cmds.push(open_external_cmd(url)),
        Destination::Location { path, anchor } => follow_location(app, &path, anchor),
        Destination::Unresolved => {
            app.set_status(
                format!("could not follow link to \"{}\"", describe_target(&target)),
                StatusSource::Other,
            );
        }
    }
}

/// The followable link, if any, sitting under `offset` — a `Use { role:
/// Link, .. }` whose `site` contains it. Never matches a `Def` or an
/// `Embed`.
fn link_target_at(catalogue: &[Ref], offset: usize) -> Option<Target> {
    catalogue.iter().find_map(|r| match &r.kind {
        RefKind::Use {
            role: UseRole::Link,
            target,
        } if r.site.contains(offset) => Some(target.clone()),
        _ => None,
    })
}

/// `Target::SameDoc` never touches `resolve`/the filesystem (plan WP5.S5):
/// the anchor is searched for directly in the ACTIVE document's own
/// catalogue (`Anchor::Named`) or its buffer (`Anchor::Line`).
fn follow_same_doc(app: &mut App, anchor: &Anchor) {
    let doc = app.active_doc();
    let Some(offset) = anchor_offset(&doc.catalogue, &doc.buffer, anchor) else {
        app.set_status(
            format!("no match for anchor \"{}\"", anchor_label(anchor)),
            StatusSource::Other,
        );
        return;
    };
    app.active_doc_mut().cursors = CursorSet::new(offset);
}

/// Opens (or reactivates) the document at `path` and, if `anchor` is
/// `Some`, lands the caret on the matching heading. `workspace::open_path`
/// only ever inserts/switches to the document — parsing otherwise happens
/// lazily in `App::sync_view`, which runs AFTER `update` returns and syncs
/// only the (now) active document. Landing on an anchor needs the target's
/// blocks and catalogue NOW, so this forces exactly the same parse
/// `Document::view` would eventually do, without the width-dependent wrap
/// pass `view` also runs (this document isn't necessarily on screen yet).
fn follow_location(app: &mut App, path: &std::path::Path, anchor: Option<Anchor>) {
    let Some(id) = workspace::open_path(app, path) else {
        return;
    };
    let Some(anchor) = anchor else {
        return;
    };
    let Some(doc) = app.doc_mut(id) else {
        return;
    };
    doc.doc.sync_content(&doc.buffer);
    doc.catalogue = rune_md::catalogue::catalogue(doc.buffer.content(), doc.doc.blocks());
    let Some(offset) = anchor_offset(&doc.catalogue, &doc.buffer, &anchor) else {
        app.set_status(
            format!("no match for anchor \"{}\"", anchor_label(&anchor)),
            StatusSource::Other,
        );
        return;
    };
    let Some(doc) = app.doc_mut(id) else {
        return;
    };
    doc.cursors = CursorSet::new(offset);
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

/// The external-URL opener `Cmd` (plan WP5.S6), the exact shape of
/// `clipboard::pbpaste_cmd`: runs off-thread, never touches the terminal.
/// `url` is passed to `/usr/bin/open` as a SEPARATE argv element, never
/// interpolated into a shell string, so a crafted link can never inject a
/// command. The scheme allowlist is enforced inside `rune_nav::resolve`,
/// which is the only thing that can produce the `Destination::Url` this
/// function consumes — so no producer of navigation targets, present or
/// future, can route an arbitrary scheme to this spawn.
fn open_external_cmd(url: String) -> Cmd {
    Cmd::new(CmdKind::OpenExternal, move || {
        match ProcessCommand::new("/usr/bin/open").arg(&url).status() {
            Ok(status) if status.success() => None,
            Ok(status) => Some(Msg::Error(format!("open exited with status {status}"))),
            Err(e) => Some(Msg::Error(format!("could not open {url}: {e}"))),
        }
    })
}
