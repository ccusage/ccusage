use super::{stable_by, stable_by_key};

#[test]
fn matches_std_stable_sort_including_ties() {
    let input = Vec::from_fn(200, |index| ((index * 37) % 11, index));
    let mut expected = input.clone();
    expected.sort_by_key(|(key, _)| *key);
    let mut sorted = input;
    stable_by_key(&mut sorted, |(key, _)| *key);
    assert_eq!(sorted, expected);
}

#[test]
fn sorts_with_a_comparator() {
    let mut values = vec![3.5, -1.0, 2.25, 9.0, 0.0];
    stable_by(&mut values, |left: &f64, right| right.total_cmp(left));
    assert_eq!(values, vec![9.0, 3.5, 2.25, 0.0, -1.0]);
}

#[test]
fn keeps_owned_values_intact() {
    let mut names = ["delta", "alpha", "charlie", "bravo"]
        .map(String::from)
        .to_vec();
    stable_by(&mut names, |left, right| left.cmp(right));
    assert_eq!(names, ["alpha", "bravo", "charlie", "delta"]);
}

#[test]
fn handles_empty_and_single_inputs() {
    let mut empty: Vec<u8> = Vec::new();
    stable_by_key(&mut empty, |value| *value);
    assert!(empty.is_empty());
    let mut single = vec![7];
    stable_by_key(&mut single, |value| *value);
    assert_eq!(single, vec![7]);
}
