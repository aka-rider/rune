#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod merge_common;

use std::path::Path;

use rune_fuzz::Session;

use merge_common::{ch, drain_materialize_round_trip, reprobe, sup, untitled_draft};

const PADDED: &str = "a               \nb               \n";
const STRIPPED: &str = "a\nb\n";

fn disk(session: &Session) -> Vec<u8> {
    session.app().vfs.read(Path::new("/doc.md")).unwrap()
}

#[test]
fn a_document_more_than_half_trailing_whitespace_saves_twice_without_a_disk_conflict() {
    assert!(
        rune_core::is_suspicious_shrink(PADDED.len(), STRIPPED.len()),
        "fixture precondition: the strip must cross the suspicious-shrink threshold"
    );

    let mut session = Session::open("/doc.md", PADDED);
    let id = session.app().active;
    assert!(
        session.app().doc(id).unwrap().doc_db().is_some(),
        "fixture precondition: the seeded document must be store-bound"
    );

    assert!(session.key(sup('s')).is_none());
    assert!(session.deliver_db().is_none());
    drain_materialize_round_trip(&mut session);

    assert!(
        session.app().guard.is_none(),
        "the stripping save must not raise a guard"
    );
    assert_eq!(disk(&session), STRIPPED.as_bytes());

    let draft = untitled_draft(session.app(), id);
    reprobe(&mut session, draft, id);
    assert!(
        session.app().merge.doc().is_none(),
        "re-reading the shrunken file must not read as a divergence"
    );
    assert!(session.app().guard.is_none());

    assert!(session.key(ch('!')).is_none());
    assert!(session.deliver_db().is_none());
    assert!(session.key(sup('s')).is_none());
    drain_materialize_round_trip(&mut session);

    assert!(
        session.app().guard.is_none(),
        "the next save must not falsely conflict against rune's own stripping write"
    );
    assert_eq!(disk(&session), b"!a\nb\n");
    assert!(!session.app().is_dirty());
}
