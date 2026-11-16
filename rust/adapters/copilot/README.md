# ccusage-adapter-copilot

The GitHub Copilot CLI adapter: it turns Copilot session-state and OpenTelemetry JSONL files
into the usage entries the reports render.

## Owns

- `loader.rs` — reading the source, dedupe, and date filtering.
- `parser.rs` — raw record parsing, token mapping, and model naming.
- `paths.rs` — environment variables, default directories, and file discovery.
- `report.rs` — the JSON and table shapes where they differ from the shared ones.

Anything that is not specific to this source belongs in `ccusage-core` or
`ccusage-adapter-common` instead.

## Data source

- `${COPILOT_HOME:-~/.copilot}/session-state/*/events.jsonl`
- `${COPILOT_HOME:-~/.copilot}/otel/**/*.jsonl`
- `COPILOT_HOME` (single relocated Copilot data root)
- `COPILOT_OTEL_FILE_EXPORTER_PATH` (one explicit JSONL file)

Session-state `session.usage_record` events are per-call receipts. A receipt's `accounting`
identity (`sourceSessionId`, `sequence`) is unique across all session files, so the loader counts
each identity once even when the receipt appears in several files. It keeps the copy in the file
whose session is `sourceSessionId`, or else the copy in the lowest session ID. A receipt without a
`sourceSessionId` or a positive `sequence` is deduplicated by its event `id` within its own file.
A receipt without tokens still counts as a request. Shutdown `modelMetrics` total every call before
them, including receipted ones, so once a session has a receipt its later shutdowns and checkpoints
are ignored. Each receipt is one entry at its own timestamp, outside the shutdown interval
reconciliation below. A receipt replaces at most one OpenTelemetry row: one with the same model that
shares its `apiCallId` (`gen_ai.response.id`) or `serviceRequestId`
(`github.copilot.service_request_id`), nearest in time. VS Code reports one response ID on every call
of a turn, so an ID alone can name several calls. A row with neither ID is never matched, so a call
with both a receipt and such a row counts twice. Receipt matching removes no other row; the
shutdown reconciliation below still applies to OpenTelemetry rows of the shutdowns kept before the
first receipt.

Session-state shutdown records are cumulative per canonical `(session, model)` pair, so
resumed sessions emit one shutdown per resume. The adapter reports each snapshot as interval
usage: the first snapshot is kept as-is and each later snapshot subtracts its predecessor, so
daily attribution follows the resume cadence while totals stay unchanged. The intervals are
preferred for a pair when both sources contain it: matching OpenTelemetry rows are suppressed
when their timestamps are at or before the latest raw shutdown visible through `--until`, and rows
emitted after it by a resumed session are retained. A Copilot process restores the breakdown of
the session's last shutdown, so the calls of a process that ended without one (killed, crashed, or
restarted by the app) are in no shutdown. When OpenTelemetry holds more calls for a pair than its
shutdowns report, counted with `requests.count` from the first shutdown interval that has
OpenTelemetry rows, those rows replace the intervals from there on. The comparison covers every
shutdown, so it does not depend on `--since` or `--until`. Other OpenTelemetry records remain
available. Chat spans the CLI exports twice, with the response ID and usage of one call, count
once. Copilot counts cache reads and writes in session-state `inputTokens` and in OpenTelemetry
`gen_ai.usage.input_tokens`, so the adapter reports the uncached remainder as input and keeps the
cache buckets separate. Session-state reasoning tokens are already included in output tokens; OpenTelemetry
reasoning is included when total usage metadata shows it is separate. Internal model suffixes such
as `-1m` and `-1m-internal` are removed before pricing and source deduplication.

`modelMetrics` only covers the running Copilot process, while the session-wide `totalNanoAiu`
on `session.shutdown` and `session.usage_checkpoint` covers the whole session (1 AIU is one
GitHub AI credit, $0.01). At every shutdown, and at checkpoints after the last one, the session total
minus the per-model `totalNanoAiu` reported so far is unexplained; what stays unexplained at every
later snapshot becomes cost-only `unknown` entries, dated where it first stays. Sessions without checkpoints
(older Copilot versions restart both totals on every resume), with a shutdown missing either total,
with usage records, or with OpenTelemetry rows get no such entries.

Reads plain files through `ccusage-adapter-common`, which handles walking, size-balanced
chunking, and ordered parallel reads.

## Public surface

- `loader::load_entries`
- `report::report_from_rows`
- `report::summarize_entries`
- `run`

## Depends on

- `ccusage-adapter-common`
- `ccusage-core`
- `jiff`
- `serde`
- `serde_json`

## Build layer

Built in the `adapters` Crane artifact layer; the layer compiles all adapters in one Cargo invocation, so they build concurrently.
