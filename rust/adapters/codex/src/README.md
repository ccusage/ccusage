# Codex Source

Data source:

```text
${CODEX_HOME:-~/.codex}/sessions/
${CODEX_HOME:-~/.codex}/archived_sessions/
```

Rollouts are plain `.jsonl` or Zstandard-compressed `.jsonl.zst`. Compression does not
change the session ID. When both encodings of a rollout exist in one directory, the
plain copy wins, as it does in Codex. When both directories contain the same relative
rollout path for one Codex home, the active `sessions/` copy wins regardless of encoding.

Relevant JSONL event:

- `type === "event_msg"`
- `payload.type === "token_count"`
- `payload.info.total_token_usage` is cumulative.
- `payload.info.last_token_usage` is the current turn delta.
- If only cumulative totals exist, subtract prior totals to recover deltas.
- Valid cumulative token counts advance the baseline even when their record has no usable
  timestamp. The rejected record is not emitted and its usage is not shifted to a later dated delta.

Automatic compaction usage is recorded separately:

- A top-level `token_usage_record` carries `payload.response_id` and `payload.usage`.
- Count it only when its response ID matches a `compacted` item's `payload.compaction_response_id`, in either record order.
- Local compactions also emit an advancing `token_count` between the response record and marker. When that snapshot accounts for the record's request usage or matches its cumulative `thread_token_usage`, do not add the usage again. Remote v2 compactions lack this advance.
- Keep its request timestamp and recorded service tier. Missing model metadata uses the active `turn_context` and marks the attribution as a fallback.
- Compaction usage does not advance the normal cumulative baseline or change the active turn model.
- Deduplicate copied compactions by response ID, independently of normal token-count replay matching. Retain the first file's session, timestamp, and model attribution even when files are parsed in parallel; merge service tiers across copies.
- Preserve all matched parent compaction IDs in the replay plan, including usage already covered by a cumulative snapshot, so a date-bounded child report does not count a copied request when its parent lies outside the report range.

Relevant speed-setting event in Codex CLI 0.144.0 and later:

- `type === "event_msg"`
- `payload.type === "thread_settings_applied"`
- `payload.thread_settings.service_tier === "priority"` (or legacy `"fast"`) selects Fast.
- `payload.thread_settings.service_tier === "flex"` selects Flex.
- `payload.thread_settings.service_tier === "ultrafast"` selects Ultrafast.
- `payload.thread_settings.service_tier === "default"` selects Standard. Codex Desktop spells the same tier `"standard"`; both appear in the same CLI version, so this is a value mapping and not a version split.
- Token usage inherits the latest recognized setting in the rollout. A settings event without a `service_tier` key leaves the previous tier in place (auto-review threads emit these); a tier that is present but unrecognized clears it so a stale value is not inherited.
- `thread_settings_applied` is not emitted per turn, so short rollouts carry no tier at all and stay unclassified for report policy to resolve.

Unclassified usage uses the `service_tier` from the retained rollout's Codex
home. Read the top-level value in `config.toml`, with an override from a legacy
named profile when the top-level `profile` selects it. Missing tiers and tiers
without a model-specific rate use Standard pricing. Profile selections made
through CLI flags or separate profile files require an explicit `--speed`
override for unclassified usage.

Forked rollouts:

- `session_meta.payload.forked_from_id` or `payload.source.subagent.thread_spawn.parent_thread_id` names the parent. This identifies how the rollout was forked, which resuming does not change, unlike `cli_version`.
- Codex 0.152 and later stamp `thread_settings_applied` with `payload.thread_id`, and every `token_usage_record` carries one. Copied items keep their original owner's ID. The fork boundary is the first record or settings line that names the child's own thread. Usage at or after it is the child's own. Usage between the first line naming another thread and that boundary is replayed. A settings line without a thread ID proves nothing: pre-0.152 writers emit one for a child's own turns too, so a fork created by an older release and resumed by a newer one keeps its pre-upgrade turns. A marker naming another thread with no own boundary after it proves nothing about which lines are copied. A blank thread ID counts as none, and when `session_meta` gives no child ID no marker is own or foreign. Timestamps are never consulted.
- A spawned subagent (`thread_spawn`) never inherits the parent's `token_usage_record` lines, so a record without a thread ID still ends any skip of replayed history. A record never starts a skip. A child created by a Codex release without records and resumed by a later one holds its own record-less turns before the first record.
- Independently of markers, the child's usage before its boundary (all of it when there is none) is matched against the parent's pre-fork usage stream. Its leading run is replayed when it exactly equals that stream as a prefix, or as a contiguous suffix (history copied after a parent compaction). Markers inside the run do not split it, because copied items keep their markers. The longest match is skipped. Timestamps are not consulted.
- When that parent log is missing, usage before the boundary that no marker proves copied is counted and a warning says replayed usage may be counted twice. That is the usage before the first marker naming another thread when the boundary follows one, and all usage before the boundary otherwise. An unreadable parent log keeps its own warning.
- Usage without a response ID is deduplicated across files on timestamp, model, token counts, and its ordinal among identical events in its own file. A copy repeats the original's ordinals, while identical requests within one rollout stay distinct.
- A `token_count` that accounts for the `token_usage_record` just before it reports that request, so the record's response ID joins its dedupe key. Spawned siblings that sent different requests with equal usage at the same instant then stay distinct, while a copy carrying the same records still collapses. A copy without records at the same fork depth, such as an archived copy of the rollout, collapses into one recorded request with the same usage, whichever file is read first. A recordless request at another fork depth is a separate request: a fork's own request that matches an ancestor's recorded request (or the reverse) is not a copy. Pairing and the settlement below measure fork depth against the same matches. A record ID matches its request at any position among equal-usage events. If one rollout contains more requests than the distinct records can account for, the extra recordless requests are retained. A recordless copy does not lend its service tier to a recorded request it cannot name. The event remains normal usage, so replay filtering still skips it.
- Reports match recorded requests across sessions before grouping. When a record replaces a recordless copy, session reports credit the session that carries the record. Recordless matches are settled once, before any report groups them, so daily and session reports count the same requests. A fork and its parent or other ancestor both advanced after the fork once its replayed prefix is filtered, so their matches are separate requests; matches from sessions without that relationship, such as an archived copy without fork metadata, are copies. Settlement groups the matches by fork depth, the number of distinct threads among the matches that are the session's ancestors, and counts each depth as one request credited to the first file's session at that depth. When an unrelated session C matches an ancestor A and A's fork B, C collapses with A at depth 0 while B counts at depth 1, so the three matches settle as two requests.

Token mapping:

- `input_tokens` - total input tokens.
- `cached_input_tokens` - cached prompt tokens.
- `output_tokens` - completion tokens, including reasoning cost.
- `reasoning_output_tokens` - informational breakdown; already included in output billing.
- `total_tokens` - provided directly or recomputed as input plus output for legacy entries.

Pricing uses model metadata from `turn_context`. Early sessions without metadata fall back to `gpt-5`, mark `isFallbackModel === true`, and expose fallback rows as approximate in aggregate JSON.

`codex-auto-review` is a server-routed alias and does not expose its effective model in the log. The adapter uses a manually curated timeline for API-equivalent cost estimates and keeps those events marked as fallback models. OpenAI announced a GPT-5.4 to GPT-5.6 Luna transition on 2026-07-30, so events from that date onward resolve to `gpt-5.6-luna`; events from 2026-03-05 through 2026-07-29 resolve to `gpt-5.4`. Server-side routing or catalog overrides can make the effective model differ from this estimate.
