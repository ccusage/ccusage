use sqlite::{Statement, Type};

// NOTE(2026-11-17, sqlite 0.37.4): `Statement::read::<i64>` and `read::<f64>` never fail on a valid index; they turn NULL into 0 and clamp an out-of-range REAL, so a numeric column is read by the type SQLite stored, and text keeps SQLite's own numeric conversion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Number {
    Integer(i64),
    Real(f64),
}

impl Number {
    pub fn as_i64(self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(value),
            Self::Real(value) => {
                let truncated = value.trunc();
                (truncated >= i64::MIN as f64 && truncated < i64::MAX as f64)
                    .then_some(truncated as i64)
            }
        }
    }

    pub fn as_f64(self) -> f64 {
        match self {
            Self::Integer(value) => value as f64,
            Self::Real(value) => value,
        }
    }
}

pub fn read_number(statement: &Statement<'_>, index: usize) -> Option<Number> {
    match statement.column_type(index).ok()? {
        Type::Integer => statement.read::<i64, _>(index).ok().map(Number::Integer),
        Type::Float | Type::String => statement
            .read::<f64, _>(index)
            .ok()
            .filter(|value| value.is_finite())
            .map(Number::Real),
        Type::Binary | Type::Null => None,
    }
}

pub fn read_i64(statement: &Statement<'_>, index: usize) -> Option<i64> {
    read_number(statement, index)?.as_i64()
}

pub fn read_f64(statement: &Statement<'_>, index: usize) -> Option<f64> {
    read_number(statement, index).map(Number::as_f64)
}

pub fn read_count(statement: &Statement<'_>, index: usize) -> u64 {
    read_i64(statement, index)
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{Number, read_count, read_f64, read_i64, read_number};

    fn with_row(values: &str, check: impl FnOnce(&sqlite::Statement<'_>)) {
        let fixture = ccusage_test_support::Fixture::new();
        let connection = sqlite::open(fixture.path("row.db")).unwrap();
        let mut statement = connection.prepare(format!("SELECT {values}")).unwrap();
        assert_eq!(statement.next().unwrap(), sqlite::State::Row);
        check(&statement);
    }

    #[test]
    fn reads_each_value_by_its_stored_type() {
        with_row(
            "NULL, 7, 2.5, '12abc', 'x', zeroblob(1), 9e999",
            |statement| {
                assert_eq!(read_number(statement, 0), None);
                assert_eq!(read_number(statement, 1), Some(Number::Integer(7)));
                assert_eq!(read_number(statement, 2), Some(Number::Real(2.5)));
                assert_eq!(read_number(statement, 3), Some(Number::Real(12.0)));
                assert_eq!(read_number(statement, 4), Some(Number::Real(0.0)));
                assert_eq!(read_number(statement, 5), None);
                assert_eq!(read_number(statement, 6), None);
            },
        );
    }

    #[test]
    fn rejects_a_real_outside_the_integer_range_instead_of_clamping_it() {
        with_row("1e300, -1e300, 9e18, 1.9", |statement| {
            assert_eq!(read_i64(statement, 0), None);
            assert_eq!(read_i64(statement, 1), None);
            assert_eq!(read_i64(statement, 2), Some(9_000_000_000_000_000_000));
            assert_eq!(read_i64(statement, 3), Some(1));
            assert_eq!(read_f64(statement, 0), Some(1e300));
        });
    }

    #[test]
    fn counts_missing_negative_and_unreadable_values_as_zero() {
        with_row("NULL, -5, 1e300, 42, 3.7", |statement| {
            assert_eq!(read_count(statement, 0), 0);
            assert_eq!(read_count(statement, 1), 0);
            assert_eq!(read_count(statement, 2), 0);
            assert_eq!(read_count(statement, 3), 42);
            assert_eq!(read_count(statement, 4), 3);
        });
    }
}
