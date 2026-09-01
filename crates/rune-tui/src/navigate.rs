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

pub fn follow(app: &mut App, effects: &mut Effects) {
    let offset = app.active_doc().cursors.primary().position.get();
    let Some(target) = link_target_at(&app.active_doc().catalogue, offset) else {
        return;
    };

    if let Target::SameDoc(anchor) = &target {
        follow_same_doc(app, anchor);
        return;
    }

    let doc_dir = app
        .active_doc()
        .path()
        .and_then(|p| p.parent())
        .map(std::path::Path::to_path_buf);
    let destination = rune_nav::resolve(
        app.vfs.as_ref(),
        &target,
        doc_dir.as_deref(),
        app.root.as_deref(),
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

// `site.touches(offset)` is edge-inclusive: a caret at the site's own end,
// where reveal already shows the raw markup, must still follow.
fn link_target_at(catalogue: &[Ref], offset: usize) -> Option<Target> {
    catalogue.iter().find_map(|r| match &r.kind {
        RefKind::Use {
            role: UseRole::Link,
            target,
        } if r.site.touches(offset) => Some(target.clone()),
        _ => None,
    })
}

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

fn follow_location(
    app: &mut App,
    path: &std::path::Path,
    anchor: Option<Anchor>,
    effects: &mut Effects,
) {
    crate::navhistory::record_departure(app, app.active);
    workspace::open_path_async(app, path, anchor, effects);
}

// Forces `id`'s catalogue to exist now, rather than waiting for the lazy
// per-active-document parse, since `id` isn't necessarily the active
// document yet.
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

fn describe_target(target: &Target) -> String {
    match target {
        Target::Url(u) => u.clone(),
        Target::Path { path, .. } => path.clone(),
        Target::Name { name, .. } => name.clone(),
        Target::SameDoc(anchor) => anchor_label(anchor),
    }
}

// `url` is passed to `/usr/bin/open` as a separate argv element, never
// interpolated into a shell string, so a crafted link can never inject a
// command. The scheme allowlist that gates what reaches here at all lives in
// `rune_nav::resolve`, the only producer of `Destination::Url`.
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
