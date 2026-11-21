use ccusage_adapter_common::billing::{host_provider, price_entry_for_provider};
use ccusage_core::cost::PriceTable;
use ccusage_core::{
    cost::{EntryPricing, UsageGranularity},
    pricing::MatchKind,
};

use crate::{TimestampMs, TokenUsageRaw};

use super::DEFAULT_MODEL;

const PROVIDER_PREFIXES: [&str; 4] = ["google", "gemini", "vertex_ai", "openrouter/google"];
pub(super) const API_PROVIDER_GOOGLE_VERTEX: u64 = 3;
pub(super) const API_PROVIDER_GOOGLE_GEMINI: u64 = 24;
const API_PROVIDER_GOOGLE_EVERGREEN: u64 = 30;

fn model_name_from_id(model_id: u64) -> String {
    match model_id {
        246 => "gemini-2.5-pro".to_string(),
        312 => "gemini-2.5-flash".to_string(),
        313 | 329 => "gemini-2.5-flash-thinking".to_string(),
        330 => "gemini-2.5-flash-lite".to_string(),
        281 | 282 => "claude-sonnet-4".to_string(),
        290 | 291 => "claude-opus-4".to_string(),
        333 | 334 => "claude-4.5-sonnet".to_string(),
        340 | 341 => "claude-4.5-haiku".to_string(),
        342 => "model_openai_gpt_oss_120b_medium".to_string(),
        1_000.. => format!("{UNRESOLVED_PLACEHOLDER_PREFIX}{}", model_id - 1_000),
        _ => format!("{UNRESOLVED_ID_PREFIX}{model_id}"),
    }
}

pub(super) fn model_from_id(model_id: u64) -> Option<String> {
    normalize_antigravity_model(&model_name_from_id(model_id))
}

pub(super) fn row_model(model: Option<&str>, model_id: Option<u64>) -> Option<String> {
    model
        .filter(|model| !model.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| model_id.and_then(model_from_id))
}

const UNRESOLVED_PLACEHOLDER_PREFIX: &str = "model_placeholder_m";
const UNRESOLVED_ID_PREFIX: &str = "antigravity-model-";

#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ModelResolution {
    Default,
    Placeholder,
    Resolved,
}

pub(super) fn model_resolution(model: &str) -> ModelResolution {
    if model == DEFAULT_MODEL {
        ModelResolution::Default
    } else if model.starts_with(UNRESOLVED_PLACEHOLDER_PREFIX)
        || model.starts_with(UNRESOLVED_ID_PREFIX)
    {
        ModelResolution::Placeholder
    } else {
        ModelResolution::Resolved
    }
}

pub(super) fn is_resolved_model(model: &str) -> bool {
    model_resolution(model) == ModelResolution::Resolved
}

