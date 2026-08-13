use std::fmt;
use std::num::NonZeroI64;

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, Value, ValueRef};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocId(pub i64);

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

}
