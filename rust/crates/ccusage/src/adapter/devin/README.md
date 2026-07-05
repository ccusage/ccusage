# Devin Source

Data source:

```text
${DEVIN_DATA_DIR:-~/.local/share/devin/cli}
${DEVIN_DATA_DIR:-%APPDATA%\devin\cli}
```

Transcript JSON files are the primary source:

```text
${DEVIN_DATA_DIR}/transcripts/*.json
```

The adapter also reads `sessions.db` for enrichment: project path, model
fallback, timestamp fallback, and hidden-session filtering.

Transcripts follow the ATIF trajectory schema with Devin-specific extensions.
Token counts are read from `step.metrics` (ATIF v1.7) and fall back to
`step.metadata.metrics` (legacy Devin). Per-step cost is read from
`step.metadata.committed_credit_cost` (USD) when available; otherwise cost is
calculated from tokens using the embedded LiteLLM pricing snapshot.

Commands:

```sh
ccusage devin daily
ccusage devin monthly
ccusage devin session
ccusage devin daily --json
ccusage devin daily --devin-path /path/to/devin/cli
```
