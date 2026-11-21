use std::sync::Arc;

use serde::Serialize;

use crate::TimestampMs;

#[derive(Debug, Clone, Copy, Default)]
pub struct TokenUsageRaw {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub speed: Option<Speed>,
    pub cache_creation: Option<CacheCreationRaw>,
}

impl TokenUsageRaw {
    pub fn cache_creation_token_count(&self) -> u64 {
        if let Some(b) = &self.cache_creation {
            b.ephemeral_5m_input_tokens
                .saturating_add(b.ephemeral_1h_input_tokens)
        } else {
            self.cache_creation_input_tokens
        }
    }

    fn checked_cache_creation_token_count(&self) -> Option<u64> {
        match &self.cache_creation {
            Some(b) => b
                .ephemeral_5m_input_tokens
                .checked_add(b.ephemeral_1h_input_tokens),
            None => Some(self.cache_creation_input_tokens),
        }
    }

    fn cache_creation_1h_token_count(&self) -> Option<u64> {
        self.cache_creation.map(CacheCreationRaw::one_hour_tokens)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CacheCreationRaw {
    pub(crate) ephemeral_5m_input_tokens: u64,
    pub(crate) ephemeral_1h_input_tokens: u64,
}

impl CacheCreationRaw {
    pub fn from_splits(ephemeral_5m_input_tokens: u64, ephemeral_1h_input_tokens: u64) -> Self {
        Self {
            ephemeral_5m_input_tokens,
            ephemeral_1h_input_tokens,
        }
    }

    pub fn one_hour_tokens(self) -> u64 {
        self.ephemeral_1h_input_tokens
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Speed {
    Standard,
    Fast,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenCounts {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub(crate) extra_total_tokens: u64,
}

impl TokenCounts {
    pub fn add_usage(&mut self, usage: TokenUsageRaw) {
        self.input_tokens = self.input_tokens.saturating_add(usage.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(usage.output_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(usage.cache_creation_token_count());
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(usage.cache_read_input_tokens);
    }

    pub fn total(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_creation_tokens)
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.extra_total_tokens)
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelBreakdown {
    pub model_name: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    /// Subset of `cache_creation_tokens` written with the 1-hour TTL. The rest used the 5-minute TTL.
    /// `None` means that no entry reported a TTL split.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_1h_tokens: Option<u64>,
    #[serde(skip_serializing)]
    pub extra_total_tokens: u64,
    pub cost: f64,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub missing_pricing: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub estimated_pricing: bool,
    /// Subset of `cost` taken from stored `costUSD` values in `auto` mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stored_cost: Option<f64>,
}

impl ModelBreakdown {
    pub fn sort_by_cost_desc(breakdowns: &mut [Self]) {
        breakdowns.sort_by(|a, b| b.cost.total_cmp(&a.cost));
    }

    pub fn add_usage(&mut self, usage: &TokenUsageRaw) {
        self.input_tokens = self.input_tokens.saturating_add(usage.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(usage.output_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(usage.cache_creation_token_count());
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(usage.cache_read_input_tokens);
        self.add_cache_creation_1h_tokens(usage.cache_creation_1h_token_count());
    }

    pub fn merge(&mut self, other: &Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(other.cache_creation_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(other.cache_read_tokens);
        self.add_cache_creation_1h_tokens(other.cache_creation_1h_tokens);
        self.extra_total_tokens = self
            .extra_total_tokens
            .saturating_add(other.extra_total_tokens);
        self.cost += other.cost;
        self.missing_pricing |= other.missing_pricing;
        self.estimated_pricing |= other.estimated_pricing;
        self.add_stored_cost(other.stored_cost);
    }

    fn add_cache_creation_1h_tokens(&mut self, tokens: Option<u64>) {
        if let Some(tokens) = tokens {
            let total = self.cache_creation_1h_tokens.get_or_insert(0);
            *total = total.saturating_add(tokens);
        }
    }

    pub fn add_stored_cost(&mut self, stored_cost: Option<f64>) {
        if let Some(stored_cost) = stored_cost {
            *self.stored_cost.get_or_insert(0.0) += stored_cost;
        }
    }

    pub fn total_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_creation_tokens)
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.extra_total_tokens)
    }
}

#[derive(Debug, Clone)]
pub struct LoadedEntry {
    pub usage: TokenUsageRaw,
    pub message_id: Option<String>,
    pub version: Option<String>,
    pub recorded_cost: Option<f64>,
    pub timestamp: TimestampMs,
    pub date: crate::cli::IsoDate,
    pub project: Arc<str>,
    pub session_id: Arc<str>,
    pub project_path: Arc<str>,
    pub cost: f64,
    pub extra_total_tokens: u64,
    pub credits: Option<f64>,
    pub message_count: Option<u64>,
    pub model: Option<String>,
    pub usage_limit_reset_time: Option<TimestampMs>,
    pub missing_pricing_model: Option<String>,
    pub estimated_pricing: bool,
    pub stored_cost: Option<f64>,
}

impl LoadedEntry {
    pub fn sort_by_timestamp(entries: &mut [Self]) {
        entries.sort_by_key(|entry| entry.timestamp);
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub month: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub week: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_activity: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    #[serde(skip_serializing)]
    pub extra_total_tokens: u64,
    pub total_cost: f64,
    pub credits: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_count: Option<u64>,
    pub models_used: Vec<String>,
    pub model_breakdowns: Vec<ModelBreakdown>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub versions: Option<Vec<String>>,
}

impl UsageSummary {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_creation_tokens)
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.extra_total_tokens)
    }

    fn totals_row(&self) -> TotalsRow<'_> {
        TotalsRow {
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_creation_tokens: self.cache_creation_tokens,
            cache_read_tokens: self.cache_read_tokens,
            total_tokens: self.total_tokens(),
            cost: self.total_cost,
            credits: self.credits,
            model_breakdowns: &self.model_breakdowns,
        }
    }
}

pub struct TotalsRow<'a> {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_tokens: u64,
    pub cost: f64,
    pub credits: Option<f64>,
    pub model_breakdowns: &'a [ModelBreakdown],
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Totals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_tokens: u64,
    pub total_cost: f64,
    pub credits: Option<f64>,
    pub unpriced_models: Vec<String>,
    pub estimated_models: Vec<String>,
}

impl Totals {
    pub fn from_rows<'a>(rows: impl IntoIterator<Item = TotalsRow<'a>>) -> Self {
        let mut totals = Self::default();
        let mut costs = Vec::new();
        let mut credits = Vec::new();
        let mut breakdowns = Vec::new();
        for row in rows {
            totals.input_tokens = totals.input_tokens.saturating_add(row.input_tokens);
            totals.output_tokens = totals.output_tokens.saturating_add(row.output_tokens);
            totals.cache_creation_tokens = totals
                .cache_creation_tokens
                .saturating_add(row.cache_creation_tokens);
            totals.cache_read_tokens = totals
                .cache_read_tokens
                .saturating_add(row.cache_read_tokens);
            totals.total_tokens = totals.total_tokens.saturating_add(row.total_tokens);
            costs.push(row.cost);
            credits.extend(row.credits);
            breakdowns.push(row.model_breakdowns);
        }
        totals.total_cost = crate::sum_costs(costs);
        let credits = crate::sum_costs(credits);
        totals.credits = (credits > 0.0).then_some(credits);
        let breakdowns: Vec<_> = breakdowns.into_iter().flatten().collect();
        totals.unpriced_models = crate::output::unpriced_models(breakdowns.iter().copied());
        totals.estimated_models = crate::output::estimated_models(breakdowns);
        totals
    }

    pub fn from_summaries(rows: &[UsageSummary]) -> Self {
        Self::from_rows(rows.iter().map(UsageSummary::totals_row))
    }
}

pub fn total_usage_tokens(usage: TokenUsageRaw) -> u64 {
    usage
        .input_tokens
        .saturating_add(usage.output_tokens)
        .saturating_add(usage.cache_creation_token_count())
        .saturating_add(usage.cache_read_input_tokens)
}

/// Sums the token buckets without wrapping, returning `None` when a log line
/// reports more tokens than a `u64` can hold.
pub fn checked_total_usage_tokens(usage: TokenUsageRaw, extra_total_tokens: u64) -> Option<u64> {
    usage
        .input_tokens
        .checked_add(usage.output_tokens)?
        .checked_add(usage.checked_cache_creation_token_count()?)?
        .checked_add(usage.cache_read_input_tokens)?
        .checked_add(extra_total_tokens)
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AdditionalTokens {
    pub reasoning: u64,
    /// Unclassified tokens included in the reported usage total.
    pub report_remainder: u64,
}

impl AdditionalTokens {
    pub fn from_total(usage: TokenUsageRaw, reasoning: u64, total: u64) -> Self {
        Self {
            reasoning,
            report_remainder: total
                .saturating_sub(total_usage_tokens(usage).saturating_add(reasoning)),
        }
    }

    pub fn total(self) -> u64 {
        self.reasoning.saturating_add(self.report_remainder)
    }

    /// Preserves the legacy report fallback that fills a missing output bucket from the recorded total.
    pub fn for_report(self, mut usage: TokenUsageRaw) -> (TokenUsageRaw, u64) {
        if usage.output_tokens == 0 {
            usage.output_tokens = self.report_remainder;
            (usage, self.reasoning)
        } else {
            (usage, self.total())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saturates_token_count_accumulation_and_total() {
        let mut counts = TokenCounts {
            input_tokens: u64::MAX,
            output_tokens: u64::MAX,
            cache_creation_tokens: u64::MAX,
            cache_read_tokens: u64::MAX,
            extra_total_tokens: u64::MAX,
        };
        counts.add_usage(TokenUsageRaw {
            input_tokens: 1,
            output_tokens: 1,
            cache_creation_input_tokens: 1,
            cache_read_input_tokens: 1,
            ..TokenUsageRaw::default()
        });

        assert_eq!(counts.input_tokens, u64::MAX);
        assert_eq!(counts.output_tokens, u64::MAX);
        assert_eq!(counts.cache_creation_tokens, u64::MAX);
        assert_eq!(counts.cache_read_tokens, u64::MAX);
        assert_eq!(counts.total(), u64::MAX);
    }

    #[test]
    fn applies_total_token_fallback_to_missing_output_tokens() {
        let usage = TokenUsageRaw {
            input_tokens: 100,
            cache_read_input_tokens: 25,
            ..Default::default()
        };
        let additional = AdditionalTokens::from_total(usage, 0, 175);
        assert_eq!(additional.report_remainder, 50);
        let (usage, extra_total_tokens) = additional.for_report(usage);

        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_read_input_tokens, 25);
        assert_eq!(extra_total_tokens, 0);
    }

    #[test]
    fn keeps_total_fallback_as_extra_when_output_is_known() {
        let usage = TokenUsageRaw {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_input_tokens: 25,
            ..Default::default()
        };
        let additional = AdditionalTokens::from_total(usage, 10, 200);
        assert_eq!(additional.reasoning, 10);
        assert_eq!(additional.report_remainder, 15);
        let (usage, extra_total_tokens) = additional.for_report(usage);

        assert_eq!(usage.output_tokens, 50);
        assert_eq!(extra_total_tokens, 25);
    }

    #[test]
    fn saturates_total_token_fallback_for_huge_counters() {
        let usage = TokenUsageRaw {
            input_tokens: u64::MAX,
            output_tokens: u64::MAX,
            cache_read_input_tokens: u64::MAX,
            cache_creation: Some(crate::CacheCreationRaw {
                ephemeral_5m_input_tokens: u64::MAX,
                ephemeral_1h_input_tokens: u64::MAX,
            }),
            ..Default::default()
        };
        let additional = AdditionalTokens::from_total(usage, u64::MAX, u64::MAX);
        let (usage, extra_total_tokens) = additional.for_report(usage);

        assert_eq!(total_usage_tokens(usage), u64::MAX);
        assert_eq!(extra_total_tokens, u64::MAX);
    }

    #[test]
    fn checked_total_usage_tokens_reports_unrepresentable_buckets() {
        let usage = TokenUsageRaw {
            input_tokens: u64::MAX,
            output_tokens: 1,
            ..TokenUsageRaw::default()
        };

        assert_eq!(checked_total_usage_tokens(usage, 0), None);
        assert_eq!(
            checked_total_usage_tokens(
                TokenUsageRaw {
                    input_tokens: u64::MAX - 1,
                    output_tokens: 1,
                    ..TokenUsageRaw::default()
                },
                0,
            ),
            Some(u64::MAX)
        );
        assert_eq!(
            checked_total_usage_tokens(
                TokenUsageRaw {
                    input_tokens: u64::MAX,
                    ..TokenUsageRaw::default()
                },
                1,
            ),
            None
        );
        assert_eq!(
            checked_total_usage_tokens(
                TokenUsageRaw {
                    cache_creation: Some(crate::CacheCreationRaw {
                        ephemeral_5m_input_tokens: u64::MAX,
                        ephemeral_1h_input_tokens: 1,
                    }),
                    ..TokenUsageRaw::default()
                },
                0,
            ),
            None
        );
    }
}
