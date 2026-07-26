# Codex Data Source (Beta)

![ccusage daily report focused on Codex usage](/codex-cli.jpeg)

> ⚠️ Codex log support is experimental while the Codex CLI log format continues to evolve.

ccusage can read OpenAI Codex CLI session logs as one of its supported local data sources. Codex uses the same unified and focused report model as Claude Code, OpenCode, Amp, Droid, Codebuff, Hermes Agent, pi-agent, Goose, OpenClaw, Kilo, Kimi, Qwen, GitHub Copilot CLI, and Gemini CLI.

## Focused Views

```bash
# Daily Codex usage
ccusage codex daily

# Monthly Codex usage
ccusage codex monthly

# Codex sessions
ccusage codex session
```

Most users can start with unified reports such as `ccusage daily`. Add the `codex` namespace only when you want to focus the same report shape on Codex usage or pass Codex-specific options such as `--speed`.

## Data Source

The CLI reads Codex session JSONL files located under `CODEX_HOME` (defaults to `~/.codex`). `CODEX_HOME` can be one directory or a comma-separated list of directories. For each entry, ccusage discovers `sessions/` and `archived_sessions/` independently, so an entry with only `archived_sessions/` still contributes archived Codex logs. When neither directory exists, the entry is read directly as a JSONL directory, which lets saved `codex exec --json` output live beside normal Codex homes. If the same relative JSONL path exists in both `sessions/` and `archived_sessions/` for one Codex home, the active `sessions/` copy wins so archived copies are not double counted.

```bash
CODEX_HOME="$HOME/.codex,$HOME/.codex-work,$HOME/codex-exec-logs" ccusage codex daily
```

## Report Views

| Focused view            | Description                  | See also                                |
| ----------------------- | ---------------------------- | --------------------------------------- |
| `ccusage codex daily`   | Aggregate usage by date      | [Daily Usage](/guide/daily-reports)     |
| `ccusage codex monthly` | Aggregate usage by month     | [Monthly Usage](/guide/monthly-reports) |
| `ccusage codex session` | Group usage by Codex session | [Session Usage](/guide/session-reports) |

These views support `--json`, `--compact`, `--offline`, and `--speed auto|standard|fast`.

## Monthly Example

![ccusage monthly report focused on Codex usage](/codex-cli-monthly.jpeg)

## What Gets Calculated

- **Token deltas** – Each `event_msg` with `payload.type === "token_count"` reports cumulative totals and, when available, the latest request delta. Current MultiAgent V2 subagent rollouts can persist a replayed parent-history prefix; the CLI uses the final inherited snapshot as the child baseline, then counts only advancing usage from the child turn. Older Codex replay formats retain timestamp-based compatibility handling.
- **Per-model grouping** – The active `turn_context` specifies the model for newly counted usage. Replayed parent contexts in current MultiAgent V2 subagent prefixes remain inherited history and do not add model usage to the child. We aggregate tokens per day/month and per model. Sessions lacking model metadata (seen in early September 2025 builds) are skipped.
- **Pricing** – Rates come from LiteLLM's pricing dataset via the shared `LiteLLMPricingFetcher`. Codex's internal review label is resolved to the newest known model for the log date before pricing is calculated.
- **Speed pricing** – `--speed auto` is the default. For rollouts written by Codex CLI 0.144.0 and later, ccusage applies recorded `thread_settings_applied` tier changes chronologically: `priority` and legacy `fast` use Fast pricing, while `default` uses Standard pricing. Unmarked usage falls back to `config.toml` detection. Pass `--speed fast` or `--speed standard` to override every recorded tier. Fast pricing uses a model-specific multiplier only when one is available; otherwise, ccusage keeps standard pricing rather than inventing a rate.
- **Legacy fallback** – Early September 2025 logs that never recorded `turn_context` metadata are still included; the CLI assumes `gpt-5` for pricing so you can review the tokens even though the model tag is missing (the JSON output also marks these rows with `"isFallback": true`).
- **Cost formula** – Non-cached input uses the standard input price; cached input uses the cache-read price (falling back to the input price when missing); and output tokens are billed at the output price. All prices are per million tokens. Reasoning tokens may be shown for reference, but they are part of the output charge and are not billed separately.
- **Totals and reports** – Daily, monthly, and session views display per-model breakdowns, overall totals, and optional JSON for automation.

