use ccusage_core::pricing::MatchKind;

use super::*;
use ccusage_test_support::fs_fixture;
use std::path::Path;

fn read_session_file(
    path: &Path,
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: PriceTable<'_>,
) -> crate::Result<Vec<PiLoadedEntry>> {
    read_session_file_data_for_store_with_diagnostics(
        path,
        path.parent().and_then(Path::parent).unwrap(),
        tz,
        mode,
        pricing,
        "pi",
        &Diagnostics::new("pi"),
    )
    .map(|data| data.entries)
    .map_err(|error| ccusage_core::cli_error(error.to_string()))
}

fn read_session_file_for_store(
    path: &Path,
    store_root: &Path,
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: PriceTable<'_>,
    store_name: &str,
) -> crate::Result<Vec<PiLoadedEntry>> {
    read_session_file_data_for_store_with_diagnostics(
        path,
        store_root,
        tz,
        mode,
        pricing,
        store_name,
        &Diagnostics::new("pi"),
    )
    .map(|data| data.entries)
    .map_err(|error| ccusage_core::cli_error(error.to_string()))
}

fn pricing_for_cost_tests() -> ccusage_core::PricingMap {
    ccusage_test_support::install_pricing_snapshots();
    let mut pricing = ccusage_core::PricingMap::default();
    pricing.load_json(
        r#"{
                "[pi] test-model": {
                    "input_cost_per_token": 0.000002,
                    "output_cost_per_token": 0.000008
                },
                "[omp] test-model": {
                    "input_cost_per_token": 0.000002,
                    "output_cost_per_token": 0.000008
                }
            }"#,
    );
    pricing
}

fn usage_for_cost_tests() -> crate::TokenUsageRaw {
    crate::TokenUsageRaw {
        input_tokens: 1000,
        output_tokens: 2000,
        ..crate::TokenUsageRaw::default()
    }
}

fn pricing_with_override(model: &str) -> ccusage_core::PricingMap {
    let model = model.to_string();
    let override_value = crate::cli::PricingOverride {
        input_cost_per_token: Some(0.001),
        output_cost_per_token: Some(0.002),
        ..Default::default()
    };
    ccusage_core::PricingMap::load_with_overrides(true, false, [(&model, &override_value)])
}

#[test]
fn default_store_auto_uses_pricing_override_over_recorded_cost() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"my-custom-model","usage":{"input":1000,"output":2000,"cost":{"total":0}}}}"#,
    });
    let file = fixture.path("sessions/project-a/agent_session-a.jsonl");
    let pricing = pricing_with_override("[pi] my-custom-model");

    let auto = read_session_file(
        &file,
        &jiff::tz::TimeZone::UTC,
        CostMode::Auto,
        PriceTable::Auto(&pricing),
    )
    .unwrap();
    assert_eq!(auto[0].cost, 5.0);
    assert_eq!(auto[0].recorded_cost, Some(0.0));
    assert_eq!(auto[0].missing_pricing_model, None);

    let display = read_session_file(
        &file,
        &jiff::tz::TimeZone::UTC,
        CostMode::Display,
        PriceTable::Display,
    )
    .unwrap();
    assert_eq!(display[0].cost, 0.0);
}

#[test]
fn named_store_auto_uses_pricing_override_over_recorded_cost() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"my-custom-model","usage":{"input":1000,"output":2000,"cost":{"total":0}}}}"#,
    });
    let file = fixture.path("sessions/project-a/agent_session-a.jsonl");
    let pricing = pricing_with_override("[omp] my-custom-model");

    let auto = read_session_file_for_store(
        &file,
        &fixture.path("sessions"),
        &jiff::tz::TimeZone::UTC,
        CostMode::Auto,
        PriceTable::Auto(&pricing),
        "omp",
    )
    .unwrap();
    assert_eq!(auto[0].cost, 5.0);
    assert_eq!(auto[0].recorded_cost, Some(0.0));
    assert_eq!(auto[0].missing_pricing_model, None);

    let display = read_session_file_for_store(
        &file,
        &fixture.path("sessions"),
        &jiff::tz::TimeZone::UTC,
        CostMode::Display,
        PriceTable::Display,
        "omp",
    )
    .unwrap();
    assert_eq!(display[0].cost, 0.0);
}

