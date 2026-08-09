//! `parse() -> catalogue()`: the producer-agnostic navigation catalogue for
//! a document, built straight off the parsed `Block` tree. Deliberately
//! separate from `emit` — `emit` is width-aware and display-shaped, while a
//! future headless vault indexer has no width and must never run it. This
//! pass is that indexer's entire path, so it must never be folded into the
//! emit walk.

use crate::element::block::{Block, HeadingM};
use crate::element::inline::{ImageM, Inline};
use rune_nav::{Anchor, AnchorRole, DefRole, Ref, RefKind, Target, UseRole};

/// The extension `rune_nav::resolve` appends to an extension-less
/// `Target::Name` candidate — this producer's resolution policy (plan
/// WP12.S1): rune-nav owns no file-type opinion of its own, so the
/// producer supplies it.
pub const NAME_RESOLUTION_EXTENSION: &str = "md";

/// Every embed's own raw target text (`ImageM::target_text`), from every
/// `Inline::Image` node anywhere in `blocks` — plan WP9.S4's "present" set,
/// which a caller (`rune-tui`'s embed reconciler) uses to decide which
/// tracked embeds to despawn. Deliberately reveal-independent: `blocks` is
/// the PARSE tree, built before any `RevealSm::sync` transition ever runs,
/// so an embed target found here is present whether its own line is
/// currently Rendered (a placeholder showing) or Revealed (raw source under
/// the caret) — the same "rendered OR revealed" union the plan's own
/// despawn rule requires, for free, just by reading the parse tree instead
/// of the emit output. A separate, narrower walk (`catalogue` itself, or
/// `rune-md::snapshot::collect_standalone_images`) answers the "rendered
/// AND alone on its line" spawn-source question instead.
pub fn embed_targets(blocks: &[Block]) -> Vec<String> {
    let mut out = Vec::new();
    for block in blocks {
        collect_embed_targets_in_block(block, &mut out);
    }
    out
}

fn collect_embed_targets_in_block(block: &Block, out: &mut Vec<String>) {
    match block {
        Block::Paragraph(m) => collect_embed_targets_in_inlines(&m.inlines, out),
        Block::Heading(h) => collect_embed_targets_in_inlines(&h.inlines, out),
        Block::Blockquote(bq) => {
            for c in &bq.children {
                collect_embed_targets_in_block(c, out);
            }
        }
        Block::List(list) => {
            for item in &list.items {
                for c in &item.children {
                    collect_embed_targets_in_block(c, out);
                }
            }
        }
        Block::Table(t) => {
            for row in &t.rows {
                for cell in &row.cells {
                    collect_embed_targets_in_inlines(&cell.inlines, out);
                }
            }
        }
        Block::CodeFence(_)
        | Block::ThematicBreak(_)
        | Block::Frontmatter(_)
        | Block::Verbatim(_) => {}
    }
}

fn collect_embed_targets_in_inlines(inlines: &[Inline], out: &mut Vec<String>) {
    for inline in inlines {
        match inline {
            Inline::Image(m) => out.push(m.target_text.clone()),
            Inline::Emphasis(m) => collect_embed_targets_in_inlines(&m.children, out),
            Inline::Link(m) => collect_embed_targets_in_inlines(&m.text, out),
            Inline::Text(_) | Inline::Code(_) | Inline::WikiLink(_) => {}
        }
    }
}

/// Build a name-based `Anchor` from a markdown heading fragment.
fn heading_anchor(name: String) -> Anchor {
    Anchor::Named {
        role: AnchorRole::Heading,
        name,
    }
}

/// Walk `blocks` in document order, recursing into every inline child, and
/// return every navigable `Ref` found, sorted by `site.start`.
pub fn catalogue(content: &str, blocks: &[Block]) -> Vec<Ref> {
    let mut out = Vec::new();
    for block in blocks {
        walk_block(content, block, &mut out);
    }
    out.sort_by_key(|r| r.site.start);
    out
}

fn walk_block(content: &str, block: &Block, out: &mut Vec<Ref>) {
    match block {
        Block::Paragraph(m) => walk_inlines(content, &m.inlines, out),
        Block::Heading(h) => {
            out.push(Ref {
                site: h.range,
                kind: RefKind::Def {
                    role: DefRole::Heading(h.level),
                    name: heading_name(content, h),
                },
            });
            walk_inlines(content, &h.inlines, out);
        }
        Block::Blockquote(bq) => {
            for c in &bq.children {
                walk_block(content, c, out);
            }
        }
        Block::List(list) => {
            for item in &list.items {
                for c in &item.children {
                    walk_block(content, c, out);
                }
            }
        }
        Block::Table(t) => {
            for row in &t.rows {
                for cell in &row.cells {
                    walk_inlines(content, &cell.inlines, out);
                }
            }
        }
        // No inline children and nothing navigable of their own.
        Block::CodeFence(_)
        | Block::ThematicBreak(_)
        | Block::Frontmatter(_)
        | Block::Verbatim(_) => {}
    }
}

