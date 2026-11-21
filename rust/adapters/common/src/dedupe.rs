use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    hash::Hash,
};

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

pub fn dedupe_preferred<T, K: Eq + Hash>(
    records: impl IntoIterator<Item = T>,
    mut key: impl FnMut(&T) -> K,
    mut prefer: impl FnMut(&T, &T) -> bool,
) -> Vec<T> {
    let mut positions = HashMap::<K, usize>::new();
    let mut kept = Vec::new();
    for record in records {
        match positions.entry(key(&record)) {
            Entry::Vacant(position) => {
                position.insert(kept.len());
                kept.push(record);
            }
            Entry::Occupied(position) => {
                let previous = &mut kept[*position.get()];
                if prefer(&record, previous) {
                    *previous = record;
                }
            }
        }
    }
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
