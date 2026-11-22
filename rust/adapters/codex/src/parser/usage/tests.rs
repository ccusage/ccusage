use super::*;

#[test]
fn normalizes_cache_usage_after_cumulative_resets_and_series_changes() {
    let reset = subtract_codex_raw_usage(
        &CodexRawUsage {
            input_tokens: 10,
            cached_input_tokens: 10,
            cache_creation_tokens: 0,
            ..CodexRawUsage::default()
        },
        Some(&CodexRawUsage {
            input_tokens: 100,
            cached_input_tokens: 0,
            cache_creation_tokens: 100,
            ..CodexRawUsage::default()
        }),
    );
    let series_change = subtract_codex_raw_usage(
        &CodexRawUsage {
            input_tokens: 110,
            cached_input_tokens: 0,
            cache_creation_tokens: 110,
            ..CodexRawUsage::default()
        },
        Some(&CodexRawUsage {
            input_tokens: 100,
            cached_input_tokens: 100,
            cache_creation_tokens: 0,
            ..CodexRawUsage::default()
        }),
    );

    assert_eq!(reset.input_tokens, 0);
    assert_eq!(reset.cached_input_tokens, 0);
    assert_eq!(reset.cache_creation_tokens, 0);
    assert_eq!(series_change.input_tokens, 10);
    assert_eq!(series_change.cached_input_tokens, 0);
    assert_eq!(series_change.cache_creation_tokens, 10);
}