#[test]
fn bare_model_override_applies_to_default_and_named_stores() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"my-custom-model","usage":{"input":1000,"output":2000,"cost":{"total":0}}}}"#,
    });
    let file = fixture.path("sessions/project-a/agent_session-a.jsonl");
    let pricing = pricing_with_override("my-custom-model");

    let default = read_session_file(
        &file,
        &jiff::tz::TimeZone::UTC,
        CostMode::Auto,
        PriceTable::Auto(&pricing),
    )
    .unwrap();
    assert_eq!(default[0].cost, 5.0);
    assert_eq!(default[0].missing_pricing_model, None);

    let named = read_session_file_for_store(
        &file,
        &fixture.path("sessions"),
        &jiff::tz::TimeZone::UTC,
        CostMode::Auto,
        PriceTable::Auto(&pricing),
        "omp",
    )
    .unwrap();
    assert_eq!(named[0].cost, 5.0);
    assert_eq!(named[0].missing_pricing_model, None);
}

#[test]
fn skips_and_diagnoses_token_counts_that_exceed_the_u64_range() {
    let fixture = fs_fixture!({
        "sessions/project-a/agent_session-a.jsonl": concat!(
            r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":18446744073709551615,"output":1}}}"#,
            "\n",
            r#"{"type":"message","timestamp":"2026-01-02T00:00:01.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":1}}}"#,
            "\n",
        ),
    });
    let file = fixture.path("sessions/project-a/agent_session-a.jsonl");
    let diagnostics = Diagnostics::new("pi");

    let data = read_session_file_data_for_store_with_diagnostics(
        &file,
        &fixture.path("sessions"),
        &jiff::tz::TimeZone::UTC,
        CostMode::Display,
        PriceTable::Display,
        "pi",
        &diagnostics,
    )
    .unwrap();

    assert_eq!(data.entries.len(), 1);
    assert_eq!(data.entries[0].entry.usage.input_tokens, 10);
    assert_eq!(diagnostics.skipped(SkipKind::InvalidData), 1);
}

#[test]
fn diagnoses_assistant_usage_without_a_valid_timestamp() {
    let fixture = fs_fixture!({
        "sessions/project-a/agent_session-a.jsonl": concat!(
            r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":1}}}"#,
            "\n",
            r#"{"type":"message","timestamp":"SECRET_CANARY","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":1}}}"#,
            "\n",
            r#"{"type":"message","message":{"role":"assistant","model":"gpt-5","usage":{"input":10,"output":1}}}"#,
            "\n",
            r#"{"type":"message","message":{"role":"assistant","model":"gpt-5","usage":{"input":0,"output":0}}}"#,
            "\n",
        ),
    });
    let file = fixture.path("sessions/project-a/agent_session-a.jsonl");
    let diagnostics = Diagnostics::new("pi");

    let data = read_session_file_data_for_store_with_diagnostics(
        &file,
        &fixture.path("sessions"),
        &jiff::tz::TimeZone::UTC,
        CostMode::Display,
        PriceTable::Display,
        "pi",
        &diagnostics,
    )
    .unwrap();

    assert_eq!(data.entries.len(), 1);
    assert_eq!(diagnostics.skipped(SkipKind::InvalidTimestamp), 2);
    let warnings = diagnostics
        .warnings(&crate::cli::SharedArgs {
            debug: true,
            ..Default::default()
        })
        .join("\n");
    assert!(warnings.contains("agent_session-a.jsonl:2"));
    assert!(warnings.contains("agent_session-a.jsonl:3"));
    assert!(!warnings.contains("SECRET_CANARY"));
}

#[test]
fn keeps_total_only_usage_out_of_output_tokens() {
    let pricing = pricing_for_cost_tests();
    let fixture = fs_fixture!({
        "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"test-model","usage":{"totalTokens":333}}}"#,
    });
    let file = fixture.path("sessions/project-a/agent_session-a.jsonl");

    let entries = read_session_file(
        &file,
        &jiff::tz::TimeZone::UTC,
        CostMode::Calculate,
        PriceTable::Calculate(&pricing),
    )
    .unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].usage.output_tokens, 0);
    assert_eq!(entries[0].extra_total_tokens, 333);
    assert_eq!(entries[0].cost, 0.0);
}