pub(super) fn normalize_antigravity_model(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if let Some(model) = gemini_flash_effort_variant(&lower) {
        return Some(model);
    }
    let base = lower
        .find('(')
        .map_or(lower.as_str(), |index| lower[..index].trim());
    let normalized = match base {
        "gemini 3.8 flash" | "gemini 3.8 flash thinking" => "gemini-3.8-flash",
        "gemini 3.7 flash" | "gemini 3.7 flash thinking" => "gemini-3.7-flash",
        "gemini 3.7 pro" | "gemini 3.7 pro thinking" => "gemini-3.7-pro",
        "gemini 3.6 flash" | "gemini 3 flash" => "gemini-3.6-flash",
        "gemini 3.6 pro" => "gemini-3.6-pro",
        "gemini 3 pro" | "gemini 3 pro thinking" => "gemini-3-pro",
        "gemini 2.5 flash" => "gemini-2.5-flash",
        "gemini 2.5 pro" => "gemini-2.5-pro",
        "gemini 2.0 flash" | "gemini 2 flash" => "gemini-2.0-flash",
        "gemini 2.0 pro" => "gemini-2.0-pro",
        "gemini 1.5 flash" => "gemini-1.5-flash",
        "gemini 1.5 pro" => "gemini-1.5-pro",
        "model_placeholder_m318" => "gemini-3.8-flash-high",
        "model_placeholder_m319" => "gemini-3.8-flash-medium",
        "model_placeholder_m320" => "gemini-3.8-flash-low",
        "model_placeholder_m298" => "gemini-3.7-flash-high",
        "model_placeholder_m299" => "gemini-3.7-flash-medium",
        "model_placeholder_m300" => "gemini-3.7-flash-low",
        "model_placeholder_m71" => "gemini-3.6-flash-high",
        "model_placeholder_m72" => "gemini-3.6-flash-medium",
        "model_placeholder_m73" => "gemini-3.6-flash-low",
        "model_placeholder_m417" | "model_placeholder_m418" | "model_placeholder_m419" => {
            "gemini-4-argon"
        }
        "model_placeholder_m402" => "claude-opus-5-5",
        "model_placeholder_m50" => "gemini-3.1-flash-lite",
        "model_placeholder_m26" => "claude-opus-4-6",
        "model_placeholder_m35" => "claude-sonnet-4-6",
        "model_placeholder_m36" | "model_placeholder_m37" | "model_placeholder_m16" => {
            "gemini-3.1-pro"
        }
        "model_placeholder_m18" | "model_placeholder_m84" | "model_placeholder_m47" => {
            "gemini-3-flash-preview"
        }
        "model_placeholder_m132" | "model_placeholder_m133" => "gemini-3.5-flash-high",
        "model_placeholder_m187" => "gemini-3.5-flash-extra-low",
        "model_placeholder_m20" => "gemini-3.5-flash-medium",
        "model_openai_gpt_oss_120b_medium" => "gpt-oss-120b-medium",
        "gemini-pro-default" | "gemini-pro-agent" => "gemini-3.1-pro",
        "gemini-3-flash-agent"
        | "gemini-3-flash-agent-a"
        | "gemini-3-flash-agent-b"
        | "gemini-3-flash-a"
        | "gemini-3-flash-b" => "gemini-3.5-flash-high",
        "gemini-3-flash-c" | "gemini-3-flash" => "gemini-3-flash-preview",
        "gemini-3.5-flash-low" => "gemini-3.5-flash-medium",
        "gemini-3.1-pro-high" | "gemini-3.1-pro-low" => "gemini-3.1-pro",
        "gemini-3-pro-high" | "gemini-3-pro-low" => "gemini-3-pro",
        "claude 3.7 sonnet" | "claude 3.7 sonnet thinking" => "claude-3-7-sonnet",
        "claude 3.5 sonnet" => "claude-3-5-sonnet",
        "claude 3.5 haiku" => "claude-3-5-haiku",
        "claude 3 opus" => "claude-3-opus",
        _ => {
            let converted = base.replace(' ', "-");
            if converted.starts_with("gemini-")
                || converted.starts_with("claude-")
                || converted.starts_with("gpt-")
            {
                return Some(converted);
            }
            return Some(trimmed.to_string());
        }
    };
    Some(normalized.to_string())
}

