use ccusage_core::cost::PriceTable;
use ccusage_core::{
    cost::{EntryPricing, UsageGranularity, price_entry},
    pricing::MatchKind,
};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
};

use jiff::tz::TimeZone as JiffTimeZone;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    LoadedEntry, TimestampMs, TokenUsageRaw, cli::CostMode, date_at, stored_cost_for_mode,
    total_usage_tokens,
};

use ccusage_adapter_common::{
    jsonl,
    units::{UnitError, read_json},
};
use ccusage_core::Diagnostics;
use ccusage_core::fast::LinePrefilter;

#[derive(Debug, Default, Deserialize)]
struct GrokUpdateLine {
    #[serde(default)]
    timestamp: Option<Value>,
    #[serde(default)]
    params: Option<GrokParams>,
}

#[derive(Debug, Default, Deserialize)]
struct GrokParams {
    #[serde(
        default,
        rename = "sessionId",
        deserialize_with = "jsonl::non_empty_string"
    )]
    session_id: Option<String>,
    #[serde(default)]
    update: Option<GrokUpdate>,
    #[serde(default, rename = "_meta")]
    meta: Option<GrokMeta>,
}

#[derive(Debug, Default, Deserialize)]
struct GrokUpdate {
    #[serde(
        default,
        rename = "sessionUpdate",
        deserialize_with = "jsonl::non_empty_string"
    )]
    session_update: Option<String>,
    #[serde(default)]
    usage: Option<GrokUsage>,
}

#[derive(Debug, Default, Deserialize)]
struct GrokMeta {
    #[serde(
        default,
        rename = "eventId",
        deserialize_with = "jsonl::non_empty_string"
    )]
    event_id: Option<String>,
    #[serde(default, rename = "agentTimestampMs")]
    agent_timestamp_ms: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrokUsage {
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    input_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    output_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cached_read_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cache_creation_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    reasoning_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cost_usd_ticks: u64,
    #[serde(default)]
    model_usage: Option<HashMap<String, GrokModelUsage>>,
}

#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrokModelUsage {
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    input_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    output_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cached_read_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cache_creation_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    reasoning_tokens: u64,
    #[serde(default, deserialize_with = "jsonl::lenient_u64")]
    cost_usd_ticks: u64,
}

#[derive(Debug, Default, Deserialize)]
struct GrokSummary {
    #[serde(default)]
    info: Option<GrokSummaryInfo>,
    #[serde(
        default,
        rename = "git_root_dir",
        deserialize_with = "jsonl::non_empty_string"
    )]
    git_root_dir: Option<String>,
    #[serde(
        default,
        rename = "current_model_id",
        deserialize_with = "jsonl::non_empty_string"
    )]
    current_model_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct GrokSummaryInfo {
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    id: Option<String>,
    #[serde(default, deserialize_with = "jsonl::non_empty_string")]
    cwd: Option<String>,
}

#[derive(Debug, Clone)]
struct SessionMeta {
    session_id: String,
    project_path: String,
    default_model: Option<String>,
}

// NOTE(2026-08-10, Grok CLI 1.0.0, #1593): Grok bills each API request separately and sums them in a `turn_completed` row, where `costUsdTicks` uses units of 1e-10 USD. The recorded ticks preserve per-request long-context tiering.
const COST_USD_TICKS_PER_USD: f64 = 1e10;

fn cost_usd_from_ticks(ticks: u64) -> Option<f64> {
    (ticks > 0).then(|| ticks as f64 / COST_USD_TICKS_PER_USD)
}

fn split_tokens(input: u64, cached: u64) -> (u64, u64) {
    let cache = cached.min(input);
    let uncached = input.saturating_sub(cache);
    (uncached, cache)
}

// NOTE(2026-11-20, Grok CLI 1.0.45, #2257): Grok Build's `PromptUsageModel` sums the full prompt into `inputTokens`, folding in both `cachedReadTokens` and `cacheCreationTokens`; only its Anthropic Messages path reports cache writes, and the xAI Responses path always reports zero. Subtracting both from the input total leaves the uncached prompt tokens.
fn split_input_tokens(input: u64, cached_read: u64, cache_creation: u64) -> (u64, u64, u64) {
    let (uncached, cache_read) = split_tokens(input, cached_read);
    let cache_creation = cache_creation.min(uncached);
    (uncached - cache_creation, cache_read, cache_creation)
}

fn pricing_candidates(raw_model: &str) -> Vec<(String, MatchKind)> {
    let mut candidates: Vec<(String, MatchKind)> = Vec::new();
    let mut push = |value: String, kind| {
        if !candidates.iter().any(|(existing, _)| existing == &value) {
            candidates.push((value, kind));
        }
    };

    let stripped = raw_model.trim();
    if stripped.is_empty() {
        return candidates;
    }

    let normalized = stripped.trim_suffix("-build");
    let raw_kind = if normalized == stripped {
        MatchKind::Fuzzy
    } else {
        MatchKind::Exact
    };

    push(stripped.to_string(), raw_kind);
    push(format!("xai/{stripped}"), raw_kind);
    push(format!("x-ai/{stripped}"), raw_kind);
    push(normalized.to_string(), MatchKind::Fuzzy);
    push(format!("xai/{normalized}"), MatchKind::Fuzzy);
    push(format!("x-ai/{normalized}"), MatchKind::Fuzzy);
    candidates
}

