# ccusage-adapter-droid

The Factory Droid adapter: it turns session JSON files
into the usage entries the reports render.

## Data source

- `${DROID_SESSIONS_DIR:-~/.factory/sessions}/**/*.json`

A settings file uses its provider-lock timestamp, falling back to the file's
modification time. If neither is usable, it is skipped with an invalid-timestamp
diagnostic. A modification-time fallback does not select historical pricing.
Pricing keeps the model ID from settings or the sidecar as a candidate ahead
of the derived report label. An explicit provider lock also supplies strict
provider-qualified candidates.

## Build layer

Built in the `adapters` Crane artifact layer.