#[test]
fn sets_missing_pricing_model_when_model_not_in_pricing() {
    let fixture = fs_fixture!({
        "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"unknown-model-xyz","usage":{"input":100,"output":200}}}"#,
    });
    let file = fixture.path("sessions/project-a/agent_session-a.jsonl");

    let pricing = ccusage_core::PricingMap::default();
    let entries = read_session_file(
        &file,
        &jiff::tz::TimeZone::UTC,
        CostMode::Calculate,
        PriceTable::Calculate(&pricing),
    )
    .unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].missing_pricing_model.as_deref(),
        Some("unknown-model-xyz")
    );
}

#[test]
fn named_store_name_does_not_price_unknown_models() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"totally-unknown-model","usage":{"input":1000000,"output":1000000}}}"#,
    });
    let file = fixture.path("sessions/project-a/agent_session-a.jsonl");
    let mut pricing = ccusage_core::PricingMap::default();
    pricing.load_json(
        r#"{
                "o3": {
                    "input_cost_per_token": 0.000002,
                    "output_cost_per_token": 0.000008
                }
            }"#,
    );

    let entries = read_session_file_for_store(
        &file,
        &fixture.path("sessions"),
        &jiff::tz::TimeZone::UTC,
        CostMode::Calculate,
        PriceTable::Calculate(&pricing),
        "o3",
    )
    .unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].cost, 0.0);
    assert_eq!(
        entries[0].missing_pricing_model.as_deref(),
        Some("totally-unknown-model")
    );
}

#[test]
fn named_store_name_does_not_outmatch_real_model_pricing() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"o3","usage":{"input":1000,"output":2000}}}"#,
    });
    let file = fixture.path("sessions/project-a/agent_session-a.jsonl");
    let mut pricing = ccusage_core::PricingMap::default();
    pricing.load_json(
        r#"{
                "o3": {
                    "input_cost_per_token": 0.000002,
                    "output_cost_per_token": 0.000008
                },
                "deepseek-chat": {
                    "input_cost_per_token": 0.001,
                    "output_cost_per_token": 0.001
                }
            }"#,
    );

    let entries = read_session_file_for_store(
        &file,
        &fixture.path("sessions"),
        &jiff::tz::TimeZone::UTC,
        CostMode::Calculate,
        PriceTable::Calculate(&pricing),
        "deepseek-chat",
    )
    .unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].cost, 0.018000000000000002);
    assert_eq!(entries[0].missing_pricing_model, None);
}

#[test]
fn named_store_prefixed_pricing_override_wins_before_unprefixed_lookup() {
    ccusage_test_support::install_pricing_snapshots();
    let fixture = fs_fixture!({
        "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5.4","usage":{"input":1000,"output":2000}}}"#,
    });
    let file = fixture.path("sessions/project-a/agent_session-a.jsonl");
    let mut pricing = ccusage_core::PricingMap::default();
    pricing.load_json(
        r#"{
                "gpt-5.4": {
                    "input_cost_per_token": 0.001,
                    "output_cost_per_token": 0.001
                },
                "[omp] gpt-5.4": {
                    "input_cost_per_token": 0.000002,
                    "output_cost_per_token": 0.000008
                }
            }"#,
    );

    let entries = read_session_file_for_store(
        &file,
        &fixture.path("sessions"),
        &jiff::tz::TimeZone::UTC,
        CostMode::Calculate,
        PriceTable::Calculate(&pricing),
        "omp",
    )
    .unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].cost, 0.018000000000000002);
    assert_eq!(entries[0].missing_pricing_model, None);
}

#[test]
fn no_missing_pricing_model_in_display_mode() {
    let fixture = fs_fixture!({
        "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"unknown-model-xyz","usage":{"input":100,"output":200}}}"#,
    });
    let file = fixture.path("sessions/project-a/agent_session-a.jsonl");

    let entries = read_session_file(
        &file,
        &jiff::tz::TimeZone::UTC,
        CostMode::Display,
        PriceTable::Display,
    )
    .unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].missing_pricing_model, None);
}

#[test]
fn no_missing_pricing_model_when_auto_mode_has_display_cost() {
    let fixture = fs_fixture!({
        "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"unknown-model-xyz","usage":{"input":100,"output":200,"cost":{"total":0.05}}}}"#,
    });
    let file = fixture.path("sessions/project-a/agent_session-a.jsonl");

    let pricing = ccusage_core::PricingMap::default();
    let entries = read_session_file(
        &file,
        &jiff::tz::TimeZone::UTC,
        CostMode::Auto,
        PriceTable::Auto(&pricing),
    )
    .unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].missing_pricing_model, None);
}

