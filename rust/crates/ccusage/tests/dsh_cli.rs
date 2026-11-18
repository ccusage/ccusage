use ccusage_test_support::Fixture;

const PARENT: &str = include_str!("../../../adapters/dsh/tests/fixtures/parent/session.v4.jsonl");
const FORK: &str = include_str!("../../../adapters/dsh/tests/fixtures/fork/session.v4.jsonl");

#[test]
fn dsh_cli_tables_snapshot_production_stdout_and_stderr() {
    let fixture = Fixture::new();
    let _ = fixture.create_dir_all("dsh/sessions/--home-user-project--/dsh-parent");
    std::fs::write(
        fixture.path("dsh/sessions/--home-user-project--/dsh-parent/session.v4.jsonl.zstd"),
        zstd::stream::encode_all(PARENT.as_bytes(), 3).unwrap(),
    )
    .unwrap();
    let _ = fixture.write_file(
        "dsh/sessions/--home-user-project--/dsh-fork/session.v4.jsonl",
        FORK,
    );

    for kind in ["daily", "weekly", "monthly", "session"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_ccusage"))
            .env_clear()
            .env("HOME", fixture.path("home"))
            .env("USERPROFILE", fixture.path("userprofile"))
            .env("XDG_CONFIG_HOME", fixture.path("xdg-config"))
            .env("DSH_HOME", fixture.path("dsh"))
            .args([
                "dsh",
                kind,
                "--since",
                "20261001",
                "--until",
                "20261101",
                "--offline",
                "--no-color",
                "--timezone",
                "UTC",
            ])
            .output()
            .expect("failed to run ccusage");

        assert!(
            output.status.success(),
            "ccusage dsh {kind} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8(output.stdout).expect("CLI stdout was not UTF-8");
        let stderr = String::from_utf8(output.stderr).expect("CLI stderr was not UTF-8");
        insta::assert_snapshot!(
            format!("dsh_cli_{kind}_table"),
            format!("stdout:\n{stdout}\nstderr:\n{stderr}")
        );
    }
}
