use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};

use crate::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObsOrigin {
    Load,
    Save,
    Watch,
    Probe,
    Resolve,
    Swap,
}

impl ObsOrigin {
    pub const ALL: [ObsOrigin; 6] = [
        ObsOrigin::Load,
        ObsOrigin::Save,
        ObsOrigin::Watch,
        ObsOrigin::Probe,
        ObsOrigin::Resolve,
        ObsOrigin::Swap,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ObsOrigin::Load => "load",
            ObsOrigin::Save => "save",
            ObsOrigin::Watch => "watch",
            ObsOrigin::Probe => "probe",
            ObsOrigin::Resolve => "resolve",
            ObsOrigin::Swap => "swap",
        }
    }

    pub fn is_ancestor_eligible(self) -> bool {
        matches!(self, ObsOrigin::Load | ObsOrigin::Save | ObsOrigin::Resolve)
    }
}

impl TryFrom<&str> for ObsOrigin {
    type Error = Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        ObsOrigin::ALL
            .into_iter()
            .find(|origin| origin.as_str() == value)
            .ok_or_else(|| Error::Invalid(format!("unknown observation origin: {value:?}")))
    }
}

impl FromSql for ObsOrigin {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        ObsOrigin::try_from(value.as_str()?).map_err(|_| FromSqlError::InvalidType)
    }
}

impl ToSql for ObsOrigin {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.as_str()))
    }
}

pub fn ancestor_eligible_sql_list() -> String {
    ObsOrigin::ALL
        .into_iter()
        .filter(|origin| origin.is_ancestor_eligible())
        .map(|origin| format!("'{}'", origin.as_str()))
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn as_str_and_try_from_round_trip_every_variant() {
        for origin in ObsOrigin::ALL {
            assert_eq!(ObsOrigin::try_from(origin.as_str()).unwrap(), origin);
        }
    }

    #[test]
    fn try_from_rejects_an_unknown_value() {
        assert!(ObsOrigin::try_from("bogus").is_err());
    }

    #[test]
    fn ancestor_eligible_sql_list_names_exactly_the_eligible_origins() {
        assert_eq!(ancestor_eligible_sql_list(), "'load','save','resolve'");
    }
}
