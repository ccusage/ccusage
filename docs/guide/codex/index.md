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

These views support `--json`, `--compact`, `--offline`, and `--speed auto|standard|flex|fast`.

## Monthly Example

![ccusage monthly report focused on Codex usage](/codex-cli-monthly.jpeg)

## What Gets Calculated

- **Token deltas** – Each `event_msg` with `payload.type === "token_count"` reports cumulative totals and, when available, the latest request delta. Current MultiAgent V2 subagent rollouts can persist a replayed parent-history prefix; the CLI uses the final inherited snapshot as the child baseline, then counts only advancing usage from the child turn. Older Codex replay formats retain timestamp-based compatibility handling.
- **Automatic compaction** – Codex can record remote compaction requests separately from cumulative token counts. ccusage includes a `token_usage_record` only when its response ID matches a `compacted` record and the intervening token-count snapshot has not already accounted for it. Copied response IDs are counted once across reports, including when the parent session is outside the selected date range. A missing model uses the active `turn_context` and is marked as a fallback estimate. Additional compaction usage does not change the normal cumulative baseline.
- **Per-model grouping** – The active `turn_context` specifies the model for newly counted usage. Replayed parent contexts in current MultiAgent V2 subagent prefixes remain inherited history and do not add model usage to the child. We aggregate tokens per day/month and per model. Sessions lacking model metadata (seen in early September 2025 builds) are skipped.
- **Pricing** – Rates come from LiteLLM's pricing dataset via the shared `LiteLLMPricingFetcher`. Codex's internal review label uses the manually curated, date-based fallback timeline below and remains marked as approximate.
- **Scheduled pricing** – DeepSeek V4 Flash and Pro use each event's timestamp: legacy rates apply before `2026-08-16T16:00:00Z`, and the later rates use UTC weekday peak windows of `01:00–04:00` and `06:00–10:00` (endpoints excluded). Cache creation follows the scheduled input rate.
- **Speed pricing** uses `--speed auto` by default. For rollouts written by Codex CLI 0.144.0 and later, ccusage applies recorded `thread_settings_applied` tier changes chronologically. `priority` and legacy `fast` use Fast pricing, `flex` uses Flex pricing, and `default` or `standard` uses Standard pricing. Unmarked usage falls back to `config.toml` detection. Pass `--speed standard`, `--speed flex`, or `--speed fast` to override every recorded tier. Fast and Flex pricing use model-specific multipliers only when one is available; otherwise, ccusage keeps standard pricing rather than inventing a rate.
- **Legacy fallback** – Early September 2025 logs that never recorded `turn_context` metadata are still included; the CLI assumes `gpt-5` for pricing so you can review the tokens even though the model tag is missing (the JSON output also marks these rows with `"isFallback": true`).
- **Cost formula** – Non-cached input uses the standard input price; cached input uses the cache-read price (falling back to the input price when missing); and output tokens are billed at the output price. All prices are per million tokens. Reasoning tokens may be shown for reference, but they are part of the output charge and are not billed separately.
- **Totals and reports** – Daily, monthly, and session views display per-model breakdowns, overall totals, and optional JSON for automation.

Reports are reconstructed from retained local logs. Missing compaction usage records cannot be recovered from their markers alone, and local totals can differ from account-wide Codex usage. Pricing aliases such as `gpt-reserve` affect cost estimates, without changing token totals or revealing an unrecorded backend model.

## Environment Variables

| Variable     | Description                                                                                                                  |
| ------------ | ---------------------------------------------------------------------------------------------------------------------------- |
| `CODEX_HOME` | Override the root directory, or comma-separated directories, containing Codex homes or saved `codex exec --json` JSONL files |
| `LOG_LEVEL`  | Adjust log verbosity (0 silent … 5 trace)                                                                                    |

When Codex emits a model alias, the CLI automatically resolves it through the LiteLLM pricing data when possible. The built-in `gpt-reserve` alias is priced as `gpt-5.6-luna`. Codex logs retain `codex-auto-review` as a routing alias rather than recording its effective model, so ccusage applies a manually curated, best-effort timeline and marks the result with `"isFallback": true`. Based on OpenAI's [July 30, 2026 Auto-review migration announcement](https://community.openai.com/t/announcing-a-major-price-drop-for-5-6-terra-and-luna-and-fast-mode-for-5-6-sol/1388484), records from that date onward resolve to `gpt-5.6-luna`, while records from March 5 through July 29 resolve to `gpt-5.4`. Server-side routing or catalog overrides can still differ from this estimate.

