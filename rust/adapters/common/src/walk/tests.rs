use ccusage_test_support::Fixture;

use super::collect_files_matching;

#[test]
fn discovery_is_sorted_and_stops_at_the_first_match() {
    let fixture = Fixture::new();
    fixture.write_file("z.jsonl", "");
    let first = fixture.write_file("a/first.jsonl", "");
    fixture.write_file("a/second.jsonl", "");
    let diagnostics = ccusage_core::Diagnostics::new("test");
    let mut visited = Vec::new();
    assert!(super::any_file_matching(
        fixture.root(),
        &|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        },
        &diagnostics,
    ));
    let result = super::visit_files_matching(
        fixture.root(),
        &|_| true,
        &mut |path| {
            visited.push(path);
            std::ops::ControlFlow::Break(())
        },
        &diagnostics,
    );
    assert_eq!(result, std::ops::ControlFlow::Break(()));
    assert_eq!(visited, [first]);
}

#[test]
fn directory_entries_and_recursive_collection_have_path_order() {
    let fixture = Fixture::new();
    for name in ["z.jsonl", "b/z.jsonl", "b/a.jsonl", "a.jsonl"] {
        fixture.write_file(name, "");
    }
    let diagnostics = ccusage_core::Diagnostics::new("test");
    let mut files = Vec::new();
    super::collect_files_with_extension(fixture.root(), "jsonl", &mut files, &diagnostics);
    assert_eq!(
        files,
        ["a.jsonl", "b/a.jsonl", "b/z.jsonl", "z.jsonl"].map(|name| fixture.path(name))
    );
}

#[cfg(unix)]
#[test]
fn discovery_follows_symlinked_files() {
    let fixture = Fixture::new();
    let target = fixture.write_file("transcript", "");
    let root = fixture.create_dir_all("root");
    let alias = root.join("session.jsonl");
    std::os::unix::fs::symlink(target, &alias).unwrap();

    let mut files = Vec::new();
    let diagnostics = ccusage_core::Diagnostics::new("test");
    super::collect_files_with_extension(&root, "jsonl", &mut files, &diagnostics);

    assert_eq!(files, [alias]);
}

#[cfg(unix)]
#[test]
fn discovery_follows_symlinked_directories() {
    let fixture = Fixture::new();
    fixture.write_file("project/nested/session.jsonl", "");
    let root = fixture.create_dir_all("root");
    let alias = root.join("linked-project");
    std::os::unix::fs::symlink(fixture.path("project"), &alias).unwrap();

    let mut files = Vec::new();
    let diagnostics = ccusage_core::Diagnostics::new("test");
    super::collect_files_with_extension(&root, "jsonl", &mut files, &diagnostics);

    assert_eq!(files, [alias.join("nested/session.jsonl")]);
}

#[cfg(unix)]
#[test]
fn discovery_reads_multiple_links_to_the_same_target_once() {
    let fixture = Fixture::new();
    let target = fixture.write_file("transcript.jsonl", "usage");
    let root = fixture.create_dir_all("root");
    let first = root.join("a.jsonl");
    std::os::unix::fs::symlink(&target, &first).unwrap();
    std::os::unix::fs::symlink(&target, root.join("b.jsonl")).unwrap();

    let diagnostics = ccusage_core::Diagnostics::new("test");
    let reads = crate::read_discovered_paths(
        true,
        |visit| super::visit_usage_files(&root, visit, &diagnostics),
        |path| std::fs::read_to_string(path).unwrap(),
    );

    assert_eq!(reads, [(first, "usage".to_owned())]);
}

#[cfg(unix)]
#[test]
fn discovery_prefers_the_original_path_over_its_aliases() {
    let fixture = Fixture::new();
    let root = fixture.create_dir_all("root");
    let target = fixture.write_file("root/b/session.jsonl", "");
    let linked = root.join("d.jsonl");
    std::os::unix::fs::symlink(root.join("b"), root.join("a")).unwrap();
    std::os::unix::fs::symlink(&target, root.join("c.jsonl")).unwrap();
    std::os::unix::fs::symlink(fixture.write_file("external.jsonl", ""), &linked).unwrap();

    let mut files = Vec::new();
    let diagnostics = ccusage_core::Diagnostics::new("test");
    super::collect_files_with_extension(&root, "jsonl", &mut files, &diagnostics);

    assert_eq!(files, [target, linked]);
}

#[cfg(unix)]
#[test]
fn discovery_stops_at_symlink_cycles() {
    let fixture = Fixture::new();
    let root = fixture.create_dir_all("root");
    let original = fixture.write_file("root/a.jsonl", "");
    fixture.write_file("project/b.jsonl", "");
    let alias = root.join("linked-project");
    std::os::unix::fs::symlink(fixture.path("project"), &alias).unwrap();
    std::os::unix::fs::symlink(&root, fixture.path("project/back")).unwrap();

    let mut files = Vec::new();
    let diagnostics = ccusage_core::Diagnostics::new("test");
    super::collect_files_with_extension(&root, "jsonl", &mut files, &diagnostics);

    assert_eq!(files, [original, alias.join("b.jsonl")]);
    assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::Unreadable), 0);
}

#[cfg(unix)]
#[test]
fn discovery_reports_dangling_symlinks_as_unreadable() {
    let fixture = Fixture::new();
    let original = fixture.write_file("a.jsonl", "");
    std::os::unix::fs::symlink(fixture.path("missing"), fixture.path("b.jsonl")).unwrap();

    let mut files = Vec::new();
    let diagnostics = ccusage_core::Diagnostics::new("test");
    super::collect_files_with_extension(fixture.root(), "jsonl", &mut files, &diagnostics);

    assert_eq!(files, [original]);
    assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::Unreadable), 1);
}

