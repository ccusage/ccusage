use std::path::PathBuf;

use ccusage_test_support::Fixture;

#[test]
fn reads_every_discovered_file_once() {
    let fixture = Fixture::new();
    let files = (0..64)
        .map(|index| {
            fixture.write_file(
                format!("d{}/file-{index:02}.txt", index % 5),
                index.to_string(),
            )
        })
        .collect::<Vec<_>>();
    let read = |path: &PathBuf| std::fs::read_to_string(path).unwrap();

    for single_thread in [true, false] {
        let mut results = super::read_discovered_paths(
            single_thread,
            |visit| files.iter().cloned().for_each(visit),
            read,
        );
        results.sort();
        let mut expected = files
            .iter()
            .map(|path| (path.clone(), read(path)))
            .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(results, expected, "single_thread={single_thread}");
    }
    assert!(super::read_discovered_paths(false, |_| {}, read).is_empty());
}

#[test]
fn reads_discovered_work_before_discovery_returns() {
    let (started, observed) = std::sync::mpsc::sync_channel(1);
    let results = super::read_discovered_files(
        false,
        &mut |emit| {
            emit(7, PathBuf::from("rollout.jsonl"));
            assert_eq!(
                observed
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap(),
                7
            );
        },
        |file| {
            started.send(file.group).unwrap();
            file.group * 2
        },
    );
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0.index, 0);
    assert_eq!(results[0].1, 14);
}

#[test]
fn reads_every_path_from_concurrent_producers() {
    let producers = 4;
    let per_producer = 2 * super::PLAIN_PATH_BACKLOG;
    let mut results = super::read_paths_discovered_in_parallel(
        |emit| {
            std::thread::scope(|scope| {
                for producer in 0..producers {
                    ccusage_core::parallel::spawn_scoped_worker(scope, move || {
                        for index in 0..per_producer {
                            emit(PathBuf::from(format!("{producer}/{index}")));
                        }
                    });
                }
            });
        },
        |path| {
            std::thread::sleep(std::time::Duration::from_micros(20));
            path.clone()
        },
    );
    results.sort();
    let mut expected = (0..producers)
        .flat_map(|producer| {
            (0..per_producer).map(move |index| PathBuf::from(format!("{producer}/{index}")))
        })
        .map(|path| (path.clone(), path))
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(results, expected);
}
