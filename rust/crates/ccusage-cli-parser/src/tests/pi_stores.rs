use super::*;

#[test]
fn invalid_explicit_named_stores_fail_statusline() {
    let fixture = fs_fixture!({
        "ccusage.json": r#"{ "pi": { "stores": [{ "name": "codex", "path": "/tmp/omp" }] } }"#,
    });
    let args = vec![
        "statusline".to_string(),
        "--config".to_string(),
        fixture.path("ccusage.json").to_string_lossy().into_owned(),
    ];
    let config = ccusage_config::ConfigContext::from_args(&args);

    let parsed = parse_from_with_config(
        std::iter::once(std::ffi::OsString::from("ccusage")).chain(
            args.iter()
                .map(|arg| std::ffi::OsString::from(arg.as_str())),
        ),
        &config,
    );

    let Err(error) = parsed else {
        panic!("expected invalid explicit config to fail")
    };
    assert!(error.contains("ccusage.json:"));
    assert!(error.ends_with("pi.stores name 'codex' collides with a built-in agent"));
}

#[test]
fn named_pi_store_validation_does_not_break_statusline() {
    assert_invalid_discovered_stores_allow(&["statusline"]);
}

#[test]
fn named_pi_store_validation_does_not_break_agent_commands() {
    assert_invalid_discovered_stores_allow(&["codex", "daily"]);
}

fn assert_invalid_discovered_stores_allow(args: &[&str]) {
    let fixture = fs_fixture!({
        "ccusage.json": r#"{"pi":{"stores":[{"name":"codex","path":"/tmp/omp"}]}}"#,
    });
    ccusage_test_support::Environment::with_var("CLAUDE_CONFIG_DIR", fixture.root()).run(|| {
        let args = args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        let config = ccusage_config::ConfigContext::from_args(&args);
        let parsed = parse_from_with_config(
            std::iter::once(std::ffi::OsString::from("ccusage"))
                .chain(args.iter().map(std::ffi::OsString::from)),
            &config,
        );
        assert!(parsed.is_ok(), "{:?}", parsed.err());
    })
}

#[test]
fn invalid_explicit_named_stores_fail_agent_commands() {
    let fixture = fs_fixture!({
        "ccusage.json": r#"{ "pi": { "stores": [{ "name": "codex", "path": "/tmp/omp" }] } }"#,
    });
    let args = vec![
        "codex".to_string(),
        "daily".to_string(),
        "--config".to_string(),
        fixture.path("ccusage.json").to_string_lossy().into_owned(),
    ];
    let config = ccusage_config::ConfigContext::from_args(&args);

    let parsed = parse_from_with_config(
        std::iter::once(std::ffi::OsString::from("ccusage")).chain(
            args.iter()
                .map(|arg| std::ffi::OsString::from(arg.as_str())),
        ),
        &config,
    );

    let Err(error) = parsed else {
        panic!("expected invalid explicit config to fail")
    };
    assert!(error.contains("ccusage.json:"));
    assert!(error.ends_with("pi.stores name 'codex' collides with a built-in agent"));
}

#[test]
fn reports_named_pi_store_validation_through_cli_config_error_path() {
    let fixture = fs_fixture!({
        "ccusage.json": r#"{ "pi": { "stores": [{ "name": "codex", "path": "/tmp/omp" }] } }"#,
    });
    let args = vec![
        "daily".to_string(),
        "--config".to_string(),
        fixture.path("ccusage.json").to_string_lossy().into_owned(),
    ];
    let config = ccusage_config::ConfigContext::from_args(&args);

    let result = parse_from_with_config(
        std::iter::once(std::ffi::OsString::from("ccusage")).chain(
            args.iter()
                .map(|arg| std::ffi::OsString::from(arg.as_str())),
        ),
        &config,
    );
    let Err(error) = result else {
        panic!("expected config error");
    };

    assert!(error.starts_with("Invalid ccusage config: "));
    assert!(error.contains("ccusage.json:"));
    assert!(error.ends_with("pi.stores name 'codex' collides with a built-in agent"));
}
