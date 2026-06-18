# pi-agent Source

Data sources (auto-detected when `--pi-path`/`PI_AGENT_DIR` are unset):

```text
${PI_AGENT_DIR:-~/.pi/agent/sessions/}
~/.omp/agent/sessions/   # oh-my-pi (omp), a pi fork with identical JSONL
```

Commands:

```sh
ccusage pi daily
ccusage pi monthly
ccusage pi session
ccusage pi daily --json
ccusage pi daily --pi-path /path/to/sessions
```
