//! Inline element machines (plan Context, "Block and inline machines").
//! Nested styling (bold-inside-italic) falls out of the tree itself via the
//! emitter's style stack — there is no `InlineMarks` bitfield here.

use std::collections::{HashMap, HashSet};

use rune_syntax::element::{ByteRange, InheritCtx, RevealSm, RevealState};

/// Plain, unconcealable text — no machine (plan: "Text(TextRun), // {
/// range } — verbatim, no machine"). Any inline node kind this crate
/// doesn't otherwise model (raw HTML, a hard line break, ...) degrades to
/// this. `![alt](url)`/`![[target]]` images are `Inline::Image` (`ImageM`,
/// below) as of WP7 — they used to flatten to a plain text run here
/// (Phase-1 scope), no longer.
#[derive(Clone, Debug)]
pub struct TextRun {
    pub range: ByteRange,
    /// One entry per physical line `range` spans (AS COMRAK PARSED
    /// THEM) — the emit path iterates this instead of pushing `range`
    /// whole through the generic per-line splitter (verification round
    /// 9's exhaustive audit: an unmodeled inline node — e.g. a raw
    /// `<span\n...>` HTML tag — can span multiple lines exactly like a
    /// fence or a setext heading can, and `range` alone can't exclude an
    /// interior container prefix any more than `CodeFenceM::range`/
    /// `VerbatimM::range` can). Always single-entry `vec![range]` for a
    /// genuinely single-line run — the overwhelmingly common case.
    pub content_lines: Vec<ByteRange>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmphasisKind {
    Bold,
    Italic,
    /// Reserved for a single node representing both at once. comrak always
    /// nests distinct `Strong`/`Emph` nodes for `***text***`, so the parser
    /// never constructs this variant directly — nesting `Bold { Italic }`
    /// already renders correctly through the tree (plan: "Nested styling
    /// ... falls out of the tree via the Emitter's style stack").
    BoldItalic,
    Strike,
}

/// Decide policy: `cursors.any_in(range)` — the OUTER token range, so
/// nesting reveals as a unit via `ctx.child` (the reveal-policy table).
#[derive(Clone, Debug)]
pub struct EmphasisM {
    pub sm: RevealSm,
    pub kind: EmphasisKind,
    pub range: ByteRange,
    pub open: ByteRange,
    pub close: ByteRange,
    pub children: Vec<Inline>,
    pub line: usize,
    /// One entry per physical line `range` spans (AS COMRAK PARSED
    /// THEM) — the Revealed emit path iterates this instead of pushing
    /// `range` whole (verification round 9's exhaustive audit: emphasis/
    /// strong/strikethrough content can soft-wrap across lines exactly
    /// like a fence or a table can, e.g. `"> *a\n> b*"`, and `range`
    /// alone can't exclude the container's own repeating prefix on the
    /// continuation line any more than `VerbatimM::range` could). `open`
    /// and `close` never need this: both are always a short, contiguous
    /// delimiter run (`"**"`, `"~~"`, ...) that can't itself contain a
    /// newline, and `child_gap_delims` bounds them against a CHILD
    /// node's own (individually reliable) position, never raw arithmetic
    /// across a line boundary.
    pub content_lines: Vec<ByteRange>,
}

impl EmphasisM {
    fn sync(&mut self, ctx: &InheritCtx) -> bool {
        let range = self.range;
        let want = ctx.grant.resolve(|| ctx.cursors.any_in(range));
        let mut dirty = self.sm.transition(want);
        let child_ctx = ctx.child(self.sm.state());
        for c in &mut self.children {
            dirty |= c.sync(&child_ctx);
        }
        dirty
    }
}

#[derive(Clone, Debug)]
pub struct InlineCodeM {
    sm: RevealSm,
    range: ByteRange,
    open: ByteRange,
    close: ByteRange,
    content: ByteRange,
    content_lines: Vec<ByteRange>,
    inner_lines: Vec<ByteRange>,
}

impl InlineCodeM {
    /// comrak's sourcepos for a `Code` node can reach past the true closing
    /// backtick run and swallow a following byte a sibling node also claims,
    /// so a code span's extent is the located delimiter runs and nothing
    /// else: every other field here is derived from `open` and `close`, and
    /// no caller can supply one.
    pub fn between_delimiters(
        open: ByteRange,
        close: ByteRange,
        per_line: impl Fn(ByteRange) -> Vec<ByteRange>,
    ) -> InlineCodeM {
        let range = ByteRange::new(open.start, close.end.max(open.start));
        let content = ByteRange::new(open.end, close.start.max(open.end));
        InlineCodeM {
            sm: RevealSm::new(RevealState::Rendered),
            range,
            open,
            close,
            content,
            content_lines: per_line(range),
            inner_lines: per_line(content),
        }
    }

