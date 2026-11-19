use ccusage_core::{CacheCreationRaw, Speed, TokenUsageRaw};
use serde::Deserialize;

mod iterations;

use iterations::{Iterations, Lenient};

#[derive(Debug, Clone, Copy, Default, Deserialize)]
struct WireCacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum WireSpeed {
    Standard,
    Fast,
}

#[derive(Deserialize)]
struct TokenUsageRawJson {
    input_tokens: u64,
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    speed: Option<WireSpeed>,
    #[serde(default)]
    cache_creation: Option<WireCacheCreation>,
    #[serde(default)]
    iterations: Lenient<Iterations>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(from = "TokenUsageRawJson")]
pub(crate) struct IteratedTokenUsage {
    pub(crate) usage: TokenUsageRaw,
    pub(crate) has_advisor_iteration: bool,
}

impl From<TokenUsageRawJson> for IteratedTokenUsage {
    fn from(json: TokenUsageRawJson) -> Self {
        Self {
            has_advisor_iteration: json.iterations.0.has_advisor_message,
            usage: json.into(),
        }
    }
}

impl From<TokenUsageRawJson> for TokenUsageRaw {
    fn from(json: TokenUsageRawJson) -> Self {
        Self {
            input_tokens: json.input_tokens,
            output_tokens: json.output_tokens,
            cache_creation_input_tokens: json.cache_creation_input_tokens,
            cache_read_input_tokens: json.cache_read_input_tokens,
            speed: json.speed.map(|speed| match speed {
                WireSpeed::Standard => Speed::Standard,
                WireSpeed::Fast => Speed::Fast,
            }),
            // NOTE(2026-11-16, #2085): In Claude Code advisor turns, the top-level `cache_creation` split covers the first `message` iteration while the total covers them all. Sum the per-message splits when complete.
            cache_creation: json
                .iterations
                .0
                .message_cache_creation
                .or(json.cache_creation)
                .map(|split| {
                    CacheCreationRaw::from_splits(
                        split.ephemeral_5m_input_tokens,
                        split.ephemeral_1h_input_tokens,
                    )
                }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_message_iteration_breakdowns_instead_of_the_truncated_top_level() {
        let usage = serde_json::from_str::<IteratedTokenUsage>(
            r#"{
                "input_tokens": 4,
                "output_tokens": 244,
                "cache_creation_input_tokens": 12323,
                "cache_read_input_tokens": 140030,
                "cache_creation": {
                    "ephemeral_5m_input_tokens": 0,
                    "ephemeral_1h_input_tokens": 11142
                },
                "iterations": [
                    {"type": "message", "input_tokens": 2, "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 11142}},
                    {"type": "advisor_message", "model": "claude-fable-5-2", "input_tokens": 77025, "cache_creation": {"ephemeral_5m_input_tokens": 999, "ephemeral_1h_input_tokens": 999}},
                    {"type": "message", "input_tokens": 2, "cache_creation": {"ephemeral_5m_input_tokens": 1181, "ephemeral_1h_input_tokens": 0}}
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(usage.usage.cache_creation_token_count(), 12323);
    }

    #[test]
    fn keeps_the_top_level_breakdown_without_message_iteration_breakdowns() {
        let usage = serde_json::from_str::<IteratedTokenUsage>(
            r#"{
                "input_tokens": 4,
                "output_tokens": 244,
                "cache_creation_input_tokens": 12323,
                "cache_creation": {
                    "ephemeral_5m_input_tokens": 0,
                    "ephemeral_1h_input_tokens": 11142
                },
                "iterations": [
                    {"type": "advisor_message", "model": "m", "input_tokens": 1}
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(usage.usage.cache_creation_token_count(), 11142);
    }

    #[test]
    fn keeps_the_top_level_breakdown_when_a_message_iteration_lacks_its_own() {
        let usage = serde_json::from_str::<IteratedTokenUsage>(
            r#"{
                "input_tokens": 4,
                "output_tokens": 244,
                "cache_creation_input_tokens": 12323,
                "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 11142},
                "iterations": [
                    {"type": "message", "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 11142}},
                    {"type": "message", "cache_creation_input_tokens": 1181}
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(usage.usage.cache_creation_token_count(), 11142);
    }

    #[test]
    fn malformed_iterations_keep_the_usage_and_its_top_level_breakdown() {
        let iterations = [
            r#""n/a""#,
            "null",
            "{}",
            r#"[1, "x", null, [], {"type": 7}]"#,
            r#"[{"type": "message", "cache_creation": "n/a"}]"#,
            r#"[{"type": "message", "cache_creation": {"ephemeral_5m_input_tokens": -1}}]"#,
            r#"[{"type": "message", "cache_creation": {"ephemeral_1h_input_tokens": "10"}}]"#,
            r#"[{"type": "message", "cache_creation": null}]"#,
        ];
        for iterations in iterations {
            let line = format!(
                r#"{{"input_tokens":1,"output_tokens":2,"cache_creation":{{"ephemeral_5m_input_tokens":3,"ephemeral_1h_input_tokens":4}},"iterations":{iterations}}}"#
            );
            let usage = serde_json::from_str::<IteratedTokenUsage>(&line)
                .unwrap_or_else(|error| panic!("{iterations}: {error}"));

            assert_eq!(usage.usage.cache_creation_token_count(), 7, "{iterations}");
            assert_eq!(
                usage
                    .usage
                    .cache_creation
                    .map(|split| split.one_hour_tokens()),
                Some(4),
                "{iterations}"
            );
        }
    }

    #[test]
    fn notes_an_advisor_message_iteration_however_its_type_is_spelled() {
        let escaped_type = ["advisor", "\\", "u005fmessage"].concat();
        for (iterations, has_advisor) in [
            r#"[{"type": "advisor_message", "model": "m"}]"#.to_owned(),
            format!(r#"[{{"type": "message"}}, {{"type": "{escaped_type}"}}]"#),
        ]
        .into_iter()
        .map(|iterations| (iterations, true))
        .chain(
            [
                r#"[{"type": "message"}, {"kind": "advisor_message"}]"#,
                r#"[{"type": ["advisor_message"]}, "advisor_message"]"#,
                r#""advisor_message""#,
            ]
            .map(|iterations| (iterations.to_owned(), false)),
        ) {
            let line =
                format!(r#"{{"input_tokens":1,"output_tokens":2,"iterations":{iterations}}}"#);
            let usage = serde_json::from_str::<IteratedTokenUsage>(&line)
                .unwrap_or_else(|error| panic!("{iterations}: {error}"));

            assert_eq!(usage.has_advisor_iteration, has_advisor, "{iterations}");
            assert_eq!(usage.usage.input_tokens, 1, "{iterations}");
        }
    }

    #[test]
    fn takes_the_split_from_message_iterations_without_a_top_level_one() {
        let usage = serde_json::from_str::<IteratedTokenUsage>(
            r#"{
                "input_tokens": 1,
                "output_tokens": 2,
                "cache_creation_input_tokens": 30,
                "iterations": [
                    {"type": "message", "cache_creation": {"ephemeral_5m_input_tokens": 10}},
                    {"type": "message", "cache_creation": {"ephemeral_1h_input_tokens": 20, "ephemeral_2h_input_tokens": 5}}
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(usage.usage.cache_creation_token_count(), 30);
        assert_eq!(
            usage
                .usage
                .cache_creation
                .map(|split| split.one_hour_tokens()),
            Some(20)
        );
    }
}
