use super::*;

#[test]
fn tier_addition_saturates_and_keeps_each_tier_separate() {
    let mut first = TierBuckets::default();
    first[CodexServiceTier::Fast].input_tokens = u64::MAX - 1;
    let mut second = TierBuckets::default();
    second[CodexServiceTier::Fast].input_tokens = 3;
    second[CodexServiceTier::Flex].input_tokens = 5;
    first += second;
    assert_eq!(first[CodexServiceTier::Standard].input_tokens, 0);
    assert_eq!(first[CodexServiceTier::Fast].input_tokens, u64::MAX);
    assert_eq!(first[CodexServiceTier::Flex].input_tokens, 5);
    assert_eq!(first[CodexServiceTier::Ultrafast].input_tokens, 0);
    assert_eq!(first.total().input_tokens, u64::MAX);
}

#[test]
fn merged_ultrafast_copies_beat_fast_and_flex_but_not_standard() {
    use CodexServiceTier::{Fast, Flex, Standard, Ultrafast};

    for (other, expected) in [
        (None, Ultrafast),
        (Some(Fast), Ultrafast),
        (Some(Flex), Ultrafast),
        (Some(Standard), Standard),
    ] {
        assert_eq!(
            merge_codex_service_tiers(Some(Ultrafast), other),
            Some(expected)
        );
        assert_eq!(
            merge_codex_service_tiers(other, Some(Ultrafast)),
            Some(expected)
        );
    }
}

#[test]
fn merged_copies_keep_a_recorded_tier_and_settle_conflicts_as_standard() {
    use CodexServiceTier::{Fast, Standard};

    for (first, second, expected) in [
        (None, Some(Fast), Fast),
        (Some(Standard), Some(Fast), Standard),
    ] {
        assert_eq!(merge_codex_service_tiers(first, second), Some(expected));
        assert_eq!(merge_codex_service_tiers(second, first), Some(expected));
    }
}

#[test]
fn derives_total_tokens_without_double_counting_reasoning() {
    let usage: CodexRawUsage = serde_json::from_str(
        r#"{
                "input_tokens": 100,
                "output_tokens": 50,
                "reasoning_output_tokens": 20
            }"#,
    )
    .expect("usage should deserialize");

    assert_eq!(usage.total_tokens, 150);
}

#[test]
fn derives_explicit_zero_total_tokens_without_double_counting_reasoning() {
    let usage: CodexRawUsage = serde_json::from_str(
        r#"{
                "input_tokens": 9,
                "output_tokens": 4,
                "reasoning_output_tokens": 1,
                "total_tokens": 0
            }"#,
    )
    .expect("usage should deserialize");

    assert_eq!(usage.total_tokens, 13);
}

#[test]
fn keeps_a_recorded_total() {
    let usage: CodexRawUsage = serde_json::from_str(
        r#"{
                "input_tokens": 100,
                "output_tokens": 50,
                "reasoning_output_tokens": 20,
                "total_tokens": 151
            }"#,
    )
    .expect("usage should deserialize");

    assert_eq!(usage.total_tokens, 151);
}

#[test]
fn saturates_a_derived_total_instead_of_overflowing() {
    let usage: CodexRawUsage = serde_json::from_str(
        r#"{
                "input_tokens": 18446744073709551615,
                "output_tokens": 5
            }"#,
    )
    .expect("usage should deserialize");

    assert_eq!(usage.total_tokens, u64::MAX);
}

#[test]
fn derives_total_tokens_from_openai_field_spellings() {
    let usage: CodexRawUsage = serde_json::from_str(
        r#"{
                "prompt_tokens": 50,
                "cached_tokens": 5,
                "completion_tokens": 12,
                "reasoning_tokens": 4
            }"#,
    )
    .expect("usage should deserialize");

    assert_eq!(usage.total_tokens, 62);
}

#[test]
fn normalizes_cache_reads_and_writes_to_the_reported_input_total() {
    let usage: CodexRawUsage = serde_json::from_str(
        r#"{
                "input_tokens": 100,
                "cached_input_tokens": 80,
                "cache_write_input_tokens": 40,
                "output_tokens": 5,
                "total_tokens": 105
            }"#,
    )
    .expect("usage should deserialize");

    assert_eq!(usage.cached_input_tokens, 80);
    assert_eq!(usage.cache_creation_tokens, 20);
}

#[test]
fn accepts_the_cache_creation_input_tokens_compatibility_spelling() {
    let usage: CodexRawUsage = serde_json::from_str(
        r#"{
                "input_tokens": 100,
                "cached_input_tokens": 20,
                "cache_creation_input_tokens": 30,
                "output_tokens": 5
            }"#,
    )
    .expect("usage should deserialize");

    assert_eq!(usage.cache_creation_tokens, 30);
}

#[test]
fn leaves_an_empty_usage_total_at_zero() {
    let usage: CodexRawUsage =
        serde_json::from_str(r#"{"total_tokens": 0}"#).expect("usage should deserialize");

    assert_eq!(usage.total_tokens, 0);
}
