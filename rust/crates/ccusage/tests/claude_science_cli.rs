use ccusage_test_support::{Fixture, claude_science::create_fixture};

#[test]
fn claude_science_cli_tables_snapshot_production_stdout_and_stderr() {
    let fixture = Fixture::new();
    let _ = fixture.create_dir_all("claude-science");
    create_fixture(fixture.path("claude-science/metadata.db"));

    for kind in ["daily", "monthly", "session"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_ccusage"))
            .env_clear()
            .env("HOME", fixture.path("home"))
            .env("USERPROFILE", fixture.path("userprofile"))
            .env("XDG_CONFIG_HOME", fixture.path("xdg-config"))
            .env(
                "CLAUDE_SCIENCE_DB",
                fixture.path("claude-science/metadata.db"),
            )
            .args([
                "claude-science",
                kind,
                "--since",
                "20990101",
                "--until",
                "20990301",
                "--mode",
                "calculate",
                "--offline",
                "--no-color",
                "--timezone",
                "UTC",
            ])
            .output()
            .expect("failed to run ccusage");

        assert!(
            output.status.success(),
            "ccusage claude-science {kind} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8(output.stdout).expect("CLI stdout was not UTF-8");
        let stderr = String::from_utf8(output.stderr).expect("CLI stderr was not UTF-8");
        insta::assert_snapshot!(
            format!("claude_science_cli_{kind}_table"),
            format!("stdout:\n{stdout}\nstderr:\n{stderr}")
        );
    }
}
