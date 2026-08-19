#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use rune_fuzz::action::Action;
use rune_fuzz::driver;
use rune_fuzz::generate;
use rune_fuzz::script;

#[test]
fn a_scripted_install_diff_left_action_installs_the_diff_view() {
    let actions = vec![Action::InstallDiffLeft { seed_index: 3 }];
    let result = driver::run(driver::DOC_PATH, "right side content", &actions);
    assert_eq!(result.violation, None, "{:?}", result.violation);

    let expected_left = generate::diff_left_content(3);
    let mut session = driver::Session::open(driver::DOC_PATH, "right side content");
    assert_eq!(session.act(Action::InstallDiffLeft { seed_index: 3 }), None);
    assert_eq!(
        session.app().diff.as_ref().map(|d| d.left.buffer.content()),
        Some(expected_left)
    );
}

#[test]
fn install_diff_left_round_trips_through_the_script_codec_and_decodes_absent_by_default() {
    let actions = vec![Action::InstallDiffLeft { seed_index: 42 }];
    let encoded = script::encode(driver::DOC_PATH, "hello", &actions);
    assert!(encoded.contains("install-diff-left 42"));
    let decoded = script::decode(&encoded).expect("decode failed");
    assert_eq!(decoded.2, actions);

    let plain = script::encode(driver::DOC_PATH, "hello", &[]);
    assert!(!plain.contains("install-diff-left"));
    let decoded_plain = script::decode(&plain).expect("decode failed");
    assert_eq!(decoded_plain.2, Vec::new());
}