fn walk_inlines(content: &str, inlines: &[Inline], out: &mut Vec<Ref>) {
    for inline in inlines {
        walk_inline(content, inline, out);
    }
}

fn walk_inline(content: &str, inline: &Inline, out: &mut Vec<Ref>) {
    match inline {
        Inline::Text(_) | Inline::Code(_) => {}
        Inline::Emphasis(m) => walk_inlines(content, &m.children, out),
        Inline::Link(m) => {
            out.push(Ref {
                site: m.range,
                kind: RefKind::Use {
                    role: UseRole::Link,
                    target: classify_link_url(&m.url),
                },
            });
            walk_inlines(content, &m.text, out);
        }
        Inline::WikiLink(m) => {
            let (name, anchor) = split_wikilink_target(&m.target);
            out.push(Ref {
                site: m.range,
                kind: RefKind::Use {
                    role: wikilink_role(content, m.range.start),
                    target: Target::Name { name, anchor },
                },
            });
        }
        Inline::Image(m) => {
            // Both image forms are always an embed (`UseRole::Embed`):
            // `![alt](url)` has no non-embed reading, and `![[target]]` is
            // recovered ONLY under the `!` prefix (`parse::inline`'s
            // scanner never matches a bare `[[target]]`) — unlike a real
            // `WikiLink` node, there is no bare-vs-embedded fork to make
            // here at all, so this never calls `wikilink_role`.
            out.push(Ref {
                site: m.range,
                kind: RefKind::Use {
                    role: UseRole::Embed,
                    target: image_target(m),
                },
            });
        }
    }
}

/// An inline image's own navigation `Target`, the SAME classification a
/// `WikiLink`/`Link` node gets (`split_wikilink_target`/`classify_link_url`)
/// — the one chokepoint both this catalogue walk and `rune-tui`'s embed
/// reconciler call, so an image embed and a followable link can never
/// classify the same raw text two different ways.
pub fn image_target(m: &ImageM) -> Target {
    if m.is_wikilink {
        let (name, anchor) = split_wikilink_target(&m.target_text);
        Target::Name { name, anchor }
    } else {
        classify_link_url(&m.target_text)
    }
}

/// Classify a `LinkM::url` into a `Target`: a leading `#` is a same-document
/// anchor, an external scheme (`rune_nav::is_external`) is a bare URL,
/// anything else is a path, with a trailing `#fragment` split off.
fn classify_link_url(url: &str) -> Target {
    if let Some(rest) = url.strip_prefix('#') {
        return Target::SameDoc(heading_anchor(rest.to_string()));
    }
    if let Some(approved) = rune_nav::is_external(url) {
        return Target::Url(approved);
    }
    match url.split_once('#') {
        Some((path, fragment)) => Target::Path {
            path: path.to_string(),
            anchor: Some(heading_anchor(fragment.to_string())),
        },
        None => Target::Path {
            path: url.to_string(),
            anchor: None,
        },
    }
}

/// `[[target#Fragment|label]]` splits `target` on the LAST `#`, so a target
/// that legitimately embeds an earlier `#` still resolves its trailing
/// anchor correctly.
fn split_wikilink_target(target: &str) -> (String, Option<Anchor>) {
    match target.rfind('#') {
        Some(idx) => (
            target[..idx].to_string(),
            Some(heading_anchor(target[idx + 1..].to_string())),
        ),
        None => (target.to_string(), None),
    }
}

/// A `WikiLinkM::range` spans `"[[" target ["|" label] "]]"` INCLUSIVE of
/// the delimiters, so the byte immediately before `range.start` is `'!'`
/// iff the source wrote `![[...]]` — there is no embed flag on the struct
/// itself. In practice this branch never fires: comrak's own wikilink
/// trigger has a `within_brackets` guard that suppresses the `WikiLink`
/// node entirely under a leading `!` (verified empirically, pinned by
/// `embed_prefixed_wikilink_comrak_behaviour_is_pinned` below), so a real
/// `WikiLink` node's `range` never has `'!'` immediately before it. As of
/// WP7, `![[target]]` is recovered separately — by `parse::inline`'s text
/// scanner, as an `Inline::Image` — and that arm above resolves its own
/// `UseRole::Embed` directly, never through this function. Kept rather than
/// deleted in case a future comrak version starts allowing the node.
fn wikilink_role(content: &str, range_start: usize) -> UseRole {
    if range_start > 0 && content.as_bytes().get(range_start - 1) == Some(&b'!') {
        UseRole::Embed
    } else {
        UseRole::Link
    }
}

