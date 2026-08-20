//! JSON payload structs mirroring `rune_core::buffer::AppliedEdit` and
//! `rune_core::cursor::Cursor` for the `events` table's `edits`/
//! `cursors_before`/`cursors_after` columns (`events.edits BLOB NOT NULL`
//! etc., stored as UTF-8 JSON text — plan decision "JSON via serde_json ...
//! payloads must be inspectable").
//!
//! `rune-core` stays dependency-free by default (plan WP3.S1: "prefer the
//! local-mirror approach ... keeps rune-core dependency-free by default") —
//! these mirror structs, not `serde` derives on the domain types themselves,
//! carry the `Serialize`/`Deserialize` impls. Field names are renamed to
//! the schema's established bare capitalized form (no serde tags) so every
//! row under `sqlite3 rune-v1.db 'SELECT edits FROM events'` reads
//! identically regardless of which encoder wrote it.

use serde::{Deserialize, Serialize};

use rune_core::buffer::AppliedEdit;
use rune_core::cursor::{Cursor, CursorId};
use rune_core::undo::EditKind;

use crate::Error;

/// Mirrors `rune_core::buffer::AppliedEdit`.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct EditPayload {
    #[serde(rename = "Start")]
    start: usize,
    #[serde(rename = "End")]
    end: usize,
    #[serde(rename = "Deleted")]
    deleted: String,
    #[serde(rename = "Insert")]
    insert: String,
}

impl From<&AppliedEdit> for EditPayload {
    fn from(e: &AppliedEdit) -> Self {
        EditPayload {
            start: e.start,
            end: e.end,
            deleted: e.deleted.clone(),
            insert: e.insert.clone(),
        }
    }
}

impl From<EditPayload> for AppliedEdit {
    fn from(p: EditPayload) -> Self {
        AppliedEdit {
            start: p.start,
            end: p.end,
            deleted: p.deleted,
            insert: p.insert,
        }
    }
}

/// Mirrors `rune_core::cursor::Cursor`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct CursorPayload {
    #[serde(rename = "Position")]
    position: usize,
    #[serde(rename = "Anchor")]
    anchor: usize,
    #[serde(rename = "DesiredCol")]
    desired_col: usize,
    #[serde(rename = "ID")]
    id: u32,
}

impl From<&Cursor> for CursorPayload {
    fn from(c: &Cursor) -> Self {
        CursorPayload {
            position: c.position,
            anchor: c.anchor,
            desired_col: c.desired_col,
            id: c.id.get(),
        }
    }
}

impl TryFrom<CursorPayload> for Cursor {
    type Error = Error;

    fn try_from(p: CursorPayload) -> Result<Self, Self::Error> {
        let id = CursorId::try_from(p.id).map_err(|_| {
            Error::CorruptPayload(crate::error::CorruptPayloadReason::InvalidCursorId { id: p.id })
        })?;
        Ok(Cursor {
            position: p.position,
            anchor: p.anchor,
            desired_col: p.desired_col,
            id,
        })
    }
}

/// Serializes an edit batch to JSON — always succeeds (every field is a
/// plain UTF-8-safe scalar; `serde_json` can only fail on non-finite floats
/// or non-string map keys, neither of which `EditPayload` has), but returns
/// `Result` rather than panicking/unwrapping per the workspace's deny-lints.
pub(crate) fn edits_to_json(edits: &[AppliedEdit]) -> Result<String, Error> {
    let payload: Vec<EditPayload> = edits.iter().map(EditPayload::from).collect();
    serde_json::to_string(&payload)
        .map_err(|e| Error::CorruptPayload(crate::error::CorruptPayloadReason::Json(e)))
}

/// Parses an edit batch from JSON. A parse failure is surfaced as
/// [`Error::CorruptPayload`], never silently treated as an empty batch.
pub(crate) fn edits_from_json(json: &str) -> Result<Vec<AppliedEdit>, Error> {
    let payload: Vec<EditPayload> = serde_json::from_str(json)
        .map_err(|e| Error::CorruptPayload(crate::error::CorruptPayloadReason::Json(e)))?;
    Ok(payload.into_iter().map(AppliedEdit::from).collect())
}

