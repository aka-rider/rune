//! Session-fuzzer catches replayed at the emitter level, from the content,
//! cursor set and width the emitter actually received when each artifact
//! fired. The `case_*` tests carry the full captured document; the
//! `minimal_*` tests carry the smallest document and cursor set that still
//! reproduces the same assertion. The two `stale_*` cases are artifacts
//! that no longer reproduce anything and are kept as evidence of that.

mod conceal_common;

use conceal_common::assert_no_duplicate_content_at;

#[test]
fn case_zero_width_spaces_around_multiline_code_span() {
    assert_no_duplicate_content_at(
        "\n \u{200b}\u{200b} \n \u{200b}\u{200b}***bold*** _em_ ` \n[***bold*** _em_ `a](b) \u{200b}\u{200b} ",
        &[34, 54],
        78,
    );
}

#[test]
fn minimal_multiline_code_span_swallows_next_byte() {
    assert_no_duplicate_content_at("a\n `\n`x", &[6], 78);
}

#[test]
fn case_conflict_markers_wrapping_multiline_code_spans() {
    assert_no_duplicate_content_at(
        "<<<<<<< editor\n\n\n\n***bold*** _em_ `c\n\t***bold*** _em_ `c=======\nfuzz***bold*** _em_ `c-external-write-1\n***bold*** _em_ `c\n>>>>>>> disk\n",
        &[36, 56, 86, 122],
        78,
    );
}

#[test]
fn minimal_multiline_code_span_after_tab_indent() {
    assert_no_duplicate_content_at("`\n\t` `\n`c", &[6], 78);
}

#[test]
fn case_conflict_markers_with_fences_and_multiline_code_span() {
    assert_no_duplicate_content_at(
        "<<<<<<< editor\nhello worldline1\r\nhello worldline2h你好世界，世界你好 hello worldhello world hello world 你好世界，世界你好# Notes\nhello world\n```rust\nfn main() {}\n```\nh\n`h``python\ndef f():\n h   return 1\n`h``\n\n```klingon\nQapla'\n```\n\n```\nuntagged fence\n```\n\ntail\n\n=======\nfuzz-external-write-2\n\n>>>>>>> disk\n",
        &[186, 189, 209, 223],
        78,
    );
}

#[test]
fn minimal_multiline_code_span_between_fences() {
    assert_no_duplicate_content_at("```\n```\nc\n`\n u\n`c", &[13], 78);
}

#[test]
fn case_tab_indented_blockquote_before_wide_text() {
    assert_no_duplicate_content_at(
        "<<<<<<< editor\n- h\n=======\nfuzz-external-write-1\n\n\t>>>>>>> disk\n\t你好世界，世界你好",
        &[64, 92],
        56,
    );
}

#[test]
fn minimal_tab_indented_blockquote_before_wide_text() {
    assert_no_duplicate_content_at("-\n\t>d\n\t你", &[0], 56);
}

#[test]
fn stale_lone_cr_before_atx_heading() {
    assert_no_duplicate_content_at(
        "line1 # \r\nline2 # # Title\n\n- item one\n- item two\n\n> a quote\n\n```rust\nfn main() {}\n```\n\n[a link](https://example.com)\n",
        &[8, 18],
        78,
    );
}

#[test]
fn stale_byte_order_mark_after_blank_lines() {
    assert_no_duplicate_content_at("hello world \n\n\n\u{feff}hello", &[15], 78);
}