/// Derive a heading's displayed name: the marker's own construction already
/// includes its trailing space, so an ATX heading's name is everything
/// after the marker; a setext heading (`h.setext`) has no marker so its
/// name is its first content line instead. Branches on `h.setext`, not
/// `h.underline` — `underline` can be `None` on a genuinely setext heading
/// (a defensive guard elsewhere degrades it when comrak's own inline tree
/// desyncs from its block tree), and this function must still find the
/// first content line in that case. Either way, trim ASCII whitespace,
/// strip a trailing run of `#` when CommonMark says it closes the heading,
/// then trim ASCII whitespace again.
fn heading_name(content: &str, h: &HeadingM) -> String {
    let raw = if h.setext {
        h.content_lines
            .first()
            .and_then(|r| content.get(r.start..r.end))
            .unwrap_or("")
    } else {
        content.get(h.marker.end..h.range.end).unwrap_or("")
    };
    let trimmed = raw.trim_matches(|c: char| c.is_ascii_whitespace());
    // A run of `#` closes an ATX heading only when preceded by whitespace, or
    // when it is the whole content. So `## Setup ##` is named `Setup`, while
    // `## C#` keeps its own trailing `#` and is named `C#`.
    let closed = trimmed.trim_end_matches('#');
    let content = if closed.len() == trimmed.len()
        || closed.is_empty()
        || closed.ends_with(|c: char| c.is_ascii_whitespace())
    {
        closed
    } else {
        trimmed
    };
    content
        .trim_matches(|c: char| c.is_ascii_whitespace())
        .to_string()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::parse::parse;

    fn refs_of(content: &str) -> Vec<Ref> {
        let blocks = parse(content);
        catalogue(content, &blocks)
    }

    #[test]
    fn relative_link_becomes_a_path_use() {
        let refs = refs_of("[a](./b.md)\n");
        assert_eq!(refs.len(), 1);
        assert_eq!(
            refs[0].kind,
            RefKind::Use {
                role: UseRole::Link,
                target: Target::Path {
                    path: "./b.md".to_string(),
                    anchor: None,
                },
            }
        );
    }

    #[test]
    fn external_link_becomes_a_url_use() {
        let refs = refs_of("[a](https://x.com)\n");
        assert_eq!(refs.len(), 1);
        assert_eq!(
            refs[0].kind,
            RefKind::Use {
                role: UseRole::Link,
                target: Target::Url("https://x.com".to_string()),
            }
        );
    }

    #[test]
    fn hash_link_becomes_a_same_doc_use() {
        let refs = refs_of("[a](#Setup)\n");
        assert_eq!(refs.len(), 1);
        assert_eq!(
            refs[0].kind,
            RefKind::Use {
                role: UseRole::Link,
                target: Target::SameDoc(heading_anchor("Setup".to_string())),
            }
        );
    }

    #[test]
    fn wikilink_with_anchor_becomes_a_name_use() {
        let refs = refs_of("[[note#Setup]]\n");
        assert_eq!(refs.len(), 1);
        assert_eq!(
            refs[0].kind,
            RefKind::Use {
                role: UseRole::Link,
                target: Target::Name {
                    name: "note".to_string(),
                    anchor: Some(heading_anchor("Setup".to_string())),
                },
            }
        );
    }

    #[test]
    fn atx_heading_becomes_a_def() {
        let refs = refs_of("## Setup\n");
        assert_eq!(refs.len(), 1);
        assert_eq!(
            refs[0].kind,
            RefKind::Def {
                role: DefRole::Heading(2),
                name: "Setup".to_string(),
            }
        );
    }

    #[test]
    fn closed_atx_heading_strips_the_trailing_hashes() {
        let refs = refs_of("## Setup ##\n");
        assert_eq!(refs.len(), 1);
        assert_eq!(
            refs[0].kind,
            RefKind::Def {
                role: DefRole::Heading(2),
                name: "Setup".to_string(),
            }
        );
    }

    /// A trailing `#` NOT preceded by whitespace is part of the heading's
    /// text, not a CommonMark closing sequence — `## C#` is a language name,
    /// and mangling it to `C` would make `[[note#C#]]` unmatchable.
    #[test]
    fn a_trailing_hash_without_a_preceding_space_stays_in_the_name() {
        for (src, expected) in [("## C#\n", "C#"), ("## F#\n", "F#"), ("## a#b#\n", "a#b#")] {
            let refs = refs_of(src);
            assert_eq!(
                refs[0].kind,
                RefKind::Def {
                    role: DefRole::Heading(2),
                    name: expected.to_string(),
                },
                "{src:?} should name the heading {expected:?}"
            );
        }
    }

    #[test]
    fn setext_heading_derives_its_name_from_the_content_line() {
        let refs = refs_of("Setup\n=====\n");
        assert_eq!(refs.len(), 1);
        assert_eq!(
            refs[0].kind,
            RefKind::Def {
                role: DefRole::Heading(1),
                name: "Setup".to_string(),
            }
        );
    }

    /// Reproduces the desync the defensive guard in
    /// `underline_of_setext_heading` degrades to `None`: not reachable from
    /// real markdown input in this repro (the guard needs a genuine
    /// comrak inline/block tree disagreement we could not trigger through
    /// `parse()` alone), so this builds the `HeadingM` directly to pin the
    /// contract `heading_name` must honor regardless of `underline`.
    #[test]
    fn setext_heading_name_survives_a_degraded_underline() {
        use rune_syntax::element::{ByteRange, RevealSm, RevealState};

        let content = "Setup\n---\n";
        let h = HeadingM {
            sm: RevealSm::new(RevealState::Rendered),
            level: 2,
            line: 0,
            last_line: 1,
            range: ByteRange::new(0, content.len()),
            setext: true,
            marker: ByteRange::new(0, 0),
            underline: None,
            inlines: Vec::new(),
            content_lines: vec![ByteRange::new(0, 6), ByteRange::new(6, 10)],
        };
        assert_eq!(heading_name(content, &h), "Setup");
    }

    #[test]
    fn a_bare_url_in_prose_becomes_a_url_use() {
        let refs = refs_of("See https://example.com for details.\n");
        assert_eq!(refs.len(), 1);
        assert_eq!(
            refs[0].kind,
            RefKind::Use {
                role: UseRole::Link,
                target: Target::Url("https://example.com".to_string()),
            }
        );
    }

    #[test]
    fn a_wikilink_containing_a_raw_newline_produces_no_ref() {
        // The parser deliberately rebuilds a subtree containing such a
        // wikilink as plain text to dodge a comrak sourcepos bug — no
        // WikiLink node ever reaches the catalogue walk.
        let refs = refs_of("[[\n]]\n");
        assert!(refs.is_empty(), "expected no refs, got {refs:?}");
    }

    #[test]
    fn embed_prefixed_wikilink_comrak_behaviour_is_pinned() {
        // comrak's wikilink trigger has a `within_brackets` guard that
        // suppresses the wikilink entirely under a leading `!` — verified
        // empirically: comrak itself never emits a `WikiLink` node for
        // `![[note]]`. Pinned here so a comrak upgrade that changes this is
        // caught here, not downstream. As of WP7 this crate recovers the
        // embed anyway, by scanning the flattened `Text` run comrak hands
        // back instead (`parse::inline`'s `![[target]]` scanner) — so the
        // catalogue still sees an embed `Ref`, just built from
        // `Inline::Image`, never `Inline::WikiLink`.
        let content = "![[note]]\n";
        let blocks = parse(content);
        assert!(matches!(
            blocks.first(),
            Some(Block::Paragraph(p)) if matches!(p.inlines.as_slice(), [Inline::Image(_)])
        ));
        let refs = catalogue(content, &blocks);
        assert_eq!(refs.len(), 1);
        assert_eq!(
            refs[0].kind,
            RefKind::Use {
                role: UseRole::Embed,
                target: Target::Name {
                    name: "note".to_string(),
                    anchor: None,
                },
            }
        );
    }

    #[test]
    fn markdown_image_becomes_an_embed_use() {
        let refs = refs_of("![alt](x.png)\n");
        assert_eq!(refs.len(), 1);
        assert_eq!(
            refs[0].kind,
            RefKind::Use {
                role: UseRole::Embed,
                target: Target::Path {
                    path: "x.png".to_string(),
                    anchor: None,
                },
            }
        );
    }
}
