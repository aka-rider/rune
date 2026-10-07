use std::sync::Arc;

use rune_core::buffer::{Buffer, Edit};
use rune_core::cursor::{Cursor, CursorId};
use rune_core::undo::EditKind;
use rune_md::continuation::{self, EnterPlan, parse_prefix};
use rune_md::element::code_region::CodeRegion;
use rune_syntax::DocumentKind;

use crate::app::App;
use crate::commands::edit::per_cursor_selection_edits;
use crate::commands::edit_core::commit_edits_with_followers;
use crate::commands::edit_renumber::{self, Claims, Insertion};
use crate::document::{Document, DocumentId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    List,
    Comment {
        tokens: &'static [&'static str],
        lead: usize,
    },
    Plain,
}

pub(crate) fn regions_of(doc: &mut Document) -> Arc<[CodeRegion]> {
    match doc.kind {
        DocumentKind::Markdown | DocumentKind::Code(_) => doc.view().code_regions.clone(),
        DocumentKind::Plain | DocumentKind::Image => Arc::from([]),
    }
}

pub(crate) fn caret_mode(
    regions: &[CodeRegion],
    kind: DocumentKind,
    buf: &Buffer,
    offset: usize,
) -> Mode {
    let line = buf.offset_to_line_col(offset).line;
    let (Some(start), Some(end)) = (buf.line_start(line), buf.line_end(line)) else {
        return Mode::Plain;
    };
    if let Some(region) = regions.iter().find(|r| r.rows.contains(&line)) {
        let content = region
            .content
            .iter()
            .find(|r| start <= r.start && r.end <= end);
        return match content {
            Some(content) => comment_mode(region, content.start - start),
            None => Mode::Plain,
        };
    }
    match kind {
        DocumentKind::Markdown => Mode::List,
        DocumentKind::Code(_) | DocumentKind::Plain | DocumentKind::Image => Mode::Plain,
    }
}

fn comment_mode(region: &CodeRegion, lead: usize) -> Mode {
    match rune_ts::lang::resolve(region.lang_token()).map(rune_syntax::LangId::line_comments) {
        Some(tokens) if !tokens.is_empty() => Mode::Comment { tokens, lead },
        _ => Mode::Plain,
    }
}

fn plain_text(buf: &Buffer, offset: usize) -> String {
    let line = buf.line(buf.offset_to_line_col(offset).line);
    let indent: String = line
        .chars()
        .take_while(|&ch| ch == ' ' || ch == '\t')
        .collect();
    format!("{}{indent}", buf.line_break_at(offset))
}

pub fn newline_plain(app: &mut App, id: DocumentId) {
    per_cursor_selection_edits(
        app,
        id,
        EditKind::Insert,
        |_i, c, buf| {
            let pos = if c.has_selection() {
                c.selection_start()
            } else {
                c.position
            };
            plain_text(buf, pos.get())
        },
        |_buf, c| Some((c.position.get(), c.position.get())),
    );
}

struct Line<'a> {
    buf: &'a Buffer,
    index: usize,
    start: usize,
    lead: &'a str,
    text: &'a str,
    caret: usize,
}

impl<'a> Line<'a> {
    fn at(buf: &'a Buffer, offset: usize) -> Option<Self> {
        let index = buf.offset_to_line_col(offset).line;
        let start = buf.line_start(index)?;
        let raw = buf.line(index);
        let text = raw.strip_suffix('\r').unwrap_or(raw);
        Some(Line {
            buf,
            index,
            start,
            lead: "",
            text,
            caret: offset.saturating_sub(start).min(text.len()),
        })
    }

    fn narrowed(&self, lead: usize) -> Option<Line<'a>> {
        Some(Line {
            buf: self.buf,
            index: self.index,
            start: self.start + lead,
            lead: self.text.get(..lead)?,
            text: self.text.get(lead..)?,
            caret: self.caret.checked_sub(lead)?,
        })
    }

    fn may_be_bare_marker(&self) -> bool {
        let bare = self.text.trim();
        !bare.is_empty()
            && self.caret == self.text.len()
            && (parse_prefix(self.text).empty
                || bare
                    .chars()
                    .all(|c| c.is_ascii_punctuation() || c.is_whitespace()))
    }

    fn previous(&self) -> Option<&'a str> {
        self.index.checked_sub(1).map(|n| self.buf.line(n))
    }

    fn above(&self) -> impl Iterator<Item = &'a str> {
        let buf = self.buf;
        (0..self.index).rev().map(move |n| buf.line(n))
    }
}

struct Planned {
    edit: Edit,
    exit: Option<Edit>,
    insertion: Option<Insertion>,
}

impl Planned {
    fn of(edit: Edit) -> Self {
        Planned {
            edit,
            exit: None,
            insertion: None,
        }
    }

    fn plain(buf: &Buffer, at: usize) -> Self {
        Planned::of(Edit {
            start: at,
            end: at,
            insert: plain_text(buf, at),
        })
    }

    fn selection(buf: &Buffer, cursor: &Cursor) -> Self {
        let (start, end) = cursor.selection_range();
        Planned::of(Edit {
            start: start.get(),
            end: end.get(),
            insert: plain_text(buf, start.get()),
        })
    }

