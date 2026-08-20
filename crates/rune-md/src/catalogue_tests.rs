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