    pub fn state(&self) -> RevealState {
        self.sm.state()
    }

    pub fn range(&self) -> ByteRange {
        self.range
    }

    pub fn open(&self) -> ByteRange {
        self.open
    }

    pub fn close(&self) -> ByteRange {
        self.close
    }

    pub fn content(&self) -> ByteRange {
        self.content
    }

    pub fn content_lines(&self) -> &[ByteRange] {
        &self.content_lines
    }

    pub fn inner_lines(&self) -> &[ByteRange] {
        &self.inner_lines
    }

    fn sync(&mut self, ctx: &InheritCtx) -> bool {
        let range = self.range;
        let want = ctx.grant.resolve(|| ctx.cursors.any_in(range));
        self.sm.transition(want)
    }
}

/// `[text](url)`. Rendered -> emit text children only, styled as a link (no
/// following — Phase-1 scope). Revealed -> raw markdown.
#[derive(Clone, Debug)]
pub struct LinkM {
    pub sm: RevealSm,
    pub range: ByteRange,
    pub line: usize,
    pub text: Vec<Inline>,
    pub url: String,
    pub url_range: ByteRange,
    /// One entry per physical line `range` spans — the Revealed emit
    /// path iterates this instead of pushing `range` whole (same shape
    /// as `EmphasisM::content_lines`; the link TEXT's own children are
    /// each individually reliable, same reasoning as `EmphasisM`'s
    /// children, and `url_range` can never contain a raw newline — a
    /// link destination can't span lines under CommonMark).
    pub content_lines: Vec<ByteRange>,
}

impl LinkM {
    fn sync(&mut self, ctx: &InheritCtx) -> bool {
        let range = self.range;
        let want = ctx.grant.resolve(|| ctx.cursors.any_in(range));
        let mut dirty = self.sm.transition(want);
        let child_ctx = ctx.child(self.sm.state());
        for c in &mut self.text {
            dirty |= c.sync(&child_ctx);
        }
        dirty
    }
}

/// `[[target|label]]` / `[[target]]`. Same reveal/style treatment as
/// `LinkM`, no following (Phase-1 scope).
#[derive(Clone, Debug)]
pub struct WikiLinkM {
    pub sm: RevealSm,
    pub range: ByteRange,
    pub line: usize,
    pub target: String,
    pub label: ByteRange,
}

impl WikiLinkM {
    fn sync(&mut self, ctx: &InheritCtx) -> bool {
        let range = self.range;
        let want = ctx.grant.resolve(|| ctx.cursors.any_in(range));
        self.sm.transition(want)
    }
}

/// `![alt](target)` (comrak's own `Image` node) or `![[target]]` (recovered
/// by `parse::inline`'s text scanner — comrak's wikilink trigger has a
/// `within_brackets` guard that suppresses the node entirely under a
/// leading `!`, so an embed never arrives as a `WikiLink` node; see that
/// module's docs, and `catalogue.rs`'s pinned
/// `embed_prefixed_wikilink_comrak_behaviour_is_pinned`). Rendered -> emit
/// `alt` (or `target` when `alt` is empty, implementing the "empty alt,
/// URL becomes the visible label" rule) styled `markup.image`.
/// Revealed -> raw markdown, the same open/close-hide treatment
/// `WikiLinkM` already uses. No nested children: an image's alt text is
/// plain and unstyled, the same Phase-1 simplification a bare wikilink
/// label already accepts.
#[derive(Clone, Debug)]
pub struct ImageM {
    pub sm: RevealSm,
    pub range: ByteRange,
    pub alt: ByteRange,
    pub target: ByteRange,
    /// The decoded target string — comrak's own decoded `url` for
    /// `![alt](url)`, or the raw `target` text for `![[target]]`.
    pub target_text: String,
    /// `true` for a `![[target]]` embed recovered by the text scanner,
    /// `false` for standard `![alt](url)` markdown image syntax — the
    /// catalogue walk needs this to resolve `target_text` as a
    /// `Target::Name` (wikilink-style) or a `Target::Path`/`Target::Url`
    /// (markdown-style), the same fork `WikiLinkM` vs `LinkM` already makes
    /// for a non-embed reference.
    pub is_wikilink: bool,
    pub line: usize,
    /// One entry per physical line `range` spans — same shape as
    /// `WikiLinkM`'s siblings; always `vec![range]` for the overwhelmingly
    /// common single-line case, and always exactly one entry for a
    /// scanner-recovered `![[target]]` embed (the scanner never lets one
    /// span a raw newline — see `parse::inline`'s docs).
    pub content_lines: Vec<ByteRange>,
}

impl ImageM {
    fn sync(&mut self, ctx: &InheritCtx) -> bool {
        let range = self.range;
        let want = ctx.grant.resolve(|| ctx.cursors.any_in(range));
        self.sm.transition(want)
    }
}

#[derive(Clone, Debug)]
pub enum Inline {
    Text(TextRun),
    Emphasis(EmphasisM),
    Code(InlineCodeM),
    Link(LinkM),
    WikiLink(WikiLinkM),
    Image(ImageM),
}

impl Inline {
    pub fn sync(&mut self, ctx: &InheritCtx) -> bool {
        match self {
            Inline::Text(_) => false,
            Inline::Emphasis(m) => m.sync(ctx),
            Inline::Code(m) => m.sync(ctx),
            Inline::Link(m) => m.sync(ctx),
            Inline::WikiLink(m) => m.sync(ctx),
            Inline::Image(m) => m.sync(ctx),
        }
    }

