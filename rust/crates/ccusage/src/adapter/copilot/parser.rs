use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader},
    path::Path,
};

use serde_json::Value;

use crate::{Result, TimestampMs};

#[derive(Debug, Clone)]
pub(super) struct CopilotUsageEntry {
    pub(super) timestamp: TimestampMs,
    pub(super) timestamp_text: String,
    pub(super) session_id: String,
    pub(super) model: String,
    pub(super) input_tokens: u64,
    pub(super) output_tokens: u64,
    pub(super) cache_creation_tokens: u64,
    pub(super) cache_read_tokens: u64,
    pub(super) reasoning_output_tokens: u64,
    pub(super) dedup_key: String,
    /// AI Credits in `nanoAiu` (10⁻⁹ AI Units) when the Copilot CLI reported
    /// them on the source event. `None` for session-state shutdowns that
    /// predate the AI Credits schema (CLI versions before ~1.0.40).
    pub(super) nano_aiu: Option<u64>,
    /// Number of premium requests this row consumed under the pre-June-2026
    /// billing model. The Copilot CLI bakes per-model multipliers into this
    /// value (e.g. Opus 4.7 with 1 request × 7.5 multiplier = 7.5 premium
    /// requests). Fractional in real data, hence `f64`.
    ///
    /// `None` in any of:
    /// * The synthetic aggregate-credit entry (no source model or request
    ///   data to attribute).
    /// * A session-state row whose source omits the `cost` field entirely
    ///   (today unreachable — every observed row ships the field — but
    ///   forward-compat against a future Copilot CLI that might drop it,
    ///   in which case Auto's dispatch falls through to token-priced
    ///   pricing rather than reporting a false `$0` bill).
    ///
    /// `Some(0.0)` is reserved for free-tier models (sonnet, haiku) under
    /// the premium-request plan where the field is explicitly zero.
    ///
    /// Used by `auto`/`display` mode as the pre-AIU fallback before
    /// token-priced cost.
    pub(super) premium_request_cost: Option<f64>,
}

// -------------------------------------------------------------------------
// session-state parser (`~/.copilot/session-state/<uuid>/events.jsonl`)
// -------------------------------------------------------------------------

fn file_modified_timestamp(path: &Path) -> TimestampMs {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| TimestampMs::from_millis(duration.as_millis().min(i64::MAX as u128) as i64))
        .unwrap_or_else(crate::utc_now)
}

// `SESSION_SHUTDOWN_TYPE` serves two roles in the parse pipeline:
//
// 1. Substring needle: `str::contains(SESSION_SHUTDOWN_TYPE)` runs on
//    the raw JSON line text BEFORE serde parsing, as a cheap pre-filter
//    that lets non-shutdown lines short-circuit without paying for a
//    full `from_str::<ShutdownEvent>`.
// 2. Exact-match: compared against the parsed `ShutdownEvent.event_type`
//    field AFTER serde, to defend against other event kinds (e.g.
//    `tool.execution_start`) whose `arguments.command` payload happens
//    to contain the substring "session.shutdown".
//
// One const used for both roles can't desync. If a future Copilot CLI
// schema renames the event type (e.g. to "session.exit"), updating this
// single literal flips both the pre-filter AND the exact-match together.
const SESSION_SHUTDOWN_TYPE: &str = "session.shutdown";
const GEMINI_PREFIX: &str = "gemini-";

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShutdownEvent {
    // Folding the discriminator into the main struct avoids re-parsing
    // each line as a `TypeOnly`-shaped second pass (one of the hot loops
    // in `parse_session_state_file`). The substring filter on
    // `SESSION_SHUTDOWN_TYPE` still runs first to skip non-shutdown
    // lines without any serde work; this field is the authoritative
    // type check that defends against other event types whose
    // arguments/content happen to contain the literal substring
    // "session.shutdown".
    #[serde(rename = "type")]
    event_type: String,
    id: String,
    timestamp: String,
    #[serde(default)]
    data: ShutdownData,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ShutdownData {
    #[serde(default)]
    model_metrics: HashMap<String, ModelMetrics>,
    // `total_nano_aiu` (and its per-model sibling on `ModelMetrics`) is typed
    // as `u64` because "nano" units are integer by definition — there's no
    // meaningful fractional nano-AIU. To stay forward-compatible against a
    // future Copilot CLI that emits the field as a JSON float (e.g.
    // `1.5e11` or `100000000000.0`), the custom deserializer
    // `deserialize_nano_aiu` accepts BOTH integer and float JSON forms and
    // truncates floats to `u64`. Without this, a float-valued shutdown
    // would fail the whole `ShutdownEvent` deserialize and drop every
    // model in that shutdown — the exact failure class fixed for
    // `requests.cost` (which IS legitimately fractional).
    #[serde(default, deserialize_with = "deserialize_nano_aiu")]
    total_nano_aiu: Option<u64>,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ModelMetrics {
    #[serde(default)]
    usage: SessionUsage,
    #[serde(default)]
    requests: SessionRequests,
    // See the `total_nano_aiu` note on `ShutdownData` above for why this
    // is `u64` rather than `f64`, and why both forms are accepted.
    #[serde(default, deserialize_with = "deserialize_nano_aiu")]
    total_nano_aiu: Option<u64>,
}

/// Tolerant deserializer for `totalNanoAiu` that accepts both JSON integer
/// (the only form observed in the local 200-file corpus) and JSON float
/// (a forward-compat shape a future Copilot CLI release could emit). Floats
/// are truncated to `u64`; negative values, NaN, and overflow are rejected
/// with a `serde` error rather than silently coerced.
///
/// Pinned by `tolerates_float_valued_per_model_total_nano_aiu`,
/// `tolerates_float_valued_aggregate_total_nano_aiu`, and
/// `rejects_negative_float_total_nano_aiu`.
fn deserialize_nano_aiu<'de, D>(deserializer: D) -> std::result::Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    use serde::de::{Error, Unexpected};
    let value = Option::<Value>::deserialize(deserializer)?;
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => {
            if let Some(u) = n.as_u64() {
                Ok(Some(u))
            } else if let Some(f) = n.as_f64() {
                if !f.is_finite() || f < 0.0 {
                    Err(D::Error::invalid_value(
                        Unexpected::Float(f),
                        &"a non-negative finite number for nano-AIU",
                    ))
                } else if f >= (u64::MAX as f64) {
                    Err(D::Error::invalid_value(
                        Unexpected::Float(f),
                        &"a nano-AIU value that fits in u64",
                    ))
                } else {
                    Ok(Some(f as u64))
                }
            } else {
                Err(D::Error::custom(format!(
                    "nano-AIU number not representable as u64 or f64: {n}"
                )))
            }
        }
        Some(other) => Err(D::Error::custom(format!(
            "nano-AIU must be a number, got {}",
            describe_value(&other),
        ))),
    }
}

fn describe_value(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SessionUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_tokens: u64,
    #[serde(default)]
    cache_write_tokens: u64,
    #[serde(default)]
    reasoning_tokens: u64,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SessionRequests {
    #[serde(default)]
    count: u64,
    // Fractional premium-request count; `None` means Auto falls back to
    // token pricing instead of inventing a false `$0` source bill.
    #[serde(default, deserialize_with = "deserialize_premium_request_cost")]
    cost: Option<f64>,
}

/// Tolerant deserializer for `requests.cost` that matches
/// [`deserialize_nano_aiu`]: accepts integer/float JSON forms and rejects
/// non-finite or negative values.
fn deserialize_premium_request_cost<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    use serde::de::{Error, Unexpected};
    let value = Option::<Value>::deserialize(deserializer)?;
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => {
            if let Some(u) = n.as_u64() {
                Ok(Some(u as f64))
            } else if let Some(f) = n.as_f64() {
                if !f.is_finite() {
                    Err(D::Error::invalid_value(
                        Unexpected::Float(f),
                        &"a finite (non-NaN, non-Infinity) number for requests.cost",
                    ))
                } else if f < 0.0 {
                    Err(D::Error::invalid_value(
                        Unexpected::Float(f),
                        &"a non-negative number for requests.cost",
                    ))
                } else {
                    Ok(Some(f))
                }
            } else {
                Err(D::Error::custom(format!(
                    "requests.cost number not representable as u64 or f64: {n}"
                )))
            }
        }
        Some(other) => Err(D::Error::custom(format!(
            "requests.cost must be a number, got {}",
            describe_value(&other),
        ))),
    }
}

