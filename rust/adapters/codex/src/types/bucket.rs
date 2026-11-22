use std::ops::{Add, AddAssign, Index, IndexMut};

use super::{CodexRawUsage, CodexServiceTier};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CodexUsageBucket {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_creation_tokens: u64,
    pub output_tokens: u64,
    pub long_context_input_tokens: u64,
    pub long_context_cached_input_tokens: u64,
    pub long_context_cache_creation_tokens: u64,
    pub long_context_output_tokens: u64,
}

impl CodexUsageBucket {
    pub(crate) fn from_usage(usage: CodexRawUsage, is_long_context: bool) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            cached_input_tokens: usage.cached_input_tokens,
            cache_creation_tokens: usage.cache_creation_tokens,
            output_tokens: usage.output_tokens,
            long_context_input_tokens: if is_long_context {
                usage.input_tokens
            } else {
                0
            },
            long_context_cached_input_tokens: if is_long_context {
                usage.cached_input_tokens
            } else {
                0
            },
            long_context_cache_creation_tokens: if is_long_context {
                usage.cache_creation_tokens
            } else {
                0
            },
            long_context_output_tokens: if is_long_context {
                usage.output_tokens
            } else {
                0
            },
        }
    }
}

impl AddAssign for CodexUsageBucket {
    fn add_assign(&mut self, other: Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.cached_input_tokens = self
            .cached_input_tokens
            .saturating_add(other.cached_input_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(other.cache_creation_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.long_context_input_tokens = self
            .long_context_input_tokens
            .saturating_add(other.long_context_input_tokens);
        self.long_context_cached_input_tokens = self
            .long_context_cached_input_tokens
            .saturating_add(other.long_context_cached_input_tokens);
        self.long_context_cache_creation_tokens = self
            .long_context_cache_creation_tokens
            .saturating_add(other.long_context_cache_creation_tokens);
        self.long_context_output_tokens = self
            .long_context_output_tokens
            .saturating_add(other.long_context_output_tokens);
    }
}

impl Add for CodexUsageBucket {
    type Output = Self;

    fn add(mut self, other: Self) -> Self {
        self += other;
        self
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TierBuckets([CodexUsageBucket; 4]);

impl CodexServiceTier {
    const fn index(self) -> usize {
        match self {
            Self::Standard => 0,
            Self::Fast => 1,
            Self::Flex => 2,
            Self::Ultrafast => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ConfiguredTiers(u8);

impl ConfiguredTiers {
    const TIERS: [CodexServiceTier; 4] = [
        CodexServiceTier::Standard,
        CodexServiceTier::Fast,
        CodexServiceTier::Flex,
        CodexServiceTier::Ultrafast,
    ];

    pub(crate) fn single(self) -> Option<CodexServiceTier> {
        (self.0.count_ones() == 1).then(|| Self::TIERS[self.0.trailing_zeros() as usize])
    }

    pub(crate) const fn is_mixed(self) -> bool {
        self.0.count_ones() > 1
    }

    pub(crate) const fn contains(self, tier: CodexServiceTier) -> bool {
        self.0 & (1 << tier.index()) != 0
    }

    pub(crate) fn merge(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

impl From<Option<CodexServiceTier>> for ConfiguredTiers {
    fn from(tier: Option<CodexServiceTier>) -> Self {
        Self(tier.map_or(0, |tier| 1 << tier.index()))
    }
}

impl Index<CodexServiceTier> for TierBuckets {
    type Output = CodexUsageBucket;

    fn index(&self, tier: CodexServiceTier) -> &Self::Output {
        &self.0[tier.index()]
    }
}

impl IndexMut<CodexServiceTier> for TierBuckets {
    fn index_mut(&mut self, tier: CodexServiceTier) -> &mut Self::Output {
        &mut self.0[tier.index()]
    }
}

impl AddAssign for TierBuckets {
    fn add_assign(&mut self, other: Self) {
        for (target, source) in self.0.iter_mut().zip(other.0) {
            *target += source;
        }
    }
}