## Speed Pricing

By default, `ccusage codex` uses `--speed auto`. Codex CLI 0.144.0 and later can persist `thread_settings_applied` events in session rollouts. ccusage associates each token event with the most recent recognized setting. `service_tier = "priority"` or legacy `"fast"` is Fast, `"flex"` is Flex, and `"default"` or `"standard"` is Standard. This supports sessions that switch modes over time instead of applying one multiplier to the entire day or session.

Some usage remains unclassified, including older rollouts, saved headless `codex exec --json` output, and startup usage before the first persisted settings event. For only that unclassified portion, auto mode reads `config.toml` from each `CODEX_HOME` root and uses Fast when any root has `service_tier = "priority"` or legacy `service_tier = "fast"`. If no root requests Fast and a root requests `service_tier = "flex"`, it uses Flex. Otherwise it uses Standard. An unsupported recorded tier is also left unclassified rather than inheriting a stale tier. Explicit `--speed standard`, `--speed flex`, and `--speed fast` override all recorded and fallback tiers.

Fallback detection reads the top-level `service_tier` and lets a named profile override it only when `profile = "name"` selects that profile in `config.toml`. Inactive profiles are ignored, and a selected profile without its own tier inherits the top-level setting. If Codex selected a profile through `--profile` or another configuration file, use an explicit `--speed` for unclassified usage because ccusage cannot infer that selection from `config.toml`.

Fast and Flex pricing use model-specific multipliers only when one is published. GPT-5.6 Sol, Terra, and Luna use the [documented 2× API Priority rate](https://learn.chatgpt.com/docs/agent-configuration/speed#fast-mode), and GPT-6 [Astra](https://developers.openai.com/api/docs/models/gpt-6-astra), [Sol](https://developers.openai.com/api/docs/models/gpt-6-sol), and [Luna](https://developers.openai.com/api/docs/models/gpt-6-luna) and [GPT-6.1 Sol](https://developers.openai.com/api/docs/models/gpt-6.1-sol) use their documented 2× Fast rates. Models listed in OpenAI's [Flex pricing table](https://developers.openai.com/api/docs/pricing) use 0.5× Standard pricing, including dated and provider-qualified names. These are distinct from ChatGPT credit-consumption rates. ccusage's `costUSD` is an API-equivalent estimate, not a ChatGPT credit balance. When a model's Fast or Flex rate is unknown, ccusage reports standard pricing rather than assuming a multiplier, which may underestimate Fast usage or overestimate Flex usage.

Use [`pricingOverrides`](/guide/config-files#pricing-overrides) to configure a model's `fastMultiplier` or `flexMultiplier`.

```bash
# Default: use recorded tiers, then config.toml for unmarked usage
ccusage codex daily --speed auto

# Force fast pricing
ccusage codex daily --speed fast

# Force Flex pricing
ccusage codex daily --speed flex

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

Session JSON includes per-model breakdowns, cached token counts, `lastActivity`, and `isFallback` flags for events that required either the legacy `gpt-5` pricing fallback or the manually curated `codex-auto-review` timeline.

Have feedback or ideas? [Open an issue](https://github.com/ccusage/ccusage/issues/new) so we can improve Codex support.

## Troubleshooting

::: details Why are there no entries before September 2025?
OpenAI's Codex CLI started emitting `token_count` events in [commit 0269096](https://github.com/openai/codex/commit/0269096229e8c8bd95185173706807dc10838c7a) (2025-09-06). Earlier session logs simply don't contain token usage metrics, so `ccusage codex` has nothing to aggregate. If you need historic data, rerun those sessions after that Codex update.
:::

::: details What if some September 2025 sessions still get skipped?
During the 2025-09 rollouts a few Codex builds emitted `token_count` events without the matching `turn_context` metadata, so the CLI could not determine which model generated the tokens. Those entries are ignored to avoid mispriced reports. If you encounter this, relaunch the Codex CLI to generate fresh logs—the current builds restore the missing metadata.
:::