pub(super) fn parse_session_state_file(path: &Path) -> Result<Vec<CopilotUsageEntry>> {
    // Stream the file via `BufReader::split(b'\n')` rather than
    // `fs::read` so peak memory stays bounded by line size (a few KB) and
    // not file size. Real-world `events.jsonl` files reach 69MB+ for
    // heavy users, where the one-shot read would briefly allocate a
    // whole-file `Vec<u8>` per session-state file. Byte-level splitting
    // (rather than `.lines()`) preserves the existing
    // tolerate-invalid-UTF-8 / tolerate-truncated-trailing-line semantics.
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let session_id = session_id_from_path(path).unwrap_or_else(|| "unknown-session".to_string());
    // Computed lazily ONCE per file (not per row): the only consumer is
    // the unparseable-timestamp fallback below, and real RFC3339 strings
    // from the Copilot CLI always parse — so this is effectively never
    // hit on observed data. Pre-computing it would do one fs::metadata
    // syscall per file regardless of whether the fallback fires; an
    // upfront syscall every parse_session_state_file call would be
    // wasteful given the volume of files the loader walks. The
    // `OnceCell` collapses to the upstream cost only on first need.
    let fallback_timestamp: std::cell::OnceCell<TimestampMs> = std::cell::OnceCell::new();

    let mut entries = Vec::new();
    for line_result in reader.split(b'\n') {
        let line_bytes = line_result?;
        let trimmed: &[u8] = match line_bytes.iter().position(|&b| !b.is_ascii_whitespace()) {
            Some(start) => &line_bytes[start..],
            None => continue,
        };
        if trimmed.is_empty() {
            continue;
        }
        let Ok(line_str) = std::str::from_utf8(trimmed) else {
            continue;
        };
        if !line_str.contains(SESSION_SHUTDOWN_TYPE) {
            continue;
        }
        let Ok(event) = serde_json::from_str::<ShutdownEvent>(line_str) else {
            continue;
        };
        if event.event_type != SESSION_SHUTDOWN_TYPE {
            continue;
        }
        let timestamp = crate::date_utils::parse_ts_timestamp(&event.timestamp)
            .unwrap_or_else(|| *fallback_timestamp.get_or_init(|| file_modified_timestamp(path)));

        for (model, metrics) in &event.data.model_metrics {
            if skip_metrics_row(model, metrics) {
                continue;
            }
            entries.push(build_session_state_entry(
                &event.id,
                &session_id,
                model,
                metrics,
                timestamp,
            ));
        }

        let any_per_model_priced_billing = event.data.model_metrics.iter().any(|(model, m)| {
            !skip_metrics_row(model, m)
                && (m.total_nano_aiu.is_some_and(|n| n > 0)
                    || m.requests.cost.is_some_and(|c| c > 0.0))
        });
        if let Some(naiu) = event
            .data
            .total_nano_aiu
            .filter(|&naiu| naiu > 0 && !any_per_model_priced_billing)
        {
            entries.push(build_aggregate_credit_entry(&session_id, naiu, timestamp));
        }
    }
    Ok(entries)
}

/// Skip all-empty rows, except nonzero AIU credit rows and Gemini reasoning-only rows.
fn skip_metrics_row(model: &str, metrics: &ModelMetrics) -> bool {
    let usage = &metrics.usage;
    let reasoning_counts_as_usage = model.starts_with(GEMINI_PREFIX);
    let no_tokens = usage.input_tokens == 0
        && usage.output_tokens == 0
        && usage.cache_read_tokens == 0
        && usage.cache_write_tokens == 0
        && (!reasoning_counts_as_usage || usage.reasoning_tokens == 0);
    let no_requests = metrics.requests.count == 0;
    let no_credits = metrics.total_nano_aiu.is_none_or(|n| n == 0);
    no_tokens && no_requests && no_credits
}

fn build_session_state_entry(
    event_id: &str,
    session_id: &str,
    model: &str,
    metrics: &ModelMetrics,
    timestamp: TimestampMs,
) -> CopilotUsageEntry {
    let usage = &metrics.usage;
    // inputTokens includes cache buckets; subtract them for fresh input.
    let input_only = usage
        .input_tokens
        .saturating_sub(usage.cache_read_tokens)
        .saturating_sub(usage.cache_write_tokens);

    // Only Gemini keeps reasoning separate; others include it in outputTokens.
    let reasoning_output_tokens = if model.starts_with(GEMINI_PREFIX) {
        usage.reasoning_tokens
    } else {
        0
    };

    let zero_token = input_only == 0
        && usage.output_tokens == 0
        && usage.cache_read_tokens == 0
        && usage.cache_write_tokens == 0
        && reasoning_output_tokens == 0;
    let credit_only = zero_token && metrics.requests.count == 0;

    let dedup_key = match (credit_only, metrics.total_nano_aiu) {
        (true, Some(naiu)) => {
            format!("credit-shutdown:{session_id}:{model}:{naiu}")
        }
        _ => format!("shutdown:{session_id}:{event_id}:{model}"),
    };

    CopilotUsageEntry {
        timestamp,
        timestamp_text: crate::format_rfc3339_millis(timestamp),
        session_id: session_id.to_string(),
        model: model.to_string(),
        input_tokens: input_only,
        output_tokens: usage.output_tokens,
        cache_creation_tokens: usage.cache_write_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        reasoning_output_tokens,
        dedup_key,
        nano_aiu: metrics.total_nano_aiu,
        premium_request_cost: metrics.requests.cost,
    }
}

fn build_aggregate_credit_entry(
    session_id: &str,
    nano_aiu: u64,
    timestamp: TimestampMs,
) -> CopilotUsageEntry {
    CopilotUsageEntry {
        timestamp,
        timestamp_text: crate::format_rfc3339_millis(timestamp),
        session_id: session_id.to_string(),
        model: String::new(),
        input_tokens: 0,
        output_tokens: 0,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        reasoning_output_tokens: 0,
        dedup_key: format!("credit-aggregate:{session_id}:{nano_aiu}"),
        nano_aiu: Some(nano_aiu),
        premium_request_cost: None,
    }
}

fn session_id_from_path(path: &Path) -> Option<String> {
    path.parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .map(str::to_string)
}

/// Strips Copilot routing suffixes and normalizes Claude dots for LiteLLM pricing lookup.
pub(super) fn normalize_copilot_model(raw: &str) -> std::borrow::Cow<'_, str> {
    const SUFFIXES: &[&str] = &["-1m-internal", "-internal", "-xhigh", "-high", "-1m"];
    let mut trimmed: &str = raw;
    for suffix in SUFFIXES {
        if trimmed.ends_with(suffix) {
            trimmed = &trimmed[..trimmed.len() - suffix.len()];
            break;
        }
    }

    if trimmed.starts_with("claude-") {
        let dotted = claude_dot_to_dash(trimmed);
        return std::borrow::Cow::Owned(dotted);
    }

    if trimmed.as_ptr() == raw.as_ptr() && trimmed.len() == raw.len() {
        std::borrow::Cow::Borrowed(raw)
    } else {
        std::borrow::Cow::Owned(trimmed.to_string())
    }
}

fn claude_dot_to_dash(value: &str) -> String {
    // Rewrites `claude-opus-4.7` → `claude-opus-4-7` etc. (Convert ASCII
    // dots between two ASCII digits to dashes.) Decode the input as `char`
    // values rather than raw bytes so multi-byte UTF-8 model identifiers
    // stay intact: today the `claude-` gate at the only call site keeps
    // inputs ASCII, but if the upstream model identifier scheme ever
    // included non-ASCII characters this helper must not corrupt them by
    // re-encoding bytes as `char`.
    //
    // Streaming impl: walk the iterator with a single-char lookahead
    // (`Peekable`) and a one-char trailing memory (`prev_ch`). Avoids
    // collecting the full `Vec<char>` that the previous implementation
    // built, dropping a transient O(n) heap allocation on every Claude
    // pricing lookup.
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    let mut prev_ch: Option<char> = None;
    while let Some(ch) = chars.next() {
        let is_dot_between_digits = ch == '.'
            && prev_ch.is_some_and(|p| p.is_ascii_digit())
            && chars.peek().is_some_and(|n| n.is_ascii_digit());
        if is_dot_between_digits {
            out.push('-');
        } else {
            out.push(ch);
        }
        prev_ch = Some(ch);
    }
    out
}

#[cfg(test)]
mod session_state_tests {
    use serde_json::json;

    use super::*;

    fn write_jsonl(lines: &[String]) -> (ccusage_test_support::Fixture, std::path::PathBuf) {
        // Mirror real layout: <root>/<session_uuid>/events.jsonl
        let fixture = ccusage_test_support::Fixture::new();
        let mut content = lines.join("\n");
        if !content.ends_with('\n') {
            content.push('\n');
        }
        let path = fixture.write_file("abc123-test-session/events.jsonl", content.as_str());
        (fixture, path)
    }

    fn shutdown_event(id: &str, ts: &str, model_metrics: serde_json::Value) -> String {
        json!({
            "type": "session.shutdown",
            "id": id,
            "timestamp": ts,
            "data": {"modelMetrics": model_metrics},
        })
        .to_string()
    }

