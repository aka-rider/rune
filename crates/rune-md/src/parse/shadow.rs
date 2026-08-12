use std::borrow::Cow;

const TAB_STOP: usize = 4;
const BYTE_ORDER_MARK: [u8; 3] = [0xEF, 0xBB, 0xBF];

/// Which way a shadow offset landing strictly inside an expanded tab
/// resolves: `Down` for the start of a range, `Up` for its end.
#[derive(Clone, Copy)]
pub(crate) enum Round {
    Down,
    Up,
}

/// The bytes comrak parses instead of the real buffer: every LONE `\r` (a
/// CR NOT immediately followed by `\n`) blanked to a space, and every tab
/// in a line's leading container-prefix region expanded to spaces. Same
/// line count and same line order as `content`, so comrak's line numbers
/// still index `line_starts(content)`; columns come back in this view's
/// coordinates and convert through `real_offset_in_line`.
///
/// Blanking a lone `\r` is what keeps the two line models identical:
/// CommonMark ends a line on a bare CR, this crate's buffer does not.
///
/// Expanding the tabs is comrak's own quirk: where a container prefix
/// consumes only part of a leading tab, comrak substitutes the tab's
/// unconsumed width as literal spaces at the head of the block's content
/// while its byte offset has already stepped past the whole tab, so every
/// column it reports on that line comes back shifted right by up to three.
/// A tab advances to the next four-column stop wherever spaces define
/// block structure, so writing those spaces out here preserves every block
/// decision comrak makes and leaves it no tab to half-consume.
pub fn parse_shadow(content: &str) -> Cow<'_, str> {
    if !content.bytes().any(|b| b == b'\t' || b == b'\r') {
        return Cow::Borrowed(content);
    }
    match rebuild(content) {
        // Every byte written is ASCII, so the decode cannot fail; the
        // fallback is what keeps this function panic-free anyway.
        Some(shadow) => {
            Cow::Owned(String::from_utf8(shadow).unwrap_or_else(|_| content.to_owned()))
        }
        None => Cow::Borrowed(content),
    }
}

/// A 0-based offset into `parse_shadow`'s copy of one line, back to a
/// 0-based offset into that line's real bytes. Offsets past the line's
/// prefix region — comrak measures a block's end column against the shadow
/// line's whole length — keep their distance from the region's end.
pub(crate) fn real_offset_in_line(
    line: &[u8],
    is_first_line: bool,
    shadow_offset: usize,
    round: Round,
) -> usize {
    let mut real = None;
    let mut shadow_len = 0;
    let region = walk_region(line, is_first_line, |byte| {
        if real.is_none() && shadow_offset < shadow_len + byte.shadow_width {
            real = Some(match round {
                Round::Up if shadow_offset > shadow_len => byte.real + 1,
                Round::Up | Round::Down => byte.real,
            });
        }
        shadow_len += byte.shadow_width;
    });
    real.unwrap_or(region.real_len + shadow_offset.saturating_sub(region.shadow_len))
}

#[derive(Clone, Copy)]
struct Line<'a> {
    bytes: &'a [u8],
    is_first: bool,
    terminated: bool,
}

impl Line<'_> {
    fn is_lone_cr(self, index: usize, byte: u8) -> bool {
        byte == b'\r' && !(self.terminated && index + 1 == self.bytes.len())
    }

    fn has_lone_cr(self) -> bool {
        self.bytes
            .iter()
            .enumerate()
            .any(|(index, &byte)| self.is_lone_cr(index, byte))
    }

    fn shadow_byte(self, index: usize, byte: u8) -> u8 {
        if self.is_lone_cr(index, byte) {
            b' '
        } else {
            byte
        }
    }
}

fn lines(content: &str) -> impl Iterator<Item = Line<'_>> {
    content
        .as_bytes()
        .split_inclusive(|&b| b == b'\n')
        .enumerate()
        .map(|(index, raw)| match raw.split_last() {
            Some((&b'\n', bytes)) => Line {
                bytes,
                is_first: index == 0,
                terminated: true,
            },
            _ => Line {
                bytes: raw,
                is_first: index == 0,
                terminated: false,
            },
        })
}

fn rebuild(content: &str) -> Option<Vec<u8>> {
    let mut shadow = Vec::with_capacity(content.len());
    let mut changed = false;
    for line in lines(content) {
        let region = walk_region(line.bytes, line.is_first, |byte| {
            if byte.byte == b'\t' {
                shadow.resize(shadow.len() + byte.shadow_width, b' ');
            } else {
                shadow.push(line.shadow_byte(byte.real, byte.byte));
            }
        });
        for (index, &byte) in line.bytes.iter().enumerate().skip(region.real_len) {
            shadow.push(line.shadow_byte(index, byte));
        }
        if line.terminated {
            shadow.push(b'\n');
        }
        changed |= region.has_tab || line.has_lone_cr();
    }
    changed.then_some(shadow)
}

