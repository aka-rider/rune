use std::borrow::Cow;

pub const RAW_BYTE_FIRST: char = '\u{10FF80}';

const RAW_BYTE_BASE: u32 = RAW_BYTE_FIRST as u32;
const FIRST_NON_ASCII: u8 = 0x80;
const RAW_RANGE_LEAD: u8 = 0xF4;

pub fn is_raw_byte(c: char) -> bool {
    c >= RAW_BYTE_FIRST
}

pub fn raw_byte_value(c: char) -> Option<u8> {
    if !is_raw_byte(c) {
        return None;
    }
    let offset = u8::try_from(u32::from(c) - RAW_BYTE_BASE).ok()?;
    offset.checked_add(FIRST_NON_ASCII)
}

pub fn raw_byte_char(b: u8) -> Option<char> {
    let offset = b.checked_sub(FIRST_NON_ASCII)?;
    char::from_u32(RAW_BYTE_BASE + u32::from(offset))
}

pub fn decode(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(text) if !has_raw_range_lead(text.as_bytes()) => text,
        Ok(text) => decode_slow(text.as_bytes()),
        Err(err) => decode_slow(err.as_bytes()),
    }
}

pub fn encode(text: &str) -> Cow<'_, [u8]> {
    if !has_raw_range_lead(text.as_bytes()) {
        return Cow::Borrowed(text.as_bytes());
    }
    let mut out = Vec::with_capacity(text.len());
    let mut utf8 = [0u8; 4];
    for c in text.chars() {
        match raw_byte_value(c) {
            Some(b) => out.push(b),
            None => out.extend_from_slice(c.encode_utf8(&mut utf8).as_bytes()),
        }
    }
    Cow::Owned(out)
}

pub fn count_raw_bytes(text: &str) -> usize {
    text.chars().filter(|&c| is_raw_byte(c)).count()
}

pub fn glyph(b: u8) -> String {
    format!("<{b:02x}>")
}

fn has_raw_range_lead(bytes: &[u8]) -> bool {
    bytes.contains(&RAW_RANGE_LEAD)
}

fn decode_slow(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        for c in chunk.valid().chars() {
            if is_raw_byte(c) {
                push_escaped(&mut out, c);
            } else {
                out.push(c);
            }
        }
        for &b in chunk.invalid() {
            out.extend(raw_byte_char(b));
        }
    }
    out
}

fn push_escaped(out: &mut String, c: char) {
    let mut utf8 = [0u8; 4];
    for &b in c.encode_utf8(&mut utf8).as_bytes() {
        out.extend(raw_byte_char(b));
    }
}

#[cfg(test)]
#[path = "codec_tests.rs"]
mod tests;
