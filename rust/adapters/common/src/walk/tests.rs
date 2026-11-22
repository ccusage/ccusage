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
