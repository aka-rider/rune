use super::Violation;
use crate::snapshot::Snapshot;

pub fn nav_bounds(snap: &Snapshot) -> Option<Violation> {
    for &(doc, offset, on_char_boundary) in &snap.nav_places {
        let Some(&len) = snap.buffer_len_by_doc.get(&doc) else {
            continue;
        };
        if offset > len || !on_char_boundary {
            return Some(Violation::new(
                "NAV-BOUNDS",
                format!(
                    "nav place doc={doc:?} offset={offset} buffer len={len} \
                     on_char_boundary={on_char_boundary}"
                ),
            ));
        }
    }
    if snap.nav_current > snap.nav_places.len() {
        return Some(Violation::new(
            "NAV-BOUNDS",
            format!(
                "nav_current={} exceeds places.len()={}",
                snap.nav_current,
                snap.nav_places.len()
            ),
        ));
    }
    None
}
