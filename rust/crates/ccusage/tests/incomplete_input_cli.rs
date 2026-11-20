#![cfg(test)]

use ccusage_test_support::{fs_fixture, instant, set_modified};

mod support;

#[test]
fn an_unchanged_incomplete_document_is_skipped_at_any_report_clock() {
    let fixture = fs_fixture!({
        "opencode/storage/message/session/broken.json":
            r#"{"id":"broken","role":"assistant","modelID":"gpt-5","tokens":{"input":"#,
    });
    set_modified(
        &fixture.path("opencode/storage/message/session/broken.json"),
        instant("2026-01-02T11:59:30Z"),
    );
    let run = |now| {
        let output = support::hermetic_command(fixture.root())
            .env("OPENCODE_DATA_DIR", fixture.path("opencode"))
            .env("CCUSAGE_TEST_NOW", now)
            .env("LOG_LEVEL", "3")
            .args([
                "opencode",
                "daily",
                "--json",
                "--offline",
                "--debug",
                "--mode",
                "display",
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        (output.stdout, String::from_utf8(output.stderr).unwrap())
    };
    let (fresh, fresh_diagnostics) = run("2026-01-02T12:00:00Z");
    let (stale, stale_diagnostics) = run("2026-01-02T12:01:30Z");
    for diagnostics in [&fresh_diagnostics, &stale_diagnostics] {
        assert!(
            diagnostics.contains("WARN  opencode: skipped 1 input (invalid data)"),
            "{diagnostics}"
        );
        assert!(
            !diagnostics.contains("still being written"),
            "{diagnostics}"
        );
    }
    assert_eq!(fresh, stale);
}