#[derive(Clone, Copy)]
struct RegionByte {
    real: usize,
    byte: u8,
    shadow_width: usize,
}

#[derive(Default)]
struct Region {
    real_len: usize,
    shadow_len: usize,
    has_tab: bool,
}

fn is_region_byte(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'>' | b'\r')
}

/// The one description of a line's leading container-prefix region — the
/// maximal run of bytes a container prefix can consume — and of how wide
/// each of those bytes is in `parse_shadow`'s copy. Both the copy and the
/// translation back walk this, so they cannot drift apart.
///
/// A byte-order mark opening the document is prefix comrak steps over
/// without counting a column, so it is carried at width one and the tab
/// stops start counting after it.
fn walk_region(line: &[u8], is_first_line: bool, mut visit: impl FnMut(RegionByte)) -> Region {
    let mark = if is_first_line && line.starts_with(&BYTE_ORDER_MARK) {
        BYTE_ORDER_MARK.len()
    } else {
        0
    };
    let mut column = 0;
    let mut region = Region::default();
    for (real, &byte) in line.iter().enumerate() {
        let inside_mark = real < mark;
        if !inside_mark && !is_region_byte(byte) {
            break;
        }
        let shadow_width = if byte == b'\t' {
            region.has_tab = true;
            TAB_STOP - column % TAB_STOP
        } else {
            1
        };
        if !inside_mark {
            column += shadow_width;
        }
        visit(RegionByte {
            real,
            byte,
            shadow_width,
        });
        region.real_len = real + 1;
        region.shadow_len += shadow_width;
    }
    region
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;

    const FIXTURES: &[&str] = &[
        "\ta\n\tb\n",
        ">\tq\n> r\n",
        "a\n\r\t\u{4f60}\n",
        "\u{feff}\tx\n",
        "- x\n\n  \t\tcode",
        ">   -    a\n>\tb\n",
    ];

    fn after_region(line: &str) -> &str {
        line.trim_start_matches([' ', '\t', '>', '\r', '\u{feff}'])
    }

    #[test]
    fn a_tab_outside_a_prefix_region_is_left_alone() {
        assert!(matches!(parse_shadow("a\tb\n"), Cow::Borrowed(_)));
    }

    #[test]
    fn a_leading_tab_is_expanded_into_a_copy() {
        assert_eq!(parse_shadow("\ta"), Cow::Owned::<str>("    a".to_owned()));
    }

    #[test]
    fn a_lone_carriage_return_is_blanked_into_a_copy() {
        assert_eq!(parse_shadow("a\rb"), Cow::Owned::<str>("a b".to_owned()));
    }

    #[test]
    fn a_blanked_carriage_return_still_indents_the_tab_after_it() {
        assert_eq!(
            parse_shadow("\r\t\u{4f60}"),
            Cow::Owned::<str>("    \u{4f60}".to_owned())
        );
    }

    #[test]
    fn a_byte_order_mark_does_not_shift_the_tab_stops() {
        assert_eq!(
            parse_shadow("\u{feff}\tx"),
            Cow::Owned::<str>("\u{feff}    x".to_owned())
        );
    }

    #[test]
    fn every_line_keeps_its_place_and_its_content() {
        for fixture in FIXTURES {
            let shadow = parse_shadow(fixture);
            let real: Vec<&str> = fixture.split('\n').collect();
            let copied: Vec<&str> = shadow.split('\n').collect();
            assert_eq!(
                real.len(),
                copied.len(),
                "line count changed for {fixture:?}"
            );
            for (real, copied) in real.iter().zip(copied.iter()) {
                assert_eq!(
                    after_region(real),
                    after_region(copied),
                    "line content changed for {fixture:?}"
                );
            }
        }
    }

    #[test]
    fn a_line_the_copy_left_alone_translates_to_itself() {
        for offset in 0..12 {
            assert_eq!(
                real_offset_in_line(b"plain text", false, offset, Round::Down),
                offset
            );
            assert_eq!(
                real_offset_in_line(b">  quoted", false, offset, Round::Up),
                offset
            );
        }
    }

    #[test]
    fn an_offset_inside_an_expanded_tab_rounds_to_the_asked_side() {
        assert_eq!(real_offset_in_line(b"\tx", false, 2, Round::Down), 0);
        assert_eq!(real_offset_in_line(b"\tx", false, 2, Round::Up), 1);
        assert_eq!(real_offset_in_line(b"\tx", false, 0, Round::Up), 0);
        assert_eq!(real_offset_in_line(b"\tx", false, 4, Round::Down), 1);
        assert_eq!(real_offset_in_line(b"\tx", false, 5, Round::Down), 2);
    }
}