    #[test]
    fn accepts_fractional_premium_request_cost() {
        // Real-data invariant: ~2% of shutdown rows ship fractional
        // `requests.cost` values (e.g. Opus 4.7's `1 × 7.5 multiplier =
        // 7.5`). If `cost` were typed as `u64`, the whole shutdown event
        // would fail to deserialize and ALL of its models would be
        // silently dropped — including the unrelated integer-cost
        // siblings that happened to be in the same event. This fixture
        // intentionally mixes a fractional row (Opus 4.7, cost 7.5) with
        // an integer-cost sibling (Sonnet 4.5, cost 1 — 1 request × 1×
        // multiplier) so the regression pins both the fractional row
        // surviving AND the sibling not being collateral damage.
        let (_dir, path) = write_jsonl(&[shutdown_event(
            "evt-frac",
            "2026-05-02T15:27:09.013Z",
            json!({
                "claude-opus-4.7": {
                    "usage": {
                        "inputTokens": 1_000u64,
                        "outputTokens": 100u64,
                        "cacheReadTokens": 0u64,
                        "cacheWriteTokens": 0u64,
                        "reasoningTokens": 0u64
                    },
                    "requests": {"count": 1, "cost": 7.5}
                },
                "claude-sonnet-4.5": {
                    "usage": {
                        "inputTokens": 2_000u64,
                        "outputTokens": 200u64,
                        "cacheReadTokens": 0u64,
                        "cacheWriteTokens": 0u64,
                        "reasoningTokens": 0u64
                    },
                    // Sonnet 4.5 has a 1× multiplier under the pre-cutover
                    // premium plan, so 1 request × 1× = cost 1 (integer).
                    "requests": {"count": 1, "cost": 1}
                }
            }),
        )]);

        let entries = parse_session_state_file(&path).unwrap();
        // Both rows must survive — the fractional cost on one row used to
        // fail u64 deserialization and silently drop the entire event,
        // including the unrelated integer-cost row it carried.
        assert_eq!(entries.len(), 2, "got {entries:?}");
        let opus = entries
            .iter()
            .find(|e| e.model == "claude-opus-4.7")
            .expect("opus row");
        assert_eq!(opus.premium_request_cost, Some(7.5));
        let sonnet = entries
            .iter()
            .find(|e| e.model == "claude-sonnet-4.5")
            .expect("sonnet row (integer-cost sibling)");
        assert_eq!(sonnet.premium_request_cost, Some(1.0));
    }

