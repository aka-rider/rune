#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use rune_fuzz::driver::{self, Session};

#[test]
fn a_real_highlight_cmd_discharges_end_to_end_via_deliver() {
    let content = "# Title\n\n```rust\nfn main() {}\n```\n";
    let mut session = Session::open(driver::DOC_PATH, content);

    assert_eq!(session.type_("x"), None);
    assert_eq!(session.deliver(), None);

    let snap = session.snapshot();
    assert_eq!(snap.highlight_version, snap.version);
    assert!(
        !snap.highlight_spans.is_empty(),
        "expected the real tree-sitter rust grammar to produce spans over the fence, got none"
    );
}