pub(crate) fn cursors_to_json(cursors: &[Cursor]) -> Result<String, Error> {
    let payload: Vec<CursorPayload> = cursors.iter().map(CursorPayload::from).collect();
    serde_json::to_string(&payload)
        .map_err(|e| Error::CorruptPayload(crate::error::CorruptPayloadReason::Json(e)))
}

pub(crate) fn cursors_from_json(json: &str) -> Result<Vec<Cursor>, Error> {
    let payload: Vec<CursorPayload> = serde_json::from_str(json)
        .map_err(|e| Error::CorruptPayload(crate::error::CorruptPayloadReason::Json(e)))?;
    payload.into_iter().map(Cursor::try_from).collect()
}

pub(crate) fn edit_kind_to_text(kind: EditKind) -> &'static str {
    match kind {
        EditKind::Insert => "Insert",
        EditKind::DeleteLeft => "DeleteLeft",
        EditKind::DeleteRight => "DeleteRight",
        EditKind::Paste => "Paste",
        EditKind::Cut => "Cut",
        EditKind::StripTrailingWhitespace => "StripTrailingWhitespace",
        EditKind::Other => "Other",
    }
}

/// `None` (an old, pre-`kind`-column row) and any text this crate does not
/// recognize both map to [`EditKind::Other`] — the same "ungrouped" default
/// a row missing the column entirely already behaved as before this column
/// existed, so a NULL row's recovered grouping cannot regress.
pub(crate) fn edit_kind_from_text(text: Option<&str>) -> EditKind {
    match text {
        Some("Insert") => EditKind::Insert,
        Some("DeleteLeft") => EditKind::DeleteLeft,
        Some("DeleteRight") => EditKind::DeleteRight,
        Some("Paste") => EditKind::Paste,
        Some("Cut") => EditKind::Cut,
        Some("StripTrailingWhitespace") => EditKind::StripTrailingWhitespace,
        Some(_) | None => EditKind::Other,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn edit_round_trips_through_json() {
        let edits = vec![AppliedEdit {
            start: 3,
            end: 7,
            deleted: "abcd".to_string(),
            insert: "xy".to_string(),
        }];
        let json = edits_to_json(&edits).expect("serialize");
        assert!(json.contains("\"Start\":3"), "json: {json}");
        let back = edits_from_json(&json).expect("deserialize");
        assert_eq!(back, edits);
    }

    #[test]
    fn cursor_round_trips_through_json() {
        let cursors = vec![Cursor {
            position: 5,
            anchor: 2,
            desired_col: 1,
            id: CursorId::try_from(3).expect("test id is non-zero"),
        }];
        let json = cursors_to_json(&cursors).expect("serialize");
        assert!(json.contains("\"ID\":3"), "json: {json}");
        let back = cursors_from_json(&json).expect("deserialize");
        assert_eq!(back, cursors);
    }

    #[test]
    fn corrupt_json_surfaces_as_error_never_silently_empty() {
        let err = edits_from_json("not valid json").expect_err("must error");
        assert!(matches!(err, Error::CorruptPayload(_)));
    }

    #[test]
    fn a_zero_cursor_id_is_refused_not_repaired() {
        let json = r#"[{"Position":0,"Anchor":0,"DesiredCol":0,"ID":0}]"#;
        let err = cursors_from_json(json).expect_err("id 0 must be refused");
        assert!(matches!(err, Error::CorruptPayload(_)));
    }

    #[test]
    fn every_edit_kind_round_trips_through_its_text_encoding() {
        for kind in [
            EditKind::Insert,
            EditKind::DeleteLeft,
            EditKind::DeleteRight,
            EditKind::Paste,
            EditKind::Cut,
            EditKind::StripTrailingWhitespace,
            EditKind::Other,
        ] {
            let text = edit_kind_to_text(kind);
            assert_eq!(edit_kind_from_text(Some(text)), kind);
        }
    }

    #[test]
    fn a_missing_or_unrecognized_edit_kind_maps_to_other() {
        assert_eq!(edit_kind_from_text(None), EditKind::Other);
        assert_eq!(edit_kind_from_text(Some("SomeFutureKind")), EditKind::Other);
    }
}
