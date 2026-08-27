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
fn embed_targets_collects_every_image_target_in_document_order() {
    let content = "![a](x.png)\n\n> ![[y]]\n\n- ![z](w.png)\n";
    let blocks = parse(content);
    assert_eq!(
        embed_targets(&blocks),
        vec!["x.png".to_string(), "y".to_string(), "w.png".to_string()]
    );
}

#[test]
fn embed_targets_is_empty_for_a_document_with_no_images() {
    let blocks = parse("plain text, no embeds here\n");
    assert!(embed_targets(&blocks).is_empty());
}

/// `wikilink_role` only ever returns `UseRole::Embed` for a `WikiLink` node
/// whose match is immediately preceded by `'!'` — a shape comrak's own
/// `within_brackets` guard never actually produces (see the function's own
/// docs), so no real `parse()` output can drive this branch. Built and
/// called directly, the same way `setext_heading_name_survives_a_degraded_
/// underline` above pins `heading_name`'s contract on a hand-built input
/// unreachable from real markdown.
#[test]
fn wikilink_role_reads_the_byte_immediately_before_the_match() {
    assert_eq!(wikilink_role("a!bc", 2), UseRole::Embed);
    assert_eq!(wikilink_role("!x", 1), UseRole::Embed);
    assert_eq!(wikilink_role("ax", 1), UseRole::Link);
    assert_eq!(wikilink_role("x", 0), UseRole::Link);
}

/// A setext heading's first content line that is ENTIRELY a run of `#`
/// strips to an EMPTY name: seven hashes is one too many to read as an ATX
/// opener (CommonMark caps that at six), so it survives as ordinary setext
/// TEXT rather than an ATX marker — exactly the shape that makes
/// `closed.len() == trimmed.len()` false (the run of `#` truly did get
/// stripped) while `closed.is_empty()` is true, the one combination that
/// tells `||` and `&&` apart at that branch.
#[test]
fn a_setext_heading_whose_text_is_entirely_hashes_strips_to_an_empty_name() {
    let refs = refs_of("#######\n===\n");
    assert_eq!(
        refs[0].kind,
        RefKind::Def {
            role: DefRole::Heading(1),
            name: String::new(),
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
