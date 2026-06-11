use std::{collections::HashMap, sync::Arc};

use jiff::tz::TimeZone as JiffTimeZone;

use super::{
    parser::{CopilotUsageEntry, normalize_copilot_model, parse_session_state_file},
    paths::session_state_paths,
};
use crate::{
    LoadedEntry, Result, TokenUsageRaw, UsageEntry, UsageMessage, calculate_cost_for_usage,
    cli::CostMode, format_date_tz, missing_pricing_model_for_usage, parse_tz, read_files_parallel,
};

/// Pre-June-2026 premium-request USD rate; `requests.cost` already includes
/// Copilot's per-model multipliers.
const PREMIUM_REQUEST_COST_USD: f64 = 0.04;

/// Post-June-2026 AI Credit USD rate; `totalNanoAiu` is nano-AIU.
const AI_CREDIT_COST_USD: f64 = 0.01;

fn nano_aiu_to_credits(naiu: u64) -> f64 {
    if naiu <= 1_u64 << f64::MANTISSA_DIGITS {
        naiu as f64 / 1_000_000_000.0
    } else {
        (naiu / 1_000_000_000) as f64 + (naiu % 1_000_000_000) as f64 / 1_000_000_000.0
    }
}

pub(crate) fn load_entries(
    shared: &crate::cli::SharedArgs,
    pricing: &crate::PricingMap,
) -> Result<Vec<LoadedEntry>> {
    crate::adapter::copilot::warn_about_inert_legacy_env_vars_once();
    crate::progress::track_usage_load(
        crate::progress::UsageLoadAgent::Copilot,
        shared.json,
        || load_entries_inner(shared, pricing),
    )
}

fn load_entries_inner(
    shared: &crate::cli::SharedArgs,
    pricing: &crate::PricingMap,
) -> Result<Vec<LoadedEntry>> {
    let tz = parse_tz(shared.timezone.as_deref());

    let paths = session_state_paths()?;
    let loaded = read_files_parallel(&paths, shared.single_thread, parse_session_state_file);
    let mut entries: Vec<CopilotUsageEntry> = Vec::new();
    for (path, result) in paths.iter().zip(loaded) {
        match result {
            Ok(file_entries) => entries.extend(file_entries),
            Err(err) => crate::debug_log(
                shared,
                format!(
                    "Failed to read GitHub Copilot session-state file {}: {err}. \
                     Skipping this file; other sessions will still be reported.",
                    path.display(),
                ),
            ),
        }
    }

    // Filter before content-key dedup so out-of-range twins cannot win last-wins collapse.
    if shared.since.is_some() || shared.until.is_some() {
        entries.retain(|entry| {
            let date = format_date_tz(entry.timestamp, tz.as_ref()).replace('-', "");
            shared.since.as_ref().is_none_or(|since| &date >= since)
                && shared.until.as_ref().is_none_or(|until| &date <= until)
        });
    }

    let mut deduped: HashMap<String, CopilotUsageEntry> = HashMap::new();
    for entry in entries {
        deduped.insert(entry.dedup_key.clone(), entry);
    }

    let mut entries: Vec<LoadedEntry> = deduped
        .into_values()
        .map(|entry| usage_entry_to_loaded(entry, tz.as_ref(), shared.mode, pricing))
        .collect();
    entries.sort_by(|a, b| {
        a.timestamp.cmp(&b.timestamp).then_with(|| {
            a.data
                .message
                .id
                .as_deref()
                .cmp(&b.data.message.id.as_deref())
        })
    });
    Ok(entries)
}

