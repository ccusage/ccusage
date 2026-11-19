use std::{collections::HashSet, hash::Hash};

pub fn dedupe_first_wins<T, K: Eq + Hash>(
    records: impl IntoIterator<Item = T>,
    mut key: impl FnMut(&T) -> K,
) -> Vec<T> {
    let mut seen = HashSet::new();
    records
        .into_iter()
        .filter(|record| seen.insert(key(record)))
        .collect()
}

pub fn dedupe_last_wins<T, K: Eq + Hash>(
    records: impl IntoIterator<Item = T>,
    mut key: impl FnMut(&T) -> K,
) -> Vec<T> {
    let records: Vec<_> = records.into_iter().collect();
    let mut kept = dedupe_first_wins(records.into_iter().rev(), |record| key(record));
    kept.reverse();
    kept
}

#[cfg(test)]
mod tests {
    #[test]
    fn first_wins_preserves_the_order_of_surviving_records() {
        let records = [(2, "first"), (1, "one"), (2, "last"), (3, "three")];
        assert_eq!(
            super::dedupe_first_wins(records, |record| record.0),
            [(2, "first"), (1, "one"), (3, "three")]
        );
    }

    #[test]
    fn last_wins_preserves_the_order_of_surviving_records() {
        let records = [(2, "first"), (1, "one"), (2, "last"), (3, "three")];
        assert_eq!(
            super::dedupe_last_wins(records, |record| record.0),
            [(1, "one"), (2, "last"), (3, "three")]
        );
    }
}
