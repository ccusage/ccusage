use ccusage_test_support::Fixture;

const TRANSCRIPT: &str = r#"{
    "schema_version": "ATIF-v1.7",
    "session_id": "veil-vibraphone",
    "agent": {"name": "devin", "version": "3000.6.12", "model_name": "SWE-1.7"},
    "steps": [
        {"step_id": 1, "timestamp": "2099-01-02T00:00:00.000000+00:00", "source": "system"},
        {"step_id": 2, "timestamp": "2099-01-02T08:00:00.000000+00:00", "source": "agent",
         "model_name": "SWE-1.7",
         "metrics": {"prompt_tokens": 1000, "completion_tokens": 100, "cached_tokens": 400,
                     "extra": {"cache_creation_input_tokens": 200}},
         "extra": {"generation_model": "swe-1.7"}},
        {"step_id": 3, "timestamp": "2099-01-03T08:00:00.000000+00:00", "source": "agent",
         "model_name": "SWE-1.7",
         "metrics": {"prompt_tokens": 500, "completion_tokens": 50, "cached_tokens": 300},
         "extra": {"generation_model": "swe-1.7"}}
    ]
}"#;

#[test]
fn devin_cli_tables_snapshot_production_stdout_and_stderr() {
    let fixture = Fixture::new();
    let _ = fixture.write_file("devin/transcripts/veil-vibraphone.json", TRANSCRIPT);

    for kind in ["daily", "monthly", "session"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_ccusage"))
            .env_clear()
            .env("HOME", fixture.path("home"))
            .env("USERPROFILE", fixture.path("userprofile"))
            .env("XDG_CONFIG_HOME", fixture.path("xdg-config"))
            .env("DEVIN_TRANSCRIPTS_DIR", fixture.path("devin/transcripts"))
            .args([
                "devin",
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
            "ccusage devin {kind} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8(output.stdout).expect("CLI stdout was not UTF-8");
        let stderr = String::from_utf8(output.stderr).expect("CLI stderr was not UTF-8");
        insta::assert_snapshot!(
            format!("devin_cli_{kind}_table"),
            format!("stdout:\n{stdout}\nstderr:\n{stderr}")
        );
    }
}
