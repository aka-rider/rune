use std::collections::{HashMap, HashSet};

use rune_syntax::element::{ByteRange, InheritCtx, RevealSm, RevealState};

#[derive(Clone, Debug)]
pub struct TextRun {
    pub range: ByteRange,
    pub content_lines: Vec<ByteRange>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmphasisKind {
    Bold,
    Italic,
    BoldItalic,
    Strike,
}

#[derive(Clone, Debug)]
pub struct EmphasisM {
    pub sm: RevealSm,
    pub kind: EmphasisKind,
    pub range: ByteRange,
    pub open: ByteRange,
    pub close: ByteRange,
    pub children: Vec<Inline>,
    pub line: usize,
    pub content_lines: Vec<ByteRange>,
}

impl EmphasisM {
    fn sync(&mut self, ctx: &InheritCtx) -> bool {
        let range = self.range;
        let want = ctx.grant.resolve(|| ctx.cursors.touches(range));
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
    // comrak's sourcepos for a Code node can reach past the true closing
    // backtick run and swallow a byte a sibling node also claims — every
    // field here is derived from open/close only.
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
        let want = ctx.grant.resolve(|| ctx.cursors.touches(range));
        self.sm.transition(want)
    }
}

#[derive(Clone, Debug)]
pub struct LinkM {
    pub sm: RevealSm,
    pub range: ByteRange,
    pub line: usize,
    pub text: Vec<Inline>,
    pub url: String,
    pub url_range: ByteRange,
    pub content_lines: Vec<ByteRange>,
}

impl LinkM {
    fn sync(&mut self, ctx: &InheritCtx) -> bool {
        let range = self.range;
        let want = ctx.grant.resolve(|| ctx.cursors.touches(range));
        let mut dirty = self.sm.transition(want);
        let child_ctx = ctx.child(self.sm.state());
        for c in &mut self.text {
            dirty |= c.sync(&child_ctx);
        }
        dirty
    }
}

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
        let want = ctx.grant.resolve(|| ctx.cursors.touches(range));
        self.sm.transition(want)
    }
}

// comrak's wikilink trigger has a within_brackets guard that suppresses the
// node under a leading `!`, so `![[target]]` never arrives as a WikiLink
// node — parse::inline recovers it via a text scan instead.
#[derive(Clone, Debug)]
pub struct ImageM {
    pub sm: RevealSm,
    pub range: ByteRange,
    pub alt: ByteRange,
    pub target: ByteRange,
    pub target_text: String,
    pub is_wikilink: bool,
    pub line: usize,
    pub content_lines: Vec<ByteRange>,
}

impl ImageM {
    fn sync(&mut self, ctx: &InheritCtx) -> bool {
        let range = self.range;
        let want = ctx.grant.resolve(|| ctx.cursors.touches(range));
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
                    if candidates.insert(m.line, m).is_some() {
                        disqualified.insert(m.line);
                    }
                } else {
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