    pub fn reveal_state(&self) -> RevealState {
        match self {
            Inline::Text(_) => RevealState::Revealed,
            Inline::Emphasis(m) => m.sm.state(),
            Inline::Code(m) => m.sm.state(),
            Inline::Link(m) => m.sm.state(),
            Inline::WikiLink(m) => m.sm.state(),
            Inline::Image(m) => m.sm.state(),
        }
    }

    pub fn range(&self) -> ByteRange {
        match self {
            Inline::Text(t) => t.range,
            Inline::Emphasis(m) => m.range,
            Inline::Code(m) => m.range,
            Inline::Link(m) => m.range,
            Inline::WikiLink(m) => m.range,
            Inline::Image(m) => m.range,
        }
    }
}

/// Qualifies per DISPLAY LINE rather than per paragraph: any other
/// substantive span (including text adjacent to the image, or a revealed
/// image) disqualifies the line, and that adjacency check is scoped to the
/// line, not the block. `inlines` is a single block's own inline sequence (a
/// paragraph's or a list item's `Vec<Inline>`); this returns every image in
/// it that sits alone on its own physical line, optionally surrounded by
/// whitespace-only text ON THAT SAME LINE. A substantive inline elsewhere in
/// the paragraph — on a different line — no longer disqualifies a
/// standalone image line the way it used to; only substantive content
/// sharing the SAME line does. This crate never represents a list marker as
/// an inline at all (it's `ListItemM::marker`, concealed or carried as the
/// row's own `decor` — see `emit::walk::emit_list_item`), so a list-item
/// image already satisfies this rule with no separate marker case to
/// special-case. Anything else on a line — text adjacent to the image, a
/// second image, a Revealed image under the caret — disqualifies THAT
/// line, so a truly-inline image falls back to its alt text instead of a
/// placeholder.
pub fn standalone_image<'a>(
    content: &str,
    starts: &[usize],
    inlines: &'a [Inline],
) -> Vec<&'a ImageM> {
    let mut candidates: HashMap<usize, &'a ImageM> = HashMap::new();
    let mut disqualified: HashSet<usize> = HashSet::new();

    for inl in inlines {
        match inl {
            Inline::Text(t) => {
                for r in &t.content_lines {
                    if !range_is_whitespace_only(content, r) {
                        disqualified.insert(crate::parse::line_at(starts, r.start));
                    }
                }
            }
            Inline::Image(m) => {
                if m.sm.state() == RevealState::Rendered {
                    // A second Rendered image already claiming this line
                    // disqualifies it — two images can't both be "alone".
                    if candidates.insert(m.line, m).is_some() {
                        disqualified.insert(m.line);
                    }
                } else {
                    // A Revealed image (caret-collapsed to raw source) is
                    // substantive on its own line — must still disqualify.
                    disqualified.insert(m.line);
                }
            }
            other => {
                let range = other.range();
                let first = crate::parse::line_at(starts, range.start);
                let last =
                    crate::parse::line_at(starts, range.end.saturating_sub(1).max(range.start));
                for l in first..=last {
                    disqualified.insert(l);
                }
            }
        }
    }

    candidates
        .into_iter()
        .filter(|(line, _)| !disqualified.contains(line))
        .map(|(_, m)| m)
        .collect()
}

fn range_is_whitespace_only(content: &str, r: &ByteRange) -> bool {
    content
        .get(r.start..r.end)
        .is_some_and(|s| s.chars().all(char::is_whitespace))
}