## Environment Variables

| Variable     | Description                                                                                                                  |
| ------------ | ---------------------------------------------------------------------------------------------------------------------------- |
| `CODEX_HOME` | Override the root directory, or comma-separated directories, containing Codex homes or saved `codex exec --json` JSONL files |
| `LOG_LEVEL`  | Adjust log verbosity (0 silent … 5 trace)                                                                                    |

When Codex emits a model alias, the CLI automatically resolves it through the LiteLLM pricing data when possible. For `codex-auto-review`, the Codex parser maps the label to the newest known Codex/OpenAI model available on the log date using a pinned models.dev snapshot before pricing uses the resolved model name. No manual override is needed.

## Speed Pricing

By default, `ccusage codex` uses `--speed auto`. Codex CLI 0.144.0 and later can persist `thread_settings_applied` events in session rollouts. ccusage associates each token event with the most recent recognized setting: `service_tier = "priority"` or legacy `"fast"` is Fast, and `"default"` is Standard. This supports sessions that switch modes over time instead of applying one multiplier to the entire day or session.

Some usage remains unclassified, including older rollouts, saved headless `codex exec --json` output, and startup usage before the first persisted settings event. For only that unclassified portion, auto mode reads `config.toml` from each `CODEX_HOME` root and uses Fast when any root has `service_tier = "priority"` or legacy `service_tier = "fast"`; otherwise it uses Standard. An unsupported recorded tier is also left unclassified rather than inheriting a stale Fast value. Explicit `--speed fast` and `--speed standard` override all recorded and fallback tiers.

Fast pricing uses a model-specific multiplier only when one is published. GPT-5.6 Sol, Terra, and Luna use the [documented 2× API Priority rate](https://learn.chatgpt.com/docs/agent-configuration/speed#fast-mode). This is distinct from the 2.5× ChatGPT credit-consumption rate shown for GPT-5.6 Fast mode: ccusage's `costUSD` is an API-equivalent estimate, not a ChatGPT credit balance. When a model's Fast rate is unknown, ccusage reports standard pricing rather than assuming a multiplier, which may underestimate actual Fast usage.

```bash
# Default: use recorded tiers, then config.toml for unmarked usage
ccusage codex daily --speed auto

# Force fast pricing
ccusage codex daily --speed fast

# Force standard pricing
ccusage codex daily --speed standard
```

## JSON Output

Codex focused views use the same JSON mode as the shared reports:

```bash
ccusage codex daily --json
ccusage codex monthly --json
ccusage codex session --json
```

Session JSON includes per-model breakdowns, cached token counts, `lastActivity`, and `isFallback` flags for any events that required the legacy `gpt-5` pricing fallback.

Have feedback or ideas? [Open an issue](https://github.com/ccusage/ccusage/issues/new) so we can improve Codex support.

## Troubleshooting

::: details Why are there no entries before September 2025?
OpenAI's Codex CLI started emitting `token_count` events in [commit 0269096](https://github.com/openai/codex/commit/0269096229e8c8bd95185173706807dc10838c7a) (2025-09-06). Earlier session logs simply don't contain token usage metrics, so `ccusage codex` has nothing to aggregate. If you need historic data, rerun those sessions after that Codex update.
:::

::: details What if some September 2025 sessions still get skipped?
During the 2025-09 rollouts a few Codex builds emitted `token_count` events without the matching `turn_context` metadata, so the CLI could not determine which model generated the tokens. Those entries are ignored to avoid mispriced reports. If you encounter this, relaunch the Codex CLI to generate fresh logs—the current builds restore the missing metadata.
:::
