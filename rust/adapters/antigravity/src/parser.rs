use std::sync::Arc;

use jiff::tz::TimeZone as JiffTimeZone;

use crate::{
    LoadedEntry, PricingMap, TimestampMs, TokenUsageRaw, UsageEntry, UsageMessage,
    calculate_cost_for_usage, cli::CostMode, format_date_tz, missing_pricing_model_for_candidates,
    proto::ModelUsage,
};

/// One model invocation, after the loader has paired it with a model name.
pub(super) struct UsageRecord {
    /// The conversation database this came from, used as the session id.
    pub(super) conversation_id: Arc<str>,
    /// `created_at` of the owning step.
    pub(super) timestamp: TimestampMs,
    /// Token counts as recorded by Antigravity.
    pub(super) usage: ModelUsage,
    /// `response_model`, when a `gen_metadata` row named this invocation.
    pub(super) model: Option<String>,
}

/// Provider prefixes to try alongside the bare model name.
///
/// Antigravity routes to more than one provider, and LiteLLM keys the same model
/// under both a bare name and provider-qualified ones. Narrowing the prefixes by
/// model family keeps a Gemini name from accidentally matching an Anthropic entry.
fn provider_prefixes(model: &str) -> &'static [&'static str] {
    let model = model.to_ascii_lowercase();
    if model.starts_with("gemini") || model.starts_with("gemma") {
        return &["gemini", "vertex_ai", "google", "openrouter/google"];
    }
    if model.starts_with("claude") {
        return &["anthropic", "vertex_ai", "bedrock", "openrouter/anthropic"];
    }
    if model.starts_with("gpt") || model.starts_with("o1") || model.starts_with("o3") {
        return &["openai", "azure", "openrouter/openai"];
    }
    &[]
}

/// Candidate LiteLLM pricing keys for a model, most specific match first.
fn model_candidates(model: &str) -> Vec<String> {
    let mut candidates = vec![model.to_string()];
    candidates.extend(
        provider_prefixes(model)
            .iter()
            .map(|prefix| format!("{prefix}/{model}")),
    );
    candidates.dedup();
    candidates
}

/// Placeholder name for an invocation `gen_metadata` never named.
///
/// Antigravity records the model as a numeric id and its shipped descriptor only
/// names those ids as `MODEL_PLACEHOLDER_M<n>`, so background calls that get no
/// `gen_metadata` row cannot be resolved to a real name. Their tokens are still
/// real, so they are reported under a traceable placeholder and left to surface as
/// missing pricing rather than being dropped or silently priced as something else.
fn placeholder_model(model_id: u64) -> String {
    format!("antigravity-model-{model_id}")
}

/// Map Antigravity's token fields onto ccusage's usage shape.
///
/// `input_tokens` and `cache_read_tokens` are disjoint in the source, matching
/// ccusage's own split, so neither needs adjusting. `output_tokens` already
/// includes thinking tokens and must not have them added again.
fn token_usage(usage: &ModelUsage) -> TokenUsageRaw {
    TokenUsageRaw {
        input_tokens: usage.input_tokens,
        output_tokens: usage.total_output_tokens(),
        cache_creation_input_tokens: usage.cache_write_tokens,
        cache_read_input_tokens: usage.cache_read_tokens,
        speed: None,
        cache_creation: None,
    }
}