pub(super) fn price_antigravity(
    model: &str,
    logged_model: Option<&str>,
    provider: Option<u64>,
    usage: TokenUsageRaw,
    timestamp: TimestampMs,
    pricing: PriceTable<'_>,
) -> (f64, Option<String>) {
    let exact = gemini_flash_pricing_alias(model).is_some();
    let kind = if exact {
        MatchKind::Exact
    } else {
        MatchKind::Fuzzy
    };
    // NOTE(2026-11-20, #2236): The resolver treats `-thinking` as price-neutral only for the families in `FAMILY_PRICE_NEUTRAL_SUFFIX_WORDS` (`ccusage-core`). A `gpt-…` or passed-through id keeps the suffix, so its base id needs a fallback. The stripped base is therefore tried only after every candidate that names the full id.
    let thinking_base = model
        .strip_suffix("-thinking")
        .into_iter()
        .flat_map(|base| model_candidates(base, provider))
        .map(|name| (name, MatchKind::Fallback));
    let candidates: Vec<_> = model_candidates(model, provider)
        .into_iter()
        .flat_map(|name| {
            let alias = crate::model_aliases::resolve_model_name(&name).into_owned();
            let extra = (exact && alias != name).then_some((alias, MatchKind::Exact));
            std::iter::once((name, kind)).chain(extra)
        })
        .chain(thinking_base)
        .collect();
    let candidates: Vec<_> = logged_model
        .map(|name| (name, MatchKind::Exact))
        .into_iter()
        .chain(candidates.iter().map(|(name, kind)| (name.as_str(), *kind)))
        .collect();
    let priced = price_entry_for_provider(
        EntryPricing {
            granularity: UsageGranularity::Request,
            usage,
            reasoning_tokens: 0,
            residual_tokens: 0,
            reported_model: Some(model),
            candidates: &candidates,
            stored_cost: None,
            timestamp: Some(timestamp),
        },
        recorded_host(provider),
        pricing,
    );
    (
        priced.cost,
        priced
            .missing_pricing
            .then(|| ccusage_core::model_aliases::resolve_model_name(model).into_owned()),
    )
}

const GEMINI_FLASH_EFFORT_FAMILIES: [&str; 3] =
    ["gemini-3.8-flash", "gemini-3.7-flash", "gemini-3.6-flash"];
const GEMINI_FLASH_EFFORTS: [&str; 3] = ["high", "medium", "low"];

fn gemini_flash_pricing_alias(model: &str) -> Option<&'static str> {
    let (family, effort) = model.rsplit_once('-')?;
    if !GEMINI_FLASH_EFFORTS.contains(&effort) {
        return None;
    }
    GEMINI_FLASH_EFFORT_FAMILIES
        .into_iter()
        .find(|candidate| *candidate == family)
}

fn gemini_flash_effort_variant(display_name: &str) -> Option<String> {
    let (base, rest) = display_name.split_once(" (")?;
    let effort = rest.strip_suffix(')')?;
    if base.contains('-') {
        return None;
    }
    let variant = format!("{}-{effort}", base.replace(' ', "-"));
    gemini_flash_pricing_alias(&variant)
        .is_some()
        .then_some(variant)
}

// NOTE(2026-11-21, #2226): Antigravity records the serving API as a number, not a name. A Vertex request bills through the same shared host route as an agent that logs `vertex_ai`, so its Vertex listing answers first and a fallback to Google's row takes Google's dated schedule; other requests keep the unrouted pricing.
fn recorded_host(provider: Option<u64>) -> Option<&'static str> {
    provider
        .filter(|provider| *provider == API_PROVIDER_GOOGLE_VERTEX)
        .and_then(|_| host_provider("vertex"))
}

pub(super) fn model_candidates(model: &str, provider: Option<u64>) -> Vec<String> {
    let mut models = vec![model];
    models.extend(gemini_flash_pricing_alias(model));

    let vertex = recorded_host(provider);
    let (host, google): (&[&str], &[&str]) = match provider {
        Some(API_PROVIDER_GOOGLE_VERTEX) => (vertex.as_slice(), &[]),
        Some(API_PROVIDER_GOOGLE_GEMINI | API_PROVIDER_GOOGLE_EVERGREEN) => {
            (&[], &PROVIDER_PREFIXES)
        }
        _ => (&[], &[]),
    };
    let mut candidates = models
        .into_iter()
        .flat_map(|model| {
            let prefixed = move |prefix: &&str| format!("{prefix}/{model}");
            host.iter()
                .map(prefixed)
                .chain(std::iter::once(model.to_string()))
                .chain(google.iter().map(prefixed))
        })
        .collect::<Vec<_>>();
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|candidate| seen.insert(candidate.clone()));
    candidates
}
