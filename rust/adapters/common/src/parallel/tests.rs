use std::path::PathBuf;

use ccusage_test_support::Fixture;

use super::{Split, chunk_file_indexes_by_size, map_parallel, worker_count};

#[test]
fn balances_file_chunks_by_size() {
    let fixture = ccusage_test_support::fs_fixture!({
        "large-a.jsonl": "x".repeat(100),
        "small-a.jsonl": "x",
        "small-b.jsonl": "x",
        "large-b.jsonl": "x".repeat(100),
    });
    let files = [
        "large-a.jsonl",
        "small-a.jsonl",
        "small-b.jsonl",
        "large-b.jsonl",
    ]
    .into_iter()
    .map(|name| fixture.path(name))
    .collect::<Vec<_>>();

    let chunks = chunk_file_indexes_by_size(&files, 2);
    assert_eq!(chunks.len(), 2);
    let mut indexes = chunks.iter().flatten().copied().collect::<Vec<_>>();
    indexes.sort_unstable();
    assert_eq!(indexes, vec![0, 1, 2, 3]);

    let chunk_sizes = chunks
        .iter()
        .map(|chunk| {
            chunk
                .iter()
                .map(|index| std::fs::metadata(&files[*index]).unwrap().len())
                .sum::<u64>()
        })
        .collect::<Vec<_>>();
    assert_eq!(chunk_sizes, vec![101, 101]);
}

#[test]
fn preserves_file_order_and_matches_single_thread() {
    let fixture = Fixture::new();
    let files = (0..256)
        .map(|index| {
            let body = "x".repeat((index % 17) * 64 + 1);
            fixture.write_file(format!("file-{index:03}.txt"), format!("{index}:{body}"))
        })
        .collect::<Vec<_>>();
    let read = |path: &PathBuf| {
        let content = std::fs::read_to_string(path).unwrap();
        content.split(':').next().unwrap().to_string()
    };

    let single = map_parallel(&files, true, Split::BySize(PathBuf::as_path), read);
    let multi = map_parallel(&files, false, Split::BySize(PathBuf::as_path), read);
    let expected = Vec::from_fn(256, |index| index.to_string());

    assert_eq!(single, expected);
    assert_eq!(multi, expected);
}

#[test]
fn treats_a_zero_chunk_count_as_one_chunk() {
    let fixture = Fixture::new();
    let files = vec![fixture.write_file("only.txt", "body")];

    assert_eq!(chunk_file_indexes_by_size(&files, 0), vec![vec![0]]);
}

#[test]
fn handles_empty_input() {
    let empty: Vec<std::path::PathBuf> = Vec::new();

    assert!(map_parallel(&empty, false, Split::BySize(PathBuf::as_path), |_| 0_u8).is_empty());
}

#[test]
fn count_split_keeps_item_order() {
    let items = Vec::from_fn(1000, |index| index);

    for single_thread in [true, false] {
        assert_eq!(
            map_parallel(&items, single_thread, Split::ByCount, |item| item * 2),
            Vec::from_fn(1000, |index| index * 2),
            "single_thread={single_thread}"
        );
    }
}

#[test]
fn worker_count_is_one_for_a_single_thread_or_at_most_one_item() {
    assert_eq!(worker_count(true, 100), 1);
    assert_eq!(worker_count(false, 0), 1);
    assert_eq!(worker_count(false, 1), 1);
    assert!((1..=3).contains(&worker_count(false, 3)));
}

#[test]
#[should_panic(expected = "worker failed")]
fn a_worker_panic_reaches_the_caller() {
    let items = Vec::from_fn(64, |index| index);

    map_parallel(&items, false, Split::ByCount, |item| {
        assert_ne!(*item, 7, "worker failed");
    });
}
