# ccusage-adapter-muse

The Muse Code adapter: it turns Muse Code's event-sourced session logs
into the usage entries the reports render.

## Owns

- `paths.rs` — XDG data directories and session log discovery.
- `parser.rs` — raw record parsing, token mapping, and model naming.
- `loader.rs` — file walking, dedupe, and date filtering entry points.
- `report.rs` — the JSON and table shapes where they differ from the shared ones.

## Data source

- `$XDG_DATA_HOME/muse/sessions/YYYY/MM/DD/<session-uuid>/session.jsonl`

Set `XDG_DATA_HOME` to relocate or fixture the discovery root. An empty
`XDG_DATA_HOME` disables Muse discovery.

| OS      | Discovery roots (scanned in order)                        |
| ------- | --------------------------------------------------------- |
| Linux   | `$XDG_DATA_HOME` (default `~/.local/share`)               |
| macOS   | `$XDG_DATA_HOME` (default `~/.local/share`), `~/Library/Application Support` |
| Windows | `$XDG_DATA_HOME` (default `~/.local/share`), `%APPDATA%`  |

Muse Code currently ships Linux and macOS builds only; the macOS
`~/Library/Application Support` and Windows `%APPDATA%` candidates are scanned
defensively so discovery keeps working if Muse later writes there. All roots
are scanned and results are sorted and deduped, so overlapping roots never
double-count a log.

Each session directory holds one append-only event-sourced log. Assistant
model calls appear as `runtime.session` records whose `event.kind` is
`model_completed`, carrying the model and the token usage. Child agents log
under `subagent/<child-uuid>/session.jsonl` with their own records; they are
read too, because the parent log does not record their tokens.

Muse logs no cost, so every entry is priced from the shared pricing map by
model name; models without a published rate card surface the usual
missing-pricing warning.

## Public surface

- `loader::load_entries`
- `report::report_from_rows`
- `report::summarize_entries`
- `run`

## Depends on

- `ccusage-adapter-common`
- `ccusage-core`
- `jiff`
- `serde_json`

See `src/README.md` for the record shapes and token rules.
