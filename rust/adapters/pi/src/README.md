# pi-agent Source

Data source:

```text
${PI_AGENT_DIR:-~/.pi/agent/sessions/}
```

Commands:

```sh
ccusage pi daily
ccusage pi monthly
ccusage pi session
ccusage pi daily --json
ccusage pi daily --pi-path /path/to/sessions
ccusage daily --by-provider
ccusage daily --agent 'pi[openai-codex]' --model 'gpt-*'
```

Assistant message records may include a `provider` field. Focused Pi reports aggregate it as before; unified reports can retain it for provider breakdowns and selectors.
