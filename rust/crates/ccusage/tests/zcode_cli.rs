use ccusage_test_support::{Fixture, zcode::create_fixture};

#[test]
fn zcode_cli_tables_snapshot_production_stdout_and_stderr() {
    let fixture = Fixture::new();
    let _ = fixture.create_dir_all("zcode/cli/db");
    create_fixture(fixture.path("zcode/cli/db/db.sqlite"));

    for kind in ["daily", "monthly", "session"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_ccusage"))
            .env_clear()
            .env("HOME", fixture.path("home"))
            .env("USERPROFILE", fixture.path("userprofile"))
            .env("XDG_CONFIG_HOME", fixture.path("xdg-config"))
            .env("ZCODE_HOME", fixture.path("zcode"))
            .args([
                "zcode",
                kind,
                "--since",
                "20990101",
                "--until",
                "20990201",
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
            "ccusage zcode {kind} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8(output.stdout).expect("CLI stdout was not UTF-8");
        let stderr = String::from_utf8(output.stderr).expect("CLI stderr was not UTF-8");
        insta::assert_snapshot!(
            format!("zcode_cli_{kind}_table"),
            format!("stdout:\n{stdout}\nstderr:\n{stderr}")
        );
    }
}