#[test]
fn keeps_record_when_cost_is_not_an_object() {
    let fixture = fs_fixture!({
        "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":100,"output":200,"cost":0}}}"#,
    });
    let file = fixture.path("sessions/project-a/agent_session-a.jsonl");

    let entries = read_session_file(
        &file,
        &jiff::tz::TimeZone::UTC,
        CostMode::Display,
        PriceTable::Display,
    )
    .unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].usage.input_tokens, 100);
    assert_eq!(entries[0].usage.output_tokens, 200);
}

#[test]
fn prefixes_named_store_models_with_store_name() {
    let fixture = fs_fixture!({
        "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":100,"output":200}}}"#,
    });
    let file = fixture.path("sessions/project-a/agent_session-a.jsonl");

    let entries = read_session_file_for_store(
        &file,
        &fixture.path("sessions"),
        &jiff::tz::TimeZone::UTC,
        CostMode::Display,
        PriceTable::Display,
        "omp",
    )
    .unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].model.as_deref(), Some("[omp] gpt-5"));
    assert_eq!(entries[0].model.as_deref(), Some("[omp] gpt-5"));
}

#[test]
fn includes_named_store_in_dedupe_identity() {
    let fixture = fs_fixture!({
        "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"gpt-5","usage":{"input":100,"output":200}}}"#,
    });
    let file = fixture.path("sessions/project-a/agent_session-a.jsonl");

    let pi = read_session_file(
        &file,
        &jiff::tz::TimeZone::UTC,
        CostMode::Display,
        PriceTable::Display,
    )
    .unwrap()
    .pop()
    .unwrap();
    let omp = read_session_file_for_store(
        &file,
        &fixture.path("sessions"),
        &jiff::tz::TimeZone::UTC,
        CostMode::Display,
        PriceTable::Display,
        "omp",
    )
    .unwrap()
    .pop()
    .unwrap();

    assert_ne!(entry_id(&pi), entry_id(&omp));
}