    fn from_plan(plan: EnterPlan, line: &Line, at: usize) -> Self {
        match plan {
            EnterPlan::Plain => Planned::plain(line.buf, at),
            EnterPlan::Insert {
                replace,
                continuation,
            } => Planned::of(Edit {
                start: line.start + replace.start,
                end: line.start + replace.end,
                insert: format!("{}{}{continuation}", line.buf.line_break_at(at), line.lead),
            }),
            EnterPlan::Replace { range, text } => Planned::of(Edit {
                start: line.start + range.start,
                end: line.start + range.end,
                insert: text,
            }),
            EnterPlan::ExitQuote { previous, current } => {
                let mut planned = Planned::of(Edit {
                    start: line.start + current.start,
                    end: line.start + current.end,
                    insert: String::new(),
                });
                planned.exit = line
                    .index
                    .checked_sub(1)
                    .and_then(|n| line.buf.line_start(n))
                    .map(|above| Edit {
                        start: above + previous.start,
                        end: above + previous.end,
                        insert: String::new(),
                    });
                planned
            }
        }
    }
}

fn plan_list(line: &Line, at: usize, slot: usize) -> Planned {
    let prefix = parse_prefix(line.text);
    let parent = (prefix.empty && prefix.marker.is_some())
        .then(|| continuation::find_parent(&prefix, line.above()))
        .flatten();
    let plan = continuation::enter_list(
        line.text,
        line.caret,
        line.previous(),
        parent.as_ref().map(|p| &p.prefix),
    );
    let (anchor, skip) = match (&plan, parent) {
        (EnterPlan::Insert { .. }, _) => {
            (Some((line.index, prefix)), line.buf.line_break_at(at).len())
        }
        (EnterPlan::Replace { text, .. }, Some(parent)) if !text.is_empty() => (
            line.index
                .checked_sub(1 + parent.lines_above)
                .map(|anchor_line| (anchor_line, parent.prefix)),
            0,
        ),
        _ => (None, 0),
    };
    let mut planned = Planned::from_plan(plan, line, at);
    planned.insertion = anchor.and_then(|(anchor_line, anchor)| {
        Insertion::of(
            slot,
            line.index,
            anchor_line,
            anchor,
            &planned.edit.insert,
            skip,
        )
    });
    planned
}

fn plan_bare(
    regions: &[CodeRegion],
    kind: DocumentKind,
    buf: &Buffer,
    cursor: &Cursor,
    slot: usize,
) -> Planned {
    let at = cursor.position.get();
    let Some(line) = Line::at(buf, at) else {
        return Planned::plain(buf, at);
    };
    match caret_mode(regions, kind, buf, at) {
        Mode::Plain => Planned::plain(buf, at),
        Mode::List => plan_list(&line, at, slot),
        Mode::Comment { tokens, lead } => match line.narrowed(lead) {
            Some(code) => {
                let plan =
                    continuation::enter_comment(code.text, code.caret, tokens, code.index == 0);
                Planned::from_plan(plan, &code, at)
            }
            None => Planned::plain(buf, at),
        },
    }
}

pub fn newline(app: &mut App, id: DocumentId) {
    let Some(doc) = app.doc_mut(id) else { return };
    let kind = doc.kind;
    let regions = regions_of(doc);
    let cursors_before = doc.cursors.clone();
    let buf = &doc.buffer;

    let mut owned: Vec<(Edit, CursorId)> = Vec::new();
    let mut exits: Vec<Edit> = Vec::new();
    let mut insertions: Vec<Insertion> = Vec::new();
    for cursor in cursors_before.all() {
        let planned = if cursor.has_selection() {
            Planned::selection(buf, cursor)
        } else {
            plan_bare(&regions, kind, buf, cursor, owned.len())
        };
        exits.extend(planned.exit);
        insertions.extend(planned.insertion);
        owned.push((planned.edit, cursor.id));
    }

    let claims = Claims::of(owned.iter().map(|(edit, _)| edit).chain(&exits));
    let numbering = edit_renumber::plan(buf, &insertions, &claims);
    for (insertion, shift) in insertions.iter().zip(&numbering.shifts) {
        if let Some((edit, _)) = owned.get_mut(insertion.slot()) {
            insertion.renumber(&mut edit.insert, *shift);
        }
    }
    exits.extend(numbering.followers);

    let _ = commit_edits_with_followers(app, id, owned, exits, &cursors_before, EditKind::Insert);
}

pub(crate) struct SmartBackspace {
    kind: DocumentKind,
    regions: Arc<[CodeRegion]>,
}

impl SmartBackspace {
    pub(crate) fn new(app: &mut App, id: DocumentId) -> Self {
        let Some(doc) = app.doc_mut(id) else {
            return SmartBackspace {
                kind: DocumentKind::Plain,
                regions: Arc::from([]),
            };
        };
        let kind = doc.kind;
        let wanted = doc.cursors.all().iter().any(|c| {
            !c.has_selection()
                && Line::at(&doc.buffer, c.position.get()).is_some_and(|l| l.may_be_bare_marker())
        });
        SmartBackspace {
            kind,
            regions: if wanted {
                regions_of(doc)
            } else {
                Arc::from([])
            },
        }
    }

    pub(crate) fn range(&self, buf: &Buffer, cursor: &Cursor) -> Option<(usize, usize)> {
        let at = cursor.position.get();
        let line = Line::at(buf, at).filter(Line::may_be_bare_marker)?;
        let (line, tokens) = match caret_mode(&self.regions, self.kind, buf, at) {
            Mode::Plain => return None,
            Mode::List => (line, None),
            Mode::Comment { tokens, lead } => (line.narrowed(lead)?, Some(tokens)),
        };
        let range = continuation::backspace(line.text, line.caret, tokens)?;
        Some((line.start + range.start, line.start + range.end))
    }
}
