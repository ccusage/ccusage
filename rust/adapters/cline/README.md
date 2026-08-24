# ccusage-adapter-cline

The Cline adapter: it turns Cline session transcripts
into the usage entries the reports render.

## Owns

- `loader.rs` — reading the source, dedupe, and date filtering.
- `parser.rs` — raw record parsing, token mapping, and model naming.
- `paths.rs` — environment variables, default directories, and file discovery.
- `report.rs` — the JSON and table shapes where they differ from the shared ones.

## Data source

Cline writes the same `*.messages.json` per-message transcript format in two
places, and both are discovered:

- `${CLINE_HOME:-~/.cline}` — the Cline CLI and the JetBrains plugin share
  `~/.cline` (`%USERPROFILE%\.cline` on Windows), with historical sessions
  under `data/sessions`. The whole root is scanned.
- VS Code extension `globalStorage` — `User/globalStorage/saoudrizwan.claude-dev`
  under every stock VS Code user-data root:
  - Linux: `~/.config/{Code,Code - Insiders,VSCodium}/User`
  - macOS: `~/Library/Application Support/{Code,Code - Insiders,VSCodium}/User`
  - Windows: `%APPDATA%\{Code,Code - Insiders,VSCodium}\User`

`CLINE_HOME` accepts a comma-separated list of directories and takes
precedence over all default roots; a blank value falls back to the defaults.

Each assistant message carries its own `modelInfo.id` and `metrics`
(token counts + cost), so sessions that switch models mid-conversation show
up as separate per-model rows in the report.

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
