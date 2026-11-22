use ccusage_core::PricingMap;

use compact_str::CompactString;

use crate::{
    CodexGroup, CodexUsageBucket, Result,
    cli::{AgentReportKind, IsoDate, WeekDay},
    fast::FxHashMap,
};

use super::{
    dedupe::CodexAttribution,
    intern::{StringId, StringInterner},
};

#[derive(Default)]
pub(super) struct CodexModels {
    logged: FxHashMap<CompactString, StringId>,
    pub(super) resolved: StringInterner<CodexModel>,
}

impl CodexModels {
    pub(super) fn intern(
        &mut self,
        logged: &str,
        pricing: Option<&PricingMap>,
    ) -> Result<StringId> {
        if let Some(id) = self.logged.get(logged) {
            return Ok(*id);
        }

        let (model, price) = match pricing {
            Some(table) => {
                let identity = table.pricing_identity(logged);
                (identity.name, identity.handle)
            }
            None => (crate::model_aliases::resolve_model_name(logged), None),
        };
        let id = self.resolved.intern(&model, || CodexModel {
            name: CompactString::new(model.as_ref()),
            long_context_threshold: price.as_ref().map_or(
                crate::pricing::DEFAULT_LONG_CONTEXT_THRESHOLD_TOKENS,
                |handle| handle.with_rates_at(None, |entry| entry.long_context_split_threshold()),
            ),
            time_dependent: price.is_some_and(|handle| handle.is_time_dependent()),
        })?;
        self.logged.insert(CompactString::new(logged), id);
        Ok(id)
    }
}

pub(super) struct CodexModel {
    name: CompactString,
    long_context_threshold: u64,
    time_dependent: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum CodexPeriod {
    Date(IsoDate),
    Session(StringId),
}

impl CodexPeriod {
    pub(super) fn label(
        self,
        kind: AgentReportKind,
        sessions: &StringInterner<CompactString>,
    ) -> String {
        match self {
            Self::Date(date) if matches!(kind, AgentReportKind::Monthly) => date.format_month(),
            Self::Date(date) => date.to_string(),
            Self::Session(session) => sessions.values[session.index()].to_string(),
        }
    }
}

pub(super) fn codex_period_for(
    timestamp: crate::TimestampMs,
    session: StringId,
    kind: AgentReportKind,
    window: &ccusage_core::ReportWindow,
) -> Option<CodexPeriod> {
    let date = ccusage_core::date_at(timestamp, window.timezone());
    if !window.contains_day(date) {
        return None;
    }
    Some(match kind {
        AgentReportKind::Daily => CodexPeriod::Date(date),
        AgentReportKind::Weekly => CodexPeriod::Date(date.week_start(WeekDay::Monday)),
        AgentReportKind::Monthly => CodexPeriod::Date(date.month_start()),
        AgentReportKind::Session => CodexPeriod::Session(session),
    })
}

pub(super) fn accumulate_codex_usage_into_group(
    group: &mut CodexGroup,
    event: crate::CodexRawUsage,
    model: &CodexModel,
    timestamp: crate::TimestampMs,
    attribution: &CodexAttribution,
) {
    group.input_tokens = group.input_tokens.saturating_add(event.input_tokens);
    group.cached_input_tokens = group
        .cached_input_tokens
        .saturating_add(event.cached_input_tokens);
    group.cache_creation_tokens = group
        .cache_creation_tokens
        .saturating_add(event.cache_creation_tokens);
    group.output_tokens = group.output_tokens.saturating_add(event.output_tokens);
    group.reasoning_output_tokens = group
        .reasoning_output_tokens
        .saturating_add(event.reasoning_output_tokens);
    group.total_tokens = group.total_tokens.saturating_add(event.total_tokens);
    if group
        .last_activity
        .as_deref()
        .is_none_or(|current| attribution.last_activity.as_str() > current)
    {
        group.last_activity = Some(attribution.last_activity.to_string());
    }

    if let Some(model_usage) = group.models.get_mut(model.name.as_str()) {
        accumulate_codex_usage_into_model_usage(model_usage, event, model, timestamp, attribution);
    } else {
        accumulate_codex_usage_into_model_usage(
            group.models.entry(model.name.to_string()).or_default(),
            event,
            model,
            timestamp,
            attribution,
        );
    }
}

fn accumulate_codex_usage_into_model_usage(
    model_usage: &mut crate::CodexModelUsage,
    event: crate::CodexRawUsage,
    model: &CodexModel,
    timestamp: crate::TimestampMs,
    attribution: &CodexAttribution,
) {
    model_usage.reasoning_output_tokens = model_usage
        .reasoning_output_tokens
        .saturating_add(event.reasoning_output_tokens);
    model_usage.total_tokens = model_usage.total_tokens.saturating_add(event.total_tokens);
    // NOTE(2026-07-09, #1414): OpenAI applies long-context rates to every token in a request once its input exceeds that model's threshold. The split has to happen before requests are summed.
    let is_long_context = event.input_tokens > model.long_context_threshold;
    let usage = CodexUsageBucket::from_usage(event, is_long_context);
    let (tier, configured_copy_tiers) = attribution.pricing_tiers();
    model_usage.add_usage(usage, tier);
    if let Some(tiers) = configured_copy_tiers {
        *model_usage.configured_copy_usage.entry(tiers).or_default() += usage;
    }
    if model.time_dependent {
        let timestamped_usage = model_usage
            .timestamped_usage
            .entry(timestamp.as_millis())
            .or_default();
        timestamped_usage.usage += usage;
        if let Some(tier) = tier {
            timestamped_usage.tier_usage[tier] += usage;
        }
        if let Some(tiers) = configured_copy_tiers {
            *timestamped_usage
                .configured_copy_usage
                .entry(tiers)
                .or_default() += usage;
        }
    }
    model_usage.is_fallback |= attribution.is_fallback;
}
