# ccusage-adapter-gjc

GJC usage-source adapter for the native ccusage CLI.

The adapter reads `*.jsonl` session transcripts recursively from GJC's sessions
directory. `GJC_CONFIG_DIR` selects a home-relative config directory name, while
`GJC_CODING_AGENT_DIR` selects an explicit agent directory. Existing
`$XDG_DATA_HOME/gjc` storage is detected using the same rule as GJC.

Assistant message records contribute their `message.usage` fields as follows:

- `input` → input tokens
- `output` → output tokens
- `cacheRead` → cache-read tokens
- `cacheWrite` → cache-creation tokens
- `cost.total` → the precomputed cost used by `auto` and `display` cost modes

The session record supplies the session ID and working directory. Non-assistant
messages and assistant messages without token usage are ignored.

Supported reports are `daily`, `monthly`, and `session`:

```sh
ccusage gjc daily
ccusage gjc monthly --json
ccusage gjc session
```
