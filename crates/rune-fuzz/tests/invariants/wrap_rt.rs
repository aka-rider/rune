//! WP6.S5 detection tests: `WRAP-RT`.

use rune_fuzz::invariant::{wrap_line_lens, wrap_rt};

use crate::support::wrap_for;

#[test]
fn wrap_rt_detects_an_out_of_domain_bound() {
    let (buf, wrap) = wrap_for("hello world\nsecond line\n", 80);
    let mut line_lens = wrap_line_lens(&wrap, buf.line_count());
    if let Some(first) = line_lens.first_mut() {
        *first += 50; // deliberately wrong: past this line's real syntax length
    }
    let v = wrap_rt(&wrap, &line_lens)
        .expect("a deliberately too-large domain bound must trip WRAP-RT");
    assert_eq!(v.id, "WRAP-RT");
}

#[test]
fn wrap_rt_accepts_the_real_in_domain_rectangle() {
    let (buf, wrap) = wrap_for("hello world\nsecond line\n# heading\n", 80);
    let line_lens = wrap_line_lens(&wrap, buf.line_count());
    assert_eq!(wrap_rt(&wrap, &line_lens), None);
}

#[test]
fn wrap_rt_accepts_a_narrow_width_that_actually_wraps() {
    let (buf, wrap) = wrap_for(
        "a fairly long line that should wrap at a narrow width\n",
        10,
    );
    let line_lens = wrap_line_lens(&wrap, buf.line_count());
    assert_eq!(wrap_rt(&wrap, &line_lens), None);
}
