//! JSON payload structs mirroring `rune_core::buffer::AppliedEdit` and
//! `rune_core::cursor::Cursor` for the `events` table's `edits`/
//! `cursors_before`/`cursors_after` columns (`events.edits BLOB NOT NULL`
//! etc., stored as UTF-8 JSON text, matching Go's `encoding/json` payloads —
//! plan decision "JSON via serde_json ... payloads must be inspectable").
//!
//! `rune-core` stays dependency-free by default (plan WP3.S1: "prefer the
//! local-mirror approach ... keeps rune-core dependency-free by default") —
//! these mirror structs, not `serde` derives on the domain types themselves,
//! carry the `Serialize`/`Deserialize` impls. Field names are renamed to
//! match Go's unadorned `encoding/json` output (`buffer.go:18-30`,
//! `cursor.go:9-14` have no json tags, so Go serializes the bare
//! capitalized field names) — a Rust-written row and a Go-written row of the
//! same shape read identically under `sqlite3 rune-v1.db 'SELECT edits FROM
//! events'`.

use serde::{Deserialize, Serialize};

use rune_core::buffer::AppliedEdit;
use rune_core::cursor::Cursor;

use crate::Error;

/// Mirrors `rune_core::buffer::AppliedEdit` (port of Go's
/// `buffer.AppliedEdit`, `buffer.go:25-30`).
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

/// Mirrors `rune_core::cursor::Cursor` (port of Go's `cursor.Cursor`,
/// `cursor.go:9-14`).
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
            id: c.id,
        }
    }
}

impl From<CursorPayload> for Cursor {
    fn from(p: CursorPayload) -> Self {
        Cursor {
            position: p.position,
            anchor: p.anchor,
            desired_col: p.desired_col,
            id: p.id,
        }
    }
}

/// Serializes an edit batch to JSON — always succeeds (every field is a
/// plain UTF-8-safe scalar; `serde_json` can only fail on non-finite floats
/// or non-string map keys, neither of which `EditPayload` has), but returns
/// `Result` rather than panicking/unwrapping per the workspace's deny-lints.
pub(crate) fn edits_to_json(edits: &[AppliedEdit]) -> Result<String, Error> {
    let payload: Vec<EditPayload> = edits.iter().map(EditPayload::from).collect();
    serde_json::to_string(&payload).map_err(|e| Error::CorruptPayload(e.to_string()))
}

/// Parses an edit batch from JSON. A parse failure is surfaced as
/// [`Error::CorruptPayload`], never silently treated as an empty batch
/// (§1.3; port of `journal.go`'s "corrupt payload is an Error, never a
/// silent none" discipline).
pub(crate) fn edits_from_json(json: &str) -> Result<Vec<AppliedEdit>, Error> {
    let payload: Vec<EditPayload> =
        serde_json::from_str(json).map_err(|e| Error::CorruptPayload(e.to_string()))?;
    Ok(payload.into_iter().map(AppliedEdit::from).collect())
}

pub(crate) fn cursors_to_json(cursors: &[Cursor]) -> Result<String, Error> {
    let payload: Vec<CursorPayload> = cursors.iter().map(CursorPayload::from).collect();
    serde_json::to_string(&payload).map_err(|e| Error::CorruptPayload(e.to_string()))
}

pub(crate) fn cursors_from_json(json: &str) -> Result<Vec<Cursor>, Error> {
    let payload: Vec<CursorPayload> =
        serde_json::from_str(json).map_err(|e| Error::CorruptPayload(e.to_string()))?;
    Ok(payload.into_iter().map(Cursor::from).collect())
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
            id: 3,
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
}