#[test]
fn collects_nested_files_accepted_by_the_predicate() {
    let fixture = Fixture::new();
    let kept = fixture.write_file("a/b/kept.jsonl", "");
    fixture.write_file("a/skipped.txt", "");
    let mut files = Vec::new();
    let diagnostics = ccusage_core::Diagnostics::new("test");

    collect_files_matching(
        fixture.root(),
        &|path| path.extension().is_some_and(|ext| ext == "jsonl"),
        &mut files,
        &diagnostics,
    );

    assert_eq!(files, vec![kept]);
}

#[test]
fn discovery_reports_failed_roots_without_losing_healthy_roots() {
    use ccusage_core::{Diagnostics, SkipKind};

    let fixture = Fixture::new();
    let failed = fixture.write_file("not-a-directory", "");
    let healthy = fixture.write_file("healthy/nested/session.jsonl", "");
    let diagnostics = Diagnostics::new("test");
    let mut files = Vec::new();
    for root in [fixture.path("missing"), failed, fixture.path("healthy")] {
        super::collect_files_with_extension(&root, "jsonl", &mut files, &diagnostics);
    }

    assert_eq!(files, [healthy]);
    assert_eq!(diagnostics.skipped(SkipKind::Unreadable), 1);
}

fn parallel_usage_files(
    root: &std::path::Path,
    diagnostics: &ccusage_core::Diagnostics,
) -> Vec<std::path::PathBuf> {
    let files = std::sync::Mutex::new(Vec::new());
    super::visit_usage_files_in_parallel(
        root,
        &|path| files.lock().unwrap().push(path),
        diagnostics,
    );
    let mut files = files.into_inner().unwrap();
    files.sort();
    files
}

fn sequential_usage_files(
    root: &std::path::Path,
    diagnostics: &ccusage_core::Diagnostics,
) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    super::visit_usage_files(root, &mut |path| files.push(path), diagnostics);
    files.sort();
    files
}

#[test]
fn parallel_discovery_finds_the_sequential_file_set() {
    let fixture = Fixture::new();
    for project in 0..12 {
        fixture.write_file(format!("project-{project}/session.jsonl"), "");
        fixture.write_file(
            format!("project-{project}/session/subagents/agent.jsonl"),
            "",
        );
        fixture.write_file(format!("project-{project}/notes.txt"), "");
    }
    fixture.write_file("top-level.jsonl", "");
    fixture.create_dir_all("empty-project");
    let diagnostics = ccusage_core::Diagnostics::new("test");

    let files = parallel_usage_files(fixture.root(), &diagnostics);

    assert_eq!(files.len(), 25);
    assert_eq!(files, sequential_usage_files(fixture.root(), &diagnostics));
}

#[cfg(unix)]
#[test]
fn parallel_discovery_keeps_ordinary_paths_and_the_first_alias_across_walkers() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let root = fixture.create_dir_all("root");
    let original = fixture.write_file("root/b/original.jsonl", "");
    fixture.create_dir_all("root/a");
    symlink(&original, root.join("a/alias.jsonl")).unwrap();
    for (name, links) in [
        ("x.jsonl", ["a/x.jsonl", "b/x.jsonl"]),
        ("y.jsonl", ["c.jsonl", "b/y.jsonl"]),
        ("z.jsonl", ["0.jsonl", "b/z.jsonl"]),
    ] {
        let target = fixture.write_file(format!("external/{name}"), "");
        for link in links {
            symlink(&target, root.join(link)).unwrap();
        }
    }
    symlink(root.join("b"), root.join("d")).unwrap();
    let diagnostics = ccusage_core::Diagnostics::new("test");

    let files = parallel_usage_files(&root, &diagnostics);

    let mut expected = ["0.jsonl", "a/x.jsonl", "b/original.jsonl", "b/y.jsonl"]
        .map(|name| root.join(name))
        .to_vec();
    expected.sort();
    assert_eq!(files, expected);
    assert_eq!(files, sequential_usage_files(&root, &diagnostics));
}

#[cfg(unix)]
#[test]
fn parallel_discovery_stops_at_symlink_cycles() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let root = fixture.create_dir_all("root");
    let first = fixture.write_file("root/a/first.jsonl", "");
    let second = fixture.write_file("root/b/second.jsonl", "");
    symlink(&root, root.join("a/back")).unwrap();
    symlink(root.join("a"), root.join("b/sibling")).unwrap();
    let diagnostics = ccusage_core::Diagnostics::new("test");

    assert_eq!(parallel_usage_files(&root, &diagnostics), [first, second]);
    assert_eq!(diagnostics.skipped(ccusage_core::SkipKind::Unreadable), 0);
}

#[cfg(unix)]
#[test]
fn parallel_discovery_reports_unreadable_entries_like_the_sequential_walk() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let fixture = Fixture::new();
    let root = fixture.create_dir_all("root");
    let healthy = fixture.write_file("root/healthy/session.jsonl", "");
    fixture.write_file("root/locked/session.jsonl", "");
    symlink(fixture.path("missing"), root.join("healthy/dangling.jsonl")).unwrap();
    let locked = root.join("locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let parallel = ccusage_core::Diagnostics::new("test");
    let sequential = ccusage_core::Diagnostics::new("test");

    let files = parallel_usage_files(&root, &parallel);
    let expected = sequential_usage_files(&root, &sequential);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(files, [healthy]);
    assert_eq!(files, expected);
    assert_eq!(parallel.skipped(ccusage_core::SkipKind::Unreadable), 2);
    assert_eq!(
        parallel.skipped(ccusage_core::SkipKind::Unreadable),
        sequential.skipped(ccusage_core::SkipKind::Unreadable)
    );
}