/// Turn a collected invocation into a report entry.
pub(super) fn record_to_entry(
    record: UsageRecord,
    tz: Option<&JiffTimeZone>,
    mode: CostMode,
    pricing: &PricingMap,
) -> Option<LoadedEntry> {
    if !record.usage.has_tokens() {
        return None;
    }
    let named = record.model.is_some();
    let model = record
        .model
        .unwrap_or_else(|| placeholder_model(record.usage.model_id));
    let usage = token_usage(&record.usage);
    let candidates = model_candidates(&model);

    let cost = match mode {
        // Antigravity leaves `model_cost`, `credit_cost` and `consumed_credits`
        // unset, so there is no precomputed cost to display.
        CostMode::Display => 0.0,
        CostMode::Auto | CostMode::Calculate => candidates
            .iter()
            .find(|candidate| pricing.find(candidate).is_some())
            .map_or(0.0, |candidate| {
                calculate_cost_for_usage(
                    Some(candidate),
                    usage,
                    None,
                    CostMode::Calculate,
                    Some(pricing),
                )
            }),
    };
    let missing_pricing_model = if mode == CostMode::Display {
        None
    } else if named {
        missing_pricing_model_for_candidates(
            &model,
            candidates,
            crate::total_usage_tokens(usage),
            Some(pricing),
        )
    } else {
        // A placeholder name can never match a pricing entry, so report it
        // directly instead of paying for the lookup.
        Some(model.clone())
    };

    let data = UsageEntry {
        session_id: Some(record.conversation_id.to_string()),
        timestamp: crate::format_rfc3339_millis(record.timestamp),
        version: None,
        message: UsageMessage {
            usage,
            model: Some(model.clone()),
            id: record.usage.response_id.clone(),
        },
        cost_usd: None,
        request_id: record.usage.response_id.clone(),
        is_api_error_message: None,
        is_sidechain: None,
    };

    Some(LoadedEntry {
        date: format_date_tz(record.timestamp, tz),
        timestamp: record.timestamp,
        project: Arc::from("antigravity"),
        session_id: record.conversation_id,
        project_path: Arc::from("Antigravity"),
        cost,
        credits: None,
        model: Some(model),
        usage_limit_reset_time: None,
        missing_pricing_model,
        extra_total_tokens: 0,
        message_count: None,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u64, output: u64, cache_read: u64) -> ModelUsage {
        ModelUsage {
            model_id: 1071,
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            response_id: Some("response-a".to_string()),
            ..ModelUsage::default()
        }
    }

    fn record(model: Option<&str>, usage: ModelUsage) -> UsageRecord {
        UsageRecord {
            conversation_id: Arc::from("conversation-a"),
            timestamp: TimestampMs::from_millis(1_785_328_986_355),
            usage,
            model: model.map(str::to_string),
        }
    }

    #[test]
    fn maps_cache_reads_to_their_own_bucket_rather_than_input() {
        let pricing = PricingMap::load_embedded();
        let entry = record_to_entry(
            record(Some("gemini-3.6-flash"), usage(4050, 375, 16275)),
            Some(&jiff::tz::TimeZone::UTC),
            CostMode::Auto,
            &pricing,
        )
        .unwrap();

        assert_eq!(entry.data.message.usage.input_tokens, 4050);
        assert_eq!(entry.data.message.usage.cache_read_input_tokens, 16275);
        assert_eq!(entry.data.message.usage.output_tokens, 375);
        assert_eq!(entry.date, "2026-07-29");
        assert_eq!(entry.session_id.as_ref(), "conversation-a");
    }

    #[test]
    fn prices_a_cache_read_below_the_input_rate() {
        // Antigravity also routes to Anthropic models, which the offline snapshot
        // embeds; Gemini rates are only available from a runtime pricing fetch.
        let pricing = PricingMap::load_embedded();
        let tz = jiff::tz::TimeZone::UTC;
        let cached = record_to_entry(
            record(Some("claude-sonnet-4-5"), usage(4050, 375, 16275)),
            Some(&tz),
            CostMode::Auto,
            &pricing,
        )
        .unwrap();
        let uncached = record_to_entry(
            record(Some("claude-sonnet-4-5"), usage(4050 + 16275, 375, 0)),
            Some(&tz),
            CostMode::Auto,
            &pricing,
        )
        .unwrap();

        assert!(cached.cost > 0.0);
        assert!(
            cached.cost < uncached.cost,
            "cached {} should undercut uncached {}",
            cached.cost,
            uncached.cost
        );
    }

    #[test]
    fn reports_an_unnamed_model_as_missing_pricing_instead_of_dropping_it() {
        let pricing = PricingMap::load_embedded();
        let entry = record_to_entry(
            record(
                None,
                ModelUsage {
                    model_id: 1050,
                    input_tokens: 93,
                    output_tokens: 3,
                    response_id: Some("response-b".to_string()),
                    ..ModelUsage::default()
                },
            ),
            Some(&jiff::tz::TimeZone::UTC),
            CostMode::Auto,
            &pricing,
        )
        .unwrap();

        assert_eq!(entry.model.as_deref(), Some("antigravity-model-1050"));
        assert_eq!(entry.data.message.usage.input_tokens, 93);
        assert_eq!(
            entry.missing_pricing_model.as_deref(),
            Some("antigravity-model-1050")
        );
        assert_eq!(entry.cost, 0.0);
    }

    #[test]
    fn reports_no_cost_in_display_mode_because_the_source_records_none() {
        let pricing = PricingMap::load_embedded();
        let entry = record_to_entry(
            record(Some("gemini-3.6-flash"), usage(4050, 375, 16275)),
            Some(&jiff::tz::TimeZone::UTC),
            CostMode::Display,
            &pricing,
        )
        .unwrap();

        assert_eq!(entry.cost, 0.0);
        assert_eq!(entry.missing_pricing_model, None);
    }

    #[test]
    fn skips_a_record_with_no_tokens() {
        let pricing = PricingMap::load_embedded();

        assert!(
            record_to_entry(
                record(Some("gemini-3.6-flash"), usage(0, 0, 0)),
                Some(&jiff::tz::TimeZone::UTC),
                CostMode::Auto,
                &pricing,
            )
            .is_none()
        );
    }

    #[test]
    fn narrows_pricing_candidates_by_model_family() {
        assert_eq!(
            model_candidates("gemini-3.6-flash"),
            vec![
                "gemini-3.6-flash",
                "gemini/gemini-3.6-flash",
                "vertex_ai/gemini-3.6-flash",
                "google/gemini-3.6-flash",
                "openrouter/google/gemini-3.6-flash",
            ]
        );
        assert_eq!(
            model_candidates("claude-sonnet-4-5"),
            vec![
                "claude-sonnet-4-5",
                "anthropic/claude-sonnet-4-5",
                "vertex_ai/claude-sonnet-4-5",
                "bedrock/claude-sonnet-4-5",
                "openrouter/anthropic/claude-sonnet-4-5",
            ]
        );
        assert_eq!(model_candidates("mystery-model"), vec!["mystery-model"]);
    }

    #[test]
    fn resolves_embedded_pricing_through_a_provider_qualified_candidate() {
        let pricing = PricingMap::load_embedded();

        assert!(
            model_candidates("claude-sonnet-4-5")
                .iter()
                .any(|candidate| pricing.find(candidate).is_some()),
            "no embedded pricing for claude-sonnet-4-5"
        );
    }
}
