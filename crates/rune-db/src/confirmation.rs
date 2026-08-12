use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, Value, ValueRef};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confirmation {
    Unclassified,
    Confirmed,
    Unconfirmed,
}

impl Confirmation {
    pub fn from_bracket(confirmed: bool) -> Confirmation {
        if confirmed {
            Confirmation::Confirmed
        } else {
            Confirmation::Unconfirmed
        }
    }
}

impl FromSql for Confirmation {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value {
            ValueRef::Null => Ok(Confirmation::Unclassified),
            _ => match value.as_i64()? {
                0 => Ok(Confirmation::Unconfirmed),
                1 => Ok(Confirmation::Confirmed),
                _ => Err(FromSqlError::InvalidType),
            },
        }
    }
}

impl ToSql for Confirmation {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Owned(match self {
            Confirmation::Unclassified => Value::Null,
            Confirmation::Confirmed => Value::Integer(1),
            Confirmation::Unconfirmed => Value::Integer(0),
        }))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn from_bracket_maps_true_and_false() {
        assert_eq!(Confirmation::from_bracket(true), Confirmation::Confirmed);
        assert_eq!(Confirmation::from_bracket(false), Confirmation::Unconfirmed);
    }
}