    #[test]
    fn absent_requests_cost_preserves_none_for_token_priced_fallback() {
        // Forward-compat: today every observed Copilot session-state
        // shutdown row ships `requests.cost`. If a future CLI ever drops
        // that field, we MUST distinguish "absent" from "explicit 0" so
        // Auto's dispatch can fall through to token-priced pricing rather
        // than reporting a false `$0` bill from `Some(0.0) × $0.04`.
        // This test pins the `cost: Option<f64>` shape: a row with no
        // `cost` field deserializes to `premium_request_cost: None`.
        let (_dir, path) = write_jsonl(&[shutdown_event(
            "evt-no-cost",
            "2026-05-02T15:27:09.013Z",
            json!({
                "claude-opus-4.7-1m-internal": {
                    "usage": {
                        "inputTokens": 1_000u64,
                        "outputTokens": 100u64,
                        "cacheReadTokens": 0u64,
                        "cacheWriteTokens": 0u64,
                        "reasoningTokens": 0u64
                    },
                    // `cost` intentionally absent — `count` only.
                    "requests": {"count": 1}
                }
            }),
        )]);

        let entries = parse_session_state_file(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].premium_request_cost, None,
            "absent `cost` field must round-trip as None, not Some(0.0)"
        );
    }

    #[test]
    fn tolerates_float_valued_per_model_total_nano_aiu() {
        // Forward-compat hardening (sibling to the fractional-cost fix):
        // a future Copilot CLI that ships `totalNanoAiu` as a JSON float
        // (e.g. `1500000000.0` after passing through a JS pipeline that
        // coerces large integers to f64) must NOT fail the whole event
        // deserialize and silently drop every model. The custom
        // deserializer accepts both u64 and f64 forms and truncates floats
        // to `u64`. Mix a float-valued per-model AIU with an integer-cost
        // sibling so the test pins both the float row surviving AND the
        // sibling not being collateral damage from a u64-only failure.
        let (_dir, path) = write_jsonl(&[shutdown_event(
            "evt-naiu-float",
            "2026-05-20T16:01:29.481Z",
            json!({
                "claude-opus-4.7-1m-internal": {
                    "usage": {
                        "inputTokens": 90_890_709u64,
                        "outputTokens": 172_544u64,
                        "cacheReadTokens": 86_415_643u64,
                        "cacheWriteTokens": 4_474_731u64,
                        "reasoningTokens": 0u64
                    },
                    "requests": {"count": 182, "cost": 4},
                    "totalNanoAiu": 1_500_000_000.0_f64
                },
                "claude-sonnet-4.5": {
                    "usage": {
                        "inputTokens": 2_000u64,
                        "outputTokens": 200u64,
                        "cacheReadTokens": 0u64,
                        "cacheWriteTokens": 0u64,
                        "reasoningTokens": 0u64
                    },
                    "requests": {"count": 1, "cost": 1}
                }
            }),
        )]);

        let entries = parse_session_state_file(&path).unwrap();
        // Both rows must survive — without the tolerant deserializer the
        // float-valued AIU would fail u64 parsing and drop both models.
        assert_eq!(entries.len(), 2, "got {entries:?}");
        let opus = entries
            .iter()
            .find(|e| e.model == "claude-opus-4.7-1m-internal")
            .expect("opus row");
        assert_eq!(opus.nano_aiu, Some(1_500_000_000));
        let sonnet = entries
            .iter()
            .find(|e| e.model == "claude-sonnet-4.5")
            .expect("sonnet row (integer-cost sibling)");
        assert_eq!(sonnet.premium_request_cost, Some(1.0));
    }

    #[test]
    fn tolerates_float_valued_aggregate_total_nano_aiu() {
        // Forward-compat hardening for the aggregate (session-level)
        // `data.totalNanoAiu`: a float-valued aggregate must round-trip
        // and reach the synthetic aggregate-credit entry.
        let event = format!(
            "{}\n",
            json!({
                "type": "session.shutdown",
                "id": "evt-agg-float",
                "timestamp": "2026-05-20T16:01:29.481Z",
                "data": {
                    "totalNanoAiu": 1_000_000_000.0_f64,
                    "modelMetrics": {}
                }
            })
        );
        let (_dir, path) = write_jsonl(&[event]);
        let entries = parse_session_state_file(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].dedup_key.starts_with("credit-aggregate:"));
        assert_eq!(entries[0].nano_aiu, Some(1_000_000_000));
    }

    #[test]
    fn rejects_negative_float_total_nano_aiu() {
        // `totalNanoAiu` is non-negative by definition (nano-units of a
        // credit count). A negative value would be a schema bug; reject
        // rather than coerce. The whole event is dropped via the
        // tolerate-malformed-line path — preferable to silently round to
        // zero and over-suppress the aggregate-credit guard.
        let event = format!(
            "{}\n",
            json!({
                "type": "session.shutdown",
                "id": "evt-naiu-neg",
                "timestamp": "2026-05-20T16:01:29.481Z",
                "data": {
                    "totalNanoAiu": -1.0_f64,
                    "modelMetrics": {}
                }
            })
        );
        let (_dir, path) = write_jsonl(&[event]);
        let entries = parse_session_state_file(&path).unwrap();
        assert!(
            entries.is_empty(),
            "negative nano-AIU must reject the event (not coerce to zero), got {entries:?}"
        );
    }

    #[test]
    fn rejects_negative_premium_request_cost() {
        // Parity with `rejects_negative_float_total_nano_aiu`. A
        // premium-request count cannot legitimately be negative
        // (`cost = count × multiplier`, both non-negative). A negative
        // value would be a schema bug; reject rather than coerce.
        // Critical concern: silently propagating `-1.5 × $0.04 = -$0.06`
        // into the billing total would make the summary cost appear
        // smaller than it really is — strictly worse than dropping the
        // single malformed row. The whole event is dropped via the
        // tolerate-malformed-line path because serde fails the WHOLE
        // event on the custom-deserializer error.
        //
        // Verified to FAIL against the pre-fix default `f64` deserializer
        // (which would have accepted `-1.5` and propagated it through
        // billing).
        //
        // NOTE on NaN / ±Infinity: these are not representable in
        // standard JSON (`serde_json` rejects the literal `NaN` /
        // `Infinity` tokens at parse time before our deserializer
        // runs), so a same-shape NaN regression test would be
        // tautological — the line-skip path handles it via JSON
        // parse failure, not via our custom-deserializer guard. The
        // guard's `is_finite()` check is still useful as
        // defense-in-depth against any future caller that constructs
        // the struct from a non-JSON source.
        let (_dir, path) = write_jsonl(&[shutdown_event(
            "evt-cost-neg",
            "2026-05-20T16:01:29.481Z",
            json!({
                "claude-opus-4.7": {
                    "usage": {
                        "inputTokens": 100u64,
                        "outputTokens": 10u64,
                        "cacheReadTokens": 0u64,
                        "cacheWriteTokens": 0u64,
                        "reasoningTokens": 0u64
                    },
                    "requests": {"count": 1, "cost": -1.5_f64}
                }
            }),
        )]);
        let entries = parse_session_state_file(&path).unwrap();
        assert!(
            entries.is_empty(),
            "negative requests.cost must reject the event (not coerce to a \
             negative bill), got {entries:?}"
        );
    }

    #[test]
    fn parses_session_shutdown_event() {
        let (_dir, path) = write_jsonl(&[shutdown_event(
            "evt-1",
            "2026-05-02T15:27:09.013Z",
            json!({
                "claude-opus-4.7-1m-internal": {
                    "usage": {
                        "inputTokens": 90_890_709u64,
                        "outputTokens": 172_544u64,
                        "cacheReadTokens": 86_415_643u64,
                        "cacheWriteTokens": 4_474_731u64,
                        "reasoningTokens": 0u64
                    },
                    "requests": {"count": 182, "cost": 4}
                },
                "gpt-5.4": {
                    "usage": {
                        "inputTokens": 30_000u64,
                        "outputTokens": 500u64,
                        "cacheReadTokens": 10_000u64,
                        "cacheWriteTokens": 5_000u64,
                        "reasoningTokens": 0u64
                    },
                    "requests": {"count": 2, "cost": 0}
                }
            }),
        )]);

        let entries = parse_session_state_file(&path).unwrap();
        assert_eq!(entries.len(), 2);

        let opus = entries
            .iter()
            .find(|e| e.model == "claude-opus-4.7-1m-internal")
            .expect("missing opus row");
        assert_eq!(opus.session_id, "abc123-test-session");
        assert_eq!(
            opus.dedup_key,
            "shutdown:abc123-test-session:evt-1:claude-opus-4.7-1m-internal"
        );
        assert_eq!(opus.timestamp_text, "2026-05-02T15:27:09.013Z");
        // input - cache_read - cache_write = 90890709 - 86415643 - 4474731 = 335
        assert_eq!(opus.input_tokens, 335);
        assert_eq!(opus.output_tokens, 172_544);
        assert_eq!(opus.cache_read_tokens, 86_415_643);
        assert_eq!(opus.cache_creation_tokens, 4_474_731);
        assert_eq!(opus.reasoning_output_tokens, 0);

        let gpt = entries.iter().find(|e| e.model == "gpt-5.4").unwrap();
        assert_eq!(gpt.input_tokens, 30_000 - 10_000 - 5_000);
    }

    #[test]
    fn treats_resumed_session_shutdowns_as_distinct_entries() {
        // Real-data pattern: same file, same model, two shutdowns with
        // non-monotonic token counts (per-process snapshots, not cumulative).
        let (_dir, path) = write_jsonl(&[
            shutdown_event(
                "evt-first",
                "2026-03-15T12:00:00.000Z",
                json!({
                    "claude-opus-4.6-1m": {
                        "usage": {
                            "inputTokens": 20_000u64,
                            "outputTokens": 1_000u64,
                            "cacheReadTokens": 15_000u64,
                            "cacheWriteTokens": 0u64,
                            "reasoningTokens": 0u64
                        },
                        "requests": {"count": 5, "cost": 3}
                    }
                }),
            ),
            shutdown_event(
                "evt-second",
                "2026-03-16T09:00:00.000Z",
                json!({
                    "claude-opus-4.6-1m": {
                        "usage": {
                            "inputTokens": 8_000u64,
                            "outputTokens": 400u64,
                            "cacheReadTokens": 5_000u64,
                            "cacheWriteTokens": 0u64,
                            "reasoningTokens": 0u64
                        },
                        "requests": {"count": 2, "cost": 0}
                    }
                }),
            ),
        ]);

        let entries = parse_session_state_file(&path).unwrap();
        assert_eq!(entries.len(), 2);
        let mut keys: Vec<_> = entries.iter().map(|e| e.dedup_key.as_str()).collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "shutdown:abc123-test-session:evt-first:claude-opus-4.6-1m",
                "shutdown:abc123-test-session:evt-second:claude-opus-4.6-1m",
            ]
        );
        // Confirm the smaller second value is preserved (not cumulative).
        let second = entries
            .iter()
            .find(|e| e.dedup_key.contains("second"))
            .unwrap();
        assert_eq!(second.output_tokens, 400);
    }

    #[test]
    fn skips_shutdown_with_empty_model_metrics() {
        let (_dir, path) = write_jsonl(&[shutdown_event(
            "evt-empty",
            "2026-03-11T15:45:43.134Z",
            json!({}),
        )]);
        assert!(parse_session_state_file(&path).unwrap().is_empty());
    }

    #[test]
    fn skips_zero_token_and_zero_request_metrics() {
        let (_dir, path) = write_jsonl(&[shutdown_event(
            "evt-zero",
            "2026-03-11T15:45:43.134Z",
            json!({
                "claude-sonnet-4.5": {
                    "usage": {
                        "inputTokens": 0u64,
                        "outputTokens": 0u64,
                        "cacheReadTokens": 0u64,
                        "cacheWriteTokens": 0u64,
                        "reasoningTokens": 0u64
                    },
                    "requests": {"count": 0, "cost": 0}
                }
            }),
        )]);
        assert!(parse_session_state_file(&path).unwrap().is_empty());
    }

    #[test]
    fn keeps_row_when_requests_present_even_with_zero_tokens() {
        // PR #957's skip rule keeps rows when requests > 0 even with zero
        // tokens. This is rare in real data but explicit in the spec.
        let (_dir, path) = write_jsonl(&[shutdown_event(
            "evt-req",
            "2026-03-11T15:45:43.134Z",
            json!({
                "claude-sonnet-4.5": {
                    "usage": {
                        "inputTokens": 0u64,
                        "outputTokens": 0u64,
                        "cacheReadTokens": 0u64,
                        "cacheWriteTokens": 0u64,
                        "reasoningTokens": 0u64
                    },
                    "requests": {"count": 1, "cost": 0}
                }
            }),
        )]);
        let entries = parse_session_state_file(&path).unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn keeps_gemini_row_with_only_reasoning_tokens() {
        // Symmetry guard for Gemini: `build_session_state_entry` preserves
        // `reasoningTokens` verbatim for Gemini (separate field from
        // output), so the skip rule must keep a Gemini row whose only
        // non-zero usage is reasoning. Without the provider-gated
        // reasoning clause in `no_tokens`, this row would be silently
        // dropped here even though the entry construction would have
        // surfaced 1234 reasoning output tokens.
        let (_dir, path) = write_jsonl(&[shutdown_event(
            "evt-reason-only",
            "2026-03-11T15:45:43.134Z",
            json!({
                "gemini-2.0-flash": {
                    "usage": {
                        "inputTokens": 0u64,
                        "outputTokens": 0u64,
                        "cacheReadTokens": 0u64,
                        "cacheWriteTokens": 0u64,
                        "reasoningTokens": 1_234u64
                    },
                    "requests": {"count": 0, "cost": 0}
                }
            }),
        )]);
        let entries = parse_session_state_file(&path).unwrap();
        assert_eq!(entries.len(), 1, "got {entries:?}");
        assert_eq!(entries[0].reasoning_output_tokens, 1_234);
    }

    #[test]
    fn skips_non_gemini_row_with_only_reasoning_tokens() {
        // Symmetry guard for OpenAI/Anthropic: their reasoning tokens
        // are subsumed into `output_tokens`, so a row with
        // `output_tokens == 0 && reasoning_tokens > 0` is a contradiction
        // the schema cannot legitimately emit. If it ever does appear
        // (data corruption or a future schema change), the builder would
        // hard-set `reasoning_output_tokens = 0` for the non-Gemini
        // provider, producing an all-zero phantom entry. The
        // provider-gated skip rule drops it instead, preserving the
        // invariant that every surviving row carries at least one
        // non-zero surfaced field.
        let (_dir, path) = write_jsonl(&[shutdown_event(
            "evt-non-gemini-reason",
            "2026-03-11T15:45:43.134Z",
            json!({
                "claude-opus-4.7": {
                    "usage": {
                        "inputTokens": 0u64,
                        "outputTokens": 0u64,
                        "cacheReadTokens": 0u64,
                        "cacheWriteTokens": 0u64,
                        "reasoningTokens": 1_234u64
                    },
                    "requests": {"count": 0, "cost": 0}
                }
            }),
        )]);
        let entries = parse_session_state_file(&path).unwrap();
        assert!(
            entries.is_empty(),
            "non-Gemini reasoning-only row must be dropped (the builder \
             would zero out reasoning_output_tokens, leaving an all-zero \
             phantom entry); got {entries:?}"
        );
    }

    #[test]
    fn ignores_string_session_shutdown_substring_inside_tool_arguments() {
        // Real-world hazard: our own session currently writes a
        // `tool.execution_start` event whose `arguments.command` contains the
        // literal `"session.shutdown"`. A substring-only filter would emit a
        // phantom row.
        let tool_record = json!({
            "type": "tool.execution_start",
            "id": "tool-1",
            "timestamp": "2026-05-02T15:27:09.013Z",
            "data": {
                "toolName": "bash",
                "arguments": {"command": "grep -l 'session.shutdown' events.jsonl"}
            }
        })
        .to_string();
        let (_dir, path) = write_jsonl(&[tool_record]);
        assert!(parse_session_state_file(&path).unwrap().is_empty());
    }

    #[test]
    fn tolerates_malformed_jsonl_lines_and_invalid_utf8() {
        let valid = shutdown_event(
            "evt-good",
            "2026-05-02T15:27:09.013Z",
            json!({
                "claude-opus-4.6": {
                    "usage": {
                        "inputTokens": 100u64,
                        "outputTokens": 50u64,
                        "cacheReadTokens": 30u64,
                        "cacheWriteTokens": 10u64,
                        "reasoningTokens": 0u64
                    },
                    "requests": {"count": 1, "cost": 1}
                }
            }),
        );

        // Build file manually so we can splice in raw invalid UTF-8 bytes.
        let fixture = ccusage_test_support::Fixture::new();
        let session_dir = fixture.create_dir_all("bad-utf8-session");
        let path = session_dir.join("events.jsonl");
        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(b"not valid json\n");
        // Truncated multi-byte UTF-8 sequence — mirrors the real-world
        // 3c9f15b5-... file at byte 69879195 (`0xe2` start of 3-byte char
        // followed by EOF/newline).
        bytes.extend_from_slice(b"\xe2\n");
        bytes.extend_from_slice(valid.as_bytes());
        bytes.push(b'\n');
        fs::write(&path, &bytes).unwrap();

        let entries = parse_session_state_file(&path).unwrap();
        assert_eq!(entries.len(), 1, "expected the valid event to survive");
        assert_eq!(entries[0].model, "claude-opus-4.6");
        assert_eq!(entries[0].input_tokens, 100 - 30 - 10);
    }

    #[test]
    fn accepts_legacy_schema_without_reasoning_tokens() {
        // 1.0.40-era data: no `reasoningTokens` field at all.
        let (_dir, path) = write_jsonl(&[json!({
            "type": "session.shutdown",
            "id": "evt-legacy",
            "timestamp": "2026-02-28T09:52:27.352Z",
            "data": {"modelMetrics": {
                "claude-opus-4.6": {
                    "usage": {
                        "inputTokens": 1000u64,
                        "outputTokens": 100u64,
                        "cacheReadTokens": 500u64,
                        "cacheWriteTokens": 0u64
                    },
                    "requests": {"count": 1, "cost": 1}
                }
            }}
        })
        .to_string()]);
        let entries = parse_session_state_file(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].reasoning_output_tokens, 0);
        assert_eq!(entries[0].input_tokens, 500);
    }

    #[test]
    fn subtracts_both_cache_read_and_cache_write_from_input() {
        // Regression pin for the documented invariant that session-state
        // `inputTokens` is inclusive of both cache-read AND cache-write
        // counts (see `build_session_state_entry`'s `input_only` derivation).
        let (_dir, path) = write_jsonl(&[shutdown_event(
            "evt-cache",
            "2026-05-02T15:27:09.013Z",
            json!({
                "claude-opus-4.7": {
                    "usage": {
                        "inputTokens": 100u64,
                        "outputTokens": 5u64,
                        "cacheReadTokens": 60u64,
                        "cacheWriteTokens": 30u64,
                        "reasoningTokens": 0u64
                    },
                    "requests": {"count": 1, "cost": 1}
                }
            }),
        )]);
        let entries = parse_session_state_file(&path).unwrap();
        assert_eq!(
            entries[0].input_tokens, 10,
            "100 - 60 - 30 = 10 fresh input"
        );
        assert_eq!(entries[0].cache_read_tokens, 60);
        assert_eq!(entries[0].cache_creation_tokens, 30);
        assert_eq!(entries[0].output_tokens, 5);
    }

    #[test]
    fn ignores_token_details_field_even_when_present() {
        // Real-data divergence case: `usage.*` and `tokenDetails.*` are
        // independent snapshots in ~31% of rows. We must use `usage.*` only.
        let event = json!({
            "type": "session.shutdown",
            "id": "evt-td",
            "timestamp": "2026-05-02T15:27:09.013Z",
            "data": {
                "tokenDetails": {
                    "input": {"tokenCount": 999_999u64},
                    "output": {"tokenCount": 888_888u64},
                    "cache_read": {"tokenCount": 777_777u64},
                    "cache_write": {"tokenCount": 666_666u64}
                },
                "modelMetrics": {
                    "claude-opus-4.7-1m-internal": {
                        "usage": {
                            "inputTokens": 100u64,
                            "outputTokens": 5u64,
                            "cacheReadTokens": 60u64,
                            "cacheWriteTokens": 30u64,
                            "reasoningTokens": 0u64
                        },
                        "requests": {"count": 1, "cost": 1},
                        "tokenDetails": {
                            "input": {"tokenCount": 50u64},
                            "output": {"tokenCount": 25u64},
                            "cache_read": {"tokenCount": 555u64},
                            "cache_write": {"tokenCount": 333u64}
                        }
                    }
                }
            }
        })
        .to_string();
        let (_dir, path) = write_jsonl(&[event]);
        let entries = parse_session_state_file(&path).unwrap();
        assert_eq!(entries.len(), 1);
        // Values come from usage.*, NOT tokenDetails.*
        assert_eq!(entries[0].input_tokens, 10);
        assert_eq!(entries[0].output_tokens, 5);
        assert_eq!(entries[0].cache_read_tokens, 60);
        assert_eq!(entries[0].cache_creation_tokens, 30);
    }

    #[test]
    fn preserves_raw_model_name_on_entry() {
        // Source-fidelity check: the raw `claude-opus-4.7-1m-internal` name
        // must reach `entry.model` unchanged. Normalization happens at
        // pricing-lookup time in the loader, not here.
        let (_dir, path) = write_jsonl(&[shutdown_event(
            "evt-raw",
            "2026-05-02T15:27:09.013Z",
            json!({
                "claude-opus-4.7-1m-internal": {
                    "usage": {
                        "inputTokens": 100u64,
                        "outputTokens": 10u64,
                        "cacheReadTokens": 0u64,
                        "cacheWriteTokens": 0u64,
                        "reasoningTokens": 0u64
                    },
                    "requests": {"count": 1, "cost": 1}
                }
            }),
        )]);
        let entries = parse_session_state_file(&path).unwrap();
        assert_eq!(entries[0].model, "claude-opus-4.7-1m-internal");
    }

    #[test]
    fn gpt_reasoning_set_to_zero_to_avoid_double_count() {
        // OpenAI convention: reasoning IS a subset of output. Adding the
        // raw `reasoning_tokens` into the summary again via
        // `TokenCounts::total` (`types.rs`) would over-count cost ~40%.
        let (_dir, path) = write_jsonl(&[shutdown_event(
            "evt-gpt",
            "2026-05-02T15:27:09.013Z",
            json!({
                "gpt-5.4": {
                    "usage": {
                        "inputTokens": 1000u64,
                        "outputTokens": 6612u64,
                        "cacheReadTokens": 0u64,
                        "cacheWriteTokens": 0u64,
                        "reasoningTokens": 536u64
                    },
                    "requests": {"count": 1, "cost": 0}
                }
            }),
        )]);
        let entries = parse_session_state_file(&path).unwrap();
        assert_eq!(entries[0].output_tokens, 6612);
        assert_eq!(entries[0].reasoning_output_tokens, 0);
    }

    #[test]
    fn claude_reasoning_set_to_zero_to_avoid_double_count() {
        // Empirically verified: 48/48 local Claude rows with `reason > 0`
        // have `reason ≤ out` (subset). Anthropic billing folds extended
        // thinking into output_tokens.
        let (_dir, path) = write_jsonl(&[shutdown_event(
            "evt-claude",
            "2026-05-02T15:27:09.013Z",
            json!({
                "claude-opus-4.7-xhigh": {
                    "usage": {
                        "inputTokens": 1000u64,
                        "outputTokens": 163_851u64,
                        "cacheReadTokens": 0u64,
                        "cacheWriteTokens": 0u64,
                        "reasoningTokens": 19_382u64
                    },
                    "requests": {"count": 1, "cost": 0}
                }
            }),
        )]);
        let entries = parse_session_state_file(&path).unwrap();
        assert_eq!(entries[0].output_tokens, 163_851);
        assert_eq!(entries[0].reasoning_output_tokens, 0);
    }

    #[test]
    fn gemini_reasoning_kept_because_separate_from_output() {
        // Google convention: candidatesTokenCount and thoughtsTokenCount are
        // SEPARATE fields. Real-data sample had reason(6340) > out(3037).
        let (_dir, path) = write_jsonl(&[shutdown_event(
            "evt-gem",
            "2026-05-02T15:27:09.013Z",
            json!({
                "gemini-3.1-pro-preview": {
                    "usage": {
                        "inputTokens": 1000u64,
                        "outputTokens": 3037u64,
                        "cacheReadTokens": 0u64,
                        "cacheWriteTokens": 0u64,
                        "reasoningTokens": 6340u64
                    },
                    "requests": {"count": 1, "cost": 0}
                }
            }),
        )]);
        let entries = parse_session_state_file(&path).unwrap();
        assert_eq!(entries[0].output_tokens, 3037);
        assert_eq!(entries[0].reasoning_output_tokens, 6340);
    }

    #[test]
    fn unknown_provider_defaults_to_subset_semantics() {
        let (_dir, path) = write_jsonl(&[shutdown_event(
            "evt-unk",
            "2026-05-02T15:27:09.013Z",
            json!({
                "vendor-x-pro": {
                    "usage": {
                        "inputTokens": 100u64,
                        "outputTokens": 100u64,
                        "cacheReadTokens": 0u64,
                        "cacheWriteTokens": 0u64,
                        "reasoningTokens": 50u64
                    },
                    "requests": {"count": 1, "cost": 0}
                }
            }),
        )]);
        let entries = parse_session_state_file(&path).unwrap();
        assert_eq!(entries[0].reasoning_output_tokens, 0);
    }

    #[test]
    fn normalize_copilot_model_strips_internal_suffixes() {
        assert_eq!(
            normalize_copilot_model("claude-opus-4.7-1m-internal").as_ref(),
            "claude-opus-4-7"
        );
        assert_eq!(
            normalize_copilot_model("claude-opus-4.7-xhigh").as_ref(),
            "claude-opus-4-7"
        );
        assert_eq!(
            normalize_copilot_model("claude-opus-4.6-1m").as_ref(),
            "claude-opus-4-6"
        );
        assert_eq!(
            normalize_copilot_model("claude-opus-4.7-high").as_ref(),
            "claude-opus-4-7"
        );
        assert_eq!(
            normalize_copilot_model("claude-opus-4.7-internal").as_ref(),
            "claude-opus-4-7"
        );
    }

    #[test]
    fn normalize_copilot_model_converts_claude_dots_to_dashes() {
        assert_eq!(
            normalize_copilot_model("claude-opus-4.6").as_ref(),
            "claude-opus-4-6"
        );
        assert_eq!(
            normalize_copilot_model("claude-sonnet-4.5").as_ref(),
            "claude-sonnet-4-5"
        );
        assert_eq!(
            normalize_copilot_model("claude-haiku-4.5").as_ref(),
            "claude-haiku-4-5"
        );
    }

    #[test]
    fn normalize_copilot_model_preserves_gpt_dotted_names() {
        assert_eq!(normalize_copilot_model("gpt-5.4").as_ref(), "gpt-5.4");
        assert_eq!(
            normalize_copilot_model("gpt-5.4-mini").as_ref(),
            "gpt-5.4-mini"
        );
        assert_eq!(normalize_copilot_model("gpt-5.5").as_ref(), "gpt-5.5");
    }

    #[test]
    fn normalize_copilot_model_passes_through_unknown_models() {
        assert_eq!(normalize_copilot_model("goldeneye").as_ref(), "goldeneye");
        assert_eq!(
            normalize_copilot_model("vendor-x-pro").as_ref(),
            "vendor-x-pro"
        );
    }

    #[test]
    fn claude_dot_to_dash_preserves_multi_byte_utf8_characters() {
        // Regression pin: the helper used to iterate `value.as_bytes()`
        // and `out.push(byte as char)`, which corrupts any multi-byte
        // UTF-8 sequence (each non-ASCII byte gets reinterpreted as a
        // U+0000..U+00FF char). The `claude-` gate at the only call
        // site keeps real inputs ASCII today, but the helper itself
        // must be UTF-8-safe for forward-compat — model identifiers
        // could plausibly grow to include non-ASCII suffixes someday.
        //
        // Pin three properties:
        //   1. Mixed ASCII-digit dots still convert to dashes.
        //   2. Multi-byte UTF-8 characters round-trip unchanged.
        //   3. A `.` adjacent to a non-ASCII-digit char is NOT converted
        //      (the guard checks `is_ascii_digit()` on both sides).
        assert_eq!(
            super::claude_dot_to_dash("claude-opus-4.7"),
            "claude-opus-4-7"
        );
        assert_eq!(
            super::claude_dot_to_dash("claude-opus-4.7-café"),
            "claude-opus-4-7-café",
            "multi-byte UTF-8 (é = 2 bytes) must round-trip"
        );
        // A dot between a digit and a non-ASCII-digit char should NOT
        // be converted — `日` is digit-shaped but not ASCII digit.
        assert_eq!(
            super::claude_dot_to_dash("claude-opus-4.日"),
            "claude-opus-4.日"
        );
    }

    // ----- AI Credits tests (totalNanoAiu) -----

    fn shutdown_event_with_naiu(
        id: &str,
        ts: &str,
        model_metrics: serde_json::Value,
        aggregate_naiu: Option<u64>,
    ) -> String {
        let mut data = json!({"modelMetrics": model_metrics});
        if let Some(naiu) = aggregate_naiu {
            data.as_object_mut()
                .unwrap()
                .insert("totalNanoAiu".to_string(), json!(naiu));
        }
        json!({
            "type": "session.shutdown",
            "id": id,
            "timestamp": ts,
            "data": data,
        })
        .to_string()
    }

    #[test]
    fn parses_per_model_total_nano_aiu() {
        let (_dir, path) = write_jsonl(&[shutdown_event_with_naiu(
            "evt-credits",
            "2026-05-20T16:01:29.481Z",
            json!({
                "claude-opus-4.7-1m-internal": {
                    "usage": {
                        "inputTokens": 90_890_709u64,
                        "outputTokens": 172_544u64,
                        "cacheReadTokens": 86_415_643u64,
                        "cacheWriteTokens": 4_474_731u64,
                        "reasoningTokens": 0u64
                    },
                    "requests": {"count": 182, "cost": 4},
                    "totalNanoAiu": 7_549_016_525_000u64
                }
            }),
            None,
        )]);
        let entries = parse_session_state_file(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].nano_aiu, Some(7_549_016_525_000));
    }

    #[test]
    fn keeps_zero_token_row_when_per_model_total_nano_aiu_is_present() {
        // Real-data shape from 1135875b-… and 76dc438a-… events.jsonl:
        // zero tokens, zero requests, non-null per-model totalNanoAiu.
        let (_dir, path) = write_jsonl(&[shutdown_event_with_naiu(
            "evt-credit-only",
            "2026-05-20T16:01:29.481Z",
            json!({
                "claude-opus-4.7-1m-internal": {
                    "usage": {
                        "inputTokens": 0u64,
                        "outputTokens": 0u64,
                        "cacheReadTokens": 0u64,
                        "cacheWriteTokens": 0u64,
                        "reasoningTokens": 0u64
                    },
                    "requests": {"count": 0, "cost": 0},
                    "totalNanoAiu": 154_481_000_000u64
                }
            }),
            None,
        )]);
        let entries = parse_session_state_file(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].nano_aiu, Some(154_481_000_000));
        // Token fields are still zero — this is informational credit-only.
        assert_eq!(entries[0].input_tokens, 0);
        assert_eq!(entries[0].output_tokens, 0);
        // Dedup key is content-based for credit-only rows.
        assert!(
            entries[0].dedup_key.starts_with("credit-shutdown:"),
            "got dedup_key={}",
            entries[0].dedup_key
        );
    }

    #[test]
    fn dedupes_identical_credit_only_rows_within_same_session() {
        // Models the real-data duplicate pair in 1135875b-…/events.jsonl:
        // two distinct shutdown events, both zero-token, both with the same
        // per-model totalNanoAiu. After the parser these get the same
        // content-based dedup key and the downstream HashMap collapses them.
        let (_dir, path) = write_jsonl(&[
            shutdown_event_with_naiu(
                "evt-first",
                "2026-05-20T16:01:29.481Z",
                json!({
                    "claude-opus-4.7-1m-internal": {
                        "usage": {
                            "inputTokens": 0u64,
                            "outputTokens": 0u64,
                            "cacheReadTokens": 0u64,
                            "cacheWriteTokens": 0u64,
                            "reasoningTokens": 0u64
                        },
                        "requests": {"count": 0, "cost": 0},
                        "totalNanoAiu": 154_481_000_000u64
                    }
                }),
                None,
            ),
            shutdown_event_with_naiu(
                "evt-second",
                "2026-05-20T18:08:30.785Z",
                json!({
                    "claude-opus-4.7-1m-internal": {
                        "usage": {
                            "inputTokens": 0u64,
                            "outputTokens": 0u64,
                            "cacheReadTokens": 0u64,
                            "cacheWriteTokens": 0u64,
                            "reasoningTokens": 0u64
                        },
                        "requests": {"count": 0, "cost": 0},
                        "totalNanoAiu": 154_481_000_000u64
                    }
                }),
                None,
            ),
        ]);
        let entries = parse_session_state_file(&path).unwrap();
        // The parser emits both candidates (it doesn't dedup itself), but
        // both share the same content-based key for the loader-side HashMap.
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].dedup_key, entries[1].dedup_key);
        assert!(entries[0].dedup_key.starts_with("credit-shutdown:"));
    }

    #[test]
    fn token_bearing_rows_keep_event_id_dedup_key_not_content_based() {
        // Two distinct shutdowns producing the same number of tokens MUST
        // stay distinct (per-process snapshots). Only zero-token
        // credit-only rows use the content-based key.
        let (_dir, path) = write_jsonl(&[
            shutdown_event_with_naiu(
                "evt-A",
                "2026-05-20T16:01:29.481Z",
                json!({
                    "claude-opus-4.7-1m-internal": {
                        "usage": {
                            "inputTokens": 100u64,
                            "outputTokens": 50u64,
                            "cacheReadTokens": 0u64,
                            "cacheWriteTokens": 0u64,
                            "reasoningTokens": 0u64
                        },
                        "requests": {"count": 1, "cost": 0},
                        "totalNanoAiu": 1_000_000_000u64
                    }
                }),
                None,
            ),
            shutdown_event_with_naiu(
                "evt-B",
                "2026-05-20T18:08:30.785Z",
                json!({
                    "claude-opus-4.7-1m-internal": {
                        "usage": {
                            "inputTokens": 100u64,
                            "outputTokens": 50u64,
                            "cacheReadTokens": 0u64,
                            "cacheWriteTokens": 0u64,
                            "reasoningTokens": 0u64
                        },
                        "requests": {"count": 1, "cost": 0},
                        "totalNanoAiu": 1_000_000_000u64
                    }
                }),
                None,
            ),
        ]);
        let entries = parse_session_state_file(&path).unwrap();
        assert_eq!(entries.len(), 2);
        let mut keys: Vec<_> = entries.iter().map(|e| e.dedup_key.clone()).collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "shutdown:abc123-test-session:evt-A:claude-opus-4.7-1m-internal".to_string(),
                "shutdown:abc123-test-session:evt-B:claude-opus-4.7-1m-internal".to_string(),
            ]
        );
    }

    #[test]
    fn gemini_reasoning_only_rows_keep_event_id_dedup_key_not_content_based() {
        // Gemini rows preserve `reasoning_output_tokens` as real billable
        // usage (the only provider where the parser keeps that field
        // non-zero — see `build_session_state_entry`). The dedup-key
        // selector's `zero_token` check MUST include
        // `reasoning_output_tokens` so a Gemini row with ONLY reasoning
        // tokens (no input/output/cache, count=0, naiu set) is treated
        // as token-bearing — using the event-id dedup key, not the
        // content-based `credit-shutdown:` key.
        //
        // Without this guard, two distinct shutdowns in the same session
        // with the same Gemini model + same `totalNanoAiu` value would
        // produce identical `credit-shutdown:{session}:{model}:{naiu}`
        // keys and collapse in the loader's `HashMap` dedup — silently
        // under-counting reasoning tokens AND credits.
        //
        // Construct two shutdowns matching exactly that shape and pin
        // that BOTH survive with distinct event-id keys.
        let (_dir, path) = write_jsonl(&[
            shutdown_event_with_naiu(
                "evt-gem-A",
                "2026-05-20T16:01:29.481Z",
                json!({
                    "gemini-2.0-flash": {
                        "usage": {
                            "inputTokens": 0u64,
                            "outputTokens": 0u64,
                            "cacheReadTokens": 0u64,
                            "cacheWriteTokens": 0u64,
                            "reasoningTokens": 500u64
                        },
                        "requests": {"count": 0, "cost": 0},
                        "totalNanoAiu": 1_000_000_000u64
                    }
                }),
                None,
            ),
            shutdown_event_with_naiu(
                "evt-gem-B",
                "2026-05-20T18:08:30.785Z",
                json!({
                    "gemini-2.0-flash": {
                        "usage": {
                            "inputTokens": 0u64,
                            "outputTokens": 0u64,
                            "cacheReadTokens": 0u64,
                            "cacheWriteTokens": 0u64,
                            "reasoningTokens": 500u64
                        },
                        "requests": {"count": 0, "cost": 0},
                        "totalNanoAiu": 1_000_000_000u64
                    }
                }),
                None,
            ),
        ]);
        let entries = parse_session_state_file(&path).unwrap();
        // Two distinct shutdowns must produce two distinct entries.
        // Before the fix, both would have collapsed to the same
        // `credit-shutdown:{session}:gemini-2.0-flash:1000000000` key.
        assert_eq!(
            entries.len(),
            2,
            "two distinct Gemini reasoning-only shutdowns must NOT collapse \
             via the credit-only dedup path; got {entries:?}"
        );
        let mut keys: Vec<_> = entries.iter().map(|e| e.dedup_key.clone()).collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "shutdown:abc123-test-session:evt-gem-A:gemini-2.0-flash".to_string(),
                "shutdown:abc123-test-session:evt-gem-B:gemini-2.0-flash".to_string(),
            ],
            "Gemini reasoning-only rows must use event-id dedup keys, not \
             the content-based credit-only key"
        );
        // Sanity: reasoning tokens preserved on each entry (Gemini path).
        assert!(entries.iter().all(|e| e.reasoning_output_tokens == 500));
    }

    #[test]
    fn aggregate_only_total_nano_aiu_emits_synthetic_entry() {
        // Synthetic / forward-compat path: shutdown has data.totalNanoAiu but
        // none of the per-model rows carry it. Not observed in real local
        // data — kept as a guard against future schema variants.
        let (_dir, path) = write_jsonl(&[shutdown_event_with_naiu(
            "evt-agg",
            "2026-05-20T16:01:29.481Z",
            // Empty modelMetrics: only the aggregate carries credits.
            json!({}),
            Some(12_345_000_000),
        )]);
        let entries = parse_session_state_file(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].model.is_empty());
        assert_eq!(entries[0].nano_aiu, Some(12_345_000_000));
        assert!(entries[0].dedup_key.starts_with("credit-aggregate:"));
    }

    #[test]
    fn aggregate_credit_dedup_key_omits_event_id_to_collapse_duplicate_snapshots() {
        // Mirror the per-model credit-only path's content-based dedup
        // (`credit-shutdown:{session_id}:{model}:{naiu}` — observed to
        // collapse duplicate snapshot pairs in real session-state data):
        // if a future Copilot CLI ever ships aggregate-only credit
        // snapshots in the same duplicate-pair pattern as per-model rows,
        // they must collapse on `(session_id, naiu)` rather than survive
        // distinct on `event_id` and silently double-bill the same total.
        //
        // This pins the symmetry: two aggregate-only shutdowns with
        // distinct event ids but identical `totalNanoAiu` produce
        // identical dedup keys, so the loader's `HashMap` collapse keeps
        // exactly one row.
        let (_dir, path) = write_jsonl(&[
            shutdown_event_with_naiu(
                "evt-agg-first",
                "2026-05-20T16:01:29.481Z",
                json!({}),
                Some(7_500_000_000),
            ),
            shutdown_event_with_naiu(
                "evt-agg-second",
                "2026-05-20T16:01:29.482Z",
                json!({}),
                Some(7_500_000_000),
            ),
        ]);
        let entries = parse_session_state_file(&path).unwrap();
        assert_eq!(entries.len(), 2);
        // Both surface from the parser at this stage; the loader collapses
        // them via the `HashMap<dedup_key>` strategy. Pin the identical-key
        // property here (the per-file step) — the loader test
        // `loader_dedup_keeps_distinct_sessions_with_colliding_event_ids`
        // exercises the actual HashMap collapse downstream.
        assert_eq!(entries[0].dedup_key, entries[1].dedup_key);
        assert!(entries[0].dedup_key.starts_with("credit-aggregate:"));
        assert!(
            !entries[0].dedup_key.contains("evt-agg"),
            "aggregate-credit dedup key must NOT include event_id (silent \
             double-count if duplicate snapshots appear); got {}",
            entries[0].dedup_key
        );
    }

    #[test]
    fn aggregate_total_nano_aiu_is_suppressed_when_per_model_already_carries_it() {
        // When the aggregate IS present AND at least one per-model row also
        // has its own naiu, we trust the per-model rows and skip the
        // synthetic aggregate path (which would otherwise double-count).
        let (_dir, path) = write_jsonl(&[shutdown_event_with_naiu(
            "evt-both",
            "2026-05-20T16:01:29.481Z",
            json!({
                "claude-opus-4.7-1m-internal": {
                    "usage": {
                        "inputTokens": 100u64,
                        "outputTokens": 10u64,
                        "cacheReadTokens": 0u64,
                        "cacheWriteTokens": 0u64,
                        "reasoningTokens": 0u64
                    },
                    "requests": {"count": 1, "cost": 0},
                    "totalNanoAiu": 5_000_000_000u64
                }
            }),
            Some(5_000_000_000),
        )]);
        let entries = parse_session_state_file(&path).unwrap();
        // Per-model entry only — no synthetic aggregate row.
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].model.is_empty());
    }

    #[test]
    fn aggregate_credit_is_suppressed_when_per_model_has_premium_cost_only() {
        // Latent double-count guard: a future shape with `data.totalNanoAiu
        // > 0` AND per-model rows that carry `requests.cost > 0` but NO
        // per-model `totalNanoAiu` would otherwise bill the SAME usage
        // twice — once via Auto's premium ladder (`cost × $0.04` on the
        // per-model row) and again via Auto's AIU ladder (`naiu × $0.01`
        // on the synthetic). The narrow guard (`cost > 0` OR `naiu > 0`)
        // suppresses the synthetic exactly in this case.
        let (_dir, path) = write_jsonl(&[shutdown_event_with_naiu(
            "evt-premium-and-aggregate",
            "2026-05-20T16:01:29.481Z",
            json!({
                "claude-opus-4.7-1m-internal": {
                    "usage": {
                        "inputTokens": 100u64,
                        "outputTokens": 10u64,
                        "cacheReadTokens": 0u64,
                        "cacheWriteTokens": 0u64,
                        "reasoningTokens": 0u64
                    },
                    "requests": {"count": 1, "cost": 7.5}
                    // No per-model totalNanoAiu — only the aggregate has AIU.
                }
            }),
            Some(1_000_000_000),
        )]);
        let entries = parse_session_state_file(&path).unwrap();
        // Only the per-model row — synthetic suppressed to avoid
        // double-counting the same usage in AIU + premium currencies.
        assert_eq!(entries.len(), 1);
        assert!(
            !entries
                .iter()
                .any(|e| e.dedup_key.starts_with("credit-aggregate:"))
        );
        assert!(!entries[0].model.is_empty());
        assert_eq!(entries[0].premium_request_cost, Some(7.5));
        // Per-model row didn't somehow inherit aggregate AIU.
        assert_eq!(entries[0].nano_aiu, None);
    }

    #[test]
    fn aggregate_credit_guard_ignores_skipped_per_model_rows() {
        // Belt-and-suspenders: the aggregate-credit suppression guard
        // (`any_per_model_priced_billing`) must mirror the
        // `skip_metrics_row` filter applied above when iterating
        // model_metrics. Otherwise a per-model row that was skipped (no
        // entry emitted) could still suppress the synthetic
        // aggregate-credit row, silently dropping the session AIU.
        //
        // Construct a row that:
        //   * Has `cost > 0` (would suppress the synthetic under the
        //     old guard that read raw metrics).
        //   * Has all 5 tokens = 0 AND `requests.count == 0` AND no
        //     per-model AIU (so `skip_metrics_row` discards it — no
        //     entry emitted).
        // This shape is unreachable on observed data because
        // `requests.cost = count × multiplier` makes
        // `count == 0 ⇒ cost == 0`. But the guard's correctness
        // shouldn't depend on that schema invariant — if the CLI ever
        // emits this shape, the aggregate must survive.
        //
        // Expected: the per-model row is skipped (not in entries), AND
        // the synthetic aggregate-credit row IS emitted (the only
        // priced signal for the session).
        let (_dir, path) = write_jsonl(&[shutdown_event_with_naiu(
            "evt-skipped-cost-with-aggregate",
            "2026-05-20T16:01:29.481Z",
            json!({
                "claude-opus-4.7-1m-internal": {
                    "usage": {
                        "inputTokens": 0u64,
                        "outputTokens": 0u64,
                        "cacheReadTokens": 0u64,
                        "cacheWriteTokens": 0u64,
                        "reasoningTokens": 0u64
                    },
                    // count == 0 but cost > 0 — unreachable today but
                    // exercises the guard's robustness against the
                    // `count × multiplier` invariant breaking.
                    "requests": {"count": 0, "cost": 7.5}
                }
            }),
            Some(1_000_000_000),
        )]);
        let entries = parse_session_state_file(&path).unwrap();
        // The per-model row is skipped — no entry for it.
        assert!(
            !entries.iter().any(|e| !e.model.is_empty()),
            "skipped row (all-zero tokens + count=0 + no AIU) must NOT \
             produce a per-model entry; got {entries:?}"
        );
        // The synthetic aggregate-credit MUST survive (the only priced
        // signal). Before the fix that iterated raw metrics, the
        // skipped row's `cost > 0` would have suppressed this and
        // silently dropped the session AIU.
        let aggregate = entries
            .iter()
            .find(|e| e.dedup_key.starts_with("credit-aggregate:"))
            .expect("synthetic aggregate-credit must survive a skipped-row with cost > 0");
        assert!(aggregate.model.is_empty());
        assert_eq!(aggregate.nano_aiu, Some(1_000_000_000));
    }

    #[test]
    fn aggregate_credit_is_emitted_when_per_model_has_only_tokens() {
        // Forward-compat preservation: when per-model rows carry tokens but
        // no priced billing (no `requests.cost`, no per-model AIU), the
        // aggregate AIU is the only **source-provided** priced signal for
        // the session. Emit the synthetic so the AIU isn't silently lost.
        //
        // Under `--mode auto`, the per-model row is then independently
        // token-priced via LiteLLM (since `premium_cost == None` ⇒
        // `has_premium_data == false`, so the Auto dispatch falls through
        // to the token-priced arm) AND the synthetic contributes
        // `aiu × $0.01` — i.e. the two stack. The stack is accepted
        // because the alternative (dropping the aggregate AIU) would
        // silently under-count a real charge; see the long comment at
        // `parser.rs::parse_session_state_file` for the full rationale.
        // The shape is unobserved in the local 200-file corpus.
        //
        // NOTE: `requests.cost` is OMITTED — not set to 0 — because
        // `cost: Some(0.0)` makes `has_premium_data == true` on the loader
        // side and routes the per-model row to the premium arm ($0 bill),
        // which would silently contradict the "billed via LiteLLM" claim
        // above. Pinning the genuinely-absent shape keeps the test honest.
        let (_dir, path) = write_jsonl(&[shutdown_event_with_naiu(
            "evt-tokens-and-aggregate",
            "2026-05-20T16:01:29.481Z",
            json!({
                "claude-opus-4.7-1m-internal": {
                    "usage": {
                        "inputTokens": 100u64,
                        "outputTokens": 10u64,
                        "cacheReadTokens": 0u64,
                        "cacheWriteTokens": 0u64,
                        "reasoningTokens": 0u64
                    },
                    "requests": {"count": 1}
                    // No per-model totalNanoAiu and `requests.cost` omitted.
                }
            }),
            Some(1_000_000_000),
        )]);
        let entries = parse_session_state_file(&path).unwrap();
        // Two entries: the per-model row plus the synthetic aggregate.
        assert_eq!(entries.len(), 2);
        let per_model = entries
            .iter()
            .find(|e| !e.model.is_empty())
            .expect("per-model entry should be emitted");
        // Locks in that the per-model row carries NO priced billing — so
        // Auto really does fall through to LiteLLM token pricing as the
        // comment claims, instead of silently hitting the premium-$0 arm.
        assert_eq!(per_model.premium_request_cost, None);
        assert_eq!(per_model.nano_aiu, None);
        let aggregate = entries
            .iter()
            .find(|e| e.dedup_key.starts_with("credit-aggregate:"))
            .expect("synthetic aggregate-credit entry should be emitted");
        assert!(aggregate.model.is_empty());
        assert_eq!(aggregate.nano_aiu, Some(1_000_000_000));
    }

    #[test]
    fn aggregate_credit_is_emitted_when_per_model_naiu_is_explicit_zero() {
        // Edge improvement over the previous `Option::is_some()` guard:
        // a per-model row with `totalNanoAiu = Some(0)` and nothing else
        // signals "this model contributed zero AIU." Under the old guard
        // that `Some(0)` would have suppressed the synthetic and silently
        // dropped the aggregate AIU. Now the synthetic is emitted because
        // the per-model row carries no PRICED billing.
        let (_dir, path) = write_jsonl(&[shutdown_event_with_naiu(
            "evt-zero-aiu-and-aggregate",
            "2026-05-20T16:01:29.481Z",
            json!({
                "claude-opus-4.7-1m-internal": {
                    "usage": {
                        "inputTokens": 0u64,
                        "outputTokens": 0u64,
                        "cacheReadTokens": 0u64,
                        "cacheWriteTokens": 0u64,
                        "reasoningTokens": 0u64
                    },
                    "requests": {"count": 0, "cost": 0},
                    "totalNanoAiu": 0u64
                }
            }),
            Some(1_000_000_000),
        )]);
        let entries = parse_session_state_file(&path).unwrap();
        // The per-model row is skipped by `skip_metrics_row` (all-zero); the
        // synthetic aggregate captures the session AIU.
        assert_eq!(entries.len(), 1);
        assert!(entries[0].dedup_key.starts_with("credit-aggregate:"));
        assert_eq!(entries[0].nano_aiu, Some(1_000_000_000));
    }

    #[test]
    fn legacy_pre_aiu_schema_yields_none_nano_aiu() {
        // 1.0.40-era data: no totalNanoAiu anywhere. Credits remain None,
        // not 0 — the caller can distinguish "no credit data" from "zero
        // credits".
        let (_dir, path) = write_jsonl(&[shutdown_event(
            "evt-legacy",
            "2026-02-28T09:52:27.352Z",
            json!({
                "claude-opus-4.6": {
                    "usage": {
                        "inputTokens": 1000u64,
                        "outputTokens": 100u64,
                        "cacheReadTokens": 500u64,
                        "cacheWriteTokens": 0u64,
                        "reasoningTokens": 0u64
                    },
                    "requests": {"count": 1, "cost": 1}
                }
            }),
        )]);
        let entries = parse_session_state_file(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].nano_aiu, None);
    }
}
