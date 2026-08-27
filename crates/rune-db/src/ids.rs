use std::fmt;
use std::num::NonZeroI64;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, Value, ValueRef};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocId(pub i64);

/// A single document binding's own identity for undo-position numbering —
/// minted fresh every time a `Document` binds (or rebinds) to a recovery
/// row, never persisted or shared across two bindings. The writer thread
/// keys its `DocUndoState` map by this instead of by [`DocId`]: two
/// bindings sharing one row (unreachable via any real open path, but not
/// structurally prevented) each get their own independent local-position
/// numbering rather than racing to fill one shared sequence, and a rebind
/// starting fresh numbering is just "mint a new token" rather than an
/// explicit reset the writer has to be told about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BindingToken(u64);

impl BindingToken {
    /// Mints a fresh, process-wide-unique token.
    pub fn next() -> BindingToken {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        BindingToken(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(pub i64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Seq(pub i64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObsId(NonZeroI64);

impl ObsId {
    pub fn new(value: i64) -> Option<ObsId> {
        NonZeroI64::new(value).map(ObsId)
    }

    pub fn get(self) -> i64 {
        self.0.get()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlobHash(pub String);

impl BlobHash {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DocId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl fmt::Display for Seq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl fmt::Display for ObsId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl fmt::Display for BlobHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

macro_rules! i64_sql {
    ($ty:ident) => {
        impl ToSql for $ty {
            fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                Ok(ToSqlOutput::Owned(Value::Integer(self.0)))
            }
        }

        impl FromSql for $ty {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                value.as_i64().map($ty)
            }
        }
    };
}

i64_sql!(DocId);
i64_sql!(SessionId);
i64_sql!(Seq);

impl ToSql for ObsId {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Owned(Value::Integer(self.0.get())))
    }
}

impl FromSql for ObsId {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let raw = value.as_i64()?;
        NonZeroI64::new(raw)
            .map(ObsId)
            .ok_or(FromSqlError::InvalidType)
    }
}

impl ToSql for BlobHash {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Borrowed(ValueRef::Text(self.0.as_bytes())))
    }
}

impl FromSql for BlobHash {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        value.as_str().map(|s| BlobHash(s.to_string()))
    }
}

pub(crate) fn obs_id_from_rowid(rowid: i64) -> Result<ObsId, crate::Error> {
    NonZeroI64::new(rowid)
        .map(ObsId)
        .ok_or_else(|| crate::Error::Invalid(format!("insert produced non-positive rowid {rowid}")))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn obs_id_from_rowid_rejects_zero() {
        assert!(obs_id_from_rowid(0).is_err());
    }

    #[test]
    fn obs_id_from_rowid_accepts_positive() {
        assert_eq!(obs_id_from_rowid(7).expect("ok").get(), 7);
    }

    #[test]
    fn seq_display_renders_the_decimal_value_not_an_empty_default() {
        assert_eq!(Seq(42).to_string(), "42");
    }

    #[test]
    fn obs_id_display_renders_the_decimal_value_not_an_empty_default() {
        assert_eq!(ObsId::new(7).expect("nonzero").to_string(), "7");
    }

    #[test]
    fn blob_hash_display_renders_the_hash_text_not_an_empty_default() {
        assert_eq!(BlobHash("deadbeef".to_string()).to_string(), "deadbeef");
    }
}
