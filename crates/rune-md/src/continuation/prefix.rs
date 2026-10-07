use std::ops::Range;

use super::plan::strip_cr;

pub const MAX_ORDERED: u32 = 999_999_999;
const MAX_DIGITS: usize = 9;
const TAB_STOP: usize = 4;
const UNCHECKED: &str = "[ ] ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delim {
    Dot,
    Paren,
}

impl Delim {
    pub fn as_char(self) -> char {
        match self {
            Delim::Dot => '.',
            Delim::Paren => ')',
        }
    }

    fn from_char(c: char) -> Option<Self> {
        match c {
            '.' => Some(Delim::Dot),
            ')' => Some(Delim::Paren),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerKind {
    Bullet(char),
    Task(char),
    Ordered { n: u32, delim: Delim, width: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    pub kind: MarkerKind,
    pub symbol: Range<usize>,
    pub gap: Range<usize>,
    pub span: Range<usize>,
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quote {
    pub depth: usize,
    pub span: Range<usize>,
    pub last: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prefix {
    pub lead: String,
    pub indent: Range<usize>,
    pub quote: Quote,
    pub marker: Option<Marker>,
    pub content_start: usize,
    pub empty: bool,
}

impl Prefix {
    pub fn marker_column(&self) -> Option<usize> {
        self.marker.as_ref().map(|m| m.column)
    }

    pub fn content_column(&self) -> usize {
        column_of(&self.lead)
    }

    pub fn encloses(&self, child: &Prefix) -> bool {
        self.quote.depth == child.quote.depth
            && matches!(
                (self.marker_column(), child.marker_column()),
                (Some(outer), Some(inner)) if outer < inner
            )
    }

    pub fn continuation(&self) -> Option<String> {
        let Some(marker) = &self.marker else {
            return Some(self.lead.clone());
        };
        let head = self.lead.get(..marker.span.start)?;
        let gap = self.lead.get(marker.gap.clone())?;
        let (symbol, checkbox) = match marker.kind {
            MarkerKind::Bullet(c) => (c.to_string(), ""),
            MarkerKind::Task(c) => (c.to_string(), UNCHECKED),
            MarkerKind::Ordered { n, delim, width } => {
                let next = n.checked_add(1).filter(|v| *v <= MAX_ORDERED)?;
                let number = format_number(next, width);
                (format!("{number}{}", delim.as_char()), "")
            }
        };
        Some(format!("{head}{symbol}{gap}{checkbox}"))
    }
}

pub fn format_number(n: u32, width: usize) -> String {
    format!("{n:0width$}")
}

pub(super) fn is_blank(c: char) -> bool {
    c == ' ' || c == '\t'
}

fn column_of(text: &str) -> usize {
    text.chars().fold(0, |col, c| match c {
        '\t' => (col / TAB_STOP + 1) * TAB_STOP,
        _ => col + 1,
    })
}

struct Scan<'a> {
    text: &'a str,
    pos: usize,
}

impl<'a> Scan<'a> {
    fn rest(&self) -> &'a str {
        self.text.get(self.pos..).unwrap_or("")
    }

    fn eat_if(&mut self, accept: impl Fn(char) -> bool) -> bool {
        match self.rest().chars().next() {
            Some(c) if accept(c) => {
                self.pos += c.len_utf8();
                true
            }
            _ => false,
        }
    }

    fn skip_blanks(&mut self) -> Range<usize> {
        let start = self.pos;
        while self.eat_if(is_blank) {}
        start..self.pos
    }
}

pub fn parse_prefix(line: &str) -> Prefix {
    let line = strip_cr(line);
    let mut scan = Scan { text: line, pos: 0 };
    let indent = scan.skip_blanks();
    let quote = parse_quote(&mut scan);
    scan.skip_blanks();
    let marker = if is_thematic_break(scan.rest()) {
        None
    } else {
        parse_marker(&mut scan, line)
    };
    let content_start = scan.pos;
    Prefix {
        lead: line.get(..content_start).unwrap_or("").to_string(),
        indent,
        quote,
        marker,
        content_start,
        empty: content_start == line.len(),
    }
}

fn parse_quote(scan: &mut Scan<'_>) -> Quote {
    let start = scan.pos;
    let mut depth = 0;
    let mut last = start..start;
    loop {
        let mut probe = Scan {
            text: scan.text,
            pos: scan.pos,
        };
        probe.skip_blanks();
        let token_start = probe.pos;
        if !probe.eat_if(|c| c == '>') {
            break;
        }
        probe.eat_if(is_blank);
        last = token_start..probe.pos;
        scan.pos = probe.pos;
        depth += 1;
    }
    Quote {
        depth,
        span: start..scan.pos,
        last,
    }
}

fn is_thematic_break(rest: &str) -> bool {
    let mut marks = rest.chars().filter(|c| !is_blank(*c));
    let Some(first) = marks.next().filter(|c| matches!(c, '-' | '*' | '_')) else {
        return false;
    };
    let mut count = 1;
    for c in marks {
        if c != first {
            return false;
        }
        count += 1;
    }
    count >= 3
}

fn symbol_of(rest: &str) -> Option<(MarkerKind, usize)> {
    let first = rest.chars().next()?;
    if matches!(first, '-' | '*' | '+') {
        return Some((MarkerKind::Bullet(first), 1));
    }
    let width = rest.bytes().take_while(u8::is_ascii_digit).count();
    if !(1..=MAX_DIGITS).contains(&width) {
        return None;
    }
    let n = rest.get(..width)?.parse().ok()?;
    let delim = rest
        .get(width..)?
        .chars()
        .next()
        .and_then(Delim::from_char)?;
    Some((MarkerKind::Ordered { n, delim, width }, width + 1))
}

fn checkbox(rest: &str) -> bool {
    matches!(rest.get(..3), Some("[ ]" | "[x]" | "[X]"))
        && rest.get(3..).is_some_and(|t| t.starts_with(is_blank))
}

fn parse_marker(scan: &mut Scan<'_>, line: &str) -> Option<Marker> {
    let start = scan.pos;
    let (mut kind, symbol_len) = symbol_of(scan.rest())?;
    if !scan.rest().get(symbol_len..)?.starts_with(is_blank) {
        return None;
    }
    let column = column_of(line.get(..start)?);
    scan.pos += symbol_len;
    let symbol = start..scan.pos;
    let gap = scan.skip_blanks();
    if let MarkerKind::Bullet(bullet) = kind
        && checkbox(scan.rest())
    {
        scan.pos += 3;
        scan.skip_blanks();
        kind = MarkerKind::Task(bullet);
    }
    Some(Marker {
        kind,
        symbol,
        gap,
        span: start..scan.pos,
        column,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#[path = "prefix_tests.rs"]
mod tests;
