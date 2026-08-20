# ccusage-adapter-antigravity

Adapter for mining token telemetry from Antigravity session databases.

Antigravity stores session state and generation telemetry in SQLite databases located at `~/.gemini/antigravity-cli/conversations/<conversation-id>.db` (or overridden via `ANTIGRAVITY_HOME` / `ANTIGRAVITY_DATA_DIR`).
Telemetry records are encoded in Protobuf binary format inside the `steps` and `gen_metadata` tables.
