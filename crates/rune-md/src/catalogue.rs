//! `parse() -> catalogue()`: the producer-agnostic navigation catalogue for
//! a document, built straight off the parsed `Block` tree. Deliberately
//! separate from `emit` — `emit` is width-aware and display-shaped, while a
//! future headless vault indexer has no width and must never run it. This
//! pass is that indexer's entire path, so it must never be folded into the
//! emit walk.

use crate::element::block::{Block, HeadingM};
use crate::element::inline::Inline;
use rune_nav::{Anchor, DefRole, Ref, RefKind, Target, UseRole};

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
    }
}

/// Classify a `LinkM::url` into a `Target`: a leading `#` is a same-document
/// anchor, an external scheme (`rune_nav::is_external`) is a bare URL,
/// anything else is a path, with a trailing `#fragment` split off.
fn classify_link_url(url: &str) -> Target {
    if let Some(rest) = url.strip_prefix('#') {
        return Target::SameDoc(Anchor::Heading(rest.to_string()));
    }
    if rune_nav::is_external(url) {
        return Target::Url(url.to_string());
    }
    match url.split_once('#') {
        Some((path, fragment)) => Target::Path {
            path: path.to_string(),
            anchor: Some(Anchor::Heading(fragment.to_string())),
        },
        None => Target::Path {
            path: url.to_string(),
            anchor: None,
        },
    }
}

/// `[[target#Fragment|label]]` splits `target` on the LAST `#` (mirrors the
/// Go reference's `bytes.LastIndex`), so a target that legitimately embeds
/// an earlier `#` still resolves its trailing anchor correctly.
fn split_wikilink_target(target: &str) -> (String, Option<Anchor>) {
    match target.rfind('#') {
        Some(idx) => (
            target[..idx].to_string(),
            Some(Anchor::Heading(target[idx + 1..].to_string())),
        ),
        None => (target.to_string(), None),
    }
}

/// A `WikiLinkM::range` spans `"[[" target ["|" label] "]]"` INCLUSIVE of
/// the delimiters, so the byte immediately before `range.start` is `'!'`
/// iff the source wrote `![[...]]` — there is no embed flag on the struct
/// itself.
fn wikilink_role(content: &str, range_start: usize) -> UseRole {
    if range_start > 0 && content.as_bytes().get(range_start - 1) == Some(&b'!') {
        UseRole::Embed
    } else {
        UseRole::Link
    }
}

/// Derive a heading's displayed name: the marker's own construction already
/// includes its trailing space, so an ATX heading's name is everything
/// after the marker; a setext heading has no marker (`h.marker` is an empty
/// range) so its name is its first content line instead. Either way, trim
/// ASCII whitespace, strip a trailing run of `#` (CommonMark's closed ATX
/// form, `## Setup ##`), then trim ASCII whitespace again.
fn heading_name(content: &str, h: &HeadingM) -> String {
    let raw = if !h.marker.is_empty() {
        content.get(h.marker.end..h.range.end).unwrap_or("")
    } else {
        h.content_lines
            .first()
            .and_then(|r| content.get(r.start..r.end))
            .unwrap_or("")
    };
    let trimmed = raw.trim_matches(|c: char| c.is_ascii_whitespace());
    let trimmed = trimmed.trim_end_matches('#');
    trimmed
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
                target: Target::SameDoc(Anchor::Heading("Setup".to_string())),
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
                    anchor: Some(Anchor::Heading("Setup".to_string())),
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
        // empirically: `![[note]]` produces a plain `Inline::Text`, never a
        // `WikiLink` node, so no embed edge ever reaches the catalogue.
        // Pinned here so a comrak upgrade that changes this is caught here,
        // not downstream.
        let content = "![[note]]\n";
        let blocks = parse(content);
        assert!(matches!(
            blocks.first(),
            Some(Block::Paragraph(p)) if matches!(p.inlines.as_slice(), [Inline::Text(_)])
        ));
        let refs = catalogue(content, &blocks);
        assert!(refs.is_empty(), "expected no refs, got {refs:?}");
    }
}
