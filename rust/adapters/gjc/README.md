# ccusage-adapter-gjc

GJC usage-source adapter for the native ccusage CLI.

The adapter reads `*.jsonl` session transcripts recursively from
`~/.gjc/agent/sessions`. Set `GJC_CONFIG_DIR` to use a different GJC config
root.

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