pub(super) fn parse_session_files_with_diagnostics(
    updates: &Path,
    tz: &JiffTimeZone,
    mode: CostMode,
    pricing: PriceTable<'_>,
    diagnostics: &Diagnostics,
) -> std::result::Result<Vec<(GrokDedupeKey, LoadedEntry)>, UnitError> {
    let meta = load_session_meta(updates, diagnostics);
    let prefilter = LinePrefilter::all(&[br#""turn_completed""#]);
    let mut entries = Vec::new();
    let mut seen = HashSet::new();

    jsonl::read_records::<GrokUpdateLine>(
        updates,
        Some(&prefilter),
        diagnostics,
        is_grok_usage,
        |number, line, diagnostics| {
            let Some(params) = line.params.as_ref() else {
                return;
            };
            let Some(update) = params.update.as_ref() else {
                return;
            };
            if update.session_update.as_deref() != Some("turn_completed") {
                return;
            }
            let Some(usage) = update.usage.as_ref() else {
                return;
            };

            let model_rows = model_usage_rows(usage, meta.default_model.as_deref())
                .into_iter()
                .filter_map(|(raw_model, model_usage)| {
                    let (uncached, cache, cache_creation) = split_input_tokens(
                        model_usage.input_tokens,
                        model_usage.cached_read_tokens,
                        model_usage.cache_creation_tokens,
                    );
                    let usage_raw = TokenUsageRaw {
                        input_tokens: uncached,
                        output_tokens: model_usage.output_tokens,
                        cache_creation_input_tokens: cache_creation,
                        cache_read_input_tokens: cache,
                        speed: None,
                        cache_creation: None,
                    };
                    let reasoning_tokens = model_usage.reasoning_tokens;
                    (total_usage_tokens(usage_raw).saturating_add(reasoning_tokens) > 0)
                        .then_some((raw_model, model_usage, usage_raw, reasoning_tokens))
                })
                .collect::<Vec<_>>();
            if model_rows.is_empty() {
                return;
            }

            let event_id = params.meta.as_ref().and_then(|meta| meta.event_id.clone());
            let Some(timestamp_ms) = resolve_timestamp_ms(&line, params.meta.as_ref()) else {
                diagnostics.record(ccusage_core::Skip::record(
                    updates,
                    number,
                    ccusage_core::SkipKind::InvalidTimestamp,
                    "Grok usage has no valid timestamp",
                ));
                return;
            };
            let session_id = params
                .session_id
                .clone()
                .unwrap_or_else(|| meta.session_id.clone());

            for (raw_model, model_usage, usage_raw, reasoning_tokens) in model_rows {
                let dedupe_key = GrokDedupeKey::new(
                    event_id.as_deref(),
                    &session_id,
                    timestamp_ms,
                    &raw_model,
                    usage_raw,
                    reasoning_tokens,
                );
                if !seen.insert(dedupe_key.clone()) {
                    continue;
                }

                let display_model = raw_model.clone();
                let cost_usd = cost_usd_from_ticks(model_usage.cost_usd_ticks);
                let (cost, missing_pricing_model, estimated_pricing) = price_grok_usage(
                    &raw_model,
                    usage_raw,
                    cost_usd,
                    Some(timestamp_ms),
                    mode,
                    pricing,
                );

                entries.push((
                    dedupe_key,
                    LoadedEntry {
                        usage: usage_raw,
                        message_id: event_id.clone(),
                        version: None,
                        recorded_cost: cost_usd,
                        timestamp: timestamp_ms,
                        date: date_at(timestamp_ms, tz),
                        project: Arc::from("grok"),
                        session_id: Arc::from(session_id.clone()),
                        project_path: Arc::from(meta.project_path.as_str()),
                        cost,
                        credits: None,
                        model: Some(display_model),
                        message_count: None,
                        usage_limit_reset_time: None,
                        missing_pricing_model,
                        estimated_pricing,
                        // NOTE(2026-08-10, Grok CLI 1.0.0, #1593): Grok's recorded total equals input plus output, with reasoning already included in output. Adding reasoning again would inflate the total.
                        extra_total_tokens: 0,
                        stored_cost: stored_cost_for_mode(cost_usd, mode),
                    },
                ));
            }
        },
    )?;

    Ok(entries)
}

fn is_grok_usage(value: &Value) -> bool {
    let Some(update) = value.get("params").and_then(|params| params.get("update")) else {
        return false;
    };
    if update.get("sessionUpdate").and_then(Value::as_str) != Some("turn_completed") {
        return false;
    }
    let Some(usage) = update.get("usage").and_then(Value::as_object) else {
        return false;
    };
    [
        "modelUsage",
        "inputTokens",
        "outputTokens",
        "cachedReadTokens",
        "cacheCreationTokens",
    ]
    .iter()
    .any(|field| usage.contains_key(*field))
}

fn model_usage_rows(
    usage: &GrokUsage,
    default_model: Option<&str>,
) -> Vec<(String, GrokModelUsage)> {
    if let Some(map) = usage.model_usage.as_ref()
        && !map.is_empty()
    {
        let mut rows: Vec<_> = map
            .iter()
            .map(|(model, usage)| (model.clone(), *usage))
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        return rows;
    }

    let model = default_model
        .map(str::to_string)
        .unwrap_or_else(|| "unknown".to_string());
    vec![(
        model,
        GrokModelUsage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cached_read_tokens: usage.cached_read_tokens,
            cache_creation_tokens: usage.cache_creation_tokens,
            reasoning_tokens: usage.reasoning_tokens,
            cost_usd_ticks: usage.cost_usd_ticks,
        },
    )]
}

fn load_session_meta(updates: &Path, diagnostics: &Diagnostics) -> SessionMeta {
    let session_dir_name = updates
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("unknown")
        .to_string();
    let project_from_path = updates
        .parent()
        .and_then(|session| session.parent())
        .and_then(|project| project.file_name())
        .and_then(|name| name.to_str())
        .map(decode_project_path_segment)
        .unwrap_or_else(|| "unknown".to_string());

    let mut session_id = session_dir_name;
    let mut project_path = project_from_path;
    let mut default_model = None;

    let summary_path = updates.with_file_name("summary.json");
    let summary = summary_path.is_file().then_some(&summary_path).and_then(|summary_path| {
        read_json::<GrokSummary>(summary_path)
            .inspect_err(|error| {
                diagnostics.debug_note(
                    summary_path,
                    format!(
                        "{}: {}: {error}; session id, project and default model come from the session path",
                        diagnostics.source(),
                        summary_path.display()
                    ),
                );
            })
            .ok()
    });
    if let Some(summary) = summary {
        if let Some(id) = summary.info.as_ref().and_then(|info| info.id.clone()) {
            session_id = id;
        }
        if let Some(cwd) = summary
            .info
            .as_ref()
            .and_then(|info| info.cwd.clone())
            .or(summary.git_root_dir)
        {
            project_path = cwd;
        }
        default_model = summary.current_model_id;
    }

    SessionMeta {
        session_id,
        project_path,
        default_model,
    }
}

// NOTE(2026-08-10, Grok CLI 1.0.0, #1593): Grok's metadata `agentTimestampMs` is Unix milliseconds and the envelope `timestamp` is Unix seconds. Convert each field in its own unit before using it for day attribution.
fn resolve_timestamp_ms(line: &GrokUpdateLine, meta: Option<&GrokMeta>) -> Option<TimestampMs> {
    meta.and_then(|meta| meta.agent_timestamp_ms.as_ref())
        .and_then(value_as_i64)
        .filter(|millis| *millis > 0)
        .and_then(TimestampMs::from_instant_millis)
        .or_else(|| {
            line.timestamp
                .as_ref()
                .and_then(value_as_i64)
                .filter(|seconds| *seconds > 0)
                .and_then(TimestampMs::from_unix_seconds)
                .and_then(|timestamp| TimestampMs::from_instant_millis(timestamp.as_millis()))
        })
}

fn value_as_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|n| i64::try_from(n).ok()))
        .or_else(|| value.as_f64().map(|n| n as i64))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum GrokDedupeKey {
    Event(GrokEventKey),
    Content {
        session_id: String,
        timestamp: TimestampMs,
        model: String,
        tokens: [u64; 5],
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct GrokEventKey {
    event_id: String,
    model: String,
}

impl GrokDedupeKey {
    fn new(
        event_id: Option<&str>,
        session_id: &str,
        timestamp: TimestampMs,
        model: &str,
        usage: TokenUsageRaw,
        reasoning: u64,
    ) -> Self {
        if let Some(event_id) = event_id {
            return Self::Event(GrokEventKey {
                event_id: event_id.to_string(),
                model: model.to_string(),
            });
        }
        Self::Content {
            session_id: session_id.to_string(),
            timestamp,
            model: model.to_string(),
            tokens: [
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_read_input_tokens,
                usage.cache_creation_input_tokens,
                reasoning,
            ],
        }
    }
}

fn price_grok_usage(
    raw_model: &str,
    usage: TokenUsageRaw,
    cost_usd: Option<f64>,
    timestamp: Option<TimestampMs>,
    mode: CostMode,
    pricing: PriceTable<'_>,
) -> (f64, Option<String>, bool) {
    let names = pricing_candidates(raw_model);
    let candidates: Vec<_> = names
        .iter()
        .map(|(name, kind)| (name.as_str(), *kind))
        .collect();
    let priced = price_entry(
        EntryPricing {
            granularity: UsageGranularity::Aggregate,
            usage,
            reasoning_tokens: 0,
            residual_tokens: 0,
            reported_model: Some(raw_model),
            candidates: &candidates,
            stored_cost: cost_usd,
            timestamp,
        },
        pricing,
    );
    (
        priced.cost,
        (mode != CostMode::Display
            && !(mode == CostMode::Auto && cost_usd.is_some())
            && total_usage_tokens(usage) != 0
            && priced.matched.is_none())
        .then(|| crate::model_aliases::resolve_model_name(raw_model).into_owned()),
        priced.estimated,
    )
}

// NOTE(2026-08-10, Grok CLI 1.0.0, #1593): Grok uses URL-encoded working directories as session parent names. Decode the name to recover the original project path.
// NOTE(2026-11-19, Grok CLI 1.0.0, #2250): Names that do not decode to UTF-8 keep their encoded form. Two invalid byte paths would otherwise collapse into one project label.
fn decode_project_path_segment(value: &str) -> String {
    match percent_encoding::percent_decode_str(value).decode_utf8() {
        Ok(decoded) => decoded.into_owned(),
        Err(_) => value.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn preserves_missing_pricing_for_blank_model_keys_in_each_cost_mode() {
        let pricing = ccusage_core::PricingMap::default();
        let usage = TokenUsageRaw {
            input_tokens: 1,
            ..Default::default()
        };
        for model in ["", "  "] {
            assert_eq!(
                price_grok_usage(
                    model,
                    usage,
                    None,
                    None,
                    CostMode::Calculate,
                    PriceTable::Calculate(&pricing)
                ),
                (0.0, Some(model.to_string()), false)
            );
            assert_eq!(
                price_grok_usage(
                    model,
                    usage,
                    None,
                    None,
                    CostMode::Auto,
                    PriceTable::Auto(&pricing)
                ),
                (0.0, Some(model.to_string()), false)
            );
            assert_eq!(
                price_grok_usage(
                    model,
                    usage,
                    Some(1.0),
                    None,
                    CostMode::Auto,
                    PriceTable::Auto(&pricing)
                ),
                (1.0, None, false)
            );
            assert_eq!(
                price_grok_usage(
                    model,
                    usage,
                    None,
                    None,
                    CostMode::Display,
                    PriceTable::Display
                ),
                (0.0, None, false)
            );
            assert_eq!(
                price_grok_usage(
                    model,
                    TokenUsageRaw::default(),
                    None,
                    None,
                    CostMode::Calculate,
                    PriceTable::Calculate(&pricing)
                ),
                (0.0, None, false)
            );
        }
    }

    use super::*;
    use ccusage_test_support::fs_fixture;

    fn sample_turn_completed_line() -> String {
        r#"{"timestamp":1750000000,"method":"_x.ai/session/update","params":{"sessionId":"sess-1","update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":100,"outputTokens":20,"cachedReadTokens":40,"reasoningTokens":10,"totalTokens":120,"modelUsage":{"grok-4.5-build":{"inputTokens":100,"outputTokens":20,"cachedReadTokens":40,"reasoningTokens":10,"totalTokens":120}}}},"_meta":{"eventId":"evt-1"}}}"#.to_string()
    }

    #[derive(Clone, Copy)]
    struct TurnTokens {
        input: u64,
        output: u64,
        cache: u64,
        reasoning: u64,
    }

    fn turn_line(
        event_id: &str,
        model: &str,
        tokens: TurnTokens,
        envelope_seconds: i64,
        agent_ms: Option<i64>,
    ) -> String {
        let mut meta = serde_json::json!({ "eventId": event_id });
        if let Some(ms) = agent_ms {
            meta["agentTimestampMs"] = serde_json::json!(ms);
        }
        let TurnTokens {
            input,
            output,
            cache,
            reasoning,
        } = tokens;
        serde_json::json!({
            "timestamp": envelope_seconds,
            "params": {
                "sessionId": "sess-1",
                "update": {
                    "sessionUpdate": "turn_completed",
                    "usage": {
                        "inputTokens": input,
                        "outputTokens": output,
                        "cachedReadTokens": cache,
                        "reasoningTokens": reasoning,
                        "modelUsage": {
                            model: {
                                "inputTokens": input,
                                "outputTokens": output,
                                "cachedReadTokens": cache,
                                "reasoningTokens": reasoning,
                            }
                        }
                    }
                },
                "_meta": meta
            }
        })
        .to_string()
    }

    fn parse_lines(content: &str, summary: Option<&str>) -> Vec<LoadedEntry> {
        let fixture = if let Some(summary) = summary {
            fs_fixture!({
                "sessions/proj/sess-1/updates.jsonl": content,
                "sessions/proj/sess-1/summary.json": summary,
            })
        } else {
            fs_fixture!({
                "sessions/proj/sess-1/updates.jsonl": content,
            })
        };
        let files = fixture.path("sessions/proj/sess-1/updates.jsonl");
        let _root = fixture.root().to_path_buf();
        parse_session_files_with_diagnostics(
            &files,
            &jiff::tz::TimeZone::UTC,
            CostMode::Display,
            PriceTable::Display,
            &Diagnostics::new("grok"),
        )
        .map(|entries| {
            entries
                .into_iter()
                .map(|(_, entry)| entry)
                .collect::<Vec<_>>()
        })
        .unwrap()
    }

    #[test]
    fn splits_uncached_input_from_cache() {
        assert_eq!(split_tokens(100, 40), (60, 40));
        assert_eq!(split_tokens(10, 40), (0, 10));
        assert_eq!(split_tokens(0, 5), (0, 0));
    }

    #[test]
    fn pricing_candidates_strip_build_and_add_xai() {
        let exact = |name: &str| (name.to_string(), MatchKind::Exact);
        let fuzzy = |name: &str| (name.to_string(), MatchKind::Fuzzy);
        assert_eq!(
            pricing_candidates("grok-4.5-build"),
            vec![
                exact("grok-4.5-build"),
                exact("xai/grok-4.5-build"),
                exact("x-ai/grok-4.5-build"),
                fuzzy("grok-4.5"),
                fuzzy("xai/grok-4.5"),
                fuzzy("x-ai/grok-4.5"),
            ]
        );
        assert_eq!(
            pricing_candidates("grok-4.5"),
            vec![
                fuzzy("grok-4.5"),
                fuzzy("xai/grok-4.5"),
                fuzzy("x-ai/grok-4.5")
            ]
        );
    }

    #[test]
    fn a_build_spelling_matches_fuzzily_only_through_the_stripped_id() {
        ccusage_test_support::install_pricing_snapshots();
        let mut pricing = ccusage_core::PricingMap::default();
        pricing.load_json(
            r#"{
            "azure_ai/grok-unreleased-9.9": {"litellm_provider":"azure_ai","input_cost_per_token":1e-6,"output_cost_per_token":1e-6},
            "grok-unreleased-9.9-build-preview": {"litellm_provider":"host","input_cost_per_token":9e-6,"output_cost_per_token":9e-6}
        }"#,
        );
        let usage = TokenUsageRaw {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..TokenUsageRaw::default()
        };
        let cost = |model| {
            price_grok_usage(
                model,
                usage,
                None,
                None,
                CostMode::Calculate,
                PriceTable::Calculate(&pricing),
            )
            .0
        };
        assert_eq!(cost("grok-unreleased-9.9"), 2.0);
        assert_eq!(cost("grok-unreleased-9.9-build"), 2.0);
    }

    #[test]
    fn pricing_candidates_are_empty_for_a_blank_model() {
        assert!(pricing_candidates("   ").is_empty());
    }

    #[test]
    fn exact_raw_model_pricing_override_beats_normalized_fallback() {
        ccusage_test_support::install_pricing_snapshots();
        let model = "grok-4.3-build".to_string();
        let pricing_override = crate::cli::PricingOverride {
            input_cost_per_token: Some(1.0),
            output_cost_per_token: Some(2.0),
            ..crate::cli::PricingOverride::default()
        };
        let pricing = ccusage_core::PricingMap::load_with_overrides(
            true,
            false,
            [(&model, &pricing_override)],
        );
        let usage = TokenUsageRaw {
            input_tokens: 1,
            output_tokens: 1,
            ..TokenUsageRaw::default()
        };

        assert_eq!(
            price_grok_usage(
                &model,
                usage,
                None,
                None,
                CostMode::Calculate,
                PriceTable::Calculate(&pricing)
            )
            .0,
            3.0
        );
    }

    #[test]
    fn prices_via_the_stripped_candidate_when_the_build_form_is_missing() {
        ccusage_test_support::install_pricing_snapshots();
        let pricing_override = crate::cli::PricingOverride {
            input_cost_per_token: Some(0.001),
            output_cost_per_token: Some(0.002),
            ..crate::cli::PricingOverride::default()
        };
        let key = "xai/grok-unreleased-9.9".to_string();
        let pricing =
            ccusage_core::PricingMap::load_with_overrides(true, false, [(&key, &pricing_override)]);
        for unpriced in [
            "grok-unreleased-9.9-build",
            "xai/grok-unreleased-9.9-build",
            "x-ai/grok-unreleased-9.9-build",
        ] {
            assert!(pricing.find(unpriced).is_none(), "{unpriced} was priced");
        }
        let usage = TokenUsageRaw {
            input_tokens: 10,
            output_tokens: 5,
            cache_read_input_tokens: 0,
            ..TokenUsageRaw::default()
        };

        assert_eq!(
            price_grok_usage(
                "grok-unreleased-9.9-build",
                usage,
                None,
                None,
                CostMode::Calculate,
                PriceTable::Calculate(&pricing)
            )
            .0,
            0.02
        );
    }

    #[test]
    fn reports_an_unpriced_model_as_missing_pricing_instead_of_dropping_it() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "sessions/proj/sess-1/updates.jsonl": turn_line(
                "evt-miss",
                "grok-never-priced-build",
                TurnTokens { input: 93, output: 3, cache: 0, reasoning: 0 },
                1_750_000_000,
                None,
            ),
        });
        let files = fixture.path("sessions/proj/sess-1/updates.jsonl");
        let pricing = ccusage_core::PricingMap::load_embedded();
        let entries = parse_session_files_with_diagnostics(
            &files,
            &jiff::tz::TimeZone::UTC,
            CostMode::Calculate,
            PriceTable::Calculate(&pricing),
            &Diagnostics::new("grok"),
        )
        .map(|entries| {
            entries
                .into_iter()
                .map(|(_, entry)| entry)
                .collect::<Vec<_>>()
        })
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("grok-never-priced-build"));
        assert_eq!(entries[0].usage.input_tokens, 93);
        assert_eq!(entries[0].cost, 0.0);
        assert_eq!(
            entries[0].missing_pricing_model.as_deref(),
            Some("grok-never-priced-build")
        );
    }

    #[test]
    fn saturated_usage_with_reasoning_keeps_the_turn() {
        ccusage_test_support::install_pricing_snapshots();
        let line = sample_turn_completed_line()
            .replace(
                "\"inputTokens\":100",
                "\"inputTokens\":18446744073709551615",
            )
            .replace("\"reasoningTokens\":10", "\"reasoningTokens\":1");
        let fixture = fs_fixture!({
            "sessions/proj/sess-1/updates.jsonl": line,
        });
        let files = fixture.path("sessions/proj/sess-1/updates.jsonl");
        let entries = parse_session_files_with_diagnostics(
            &files,
            &jiff::tz::TimeZone::UTC,
            CostMode::Display,
            PriceTable::Display,
            &Diagnostics::new("grok"),
        )
        .map(|entries| {
            entries
                .into_iter()
                .map(|(_, entry)| entry)
                .collect::<Vec<_>>()
        })
        .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].usage.input_tokens, u64::MAX - 40);
    }

    #[test]
    fn turn_completed_model_usage_maps_tokens_without_double_count() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "sessions/proj/sess-1/updates.jsonl": sample_turn_completed_line(),
            "sessions/proj/sess-1/summary.json": r#"{"info":{"id":"sess-1","cwd":"D:\\work\\proj"},"current_model_id":"grok-4.5"}"#,
        });
        let files = fixture.path("sessions/proj/sess-1/updates.jsonl");
        let pricing = ccusage_core::PricingMap::load_embedded();
        let entries = parse_session_files_with_diagnostics(
            &files,
            &jiff::tz::TimeZone::UTC,
            CostMode::Calculate,
            PriceTable::Calculate(&pricing),
            &Diagnostics::new("grok"),
        )
        .map(|entries| {
            entries
                .into_iter()
                .map(|(_, entry)| entry)
                .collect::<Vec<_>>()
        })
        .unwrap();

        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.model.as_deref(), Some("grok-4.5-build"));
        assert_eq!(entry.usage.input_tokens, 60);
        assert_eq!(entry.usage.cache_read_input_tokens, 40);
        assert_eq!(entry.usage.output_tokens, 20);
        assert_eq!(entry.usage.cache_creation_input_tokens, 0);
        assert_eq!(entry.extra_total_tokens, 0);
        assert!(entry.recorded_cost.is_none());
        assert_eq!(entry.session_id.as_ref(), "sess-1");
        assert_eq!(entry.project_path.as_ref(), "D:\\work\\proj");
    }

    #[test]
    fn does_not_add_reasoning_tokens_to_the_total() {
        ccusage_test_support::install_pricing_snapshots();
        let tokens = TurnTokens {
            input: 0,
            output: 0,
            cache: 0,
            reasoning: 42,
        };
        let line = turn_line("evt-r", "grok-4.5-build", tokens, 1_750_000_000, None);
        let entries = parse_lines(&line, None);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].usage.input_tokens, 0);
        assert_eq!(entries[0].usage.output_tokens, 0);
        assert_eq!(entries[0].extra_total_tokens, 0);
    }

    #[test]
    fn falls_back_to_top_level_usage_when_model_usage_is_absent() {
        ccusage_test_support::install_pricing_snapshots();
        let line = r#"{"timestamp":1750000000,"params":{"sessionId":"sess-top","update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":50,"outputTokens":5,"cachedReadTokens":10,"reasoningTokens":2}},"_meta":{"eventId":"evt-top"}}}"#;
        let summary =
            r#"{"info":{"id":"sess-top","cwd":"/tmp/proj"},"current_model_id":"grok-4.5-build"}"#;
        let entries = parse_lines(line, Some(summary));

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("grok-4.5-build"));
        assert_eq!(entries[0].usage.input_tokens, 40);
        assert_eq!(entries[0].usage.cache_read_input_tokens, 10);
        assert_eq!(entries[0].usage.output_tokens, 5);
        assert_eq!(entries[0].extra_total_tokens, 0);
    }

    #[test]
    fn names_top_level_usage_unknown_when_summary_has_no_default_model() {
        ccusage_test_support::install_pricing_snapshots();
        let line = r#"{"timestamp":1750000000,"params":{"sessionId":"sess-u","update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":10,"outputTokens":1,"cachedReadTokens":0,"reasoningTokens":0}},"_meta":{"eventId":"evt-u"}}}"#;
        let entries = parse_lines(line, None);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model.as_deref(), Some("unknown"));
    }

    #[test]
    fn prefers_agent_timestamp_ms_over_envelope_seconds() {
        ccusage_test_support::install_pricing_snapshots();
        let tokens = TurnTokens {
            input: 10,
            output: 1,
            cache: 0,
            reasoning: 0,
        };
        let line = turn_line(
            "evt-ts",
            "grok-4.5-build",
            tokens,
            1_750_000_000,
            Some(1_785_328_986_355),
        );
        let entries = parse_lines(&line, None);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].timestamp.as_millis(), 1_785_328_986_355);
        assert_eq!(entries[0].date.to_string(), "2026-07-29");
    }

    #[test]
    fn converts_envelope_unix_seconds_to_millis_when_agent_ms_is_absent() {
        ccusage_test_support::install_pricing_snapshots();
        let tokens = TurnTokens {
            input: 10,
            output: 1,
            cache: 0,
            reasoning: 0,
        };
        let line = turn_line("evt-sec", "grok-4.5-build", tokens, 1_750_000_000, None);
        let entries = parse_lines(&line, None);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].timestamp.as_millis(), 1_750_000_000_000);
        assert_eq!(entries[0].date.to_string(), "2025-06-15");
    }

    #[test]
    fn diagnoses_usage_without_a_timestamp() {
        let fixture = fs_fixture!({
            "sessions/project/session/updates.jsonl": r#"{"params":{"update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":1,"outputTokens":1}}}}"#,
        });
        let files = fixture.path("sessions/project/session/updates.jsonl");
        let diagnostics = Diagnostics::new("grok");
        let entries = parse_session_files_with_diagnostics(
            &files,
            &JiffTimeZone::UTC,
            CostMode::Calculate,
            PriceTable::Calculate(&ccusage_core::PricingMap::default()),
            &diagnostics,
        )
        .unwrap();
        assert!(entries.is_empty());
        assert_eq!(
            diagnostics.skipped(ccusage_core::SkipKind::InvalidTimestamp),
            1
        );
    }

    #[test]
    fn undated_turn_without_countable_usage_is_not_diagnosed() {
        let fixture = fs_fixture!({
            "sessions/project/session/updates.jsonl": r#"{"params":{"update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":0,"outputTokens":0}}}}"#,
        });
        let files = fixture.path("sessions/project/session/updates.jsonl");
        let diagnostics = Diagnostics::new("grok");
        let entries = parse_session_files_with_diagnostics(
            &files,
            &JiffTimeZone::UTC,
            CostMode::Calculate,
            PriceTable::Calculate(&ccusage_core::PricingMap::default()),
            &diagnostics,
        )
        .unwrap();
        assert!(entries.is_empty());
        assert_eq!(
            diagnostics.skipped(ccusage_core::SkipKind::InvalidTimestamp),
            0
        );
    }

    #[test]
    fn summary_cwd_overrides_path_derived_project_and_fills_session_when_line_omits_it() {
        ccusage_test_support::install_pricing_snapshots();
        let line = r#"{"timestamp":1750000000,"params":{"update":{"sessionUpdate":"turn_completed","usage":{"modelUsage":{"grok-4.5-build":{"inputTokens":10,"outputTokens":1,"cachedReadTokens":0,"reasoningTokens":0}}}},"_meta":{"eventId":"evt-meta"}}}"#;
        let summary = r#"{"info":{"id":"canonical-session","cwd":"D:\\canonical\\cwd"},"git_root_dir":"should-not-win"}"#;
        let entries = parse_lines(line, Some(summary));

        assert_eq!(entries[0].session_id.as_ref(), "canonical-session");
        assert_eq!(entries[0].project_path.as_ref(), "D:\\canonical\\cwd");
    }

    #[test]
    fn a_malformed_summary_falls_back_to_path_metadata_without_a_skip() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "sessions/proj/sess-1/updates.jsonl": sample_turn_completed_line(),
            "sessions/proj/sess-1/summary.json": r#"{"info":{"id":"SECRET_CANARY""#,
        });
        let files = fixture.path("sessions/proj/sess-1/updates.jsonl");
        let diagnostics = Diagnostics::new("grok");

        let entries = parse_session_files_with_diagnostics(
            &files,
            &jiff::tz::TimeZone::UTC,
            CostMode::Display,
            PriceTable::Display,
            &diagnostics,
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].1.project_path.as_ref(), "proj");
        assert!(
            diagnostics
                .warnings(&crate::cli::SharedArgs::default())
                .is_empty()
        );
        let debug = diagnostics
            .warnings(&crate::cli::SharedArgs {
                debug: true,
                ..Default::default()
            })
            .join("\n");
        assert!(debug.contains("summary.json"));
        assert!(!debug.contains("SECRET_CANARY"));
    }

    #[test]
    fn line_session_id_beats_summary_id() {
        ccusage_test_support::install_pricing_snapshots();
        let line = sample_turn_completed_line();
        let summary = r#"{"info":{"id":"canonical-session","cwd":"D:\\canonical\\cwd"}}"#;
        let entries = parse_lines(&line, Some(summary));

        assert_eq!(entries[0].session_id.as_ref(), "sess-1");
        assert_eq!(entries[0].project_path.as_ref(), "D:\\canonical\\cwd");
    }

    #[test]
    fn url_decodes_project_segment_when_summary_is_absent() {
        ccusage_test_support::install_pricing_snapshots();
        let line = r#"{"timestamp":1750000000,"params":{"update":{"sessionUpdate":"turn_completed","usage":{"modelUsage":{"grok-4.5-build":{"inputTokens":10,"outputTokens":1,"cachedReadTokens":0,"reasoningTokens":0}}}},"_meta":{"eventId":"evt-url"}}}"#;
        let fixture = fs_fixture!({
            "sessions/D%3A%5Cwork%5Cproj/019fa1b1-0000-7000-8000-000000000001/updates.jsonl": line,
        });
        let files = fixture
            .path("sessions/D%3A%5Cwork%5Cproj/019fa1b1-0000-7000-8000-000000000001/updates.jsonl");
        let entries = parse_session_files_with_diagnostics(
            &files,
            &jiff::tz::TimeZone::UTC,
            CostMode::Display,
            PriceTable::Display,
            &Diagnostics::new("grok"),
        )
        .map(|entries| {
            entries
                .into_iter()
                .map(|(_, entry)| entry)
                .collect::<Vec<_>>()
        })
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].session_id.as_ref(),
            "019fa1b1-0000-7000-8000-000000000001"
        );
        assert_eq!(entries[0].project_path.as_ref(), "D:\\work\\proj");
    }

    #[test]
    fn project_path_segment_keeps_distinct_names_for_invalid_utf8() {
        assert_eq!(
            decode_project_path_segment("%2Ftmp%2F%E6%97%A5%E6%9C%AC"),
            "/tmp/日本"
        );
        assert_eq!(
            decode_project_path_segment("D%3A%5Cwork%5Cproj"),
            "D:\\work\\proj"
        );
        assert_eq!(decode_project_path_segment("%2Ftmp%2Fa+b"), "/tmp/a+b");
        assert_eq!(decode_project_path_segment("%2Ftmp%2F%zz"), "/tmp/%zz");
        assert_eq!(
            decode_project_path_segment("%2Ftmp%2F%FFx"),
            "%2Ftmp%2F%FFx"
        );
        assert_ne!(
            decode_project_path_segment("%2Ftmp%2F%FFx"),
            decode_project_path_segment("%2Ftmp%2F%FEx")
        );
    }

    #[test]
    fn url_decodes_multi_byte_project_segment() {
        ccusage_test_support::install_pricing_snapshots();
        let line = r#"{"timestamp":1750000000,"params":{"update":{"sessionUpdate":"turn_completed","usage":{"modelUsage":{"grok-4.5-build":{"inputTokens":10,"outputTokens":1,"cachedReadTokens":0,"reasoningTokens":0}}}},"_meta":{"eventId":"evt-utf8"}}}"#;
        let fixture = fs_fixture!({
            "sessions/%2Fhome%2F%C3%A9projet/019fa1b1-0000-7000-8000-000000000002/updates.jsonl": line,
        });
        let files = fixture.path(
            "sessions/%2Fhome%2F%C3%A9projet/019fa1b1-0000-7000-8000-000000000002/updates.jsonl",
        );
        let entries = parse_session_files_with_diagnostics(
            &files,
            &jiff::tz::TimeZone::UTC,
            CostMode::Display,
            PriceTable::Display,
            &Diagnostics::new("grok"),
        )
        .map(|entries| {
            entries
                .into_iter()
                .map(|(_, entry)| entry)
                .collect::<Vec<_>>()
        })
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].project_path.as_ref(), "/home/éprojet");
    }

    #[test]
    fn skips_turn_completed_without_usage_and_zero_rows() {
        ccusage_test_support::install_pricing_snapshots();
        let lines = [
            r#"{"timestamp":1750000001,"params":{"update":{"sessionUpdate":"tool_call"}}}"#,
            r#"{"timestamp":1750000002,"params":{"update":{"sessionUpdate":"turn_completed"},"_meta":{"eventId":"no-usage"}}}"#,
            r#"{"timestamp":1750000003,"params":{"update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":0,"outputTokens":0,"cachedReadTokens":0,"reasoningTokens":0,"modelUsage":{"grok-4.5":{"inputTokens":0,"outputTokens":0,"cachedReadTokens":0,"reasoningTokens":0}}}},"_meta":{"eventId":"zero"}}}"#,
            r#"{"this is not json"#,
            &sample_turn_completed_line(),
        ]
        .join("\n");
        let entries = parse_lines(&lines, None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].usage.input_tokens, 60);
    }

    #[test]
    fn multi_model_turn_emits_one_entry_per_model() {
        ccusage_test_support::install_pricing_snapshots();
        let line = r#"{"timestamp":1750000100,"params":{"sessionId":"sess-m","update":{"sessionUpdate":"turn_completed","usage":{"modelUsage":{"model-a":{"inputTokens":10,"outputTokens":2,"cachedReadTokens":0,"reasoningTokens":1},"model-b":{"inputTokens":20,"outputTokens":4,"cachedReadTokens":5,"reasoningTokens":0}}}},"_meta":{"eventId":"evt-multi"}}}"#;
        let mut entries = parse_lines(line, None);
        entries.sort_by(|a, b| a.model.cmp(&b.model));
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].model.as_deref(), Some("model-a"));
        assert_eq!(entries[0].usage.input_tokens, 10);
        assert_eq!(entries[0].extra_total_tokens, 0);
        assert_eq!(entries[1].model.as_deref(), Some("model-b"));
        assert_eq!(entries[1].usage.input_tokens, 15);
        assert_eq!(entries[1].usage.cache_read_input_tokens, 5);
    }

    #[test]
    fn dedupes_same_event_id_and_model() {
        ccusage_test_support::install_pricing_snapshots();
        let line = sample_turn_completed_line();
        let content = format!("{line}\n{line}\n");
        let entries = parse_lines(&content, None);
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn dedupes_identical_rows_without_event_id_by_content_key() {
        ccusage_test_support::install_pricing_snapshots();
        let line = r#"{"timestamp":1750000000,"params":{"sessionId":"sess-d","update":{"sessionUpdate":"turn_completed","usage":{"modelUsage":{"grok-4.5-build":{"inputTokens":100,"outputTokens":20,"cachedReadTokens":40,"reasoningTokens":10}}}}}}"#;
        let content = format!("{line}\n{line}\n");
        let entries = parse_lines(&content, None);
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn keeps_rows_without_event_id_that_differ_only_in_cache_creation() {
        ccusage_test_support::install_pricing_snapshots();
        let with_creation = r#"{"timestamp":1750000000,"params":{"sessionId":"sess-cc","update":{"sessionUpdate":"turn_completed","usage":{"modelUsage":{"grok-4.5-build":{"inputTokens":100,"outputTokens":5,"cachedReadTokens":0,"cacheCreationTokens":20,"reasoningTokens":0}}}}}}"#;
        let without_creation = r#"{"timestamp":1750000000,"params":{"sessionId":"sess-cc","update":{"sessionUpdate":"turn_completed","usage":{"modelUsage":{"grok-4.5-build":{"inputTokens":80,"outputTokens":5,"cachedReadTokens":0,"reasoningTokens":0}}}}}}"#;
        let entries = parse_lines(&format!("{with_creation}\n{without_creation}\n"), None);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].usage.cache_creation_input_tokens, 20);
        assert_eq!(entries[1].usage.cache_creation_input_tokens, 0);
    }

    #[test]
    fn keeps_distinct_models_that_share_an_event_id() {
        ccusage_test_support::install_pricing_snapshots();
        let line = r#"{"timestamp":1750000100,"params":{"sessionId":"sess-m","update":{"sessionUpdate":"turn_completed","usage":{"modelUsage":{"model-a":{"inputTokens":10,"outputTokens":1,"cachedReadTokens":0,"reasoningTokens":0},"model-b":{"inputTokens":20,"outputTokens":2,"cachedReadTokens":0,"reasoningTokens":0}}}},"_meta":{"eventId":"evt-shared"}}}"#;
        let entries = parse_lines(line, None);
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn display_mode_cost_is_zero_and_does_not_flag_missing_pricing() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "sessions/proj/sess-1/updates.jsonl": sample_turn_completed_line(),
        });
        let files = fixture.path("sessions/proj/sess-1/updates.jsonl");
        let entries = parse_session_files_with_diagnostics(
            &files,
            &jiff::tz::TimeZone::UTC,
            CostMode::Display,
            PriceTable::Display,
            &Diagnostics::new("grok"),
        )
        .map(|entries| {
            entries
                .into_iter()
                .map(|(_, entry)| entry)
                .collect::<Vec<_>>()
        })
        .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].cost, 0.0);
        assert_eq!(entries[0].missing_pricing_model, None);
    }

    #[test]
    fn reads_the_recorded_cost_usd_ticks() {
        ccusage_test_support::install_pricing_snapshots();
        let line = r#"{"timestamp":1750000000,"params":{"sessionId":"sess-1","update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":18444,"outputTokens":130,"cachedReadTokens":11264,"reasoningTokens":73,"costUsdTicks":185192000,"modelUsage":{"grok-4.5-build":{"inputTokens":18444,"outputTokens":130,"cachedReadTokens":11264,"reasoningTokens":73,"costUsdTicks":185192000}}}},"_meta":{"eventId":"evt-cost"}}}"#;
        let entries = parse_lines(line, None);

        assert_eq!(entries.len(), 1);
        assert!(
            entries[0]
                .recorded_cost
                .is_some_and(|cost| (cost - 0.0185192).abs() < 1e-12)
        );
        assert!((entries[0].cost - 0.0185192).abs() < 1e-12);
        assert_eq!(entries[0].missing_pricing_model, None);
    }

    #[test]
    fn auto_mode_marks_recorded_ticks_as_stored_cost() {
        ccusage_test_support::install_pricing_snapshots();
        let fixture = fs_fixture!({
            "sessions/proj/sess-1/updates.jsonl": r#"{"timestamp":1750000000,"params":{"sessionId":"sess-1","update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":18444,"outputTokens":130,"costUsdTicks":185192000,"modelUsage":{"grok-4.5-build":{"inputTokens":18444,"outputTokens":130,"costUsdTicks":185192000}}}},"_meta":{"eventId":"evt-cost"}}}"#,
        });
        let files = fixture.path("sessions/proj/sess-1/updates.jsonl");
        let pricing = ccusage_core::PricingMap::load_embedded();
        let stored_cost = |mode| {
            parse_session_files_with_diagnostics(
                &files,
                &jiff::tz::TimeZone::UTC,
                mode,
                PriceTable::for_map(mode, &pricing),
                &Diagnostics::new("grok"),
            )
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|(_, entry)| entry)
                    .collect::<Vec<_>>()
            })
            .unwrap()[0]
                .stored_cost
        };

        assert!(stored_cost(CostMode::Auto).is_some_and(|cost| (cost - 0.0185192).abs() < 1e-12));
        assert_eq!(stored_cost(CostMode::Calculate), None);
        assert_eq!(stored_cost(CostMode::Display), None);
    }

    #[test]
    fn splits_cache_creation_out_of_the_uncached_input() {
        assert_eq!(split_input_tokens(100, 40, 25), (35, 40, 25));
        assert_eq!(split_input_tokens(100, 40, 0), (60, 40, 0));
        assert_eq!(split_input_tokens(100, 40, 999), (0, 40, 60));
    }

    #[test]
    fn reads_cache_creation_tokens() {
        ccusage_test_support::install_pricing_snapshots();
        let line = r#"{"timestamp":1750000000,"params":{"sessionId":"sess-1","update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":100,"outputTokens":20,"cachedReadTokens":40,"cacheCreationTokens":25,"reasoningTokens":10,"modelUsage":{"grok-4.5-build":{"inputTokens":100,"outputTokens":20,"cachedReadTokens":40,"cacheCreationTokens":25,"reasoningTokens":10}}}},"_meta":{"eventId":"evt-cc"}}}"#;
        let entries = parse_lines(line, None);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].usage.input_tokens, 35);
        assert_eq!(entries[0].usage.cache_read_input_tokens, 40);
        assert_eq!(entries[0].usage.cache_creation_input_tokens, 25);
        assert_eq!(entries[0].usage.output_tokens, 20);
    }

    fn calculated_turn(cache_creation: u64) -> (LoadedEntry, Diagnostics) {
        ccusage_test_support::install_pricing_snapshots();
        let key = "grok-4.5".to_string();
        let pricing_override = crate::cli::PricingOverride {
            input_cost_per_token: Some(1.0),
            cache_creation_input_token_cost: Some(10.0),
            cache_read_input_token_cost: Some(100.0),
            output_cost_per_token: Some(1000.0),
            ..crate::cli::PricingOverride::default()
        };
        let pricing =
            ccusage_core::PricingMap::load_with_overrides(true, false, [(&key, &pricing_override)]);
        let usage = serde_json::json!({
            "inputTokens": 1000,
            "outputTokens": 5,
            "cachedReadTokens": 600,
            "cacheCreationTokens": cache_creation,
        });
        let mut total = usage.clone();
        total["modelUsage"] = serde_json::json!({ "grok-4.5-build": usage });
        let line = serde_json::json!({
            "timestamp": 1_750_000_000,
            "params": {
                "sessionId": "sess-1",
                "update": { "sessionUpdate": "turn_completed", "usage": total },
                "_meta": { "eventId": "evt-calc" },
            },
        })
        .to_string();
        let fixture = fs_fixture!({ "sessions/proj/sess-1/updates.jsonl": line });
        let diagnostics = Diagnostics::new("grok");
        let mut entries = parse_session_files_with_diagnostics(
            &fixture.path("sessions/proj/sess-1/updates.jsonl"),
            &jiff::tz::TimeZone::UTC,
            CostMode::Calculate,
            PriceTable::Calculate(&pricing),
            &diagnostics,
        )
        .unwrap();
        assert_eq!(entries.len(), 1);
        (entries.remove(0).1, diagnostics)
    }

    #[test]
    fn prices_cache_creation_carved_from_the_full_prompt_input() {
        let (entry, diagnostics) = calculated_turn(300);

        assert_eq!(entry.usage.input_tokens, 100);
        assert_eq!(entry.usage.cache_read_input_tokens, 600);
        assert_eq!(entry.usage.cache_creation_input_tokens, 300);
        assert_eq!(entry.cost, 100.0 + 3_000.0 + 60_000.0 + 5_000.0);
        assert!(
            diagnostics
                .warnings(&crate::cli::SharedArgs::default())
                .is_empty()
        );
    }

    #[test]
    fn prices_the_full_uncached_input_without_cache_creation() {
        let (entry, diagnostics) = calculated_turn(0);

        assert_eq!(entry.usage.input_tokens, 400);
        assert_eq!(entry.usage.cache_read_input_tokens, 600);
        assert_eq!(entry.usage.cache_creation_input_tokens, 0);
        assert_eq!(entry.cost, 400.0 + 60_000.0 + 5_000.0);
        assert!(
            diagnostics
                .warnings(&crate::cli::SharedArgs::default())
                .is_empty()
        );
    }

    #[test]
    fn auto_prefers_recorded_ticks_while_calculate_recomputes() {
        ccusage_test_support::install_pricing_snapshots();
        let key = "grok-4.5".to_string();
        let pricing_override = crate::cli::PricingOverride {
            input_cost_per_token: Some(1.0),
            output_cost_per_token: Some(1.0),
            ..crate::cli::PricingOverride::default()
        };
        let pricing =
            ccusage_core::PricingMap::load_with_overrides(true, false, [(&key, &pricing_override)]);
        let usage = TokenUsageRaw {
            input_tokens: 10,
            output_tokens: 10,
            ..TokenUsageRaw::default()
        };

        assert_eq!(
            price_grok_usage(
                "grok-4.5-build",
                usage,
                Some(0.25),
                None,
                CostMode::Auto,
                PriceTable::Auto(&pricing)
            )
            .0,
            0.25
        );
        assert_eq!(
            price_grok_usage(
                "grok-4.5-build",
                usage,
                Some(0.25),
                None,
                CostMode::Calculate,
                PriceTable::Calculate(&pricing)
            )
            .0,
            20.0
        );
    }

    #[test]
    fn auto_falls_back_to_the_pricing_table_without_ticks() {
        ccusage_test_support::install_pricing_snapshots();
        let key = "grok-4.5".to_string();
        let pricing_override = crate::cli::PricingOverride {
            input_cost_per_token: Some(1.0),
            output_cost_per_token: Some(1.0),
            ..crate::cli::PricingOverride::default()
        };
        let pricing =
            ccusage_core::PricingMap::load_with_overrides(true, false, [(&key, &pricing_override)]);
        let usage = TokenUsageRaw {
            input_tokens: 10,
            output_tokens: 10,
            ..TokenUsageRaw::default()
        };

        assert_eq!(
            price_grok_usage(
                "grok-4.5-build",
                usage,
                None,
                None,
                CostMode::Auto,
                PriceTable::Auto(&pricing)
            )
            .0,
            20.0
        );
    }
}

#[cfg(test)]
mod pricing_identity_tests {
    use super::*;

    #[test]
    fn adapter_pricing_identity() {
        assert_eq!(
            pricing_candidates(" grok-4.5-build ")
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            [
                "grok-4.5-build",
                "xai/grok-4.5-build",
                "x-ai/grok-4.5-build",
                "grok-4.5",
                "xai/grok-4.5",
                "x-ai/grok-4.5"
            ]
        );
    }
}
