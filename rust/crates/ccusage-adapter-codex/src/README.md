# Codex Source

Data source:

```text
${CODEX_HOME:-~/.codex}/sessions/
${CODEX_HOME:-~/.codex}/archived_sessions/
```

When both directories contain the same relative JSONL path for one Codex home,
the active `sessions/` copy wins.

Relevant JSONL event:

- `type === "event_msg"`
- `payload.type === "token_count"`
- `payload.info.total_token_usage` is cumulative.
- `payload.info.last_token_usage` is the current turn delta.
- If only cumulative totals exist, subtract prior totals to recover deltas.

Relevant speed-setting event in Codex CLI 0.144.0 and later:

- `type === "event_msg"`
- `payload.type === "thread_settings_applied"`
- `payload.thread_settings.service_tier === "priority"` (or legacy `"fast"`) selects Fast.
- `payload.thread_settings.service_tier === "default"` selects Standard. Codex Desktop spells the same tier `"standard"`; both appear in the same CLI version, so this is a value mapping and not a version split.
- Token usage inherits the latest recognized setting in the rollout. A settings event without a `service_tier` key leaves the previous tier in place (auto-review threads emit these); a tier that is present but unrecognized clears it so a stale value is not inherited.
- `thread_settings_applied` is not emitted per turn, so short rollouts carry no tier at all and stay unclassified for report policy to resolve.

Relevant MultiAgent V2 subagent replay markers:

- A subagent rollout replays its parent's history before the child's own turn, so the prefix must not be counted again.
- `type === "event_msg"` with `payload.type === "task_started"` ends the replayed prefix.
- `type === "inter_agent_communication_metadata"` with `payload.trigger_turn === true` starts the child turn. The event was named `inter_agent_communication` before Codex CLI 0.143.0-alpha.15; both spellings are matched. `trigger_turn` is also emitted as `false`, so the value must be checked.
- Usage between those two markers belongs to the child; usage before them is inherited history.
- Which replay boundary a rollout uses is decided by scanning it for the marker, never from `session_meta`. Resuming a session re-records the original session's `cli_version`, so that field cannot be used to gate behavior.

Token mapping:

- `input_tokens` - total input tokens.
- `cached_input_tokens` - cached prompt tokens.
- `output_tokens` - completion tokens, including reasoning cost.
- `reasoning_output_tokens` - informational breakdown; already included in output billing.
- `total_tokens` - provided directly or recomputed as input plus output for legacy entries.

Pricing uses model metadata from `turn_context`. Early sessions without metadata fall back to `gpt-5`, mark `isFallbackModel === true`, and expose fallback rows as approximate in aggregate JSON.
