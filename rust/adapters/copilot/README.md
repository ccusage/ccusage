# ccusage-adapter-copilot

The GitHub Copilot CLI adapter: it turns Copilot session-state and OpenTelemetry JSONL files
into the usage entries the reports render.

## Data source

- `${COPILOT_HOME:-~/.copilot}/session-state/*/events.jsonl`
- `${COPILOT_HOME:-~/.copilot}/otel/**/*.jsonl`
- `COPILOT_HOME` (single relocated Copilot data root)
- `COPILOT_OTEL_FILE_EXPORTER_PATH` (one explicit JSONL file)

Usage records without a valid timestamp are skipped with a diagnostic.

Session-state `session.usage_record` events are per-call receipts. A receipt's `accounting`
identity (`sourceSessionId`, `sequence`) is unique across all session files, so the loader counts
each identity once even when the receipt appears in several files. It keeps the copy in the file
whose session is `sourceSessionId`, or else the copy in the lowest session ID. A receipt without a
`sourceSessionId` or a positive `sequence` is deduplicated by its event `id` within its own file.
A receipt without tokens still counts as a request. Shutdown `modelMetrics` total every call before
them, including receipted ones, so once a session has a receipt its later shutdowns and checkpoints
are ignored. For token reporting, a shutdown or checkpoint with a positive `usageAccountingWatermarks` entry is ignored the
same way, along with every later one, even without a receipt in its file: its watermarks say its
totals include receipts that may be stored with the session they belong to. Each receipt is one
entry at its own timestamp, outside the shutdown interval reconciliation below. A receipt replaces at
most one OpenTelemetry row of the same model. Every receipt first takes the row whose
`github.copilot.service_request_id` matches its `serviceRequestId`, since that ID names one request.
VS Code reports one response ID on every call of a turn, so receipts still without a row then take,
in time order, the nearest remaining row whose `gen_ai.response.id` matches their `apiCallId`. A row
with neither ID is never matched, so a call with both a receipt and such a row counts twice. Receipt matching removes no other row; the
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
available. OpenTelemetry identities are reconciled across all OpenTelemetry files at once, so the
result does not depend on how rows are split between files. A row without a model or session takes them from
the other records of its trace in any OpenTelemetry file. A row whose key built from its trace
and span IDs (or trace ID and turn index) was already seen counts once, and chat spans the CLI
exports twice, with the response ID and usage of one call, count once at the earliest timestamp;
keys that fall back to the row's line or time are file-local and never match across files. Copilot counts cache reads and writes in session-state `inputTokens` and in OpenTelemetry
`gen_ai.usage.input_tokens`, so the adapter reports the uncached remainder as input and keeps the
cache buckets separate. Session-state reasoning tokens are already included in output tokens; OpenTelemetry
reasoning is included when total usage metadata shows it is separate. Internal model suffixes such
as `-1m` and `-1m-internal` are removed for reporting and source deduplication.
Pricing tries the logged ID first for identity matches. The ID with the
suffix removed also supports fuzzy matches.

Copilot records what it billed in nano-AIU (1 AIU is one GitHub AI credit, $0.01): a receipt's
`copilotUsage.totalNanoAiu` for its call, and a shutdown's per-model `totalNanoAiu` for every call
of that model so far. These are the entries' precomputed cost, per receipt and per shutdown
interval as the credits it adds over its predecessor; an interval that lacks credits on either
snapshot, a receipt without them, and every OpenTelemetry row have none. Through the shared cost
modes, `auto` uses the recorded cost and prices entries without one from their tokens, `display`
uses the recorded cost and reports $0 for entries without one, and `calculate` prices every entry
from its tokens. An interval whose credits grow without new tokens is still reported. `usage.cost`
and `requests.cost` are premium-request multipliers and are ignored.

`modelMetrics` only covers the running Copilot process, while the session-wide `totalNanoAiu`
on `session.shutdown` and `session.usage_checkpoint` covers the whole session (1 AIU is one
GitHub AI credit, $0.01). At each shutdown and each checkpoint after the last shutdown, the
adapter subtracts the recorded model credits and any credits covered by a verified local receipt
watermark. A watermark needs a contiguous sequence of priced receipts that follows any legacy
shutdown or checkpoint. An OpenTelemetry span covered by a shutdown or a same-session receipt
adds no credits to reconcile. A retained span with unknown billed credits leaves subsequent
residuals unresolved. A residual that persists across later snapshots becomes a cost-only
`unknown` entry at the first snapshot where it persists. The credit reconciliation uses the full
ledger before applying date filters. It requires a checkpoint and complete shutdown credit
totals.

## Build layer

Built in the `adapters` Crane artifact layer.
