use ccusage_cli::PricingOverride;

use crate::{MILLIS_PER_DAY, MILLIS_PER_HOUR, TimestampMs};

use super::data;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pricing {
    pub input: f64,
    pub output: f64,
    pub(super) cache_create: Option<f64>,
    pub(super) cache_read: Option<f64>,
    pub(super) cache_create_1h: Option<f64>,
    pub long_context: Option<LongContext>,
    pub fast_multiplier: f64,
    pub ultrafast_multiplier: f64,
    pub flex_multiplier: f64,
    pub(super) cache_defaults: &'static CacheDefaults,
    pub(super) explicit_fields: u16,
    pub(super) flat_long_context_in_fast_mode: bool,
}

pub const DEFAULT_LONG_CONTEXT_THRESHOLD_TOKENS: u64 = 200_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LongContextMode {
    Marginal,
    WholeRequest,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LongContextRates {
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_create: Option<f64>,
    pub(super) cache_create_1h: Option<f64>,
    pub cache_read: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LongContext {
    pub(crate) threshold: u64,
    pub rates: LongContextRates,
    pub(crate) mode: LongContextMode,
}

impl LongContext {
    const fn empty_marginal() -> Self {
        Self {
            threshold: DEFAULT_LONG_CONTEXT_THRESHOLD_TOKENS,
            mode: LongContextMode::Marginal,
            rates: LongContextRates {
                input: None,
                output: None,
                cache_create: None,
                cache_create_1h: None,
                cache_read: None,
            },
        }
    }
}

impl LongContextRates {
    pub(super) fn marginal(self) -> Option<LongContext> {
        [
            self.input,
            self.output,
            self.cache_create,
            self.cache_create_1h,
            self.cache_read,
        ]
        .iter()
        .any(Option::is_some)
        .then_some(LongContext {
            rates: self,
            ..LongContext::empty_marginal()
        })
    }
}

#[derive(Debug, PartialEq)]
pub(super) struct CacheDefaults {
    pub(super) write_x: f64,
    pub(super) write_1h_x: f64,
    pub(super) read_x: f64,
}

impl Pricing {
    pub fn long_context_split_threshold(self) -> u64 {
        self.long_context
            .map_or(DEFAULT_LONG_CONTEXT_THRESHOLD_TOKENS, |context| {
                context.threshold
            })
    }

    pub fn cache_creation_rate(self) -> f64 {
        self.cache_create
            .unwrap_or(self.input * self.cache_defaults.write_x)
    }

    pub(crate) fn one_hour_cache_creation_rate(self) -> f64 {
        self.cache_create_1h
            .unwrap_or(self.input * self.cache_defaults.write_1h_x)
    }

    pub(crate) fn one_hour_cache_creation_rate_above_200k(self) -> Option<f64> {
        let rates = self.long_context?.rates;
        rates.cache_create_1h.or_else(|| {
            rates
                .input
                .map(|input| input * self.cache_defaults.write_1h_x)
        })
    }

    pub fn cache_read_rate_at(self, input_rate: f64) -> f64 {
        self.cache_read
            .unwrap_or(input_rate * self.cache_defaults.read_x)
    }

    pub(super) const fn empty() -> Self {
        Self {
            input: 0.0,
            output: 0.0,
            cache_create: None,
            cache_read: None,
            cache_create_1h: None,
            long_context: None,
            fast_multiplier: 1.0,
            ultrafast_multiplier: 1.0,
            flex_multiplier: 1.0,
            cache_defaults: &data::WRITES_SURCHARGED,
            explicit_fields: 0,
            flat_long_context_in_fast_mode: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ScheduledRates {
    pub(super) input: f64,
    pub(super) output: f64,
    pub(super) cache_create: f64,
    pub(super) cache_read: f64,
    pub(super) long_context: Option<LongContext>,
}

#[derive(Debug)]
pub(super) struct PricingSchedule {
    pub(super) cutoff_ms: i64,
    pub(super) peak_weekdays: std::ops::RangeInclusive<u32>,
    pub(super) peak_hours: &'static [std::ops::Range<i64>],
    pub(super) before: ScheduledRates,
    pub(super) off_peak: ScheduledRates,
    pub(super) peak: ScheduledRates,
    pub(super) successor: Option<Successor>,
    pub(super) follows_row: bool,
    pub(super) flat_in_fast_mode: bool,
    pub(super) vendor: &'static str,
}

#[derive(Debug)]
pub(super) struct Successor {
    pub(super) from_ms: i64,
    pub(super) schedule: &'static PricingSchedule,
}

impl PricingSchedule {
    fn rates_at(&self, timestamp: TimestampMs) -> ScheduledRates {
        if let Some(successor) = &self.successor
            && timestamp.as_millis() >= successor.from_ms
        {
            return successor.schedule.rates_at(timestamp);
        }
        if timestamp.as_millis() < self.cutoff_ms {
            return self.before;
        }
        let weekday =
            ccusage_cli::weekday_from_epoch_days(timestamp.as_millis().div_euclid(MILLIS_PER_DAY));
        let hour = timestamp.as_millis().rem_euclid(MILLIS_PER_DAY) / MILLIS_PER_HOUR;
        if self.peak_weekdays.contains(&weekday)
            && self.peak_hours.iter().any(|window| window.contains(&hour))
        {
            self.peak
        } else {
            self.off_peak
        }
    }
}

struct OverrideFields;

impl OverrideFields {
    const INPUT: u16 = 1;
    const OUTPUT: u16 = 2;
    const CACHE_CREATE: u16 = 4;
    const CACHE_READ: u16 = 8;
    const LONG_INPUT: u16 = 16;
    const LONG_OUTPUT: u16 = 32;
    const LONG_CACHE_CREATE: u16 = 64;
    const LONG_CACHE_READ: u16 = 128;
    const CACHE_CREATE_1H: u16 = 256;
    const FAST: u16 = 512;
    const FLEX: u16 = 1024;
    const RATES: u16 =
        Self::INPUT | Self::OUTPUT | Self::CACHE_CREATE | Self::CACHE_READ | Self::CACHE_CREATE_1H;
    const LONG_RATES: u16 =
        Self::LONG_INPUT | Self::LONG_OUTPUT | Self::LONG_CACHE_CREATE | Self::LONG_CACHE_READ;
}

impl Pricing {
    pub(super) fn at(mut self, schedule: &PricingSchedule, timestamp: TimestampMs) -> Self {
        let explicit = (self.explicit_fields != 0).then_some(self);
        let rates = schedule.rates_at(timestamp);
        let scheduled_tier = rates.long_context.filter(|_| {
            explicit.is_none_or(|entry| entry.explicit_fields & OverrideFields::RATES == 0)
        });
        let mut long_context = scheduled_tier
            .or(self.long_context)
            .unwrap_or(LongContext::empty_marginal());
        macro_rules! scheduled {
            ($field:ident, $rate:expr, $bit:expr) => {
                self.$field = match explicit {
                    Some(entry) if entry.explicit_fields & $bit != 0 => entry.$field,
                    _ => $rate,
                };
            };
        }
        scheduled!(input, rates.input, OverrideFields::INPUT);
        scheduled!(output, rates.output, OverrideFields::OUTPUT);
        scheduled!(
            cache_create,
            Some(rates.cache_create),
            OverrideFields::CACHE_CREATE
        );
        scheduled!(
            cache_read,
            Some(rates.cache_read),
            OverrideFields::CACHE_READ
        );
        let overridden = |bit: u16| explicit.is_some_and(|entry| entry.explicit_fields & bit != 0);
        // NOTE(2026-11-20, #2373): `scheduled!` already resolved each base field to its override or schedule rate. The fields above the split therefore inherit `self.$field` unless a long rate is explicit.
        macro_rules! long_scheduled {
            ($field:ident, $bit:expr, $wrap:expr) => {
                long_context.rates.$field = if overridden($bit) {
                    explicit
                        .and_then(|entry| entry.long_context)
                        .and_then(|context| context.rates.$field)
                } else {
                    match scheduled_tier {
                        Some(tier) => tier.rates.$field,
                        None => $wrap(self.$field),
                    }
                };
            };
        }
        long_scheduled!(input, OverrideFields::LONG_INPUT, Some);
        long_scheduled!(output, OverrideFields::LONG_OUTPUT, Some);
        long_scheduled!(
            cache_create,
            OverrideFields::LONG_CACHE_CREATE,
            std::convert::identity
        );
        long_scheduled!(
            cache_read,
            OverrideFields::LONG_CACHE_READ,
            std::convert::identity
        );
        scheduled!(
            cache_create_1h,
            self.cache_create_1h,
            OverrideFields::CACHE_CREATE_1H
        );
        scheduled!(fast_multiplier, self.fast_multiplier, OverrideFields::FAST);
        scheduled!(flex_multiplier, self.flex_multiplier, OverrideFields::FLEX);
        if scheduled_tier.is_none()
            && overridden(OverrideFields::RATES | OverrideFields::LONG_RATES)
        {
            long_context.rates.cache_create_1h = Some(self.one_hour_cache_creation_rate());
        }
        self.long_context = Some(long_context);
        self.flat_long_context_in_fast_mode =
            scheduled_tier.is_some() && schedule.flat_in_fast_mode;
        self
    }

    pub(crate) fn at_speed(mut self, fast: bool) -> Self {
        if fast && self.flat_long_context_in_fast_mode {
            self.long_context = self.explicit_long_context();
        }
        self
    }

    fn explicit_long_context(&self) -> Option<LongContext> {
        let explicit = self.explicit_fields;
        let kept = |bit: u16, rate: Option<f64>| rate.filter(|_| explicit & bit != 0);
        let one_hour = self.one_hour_cache_creation_rate();
        self.long_context.and_then(|context| {
            LongContextRates {
                input: kept(OverrideFields::LONG_INPUT, context.rates.input),
                output: kept(OverrideFields::LONG_OUTPUT, context.rates.output),
                cache_create: kept(
                    OverrideFields::LONG_CACHE_CREATE,
                    context.rates.cache_create,
                ),
                cache_create_1h: None,
                cache_read: kept(OverrideFields::LONG_CACHE_READ, context.rates.cache_read),
            }
            .marginal()
            .map(|marginal| LongContext {
                rates: LongContextRates {
                    cache_create_1h: Some(one_hour),
                    ..marginal.rates
                },
                ..context
            })
        })
    }

    pub(super) fn explicit_long_rate_fields(&self) -> u16 {
        self.explicit_fields & OverrideFields::LONG_RATES
    }

    pub(super) fn replace_tier_with_explicit_rates(&mut self) {
        if self.explicit_fields & OverrideFields::RATES == 0 {
            return;
        }
        self.long_context = self.explicit_long_context();
    }

    pub(super) fn apply_override(&mut self, value: &PricingOverride) {
        self.explicit_fields = 0;
        macro_rules! rate {
            ($source:ident, $field:ident, $wrap:expr, $bit:expr) => {
                if let Some(value) = value.$source {
                    self.$field = $wrap(value);
                    self.explicit_fields |= $bit;
                }
            };
        }
        rate!(
            input_cost_per_token,
            input,
            std::convert::identity,
            OverrideFields::INPUT
        );
        rate!(
            output_cost_per_token,
            output,
            std::convert::identity,
            OverrideFields::OUTPUT
        );
        rate!(
            cache_creation_input_token_cost,
            cache_create,
            Some,
            OverrideFields::CACHE_CREATE
        );
        rate!(
            cache_read_input_token_cost,
            cache_read,
            Some,
            OverrideFields::CACHE_READ
        );
        let mut context = self.long_context.unwrap_or(LongContext::empty_marginal());
        let mut changed = false;
        macro_rules! long_rate {
            ($source:ident, $field:ident, $bit:expr) => {
                if let Some(value) = value.$source {
                    context.rates.$field = Some(value);
                    self.explicit_fields |= $bit;
                    changed = true;
                }
            };
        }
        long_rate!(
            input_cost_per_token_above_200k_tokens,
            input,
            OverrideFields::LONG_INPUT
        );
        long_rate!(
            output_cost_per_token_above_200k_tokens,
            output,
            OverrideFields::LONG_OUTPUT
        );
        long_rate!(
            cache_creation_input_token_cost_above_200k_tokens,
            cache_create,
            OverrideFields::LONG_CACHE_CREATE
        );
        long_rate!(
            cache_read_input_token_cost_above_200k_tokens,
            cache_read,
            OverrideFields::LONG_CACHE_READ
        );
        if changed {
            self.long_context = Some(context);
        }
        rate!(
            cache_creation_input_token_cost_above_1hr,
            cache_create_1h,
            Some,
            OverrideFields::CACHE_CREATE_1H
        );
        rate!(
            fast_multiplier,
            fast_multiplier,
            std::convert::identity,
            OverrideFields::FAST
        );
        rate!(
            flex_multiplier,
            flex_multiplier,
            std::convert::identity,
            OverrideFields::FLEX
        );
    }
}

pub(super) const fn rates(input: f64, output: f64, cache_create: f64, cache_read: f64) -> Pricing {
    Pricing {
        input,
        output,
        cache_create: Some(cache_create),
        cache_read: Some(cache_read),
        ..Pricing::empty()
    }
}
