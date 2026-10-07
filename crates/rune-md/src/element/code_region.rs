use std::ops::Range;

use crate::element::block::{Block, VerbatimKind};
use rune_core::buffer::Buffer;

#[derive(Clone, Debug)]
pub struct CodeRegion {
    pub info: String,
    pub content: Vec<Range<usize>>,
    pub rows: Range<usize>,
}

impl CodeRegion {
    pub fn lang_token(&self) -> &str {
        lang_token_of(&self.info)
    }
}

pub fn lang_token_of(info: &str) -> &str {
    info.split(|c: char| c.is_whitespace() || c == ',')
        .find(|s| !s.is_empty())
        .unwrap_or("")
}

pub(crate) fn collect(blocks: &[Block], buf: &Buffer, out: &mut Vec<CodeRegion>) {
    for block in blocks {
        match block {
            Block::CodeFence(cf) => {
                if cf.content_lines.is_empty() {
                    continue;
                }
                out.push(CodeRegion {
                    info: cf.language.clone(),
                    content: cf.content_lines.iter().map(|l| l.start..l.end).collect(),
                    rows: cf.first_line..cf.last_line.saturating_add(1),
                });
            }
            Block::Verbatim(v) if v.kind == VerbatimKind::IndentedCode => {
                let content: Vec<Range<usize>> =
                    v.content_lines.iter().map(|l| l.start..l.end).collect();
                let Some(rows) = rows_of(&content, buf) else {
                    continue;
                };
                out.push(CodeRegion {
                    info: String::new(),
                    content,
                    rows,
                });
            }
            Block::Frontmatter(fm) => out.push(CodeRegion {
                info: crate::parse::frontmatter::LANGUAGE.to_string(),
                content: fm.content_lines.iter().map(|l| l.start..l.end).collect(),
                rows: fm.first_line..fm.last_line.saturating_add(1),
            }),
            Block::Blockquote(bq) => collect(&bq.children, buf, out),
            Block::List(list) => {
                for item in &list.items {
                    collect(&item.children, buf, out);
                }
            }
            _ => {}
        }
    }
}

pub(crate) fn whole_document(info: &str, buf: &Buffer) -> CodeRegion {
    let content = (0..buf.line_count())
        .filter_map(|n| Some(buf.line_start(n)?..buf.line_end(n)?))
        .collect();
    CodeRegion {
        info: info.to_string(),
        content,
        rows: 0..buf.line_count(),
    }
}

fn rows_of(content: &[Range<usize>], buf: &Buffer) -> Option<Range<usize>> {
    let first = buf.offset_to_line_col(content.first()?.start).line;
    let last = buf.offset_to_line_col(content.last()?.start).line;
    Some(first..last.saturating_add(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(info: &str) -> CodeRegion {
        CodeRegion {
            info: info.to_string(),
            content: Vec::new(),
            rows: 0..0,
        }
    }

    #[test]
    fn lang_token_is_first_token_split_on_whitespace_and_comma() {
        assert_eq!(region("python").lang_token(), "python");
        assert_eq!(region("rust,ignore").lang_token(), "rust");
        assert_eq!(region("  py  title=x").lang_token(), "py");
        assert_eq!(region("").lang_token(), "");
        assert_eq!(region("{.python}").lang_token(), "{.python}");
    }
}