#[test]
fn classifies_the_parent_session_of_a_header() {
    let header = |parent: &str| {
        let line =
            format!(r#"{{"type":"session","timestamp":"2026-01-01T00:00:00.000Z"{parent}}}"#);
        parse_session_header(line.as_bytes())
            .unwrap()
            .parent_session
    };

    assert_eq!(header(""), ParentSession::Absent);
    assert_eq!(header(r#","parentSession":null"#), ParentSession::Malformed);
    assert_eq!(header(r#","parentSession":5"#), ParentSession::Malformed);
    assert_eq!(header(r#","parentSession":"  ""#), ParentSession::Malformed);
    assert_eq!(
        header(r#","parentSession":" /a/b.jsonl ""#),
        ParentSession::Path(PathBuf::from("/a/b.jsonl"))
    );
}

#[test]
fn auto_recalculates_negative_but_keeps_zero_source_cost_for_default_and_named_stores() {
    let fixture = fs_fixture!({
        "sessions/project-a/agent_session-a.jsonl": [
            r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"test-model","usage":{"input":1000,"output":2000,"cost":{"total":-1.25}}}}"#,
            r#"{"type":"message","timestamp":"2026-01-02T00:01:00.000Z","message":{"role":"assistant","model":"test-model","usage":{"input":1000,"output":2000,"cost":{"total":0}}}}"#,
        ]
        .join("\n"),
    });
    let file = fixture.path("sessions/project-a/agent_session-a.jsonl");
    let pricing = pricing_for_cost_tests();
    let expected_cost = 0.018000000000000002;

    let default_entries = read_session_file(
        &file,
        &jiff::tz::TimeZone::UTC,
        CostMode::Auto,
        PriceTable::Auto(&pricing),
    )
    .unwrap();
    assert_eq!(default_entries.len(), 2);
    assert_eq!(default_entries[0].cost, expected_cost);
    assert_eq!(default_entries[0].recorded_cost, Some(-1.25));
    assert_eq!(default_entries[1].cost, 0.0);
    assert_eq!(default_entries[1].recorded_cost, Some(0.0));

    let named_entries = read_session_file_for_store(
        &file,
        &fixture.path("sessions"),
        &jiff::tz::TimeZone::UTC,
        CostMode::Auto,
        PriceTable::Auto(&pricing),
        "omp",
    )
    .unwrap();
    assert_eq!(named_entries.len(), 2);
    assert_eq!(named_entries[0].cost, expected_cost);
    assert_eq!(named_entries[0].recorded_cost, Some(-1.25));
    assert_eq!(named_entries[1].cost, 0.0);
    assert_eq!(named_entries[1].recorded_cost, Some(0.0));
}

#[test]
fn auto_recalculates_non_finite_source_cost_while_display_preserves_it() {
    let pricing = pricing_for_cost_tests();
    let usage = usage_for_cost_tests();
    let candidates = [("test-model", MatchKind::Fuzzy)];
    let cost = |stored_cost, mode| {
        price_entry(
            EntryPricing {
                granularity: UsageGranularity::Request,
                usage,
                reasoning_tokens: 0,
                reported_model: None,
                candidates: &candidates,
                stored_cost,
                timestamp: None,
            },
            PriceTable::for_map(mode, &pricing),
        )
        .cost
    };
    assert!(cost(Some(f64::NAN), CostMode::Display).is_nan());
    assert_eq!(
        cost(
            source_cost_for_mode(Some(f64::INFINITY), CostMode::Auto, false),
            CostMode::Auto,
        ),
        0.018000000000000002
    );
    assert_eq!(
        cost(Some(f64::NEG_INFINITY), CostMode::Calculate),
        0.018000000000000002
    );
}

#[test]
fn auto_reports_missing_pricing_for_invalid_source_cost() {
    let fixture = fs_fixture!({
        "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"unknown-model","usage":{"input":1000,"output":2000,"cost":{"total":-1.25}}}}"#,
    });
    let file = fixture.path("sessions/project-a/agent_session-a.jsonl");
    let pricing = ccusage_core::PricingMap::default();

    let default_entry = read_session_file(
        &file,
        &jiff::tz::TimeZone::UTC,
        CostMode::Auto,
        PriceTable::Auto(&pricing),
    )
    .unwrap()
    .pop()
    .unwrap();
    assert_eq!(default_entry.cost, 0.0);
    assert_eq!(
        default_entry.missing_pricing_model.as_deref(),
        Some("unknown-model")
    );

    let named_entry = read_session_file_for_store(
        &file,
        &fixture.path("sessions"),
        &jiff::tz::TimeZone::UTC,
        CostMode::Auto,
        PriceTable::Auto(&pricing),
        "omp",
    )
    .unwrap()
    .pop()
    .unwrap();
    assert_eq!(named_entry.cost, 0.0);
    assert_eq!(
        named_entry.missing_pricing_model.as_deref(),
        Some("unknown-model")
    );
}

#[test]
fn a_raw_id_override_beats_a_configured_alias_for_a_labeled_model() {
    ccusage_test_support::install_pricing_snapshots();
    let _aliases = ccusage_core::model_aliases::set_model_aliases_for_tests([(
        "pi-alias-source",
        "pi-alias-target",
    )]);
    let fixture = fs_fixture!({
        "sessions/project-a/agent_session-a.jsonl": r#"{"type":"message","timestamp":"2026-01-02T00:00:00.000Z","message":{"role":"assistant","model":"pi-alias-source","usage":{"input":1000,"output":2000}}}"#,
    });
    let mut pricing = ccusage_core::PricingMap::default();
    pricing.load_json(
        r#"{"pi-alias-target": {"input_cost_per_token": 0.1, "output_cost_per_token": 0.1}}"#,
    );
    let model = "pi-alias-source".to_owned();
    let rates = crate::cli::PricingOverride {
        input_cost_per_token: Some(0.001),
        output_cost_per_token: Some(0.002),
        ..Default::default()
    };
    let pricing = pricing.with_user_overrides([(&model, &rates)]);
    let entries = read_session_file(
        &fixture.path("sessions/project-a/agent_session-a.jsonl"),
        &jiff::tz::TimeZone::UTC,
        CostMode::Auto,
        PriceTable::Auto(&pricing),
    )
    .unwrap();
    assert_eq!(entries[0].cost, 5.0);
}
