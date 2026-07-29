# ccusage-adapter-antigravity

The Antigravity adapter: it turns the per-conversation SQLite databases the
Antigravity CLI writes into the usage entries the reports render.

## Commands

```sh
ccusage antigravity daily
ccusage antigravity monthly
ccusage antigravity session
```

Antigravity also joins `ccusage daily` and the other unified reports whenever
conversation databases are detected.

The user-facing documentation lives in
[docs/guide/antigravity](../../../docs/guide/antigravity/index.md), including the
accuracy notes that explain what these numbers can and cannot tell you. See
[docs/guide/all-reports](../../../docs/guide/all-reports.md) for the unified
reports and [docs/guide/cost-modes](../../../docs/guide/cost-modes.md) for how
`--mode` changes cost reporting.

## Owns

- `loader.rs` — reading the source, dedupe, and date filtering.
- `parser.rs` — token mapping, model naming, and pricing candidates.
- `paths.rs` — environment variables, default directories, and file discovery.
- `proto.rs` — the minimal protobuf wire reader for the stored blobs.
- `report.rs` — the JSON and table shapes where they differ from the shared ones.

Anything that is not specific to this source belongs in `ccusage-core` or
`ccusage-adapter-common` instead.

## Data source

- `${ANTIGRAVITY_DATA_DIR}/conversations/**/*.db`, or
- `~/.gemini/antigravity-cli/conversations/**/*.db`

`ANTIGRAVITY_DATA_DIR` accepts comma-separated roots, and each root's
`conversations/` directory is searched recursively, so nested sub-conversation
databases are collected too.

Antigravity is a Gemini-family tool, so its CLI state lives under the shared
`.gemini` directory. Reads SQLite with the bundled `sqlite` crate, which is why
this crate declares it and most adapters do not.

Usage is read from two tables per conversation:

- `steps.metadata` (`CortexStepMetadata`) — one row per invocation, including the
  background calls no other table records. This is the primary source.
- `gen_metadata.data` (`ChatModelMetadata`) — the only place a human-readable
  model name appears, and a backstop for a step that has since been pruned.

## Wire format

Antigravity ships no `.proto` files, so `proto.rs` decodes the blobs by field
number. The numbers were recovered from the `FileDescriptorProto` blobs embedded
in the `agy` binary rather than guessed from the data, and Antigravity has to read
these same blobs back after an upgrade, so it cannot renumber a field without
breaking its own persistence.

The fields consumed, from `ModelUsageStats`:

| Field | Name                     | Mapped to                     |
| ----- | ------------------------ | ----------------------------- |
| 1     | `model`                  | placeholder name when unnamed |
| 2     | `input_tokens`           | `inputTokens`                 |
| 3     | `output_tokens`          | `outputTokens`                |
| 4     | `cache_write_tokens`     | `cacheCreationTokens`         |
| 5     | `cache_read_tokens`      | `cacheReadTokens`             |
| 9     | `thinking_output_tokens` | fallback for field 3          |
| 10    | `response_output_tokens` | fallback for field 3          |
| 7     | `message_id`             | dedup identity                |
| 11    | `response_id`            | dedup identity                |
| 12    | `provider_assigned_...`  | dedup identity                |

`input_tokens` and `cache_read_tokens` are disjoint, so neither needs adjusting: a
real record with `input_tokens` of 4050 against `cache_read_tokens` of 16275 rules
out the total-inclusive reading. `output_tokens` already contains
`thinking_output_tokens`, so the thinking count is only ever used to rebuild a
missing total.

## Dedupe

Every collected invocation is deduplicated on `response_id`, falling back to
`provider_assigned_message_id` then `message_id`. One model call can be recorded
in more than one place — `retry_infos` repeats the attempt that succeeded
alongside those that failed, and a sub-conversation may repeat a call its parent
already recorded — and this is what keeps those from being counted twice while
still counting the failed attempts, which consumed tokens of their own.

## Cost

Antigravity leaves `model_cost`, `credit_cost` and `consumed_credits` unset on
consumer accounts, so there is no precomputed cost to read and `--mode display`
reports zero. Cost is calculated from LiteLLM rates for the `response_model` name,
which makes it the equivalent public API cost rather than an amount billed.

Invocations `gen_metadata` never named are reported under
`antigravity-model-<id>`. The model id is an opaque number and Antigravity's own
descriptor only names those as `MODEL_PLACEHOLDER_M<n>`, so they cannot be priced;
their tokens are still counted and they surface as missing pricing.

## Public surface

- `loader::load_entries`
- `report::summarize_entries`
- `run`

## Depends on

- `ccusage-adapter-common`
- `ccusage-core`
- `jiff`
- `serde_json`
- `sqlite`

## Build layer

Built in the `adapters` Crane artifact layer; the layer compiles all adapters in one Cargo invocation, so they build concurrently.