fn usage_entry_to_loaded(
    entry: CopilotUsageEntry,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: &crate::PricingMap,
) -> LoadedEntry {
    let usage = TokenUsageRaw {
        input_tokens: entry.input_tokens,
        output_tokens: entry.output_tokens,
        cache_creation_input_tokens: entry.cache_creation_tokens,
        cache_read_input_tokens: entry.cache_read_tokens,
        speed: None,
        cache_creation: None,
    };
    let cost_usage = TokenUsageRaw {
        output_tokens: entry.output_tokens + entry.reasoning_output_tokens,
        cache_creation: None,
        ..usage
    };
    let model_for_data = if entry.model.is_empty() {
        // Synthetic aggregate-credit rows have no model; don't surface a blank model.
        None
    } else {
        Some(entry.model.clone())
    };
    let data = UsageEntry {
        session_id: Some(entry.session_id.clone()),
        timestamp: entry.timestamp_text,
        version: None,
        message: UsageMessage {
            usage,
            // Preserve raw model name in JSON / `modelsUsed` / table for
            // source fidelity. Normalization happens only at the pricing
            // call sites below (Copilot routing suffixes are stripped for
            // LiteLLM lookup but kept in user-visible output).
            model: model_for_data.clone(),
            id: Some(entry.dedup_key),
        },
        cost_usd: None,
        request_id: None,
        is_api_error_message: None,
        is_sidechain: None,
    };

    let credits = entry.nano_aiu.map(nano_aiu_to_credits);
    let credit_cost = credits.map(|c| c * AI_CREDIT_COST_USD);
    let has_aiu = credits.is_some();

    let premium_cost = entry
        .premium_request_cost
        .map(|requests| requests * PREMIUM_REQUEST_COST_USD);
    let has_premium_data = premium_cost.is_some();

    let (cost, missing_pricing_model) = if entry.model.is_empty() {
        let cost = match mode {
            CostMode::Auto | CostMode::Display => credit_cost.unwrap_or(0.0),
            CostMode::Calculate => 0.0,
        };
        (cost, None)
    } else {
        match mode {
            CostMode::Display => {
                let cost = credit_cost.or(premium_cost).unwrap_or(0.0);
                (cost, None)
            }
            CostMode::Auto if has_aiu => (
                credit_cost.expect("has_aiu guards credit_cost.is_some()"),
                None,
            ),
            CostMode::Auto if has_premium_data => (
                premium_cost.expect("has_premium_data guards premium_cost.is_some()"),
                None,
            ),
            CostMode::Auto | CostMode::Calculate => {
                let normalized = normalize_copilot_model(&entry.model);
                let cost = calculate_cost_for_usage(
                    Some(normalized.as_ref()),
                    cost_usage,
                    None,
                    CostMode::Calculate,
                    Some(pricing),
                );
                let missing_pricing_model = missing_pricing_model_for_usage(
                    Some(normalized.as_ref()),
                    cost_usage,
                    None,
                    CostMode::Calculate,
                    Some(pricing),
                );
                (cost, missing_pricing_model)
            }
        }
    };
    LoadedEntry {
        date: format_date_tz(entry.timestamp, tz),
        timestamp: entry.timestamp,
        project: Arc::from("copilot"),
        session_id: Arc::from(entry.session_id),
        project_path: Arc::from("GitHub Copilot CLI"),
        cost,
        extra_total_tokens: entry.reasoning_output_tokens,
        credits,
        message_count: None,
        // Reports show the raw model name (`claude-opus-4.7-1m-internal`),
        // not the normalized pricing key — matches PR #957's choice.
        model: model_for_data,
        data,
        usage_limit_reset_time: None,
        missing_pricing_model,
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use serde_json::json;

    use super::*;

    use ccusage_test_support::EnvVarGuard;

    use super::super::paths::COPILOT_CONFIG_DIR_ENV;
    use crate::cli::SharedArgs;

    fn env_scope(overrides: &[(&'static str, Option<&str>)]) -> EnvVarGuard {
        let overrides = overrides
            .iter()
            .map(|(key, value)| (*key, value.map(OsStr::new)))
            .collect::<Vec<_>>();
        EnvVarGuard::set_many(&overrides)
    }

    fn shared_args_default() -> SharedArgs {
        SharedArgs {
            json: true, // suppress progress UI under tests
            offline: true,
            mode: CostMode::Calculate,
            ..SharedArgs::default()
        }
    }

    fn shutdown_line(
        event_id: &str,
        ts: &str,
        model: &str,
        in_tokens: u64,
        out_tokens: u64,
    ) -> String {
        // `requests.cost` is OMITTED — not set to 0 — so the per-model row
        // genuinely has `premium_request_cost = None` once parsed. This
        // exercises the "no precomputed cost field" forward-compat shape;
        // `cost: Some(0.0)` would flatten to the same `$0` in Display via
        // `credit_cost.or(premium_cost).unwrap_or(0.0)`, but it routes
        // through the `has_premium_data == true` arm and misrepresents the
        // genuinely-absent narrative.
        json!({
            "type": "session.shutdown",
            "id": event_id,
            "timestamp": ts,
            "data": {"modelMetrics": {
                model: {
                    "usage": {
                        "inputTokens": in_tokens,
                        "outputTokens": out_tokens,
                        "cacheReadTokens": 0u64,
                        "cacheWriteTokens": 0u64,
                        "reasoningTokens": 0u64
                    },
                    "requests": {"count": 1}
                }
            }}
        })
        .to_string()
    }

    /// Sets `COPILOT_CONFIG_DIR` to a fixture root containing the
    /// `session-state/<uuid>/events.jsonl` files described by the test input.
    fn build_session_fixture(session_files: &[(&str, &str)]) -> ccusage_test_support::Fixture {
        let fixture = ccusage_test_support::Fixture::new();
        for (uuid, contents) in session_files {
            let _ = fixture.write_file(format!("session-state/{uuid}/events.jsonl"), *contents);
        }
        fixture
    }

    #[test]
    fn loader_dedup_keeps_distinct_sessions_with_colliding_event_ids() {
        // Defense-in-depth: the parser's token-bearing dedup key is
        // `shutdown:{session_id}:{event_id}:{model}` precisely so that if
        // a future Copilot CLI release ever scopes `event.id` per-session
        // (instead of the currently-observed global UUIDs), two rows from
        // different sessions with the same event id will NOT collapse in
        // the loader's HashMap. This fixture creates that exact scenario
        // and asserts both rows survive — guarding against silent
        // under-counting if the upstream invariant changes.
        let make_shutdown = || {
            format!(
                "{}\n",
                json!({
                    "type": "session.shutdown",
                    // Identical event.id across both sessions on purpose.
                    "id": "evt-collision",
                    "timestamp": "2026-05-02T15:27:09.013Z",
                    "data": {"modelMetrics": {
                        "claude-opus-4.7": {
                            "usage": {
                                "inputTokens": 1_000u64,
                                "outputTokens": 100u64,
                                "cacheReadTokens": 0u64,
                                "cacheWriteTokens": 0u64,
                                "reasoningTokens": 0u64
                            },
                            "requests": {"count": 1, "cost": 1}
                        }
                    }}
                })
            )
        };
        let fixture = build_session_fixture(&[
            ("session-A", &make_shutdown()),
            ("session-B", &make_shutdown()),
        ]);
        let _env = env_scope(&[(
            COPILOT_CONFIG_DIR_ENV,
            Some(fixture.root().to_str().unwrap()),
        )]);

        let pricing = crate::PricingMap::default();
        let entries = load_entries_inner(&shared_args_default(), &pricing).unwrap();
        // Without `session_id` in the dedup key, both rows would collapse
        // to a single HashMap entry and we'd lose half the usage. With
        // it, both survive.
        assert_eq!(entries.len(), 2, "got {entries:?}");
        let session_ids: std::collections::HashSet<_> =
            entries.iter().map(|e| e.session_id.as_ref()).collect();
        assert_eq!(
            session_ids,
            ["session-A", "session-B"].into_iter().collect()
        );
    }

    #[test]
    fn loader_sort_is_deterministic_for_equal_timestamps() {
        // Two rows sharing a millisecond timestamp must emit in the same
        // order every run. Without a secondary sort key, upstream
        // `HashMap::into_values` iteration order would make the tie
        // non-deterministic, breaking JSON-snapshot stability and
        // diff-friendliness of `--json` output.
        let same_ts = "2026-05-02T15:27:09.013Z";
        let make_row = |event_id: &str, model: &str| {
            json!({
                "type": "session.shutdown",
                "id": event_id,
                "timestamp": same_ts,
                "data": {"modelMetrics": {
                    model: {
                        "usage": {
                            "inputTokens": 100u64,
                            "outputTokens": 50u64,
                            "cacheReadTokens": 0u64,
                            "cacheWriteTokens": 0u64,
                            "reasoningTokens": 0u64
                        },
                        "requests": {"count": 1, "cost": 1}
                    }
                }}
            })
            .to_string()
        };
        let fixture = build_session_fixture(&[(
            "session-tied",
            &format!(
                "{}\n{}\n{}\n",
                make_row("evt-c", "claude-sonnet-4.5"),
                make_row("evt-a", "claude-opus-4.7"),
                make_row("evt-b", "claude-haiku-4.5"),
            ),
        )]);
        let _env = env_scope(&[(
            COPILOT_CONFIG_DIR_ENV,
            Some(fixture.root().to_str().unwrap()),
        )]);

        let pricing = crate::PricingMap::default();
        // Run the loader multiple times; every run must produce the same
        // ordering across equal-timestamp rows.
        let runs: Vec<Vec<String>> = (0..5)
            .map(|_| {
                load_entries_inner(&shared_args_default(), &pricing)
                    .unwrap()
                    .into_iter()
                    .map(|e| e.data.message.id.unwrap_or_default())
                    .collect()
            })
            .collect();
        for (i, run) in runs.iter().enumerate() {
            assert_eq!(
                run, &runs[0],
                "run {i} differs from run 0; loader sort is non-deterministic"
            );
        }
        // And concretely: the order must follow the secondary key
        // (`dedup_key`, which sorts lexicographically by event_id within
        // the same session). Token-bearing keys are
        // `shutdown:{session}:{event}:{model}`, so for session "session-tied"
        // the lexicographic order is evt-a < evt-b < evt-c regardless of
        // model name.
        let ids = &runs[0];
        let positions: Vec<_> = ids
            .iter()
            .map(|id| id.split(':').nth(2).unwrap_or("").to_string())
            .collect();
        assert_eq!(positions, vec!["evt-a", "evt-b", "evt-c"]);
    }

    #[test]
    fn pricing_resolves_via_normalization_for_internal_suffix_models() {
        let fixture = build_session_fixture(&[(
            "session-normalize",
            &format!(
                "{}\n",
                json!({
                    "type": "session.shutdown",
                    "id": "evt-norm",
                    "timestamp": "2026-05-02T15:27:09.013Z",
                    "data": {"modelMetrics": {
                        "claude-opus-4.7-1m-internal": {
                            "usage": {
                                "inputTokens": 1_000u64,
                                "outputTokens": 100u64,
                                "cacheReadTokens": 0u64,
                                "cacheWriteTokens": 0u64,
                                "reasoningTokens": 0u64
                            },
                            "requests": {"count": 1, "cost": 0}
                        }
                    }}
                })
            ),
        )]);
        let _env = env_scope(&[(
            COPILOT_CONFIG_DIR_ENV,
            Some(fixture.root().to_str().unwrap()),
        )]);

        // Minimal embedded pricing keyed by the normalized name.
        let mut pricing = crate::PricingMap::default();
        pricing.load_json(
            r#"{"claude-opus-4-7":{"input_cost_per_token":0.0000015,"output_cost_per_token":0.0000075}}"#,
        );

        let entries = load_entries_inner(&shared_args_default(), &pricing).unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        // Raw model preserved for display (normalization happens only at
        // the pricing call sites, not on the user-visible field).
        assert_eq!(
            entry.model.as_deref(),
            Some("claude-opus-4.7-1m-internal"),
            "raw model name must reach LoadedEntry.model"
        );
        // Cost > 0 — pricing lookup succeeded via normalize_copilot_model.
        assert!(
            entry.cost > 0.0,
            "expected non-zero cost, got {}",
            entry.cost
        );
        // missing_pricing_model must be None — otherwise the breakdown
        // branch of `UsageAccumulator::add_entry` (summary.rs) would flip
        // `breakdown.missing_pricing` and emit a contradictory warning.
        // Pins the invariant that BOTH the pricing call site AND the
        // missing-pricing-detection call site receive the normalized model
        // name (loader.rs's token-priced arm).
        assert!(
            entry.missing_pricing_model.is_none(),
            "expected missing_pricing_model None, got {:?}",
            entry.missing_pricing_model
        );
    }

    #[test]
    fn display_mode_returns_zero_for_session_state_without_aiu() {
        // Session-state shutdowns without `totalNanoAiu` produce zero cost in
        // display mode (no precomputed cost field exists) — same convention
        // as Gemini and Kimi adapters whose sources have no precomputed cost.
        // The `shutdown_line` helper genuinely omits `requests.cost`, so
        // `premium_request_cost == None` and Display reaches `0.0` via
        // `credit_cost.or(premium_cost).unwrap_or(0.0)` taking the
        // `None.or(None)` path — pinning the genuinely-absent shape rather
        // than a `Some(0.0)` masquerade.
        let fixture = build_session_fixture(&[(
            "session-display",
            &format!(
                "{}\n",
                shutdown_line(
                    "evt-1",
                    "2026-05-02T15:27:09.013Z",
                    "claude-sonnet-4",
                    500,
                    50
                )
            ),
        )]);
        let _env = env_scope(&[(
            COPILOT_CONFIG_DIR_ENV,
            Some(fixture.root().to_str().unwrap()),
        )]);

        let mut args = shared_args_default();
        args.mode = CostMode::Display;
        let pricing = crate::PricingMap::default();
        let entries = load_entries_inner(&args, &pricing).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].cost, 0.0,
            "Display mode must yield 0 for session-state data without AIU"
        );
        // The genuinely-absent `requests.cost` and `totalNanoAiu` are
        // pinned at the helper site (`shutdown_line` omits `cost`; this
        // test omits `totalNanoAiu`); the loaded view collapses them to
        // `cost = 0.0` via `credit_cost.or(premium_cost).unwrap_or(0.0)`
        // taking the `None.or(None)` path — not the `Some(0.0)` masquerade.
    }

    // ----- AIU billing under --mode auto -----

    fn shutdown_with_credits_line(
        event_id: &str,
        ts: &str,
        model: &str,
        in_tokens: u64,
        out_tokens: u64,
        naiu: u64,
        request_count: u64,
    ) -> String {
        json!({
            "type": "session.shutdown",
            "id": event_id,
            "timestamp": ts,
            "data": {"modelMetrics": {
                model: {
                    "usage": {
                        "inputTokens": in_tokens,
                        "outputTokens": out_tokens,
                        "cacheReadTokens": 0u64,
                        "cacheWriteTokens": 0u64,
                        "reasoningTokens": 0u64
                    },
                    "requests": {"count": request_count, "cost": 0},
                    "totalNanoAiu": naiu
                }
            }}
        })
        .to_string()
    }

    #[test]
    fn nano_aiu_conversion_preserves_fractional_precision() {
        assert_eq!(
            nano_aiu_to_credits(9_007_199_254_740_995),
            9_007_199.254_740_995
        );
        assert_eq!(nano_aiu_to_credits(1_527_255_393_325), 1_527.255_393_325);
    }

    #[test]
    fn auto_mode_bills_zero_token_credit_only_row_from_aiu() {
        // Real-data shape: zero tokens, zero requests, non-null per-model
        // totalNanoAiu = 154_481_000_000 ≈ $1.54481. The parser must keep
        // the row (skip rule already accommodates AIU presence) and Auto
        // must bill it from AIU.
        let fixture = build_session_fixture(&[(
            "session-credit-only",
            &format!(
                "{}\n",
                shutdown_with_credits_line(
                    "evt-only",
                    "2026-05-20T16:01:29.481Z",
                    "claude-opus-4.7-1m-internal",
                    0,
                    0,
                    154_481_000_000,
                    0,
                )
            ),
        )]);
        let _env = env_scope(&[(
            COPILOT_CONFIG_DIR_ENV,
            Some(fixture.root().to_str().unwrap()),
        )]);

        let mut args = shared_args_default();
        args.mode = CostMode::Auto;
        let pricing = crate::PricingMap::default();
        let entries = load_entries_inner(&args, &pricing).unwrap();
        assert_eq!(entries.len(), 1);
        assert!((entries[0].cost - 1.54481).abs() < 1e-9);
        assert_eq!(entries[0].credits, Some(154.481));
    }

    #[test]
    fn auto_mode_dedupes_duplicate_credit_only_rows_within_same_session() {
        // Two distinct shutdown events with identical per-model totalNanoAiu
        // (same shape as real-data `1135875b-…` pair). The content-based
        // dedup key + loader HashMap must collapse them to one entry, so the
        // total cost is `1.54481`, not `2 × 1.54481`.
        let dup = shutdown_with_credits_line(
            "evt-first",
            "2026-05-20T16:01:29.481Z",
            "claude-opus-4.7-1m-internal",
            0,
            0,
            154_481_000_000,
            0,
        );
        let dup2 = shutdown_with_credits_line(
            "evt-second",
            "2026-05-20T18:08:30.785Z",
            "claude-opus-4.7-1m-internal",
            0,
            0,
            154_481_000_000,
            0,
        );
        let fixture = build_session_fixture(&[("session-dup", &format!("{dup}\n{dup2}\n"))]);
        let _env = env_scope(&[(
            COPILOT_CONFIG_DIR_ENV,
            Some(fixture.root().to_str().unwrap()),
        )]);

        let mut args = shared_args_default();
        args.mode = CostMode::Auto;
        let pricing = crate::PricingMap::default();
        let entries = load_entries_inner(&args, &pricing).unwrap();
        assert_eq!(
            entries.len(),
            1,
            "expected the duplicate pair to collapse into one entry"
        );
        assert!((entries[0].cost - 1.54481).abs() < 1e-9);
    }

    #[test]
    fn credit_only_rows_with_identical_naiu_but_distinct_event_ids_are_treated_as_duplicates() {
        // Locks the deliberate dedup-key design choice documented in
        // `adapter/copilot/README.md` under "Resumed sessions and dedup-key
        // design": credit-only rows use `credit-shutdown:{session}:{model}:{naiu}`
        // and OMIT `event_id`, because real-data ships them in paired
        // snapshots with distinct event ids but identical naiu (the
        // 1135875b-… / 76dc438a-… pattern; ~$2.65 of credits would be
        // double-billed by event-id-based keying in the local 200-file
        // corpus).
        //
        // This test makes the intentional under-count behavior explicit:
        // if a future Copilot CLI ever ships TWO genuinely distinct
        // credit-only charges in the same session that happen to share
        // model + naiu, the loader will collapse them to ONE entry. This
        // is the deliberate trade-off — the alternative (keying on
        // event_id) would silently double-bill every real-data paired
        // snapshot, which is the failure mode the design optimises
        // against. The collapse surfaces in `credits` JSON output (the
        // soft signal) rather than producing a silent wrong number.
        //
        // Companion to (token-bearing) `loader_dedup_keeps_distinct_sessions_with_colliding_event_ids`
        // and (parser-side) `dedupes_identical_credit_only_rows_within_same_session`.
        // This test pins the SAME shape but at the LOADER HashMap-collapse
        // boundary and asserts the resulting cost (Auto mode bills
        // 154.481 credits × $0.01 = $1.54481 — ONCE, not twice).
        let first = shutdown_with_credits_line(
            "evt-genuine-charge-a",
            "2026-05-20T10:00:00.000Z",
            "claude-opus-4.7-1m-internal",
            0,
            0,
            154_481_000_000,
            0,
        );
        let second = shutdown_with_credits_line(
            "evt-genuine-charge-b",
            "2026-05-21T10:00:00.000Z", // Note: a full day later — distinct
            "claude-opus-4.7-1m-internal",
            0,
            0,
            154_481_000_000, // ...but coincidentally identical naiu.
            0,
        );
        let fixture = build_session_fixture(&[(
            "session-distinct-collisions",
            &format!("{first}\n{second}\n"),
        )]);
        let _env = env_scope(&[(
            COPILOT_CONFIG_DIR_ENV,
            Some(fixture.root().to_str().unwrap()),
        )]);

        let mut args = shared_args_default();
        args.mode = CostMode::Auto;
        let pricing = crate::PricingMap::default();
        let entries = load_entries_inner(&args, &pricing).unwrap();
        assert_eq!(
            entries.len(),
            1,
            "two credit-only rows with same model + naiu in same session \
             MUST collapse to one entry — this is the deliberate dedup \
             choice optimising for the real-data paired-snapshot shape; \
             see README 'Resumed sessions and dedup-key design'"
        );
        assert!(
            (entries[0].cost - 1.54481).abs() < 1e-9,
            "expected single collapsed bill of $1.54481 (not $3.08962 if \
             event_id had been keyed), got {}",
            entries[0].cost
        );
    }

    #[test]
    fn date_filter_runs_before_credit_only_dedup_collapse_to_preserve_in_range_row() {
        // Regression: the credit-only dedup key
        // (`credit-shutdown:{session}:{model}:{naiu}`) deliberately omits
        // `event_id` so paired snapshots collapse (see the locking test
        // above and README "Resumed sessions and dedup-key design"). The
        // collapse is `HashMap::insert` last-wins by parse order. Before
        // this fix, the `--since` / `--until` filter ran AFTER the
        // dedup HashMap, so when two credit-only rows with the same
        // `(session, model, naiu)` were parsed in the order
        // [in-range, out-of-range], the HashMap kept the out-of-range
        // (later-inserted) row and the subsequent date filter dropped it,
        // silently losing the in-range row's credits entirely. The fix
        // applies the filter to the `CopilotUsageEntry` vector BEFORE the
        // dedup collapse, so each surviving entry already satisfies the
        // date range and the collapse can never pick an out-of-range
        // winner. This test pins the worst-case parse-order
        // [in-range, out-of-range] inside a single events.jsonl file with
        // `--until` cutting between the two timestamps; asserts the
        // in-range row's credits survive.
        let in_range = shutdown_with_credits_line(
            "evt-in-range",
            "2026-05-15T10:00:00.000Z",
            "claude-opus-4.7-1m-internal",
            0,
            0,
            154_481_000_000,
            0,
        );
        let out_of_range = shutdown_with_credits_line(
            "evt-out-of-range",
            "2026-05-20T10:00:00.000Z",
            "claude-opus-4.7-1m-internal",
            0,
            0,
            154_481_000_000, // same naiu — would collide on credit-shutdown key
            0,
        );
        // Parse order matters: in-range FIRST so the HashMap collapse
        // would otherwise overwrite it with the out-of-range row.
        let fixture = build_session_fixture(&[(
            "session-date-filter-vs-credit-dedup",
            &format!("{in_range}\n{out_of_range}\n"),
        )]);
        let _env = env_scope(&[(
            COPILOT_CONFIG_DIR_ENV,
            Some(fixture.root().to_str().unwrap()),
        )]);

        let mut args = shared_args_default();
        args.mode = CostMode::Auto;
        // Cut the date range so only the 2026-05-15 row qualifies.
        // `SharedArgs::since` / `::until` are normalized to `YYYYMMDD`
        // (no dashes) by the CLI parser; mirror that here.
        args.since = Some("20260101".to_string());
        args.until = Some("20260516".to_string());
        // Use UTC so the test's RFC3339 timestamps map predictably to
        // calendar dates regardless of the host timezone.
        args.timezone = Some("UTC".to_string());
        let pricing = crate::PricingMap::default();
        let entries = load_entries_inner(&args, &pricing).unwrap();
        assert_eq!(
            entries.len(),
            1,
            "the in-range credit-only row MUST survive even though it \
             shares the content-based dedup key with an out-of-range row \
             parsed later in the same file. Pre-fix this returned 0 \
             entries (HashMap kept the later out-of-range row, then the \
             date filter dropped it). See README 'Resumed sessions and \
             dedup-key design'."
        );
        assert!(
            entries[0].date.starts_with("2026-05-15"),
            "expected the in-range row (2026-05-15) to survive, got date {}",
            entries[0].date,
        );
        assert!(
            (entries[0].cost - 1.54481).abs() < 1e-9,
            "expected the in-range row's $1.54481 to survive intact, \
             got {}",
            entries[0].cost,
        );
    }

    #[test]
    fn calculate_mode_uses_token_pricing_even_with_naiu_present() {
        // Calculate / api always uses LiteLLM token pricing — the
        // credit channel stays informational in JSON, but cost comes from
        // the api-equivalent lookup.
        let fixture = build_session_fixture(&[(
            "session-calc-with-naiu",
            &format!(
                "{}\n",
                shutdown_with_credits_line(
                    "evt-calc",
                    "2026-05-02T15:27:09.013Z",
                    "claude-opus-4.7-1m-internal",
                    1_000,
                    100,
                    2_500_000_000,
                    1,
                )
            ),
        )]);
        let _env = env_scope(&[(
            COPILOT_CONFIG_DIR_ENV,
            Some(fixture.root().to_str().unwrap()),
        )]);

        let mut pricing = crate::PricingMap::default();
        pricing.load_json(
            r#"{"claude-opus-4-7":{"input_cost_per_token":0.0000015,"output_cost_per_token":0.0000075}}"#,
        );
        let args = shared_args_default(); // mode = Calculate
        let entries = load_entries_inner(&args, &pricing).unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        // Cost from tokens, not credits.
        assert!(
            entry.cost > 0.0 && (entry.cost - 0.025).abs() > 1e-6,
            "expected api-equivalent cost, got {} (credits would be 0.025)",
            entry.cost
        );
        // Credits channel still populated for the JSON / report layer.
        assert_eq!(entry.credits, Some(2.5));
    }

    #[test]
    fn calculate_mode_bills_zero_for_credit_only_zero_token_row() {
        // A credit-only row (zero tokens, zero requests, non-zero AIU)
        // under `--mode calculate` / `--mode api` should bill $0: there
        // are no tokens to price, AIU is deliberately not consulted in
        // calculate mode, and the row should NOT fall back to surfacing
        // the credit-converted cost (that's `auto`/`display`'s job).
        // The credits channel must still be exposed in JSON so callers
        // who want the raw AIU count can still see it regardless of
        // mode.
        let fixture = build_session_fixture(&[(
            "session-credit-only-calc",
            &format!(
                "{}\n",
                shutdown_with_credits_line(
                    "evt-credit-only",
                    "2026-05-20T16:01:29.481Z",
                    "claude-opus-4.7-1m-internal",
                    0,
                    0,
                    2_500_000_000, // 2.5 credits — would bill $0.025 in auto
                    0,
                )
            ),
        )]);
        let _env = env_scope(&[(
            COPILOT_CONFIG_DIR_ENV,
            Some(fixture.root().to_str().unwrap()),
        )]);

        let mut pricing = crate::PricingMap::default();
        pricing.load_json(
            r#"{"claude-opus-4-7":{"input_cost_per_token":0.0000015,"output_cost_per_token":0.0000075}}"#,
        );
        let args = shared_args_default(); // mode = Calculate
        let entries = load_entries_inner(&args, &pricing).unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        // Zero tokens × any pricing = $0.
        assert_eq!(
            entry.cost, 0.0,
            "calculate mode must bill $0 for a credit-only zero-token row; \
             got {} (auto would bill 0.025)",
            entry.cost
        );
        // Credits stay surfaced regardless of mode.
        assert_eq!(entry.credits, Some(2.5));
    }

    #[test]
    fn auto_mode_prefers_credits_when_aiu_is_present() {
        // Auto is the user-facing default. For Copilot, the "true bill" is
        // AI Credits whenever the source carries them — Auto must surface
        // that, not the api-equivalent hypothetical.
        let fixture = build_session_fixture(&[(
            "session-auto-aiu",
            &format!(
                "{}\n",
                shutdown_with_credits_line(
                    "evt-auto-aiu",
                    "2026-05-02T15:27:09.013Z",
                    "claude-opus-4.7-1m-internal",
                    1_000,
                    100,
                    2_500_000_000, // 2.5 credits = $0.025
                    1,
                )
            ),
        )]);
        let _env = env_scope(&[(
            COPILOT_CONFIG_DIR_ENV,
            Some(fixture.root().to_str().unwrap()),
        )]);

        let mut args = shared_args_default();
        args.mode = CostMode::Auto;
        // Token pricing is present but must be ignored when AIU is present.
        let mut pricing = crate::PricingMap::default();
        pricing.load_json(
            r#"{"claude-opus-4-7":{"input_cost_per_token":0.0000015,"output_cost_per_token":0.0000075}}"#,
        );
        let entries = load_entries_inner(&args, &pricing).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(
            (entries[0].cost - 0.025).abs() < 1e-9,
            "Auto must prefer credits ($0.025), got ${}",
            entries[0].cost
        );
    }

    #[test]
    fn auto_mode_uses_premium_request_billing_when_no_aiu_but_premium_requests_present() {
        // Pre-cutover sessions (CLI < 1.0.40) have no `totalNanoAiu` but
        // do ship `requests.cost`. Auto must surface the pre-cutover true
        // bill: `requests.cost × $0.04`. This is closer to what GitHub
        // actually charged the user than the LiteLLM-equivalent fallback.
        let fixture = build_session_fixture(&[(
            "session-pre-cutover",
            &format!(
                "{}\n",
                json!({
                    "type": "session.shutdown",
                    "id": "evt-pre",
                    "timestamp": "2026-04-15T09:52:27.352Z",
                    "data": {"modelMetrics": {
                        // Pre-cutover billable model: 3 requests at the
                        // Opus 4.6 multiplier (1×) = 3 premium requests.
                        "claude-opus-4.6": {
                            "usage": {
                                "inputTokens": 1_000u64,
                                "outputTokens": 100u64,
                                "cacheReadTokens": 0u64,
                                "cacheWriteTokens": 0u64,
                                "reasoningTokens": 0u64
                            },
                            "requests": {"count": 3, "cost": 3}
                        }
                    }}
                })
            ),
        )]);
        let _env = env_scope(&[(
            COPILOT_CONFIG_DIR_ENV,
            Some(fixture.root().to_str().unwrap()),
        )]);

        let mut args = shared_args_default();
        args.mode = CostMode::Auto;
        let mut pricing = crate::PricingMap::default();
        pricing.load_json(
            r#"{"claude-opus-4-6":{"input_cost_per_token":0.0000015,"output_cost_per_token":0.0000075}}"#,
        );
        let entries = load_entries_inner(&args, &pricing).unwrap();
        assert_eq!(entries.len(), 1);
        // 3 premium requests × $0.04 = $0.12.
        assert!(
            (entries[0].cost - 0.12).abs() < 1e-9,
            "expected pre-cutover premium billing ($0.12), got {}",
            entries[0].cost
        );
    }

    #[test]
    fn auto_mode_falls_through_to_token_pricing_when_cost_field_is_absent() {
        // Forward-compat: if a future Copilot CLI ever stops emitting
        // `requests.cost` while still reporting tokens and no AIU, Auto
        // must fall through to LiteLLM token pricing rather than report
        // a false `$0` bill from the premium-request branch. This is the
        // loader-level counterpart to the parser regression test
        // `absent_requests_cost_preserves_none_for_token_priced_fallback`.
        let fixture = build_session_fixture(&[(
            "session-absent-cost",
            &format!(
                "{}\n",
                json!({
                    "type": "session.shutdown",
                    "id": "evt-absent-cost",
                    "timestamp": "2026-05-02T15:27:09.013Z",
                    "data": {"modelMetrics": {
                        "claude-opus-4.7": {
                            "usage": {
                                "inputTokens": 1_000u64,
                                "outputTokens": 100u64,
                                "cacheReadTokens": 0u64,
                                "cacheWriteTokens": 0u64,
                                "reasoningTokens": 0u64
                            },
                            // No `cost` field; no `totalNanoAiu`. Auto must
                            // not bill $0 — it must compute from tokens.
                            "requests": {"count": 1}
                        }
                    }}
                })
            ),
        )]);
        let _env = env_scope(&[(
            COPILOT_CONFIG_DIR_ENV,
            Some(fixture.root().to_str().unwrap()),
        )]);

        let mut args = shared_args_default();
        args.mode = CostMode::Auto;
        let mut pricing = crate::PricingMap::default();
        pricing.load_json(
            r#"{"claude-opus-4-7":{"input_cost_per_token":0.0000015,"output_cost_per_token":0.0000075}}"#,
        );
        let entries = load_entries_inner(&args, &pricing).unwrap();
        assert_eq!(entries.len(), 1);
        // 1000 input × $0.0000015 + 100 output × $0.0000075 = $0.0015 + $0.00075 = $0.00225
        assert!(
            (entries[0].cost - 0.00225).abs() < 1e-9,
            "Auto with absent `cost` field must fall through to token pricing ($0.00225), got {}",
            entries[0].cost
        );
        assert_eq!(entries[0].missing_pricing_model, None);
    }

    #[test]
    fn auto_mode_handles_fractional_premium_request_cost() {
        // Real-data invariant (~2% of rows): `requests.cost` can be
        // fractional — Opus 4.7 multiplies 1 request by 7.5. Auto must
        // bill `7.5 × $0.04 = $0.30` end-to-end without losing
        // fidelity. This is a loader-level regression for the same bug
        // the parser test pins (`accepts_fractional_premium_request_cost`).
        let fixture = build_session_fixture(&[(
            "session-frac-cost",
            &format!(
                "{}\n",
                json!({
                    "type": "session.shutdown",
                    "id": "evt-frac",
                    "timestamp": "2026-04-15T09:52:27.352Z",
                    "data": {"modelMetrics": {
                        "claude-opus-4.7": {
                            "usage": {
                                "inputTokens": 1_000u64,
                                "outputTokens": 100u64,
                                "cacheReadTokens": 0u64,
                                "cacheWriteTokens": 0u64,
                                "reasoningTokens": 0u64
                            },
                            "requests": {"count": 1, "cost": 7.5}
                        }
                    }}
                })
            ),
        )]);
        let _env = env_scope(&[(
            COPILOT_CONFIG_DIR_ENV,
            Some(fixture.root().to_str().unwrap()),
        )]);

        let mut args = shared_args_default();
        args.mode = CostMode::Auto;
        let pricing = crate::PricingMap::default();
        let entries = load_entries_inner(&args, &pricing).unwrap();
        assert_eq!(entries.len(), 1);
        // 7.5 premium requests × $0.04 = $0.30 — without f64 parsing this
        // would either round to $0.04 (cost=1) or drop the row entirely.
        assert!(
            (entries[0].cost - 0.30).abs() < 1e-9,
            "expected fractional premium billing ($0.30), got {}",
            entries[0].cost
        );
    }

    #[test]
    fn auto_mode_some_zero_aiu_short_circuits_to_zero_over_nonzero_premium() {
        // Pin the documented `has_aiu = credits.is_some()` semantics: a
        // post-cutover row that explicitly reports `totalNanoAiu: 0`
        // (genuine "AIU charged, value zero") alongside a non-zero
        // `requests.cost` MUST bill `$0.00` via the AIU arm — NOT
        // `cost × $0.04` via the premium arm. The `requests.cost` field is
        // an inert legacy artifact on post-cutover sessions; treating
        // `Some(0)` AIU as "AIU absent, fall through" would invent cost
        // the user never owed. This is the one edge where the
        // "presence, not value" dispatch framing diverges from intuition.
        let fixture = build_session_fixture(&[(
            "session-zero-aiu-with-cost",
            &format!(
                "{}\n",
                json!({
                    "type": "session.shutdown",
                    "id": "evt-zero-aiu-with-cost",
                    "timestamp": "2026-06-15T10:00:00.000Z",
                    "data": {"modelMetrics": {
                        "claude-opus-4.7": {
                            "usage": {
                                "inputTokens": 1_000u64,
                                "outputTokens": 100u64,
                                "cacheReadTokens": 0u64,
                                "cacheWriteTokens": 0u64,
                                "reasoningTokens": 0u64
                            },
                            "requests": {"count": 1, "cost": 4.0},
                            "totalNanoAiu": 0u64
                        }
                    }}
                })
            ),
        )]);
        let _env = env_scope(&[(
            COPILOT_CONFIG_DIR_ENV,
            Some(fixture.root().to_str().unwrap()),
        )]);

        let mut args = shared_args_default();
        args.mode = CostMode::Auto;
        let pricing = crate::PricingMap::default();
        let entries = load_entries_inner(&args, &pricing).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].credits, Some(0.0));
        // Would be `4.0 × $0.04 = $0.16` if `Some(0)` AIU were treated as
        // "AIU absent" and Auto fell through to the premium arm. The AIU
        // arm wins because `credits.is_some()`.
        assert_eq!(
            entries[0].cost, 0.0,
            "Some(0) AIU must short-circuit Auto to $0 over a non-zero premium cost",
        );
    }

    #[test]
    fn auto_mode_uses_zero_for_pre_cutover_free_tier_rows() {
        // Pre-cutover free-tier models (sonnet, haiku) report
        // `requests.count > 0` but `requests.cost == 0` because they
        // didn't burn premium-request allotment. Under the premium
        // billing model the user genuinely paid $0 for these rows, so
        // Auto must NOT fall through to token-priced fallback — that
        // would invent cost the user never owed.
        let fixture = build_session_fixture(&[(
            "session-free-tier",
            &format!(
                "{}\n",
                json!({
                    "type": "session.shutdown",
                    "id": "evt-free",
                    "timestamp": "2026-04-15T09:52:27.352Z",
                    "data": {"modelMetrics": {
                        "claude-sonnet-4.5": {
                            "usage": {
                                "inputTokens": 50_000u64,
                                "outputTokens": 5_000u64,
                                "cacheReadTokens": 10_000u64,
                                "cacheWriteTokens": 0u64,
                                "reasoningTokens": 0u64
                            },
                            "requests": {"count": 18, "cost": 0}
                        }
                    }}
                })
            ),
        )]);
        let _env = env_scope(&[(
            COPILOT_CONFIG_DIR_ENV,
            Some(fixture.root().to_str().unwrap()),
        )]);

        let mut args = shared_args_default();
        args.mode = CostMode::Auto;
        let mut pricing = crate::PricingMap::default();
        pricing.load_json(
            r#"{"claude-sonnet-4-5":{"input_cost_per_token":0.000003,"output_cost_per_token":0.000015}}"#,
        );
        let entries = load_entries_inner(&args, &pricing).unwrap();
        assert_eq!(entries.len(), 1);
        // Premium-request cost is 0 → free-tier under the pre-cutover plan
        // → bill $0 even though tokens were used.
        assert_eq!(
            entries[0].cost, 0.0,
            "free-tier pre-cutover rows must bill $0 under premium model"
        );
    }

    #[test]
    fn display_mode_returns_credits_when_aiu_is_present() {
        // Realigned semantics: Display = "show what the source precomputed".
        // For Copilot, that's AIU when present (the same number Credits
        // mode shows) — analogous to how Claude Display returns Anthropic's
        // `costUSD`.
        let fixture = build_session_fixture(&[(
            "session-display",
            &format!(
                "{}\n",
                shutdown_with_credits_line(
                    "evt-display",
                    "2026-05-02T15:27:09.013Z",
                    "claude-opus-4.7-1m-internal",
                    1_000,
                    100,
                    2_500_000_000,
                    1,
                )
            ),
        )]);
        let _env = env_scope(&[(
            COPILOT_CONFIG_DIR_ENV,
            Some(fixture.root().to_str().unwrap()),
        )]);

        let mut args = shared_args_default();
        args.mode = CostMode::Display;
        let pricing = crate::PricingMap::default();
        let entries = load_entries_inner(&args, &pricing).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(
            (entries[0].cost - 0.025).abs() < 1e-9,
            "Display must surface AIU-derived cost, got ${}",
            entries[0].cost
        );
    }

    #[test]
    fn display_mode_surfaces_premium_request_cost_when_aiu_is_absent() {
        // Display walks the true-bill ladder: AIU first, then premium
        // requests, then $0. For pre-cutover data without AIU, that means
        // surfacing the premium-request cost the source already shipped
        // (no token-priced fallback in Display mode).
        let fixture = build_session_fixture(&[(
            "session-pre-display",
            &format!(
                "{}\n",
                json!({
                    "type": "session.shutdown",
                    "id": "evt-pre-display",
                    "timestamp": "2026-04-15T09:52:27.352Z",
                    "data": {"modelMetrics": {
                        "claude-opus-4.7": {
                            "usage": {
                                "inputTokens": 1_000u64,
                                "outputTokens": 100u64,
                                "cacheReadTokens": 0u64,
                                "cacheWriteTokens": 0u64,
                                "reasoningTokens": 0u64
                            },
                            // 1 request × 7.5 multiplier = 7.5 premium
                            "requests": {"count": 1, "cost": 7.5}
                        }
                    }}
                })
            ),
        )]);
        let _env = env_scope(&[(
            COPILOT_CONFIG_DIR_ENV,
            Some(fixture.root().to_str().unwrap()),
        )]);

        let mut args = shared_args_default();
        args.mode = CostMode::Display;
        let pricing = crate::PricingMap::default();
        let entries = load_entries_inner(&args, &pricing).unwrap();
        assert_eq!(entries.len(), 1);
        // 7.5 × $0.04 = $0.30.
        assert!(
            (entries[0].cost - 0.30).abs() < 1e-9,
            "Display must surface premium-request cost when AIU absent, got ${}",
            entries[0].cost
        );
    }

    #[test]
    fn auto_mode_aggregate_only_synthetic_entry_bills_from_credits() {
        // Forward-compat: when only data.totalNanoAiu is present (not in
        // current local real data), Auto still bills the aggregate.
        let fixture = build_session_fixture(&[(
            "session-agg",
            &format!(
                "{}\n",
                json!({
                    "type": "session.shutdown",
                    "id": "evt-agg",
                    "timestamp": "2026-05-20T16:01:29.481Z",
                    "data": {
                        "modelMetrics": {},
                        "totalNanoAiu": 5_000_000_000u64
                    }
                })
            ),
        )]);
        let _env = env_scope(&[(
            COPILOT_CONFIG_DIR_ENV,
            Some(fixture.root().to_str().unwrap()),
        )]);

        let mut args = shared_args_default();
        args.mode = CostMode::Auto;
        let pricing = crate::PricingMap::default();
        let entries = load_entries_inner(&args, &pricing).unwrap();
        assert_eq!(entries.len(), 1);
        assert!((entries[0].cost - 0.05).abs() < 1e-9);
        // Synthetic entry — no model assigned.
        assert!(entries[0].model.is_none());
    }

    #[test]
    #[cfg(unix)]
    fn one_unreadable_session_file_does_not_abort_whole_report() {
        // Per-file error isolation: one bad/locked/corrupted session-state
        // file MUST NOT abort the whole report. Other ccusage adapters
        // treat parse errors as per-file skips; the same semantic must
        // hold here. A locked file (Windows, antivirus scanning), a
        // partial write from a still-running CLI, or one file with
        // permission errors shouldn't make `ccusage copilot daily` print
        // zero — it should report whatever the OTHER sessions show.
        //
        // Hermetic: build a fixture with TWO session directories. The
        // first is a real, readable shutdown event ($0.04 under Auto).
        // The second is a real `events.jsonl` file whose POSIX mode is
        // `0o000` — `is_file()` returns true (so path discovery
        // enumerates it) but `fs::File::open` returns
        // `PermissionDenied`. This routes through the new
        // `match parse_session_state_file(...) { Err(_) => skip }` arm
        // in `load_entries_inner` rather than being filtered upstream.
        //
        // CAUTION: an earlier version of this test created a *directory*
        // at the events.jsonl position. That fixture was vacuous: a
        // directory fails `is_file()`, so path discovery silently drops
        // it before the loader ever opens anything — the test would
        // pass even when the production fix was reverted. EISDIR alone
        // does NOT reach the loader. The chmod-0o000 trigger here does,
        // and the test fails deterministically against the buggy
        // pre-fix code (`entries.extend(parse_session_state_file(&path)?)`).
        use std::os::unix::fs::PermissionsExt;
        let fixture = ccusage_test_support::Fixture::new();
        let _ = fixture.write_file(
            "session-state/good-uuid/events.jsonl",
            format!(
                "{}\n",
                json!({
                    "type": "session.shutdown",
                    "id": "evt-good",
                    "timestamp": "2026-04-15T09:52:27.352Z",
                    "data": {"modelMetrics": {
                        "claude-opus-4.7": {
                            "usage": {
                                "inputTokens": 1_000u64,
                                "outputTokens": 100u64,
                                "cacheReadTokens": 0u64,
                                "cacheWriteTokens": 0u64,
                                "reasoningTokens": 0u64
                            },
                            "requests": {"count": 1, "cost": 1}
                        }
                    }}
                })
            ),
        );
        // Write events.jsonl as a real file (so `is_file()` lets path
        // discovery enumerate it), then strip all permissions so
        // `File::open` returns `PermissionDenied` — the real io error
        // class the loader's `match` arm must isolate.
        let bad_file = fixture.write_file(
            "session-state/bad-uuid/events.jsonl",
            "{\"this content should never be read; mode is 0o000\"}\n",
        );
        std::fs::set_permissions(&bad_file, std::fs::Permissions::from_mode(0o000))
            .expect("chmod 0o000 on bad-file fixture");

        let _env = env_scope(&[(
            COPILOT_CONFIG_DIR_ENV,
            Some(fixture.root().to_str().unwrap()),
        )]);

        let mut args = shared_args_default();
        args.mode = CostMode::Auto;
        let pricing = crate::PricingMap::default();
        // Critical assertion: the call must succeed (Ok), not propagate
        // the bad-file error. Before the fix this would return Err.
        let result = load_entries_inner(&args, &pricing);

        // Restore permissions BEFORE any assertion can panic, so the
        // tempdir cleanup at fixture drop can remove the file. Without
        // this, a failing assertion leaves a 0o000 file that prevents
        // TempDir from rmdir-ing the parent.
        let _ = std::fs::set_permissions(&bad_file, std::fs::Permissions::from_mode(0o644));

        let entries = result.expect("loader must succeed despite the bad file");
        // The good file's one entry must survive — the bad file's read
        // error was isolated and debug-logged when requested.
        assert_eq!(
            entries.len(),
            1,
            "good-file entry must survive bad-file read error; got {entries:?}"
        );
        // 1 premium request × $0.04 = $0.04
        assert!(
            (entries[0].cost - 0.04).abs() < 1e-9,
            "expected $0.04 from the good file, got {}",
            entries[0].cost
        );
    }
}
