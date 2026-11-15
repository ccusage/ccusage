use ccusage_adapter_common::source::UsageSource;

use crate::adapter::{
    amp, antigravity, claude, cline, codebuff, codex, copilot, droid, gemini, goose, grok, hermes,
    kilo, kimi, openclaw, opencode, pi, qwen, zcode,
};

pub(super) static SOURCES: [&dyn UsageSource; 19] = [
    &claude::SOURCE,
    &codex::SOURCE,
    &opencode::SOURCE,
    &amp::SOURCE,
    &droid::SOURCE,
    &codebuff::SOURCE,
    &hermes::SOURCE,
    &pi::SOURCE,
    &goose::SOURCE,
    &openclaw::SOURCE,
    &kilo::SOURCE,
    &copilot::SOURCE,
    &gemini::SOURCE,
    &antigravity::SOURCE,
    &kimi::SOURCE,
    &qwen::SOURCE,
    &grok::SOURCE,
    &zcode::SOURCE,
    &cline::SOURCE,
];

#[cfg(test)]
mod tests {
    use super::SOURCES;

    #[test]
    fn registry_matches_the_cli_source_order() {
        let registered = SOURCES.iter().map(|source| source.id()).collect::<Vec<_>>();
        assert_eq!(registered, ccusage_core::BUILT_IN_AGENT_NAMES);
        assert!(SOURCES.iter().all(|source| !source.reports().is_empty()));
    }
}
