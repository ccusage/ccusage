# Cost Modes

Use `--mode auto`, `--mode calculate`, or `--mode display` to select how a report obtains costs. The default is `auto`.

The entry rules below describe Claude Code. Auto's stored-cost preference depends on the source:

| Source                                                                           | Auto                                                                          | Calculate | Display                                         |
| -------------------------------------------------------------------------------- | ----------------------------------------------------------------------------- | --------- | ----------------------------------------------- |
| Claude Code, Amp, Cline, OpenClaw                                                | Recorded cost when present, including zero; otherwise tokens                  | Tokens    | Recorded cost only                              |
| Codex, Goose, Droid, Codebuff, Antigravity, Gemini, Grok, Kimi, Qwen, ZCode, dsh | Tokens                                                                        | Tokens    | $0; these logs contain no recorded USD cost     |
| pi-agent                                                                         | Recorded cost when valid, unless a pricing override applies; otherwise tokens | Tokens    | Recorded cost only                              |
| GitHub Copilot CLI                                                               | Recorded AI credits at $0.01 each when present; otherwise tokens              | Tokens    | Recorded credits converted to USD; otherwise $0 |
| Hermes                                                                           | Positive recorded actual/estimated cost; otherwise tokens                     | Tokens    | Recorded actual/estimated cost only             |
| Kilo, OpenCode                                                                   | Positive recorded cost; otherwise tokens                                      | Tokens    | Recorded cost only                              |

Tokens remain in Display reports even when their cost is zero. Focused and unified Display reports load no pricing table and emit neither `missingPricing` nor `unpricedModels`.

Rows with neither tokens nor a cost are omitted from focused and unified reports. A tokenless recorded cost remains when the selected mode uses it.

## auto (Default)

For each entry, use its stored `costUSD` when present, otherwise calculate from tokens and model pricing. A stored value of zero is still used.

```bash
ccusage claude daily --mode auto
```

In JSON, each model breakdown with a stored contribution includes [`storedCost`](/guide/json-output#stored-costs). It is the part of `cost` that came from the logs.

## calculate

Ignore stored costs and calculate from tokens and model pricing:

```bash
ccusage claude daily --mode calculate --offline
ccusage claude monthly --mode calculate --breakdown
```

## display

Use stored `costUSD` values only. Entries without a stored cost contribute zero cost; their tokens still count.

```bash
ccusage claude daily --mode display
ccusage claude session --mode display --json
```

Stored costs are values from the logs, not a billing statement. ccusage does not retrieve invoices or subscription charges.

## Token-Based Calculation

The Claude calculation prices input, output, cache writes, and cache reads separately:

```text
cost = inputTokens         × input price
     + outputTokens        × output price
     + cacheCreate5mTokens × cache creation price
     + cacheCreate1hTokens × one-hour cache creation price
     + cacheReadTokens     × cache read price
```

When a record includes a cache-write duration split, the calculation uses its 5-minute and 1-hour counts. Otherwise, all `cache_creation_input_tokens` use the standard cache creation rate.

The one-hour rate uses the published duration price or `cacheCreationInputTokenCostAbove1hr` override, and above 200K tokens the published long-context one-hour price. When a pricing row publishes no cache rate, ccusage uses the cache multipliers documented by the model's vendor, recognized from the model id wherever the model is hosted. Vendors covered include Anthropic, OpenAI, Google Gemini, DeepSeek, xAI, Moonshot, Z.ai, Qwen, MiniMax, Mistral, and Amazon Nova. For example, Claude models write at 1.25× (five minutes) and 2× (one hour), GPT-5.5 and earlier write at the input price, GPT-5.6 and later write at 1.25× and 2.5×, and Gemini reads cache at 0.1× input. When the model id names no known vendor, the pricing row's provider decides (an OpenAI-provided row writes at the input price). Only when neither identifies a vendor does ccusage estimate with Anthropic's multipliers: 1.25× and 2× for writes and 0.1× for reads. Published rates and pricing overrides always take precedence.

For whole-request tiers, the input context (including cache reads and writes) selects the rates for every token in the request. For example, a Gemini prompt over 200,000 tokens uses the higher input and output rates for the entire request. In marginal tiers, the higher rate applies to tokens in each category above the threshold. Tiered rates require per-request token counts. Goose, Hermes and Droid sessions, OpenCode session aggregates, Grok turns, and Copilot shutdown intervals record one total for several requests, so their calculated cost uses the base rates. Fast-mode usage applies the model fast multiplier.

Pricing comes from LiteLLM, models.dev, built-in model data, and configured [pricing overrides](/guide/config-files#pricing-overrides). Timestamp-aware historical rates use the entry timestamp where supported. `--offline` uses embedded pricing without network requests; `--no-offline` enables online pricing retrieval, including its cache.

When ccusage calculates a cost, it first looks for a pricing override on the reported model label, its configured alias, or the model ID behind its provider path. A full-label override takes precedence over overrides on shorter paths. The adapter supplies the model names and any provider information from its logs. Dots, at signs, and dashes are interchangeable in a pricing ID.

When the agent logged the host that served a request, ccusage first uses that host's own LiteLLM listing for the model, ahead of the vendor's list price. Logged host names that differ from LiteLLM's are treated as the same host: `amazon-bedrock` and `aws-bedrock` as Bedrock (including its default-region rows such as `deepseek.v3.2`), `azure-openai` as `azure`, and `google-vertex` as `vertex_ai`. A model ID in Bedrock's dotted spelling, such as `moonshotai.kimi-k2-thinking` or `us.anthropic.<id>`, can identify Bedrock as the host. A logged direct vendor such as Anthropic or DeepSeek rules out Bedrock pricing, even for a dotted model ID. A regional Bedrock path absent from the offline catalogue uses Bedrock's default-region listing. Then ccusage checks LiteLLM and its built-in entries for every candidate's identity. It then checks each candidate in order for a paid entry behind its provider path and for a models.dev entry for the candidate or for the model ID behind its provider path, so `openrouter/anthropic/<id>` uses the models.dev entry for `anthropic/<id>`. A LiteLLM or built-in entry for a model ID behind the path still comes first. A vendor's free listing can answer behind the path only after every candidate has missed both checks. A free hosted listing with a provider path applies only to its full pricing ID.

These identity checks first use the logged case. If they find no match, ccusage repeats them ignoring ASCII case before trying fuzzy matching. A hosted identity that matches the logged case keeps its own rates and context limit. The initial override lookup and fuzzy matching both compare case-sensitive IDs.

If these checks find no price, ccusage uses the longest matching pricing key it can find, checking LiteLLM and built-in entries before models.dev. A key matches when it names the same model ID: the logged ID may add a host or provider prefix in front, and after it a date (`-20250514`, `-2025-05-14`), a bracketed note such as `[1m]`, a Bedrock revision such as `-v1:0`, a reasoning-effort level (`-minimal`, `-low`, `-medium`, `-high`, `-xhigh`, `-extra-low`, `-extra-high`), or `latest`, `default` or `beta`. For vendors that bill a thinking or reasoning run as ordinary output tokens, the variant word also matches: `thinking` after a Claude, Gemini, DeepSeek, GLM, Kimi or Mistral key, or after a Qwen key whose model Alibaba prices the same in both modes (such as `qwen4-max` or `qwen3.5-plus`), and `reasoning` or `non-reasoning` after a Grok key. Qwen models that Alibaba bills at a higher thinking-mode output rate (`qwen-plus`, `qwen-turbo`, `qwen3-30b-a3b`, `qwen3-14b`, `qwen3-8b`) have built-in `-thinking` entries at that rate. Other families, such as Gemma, keep these words as a different model. A longer key matches a shorter logged ID only when it adds nothing but a host or provider prefix, so a logged `claude-3-5-sonnet` does not take a `claude-3-5-sonnet-20240620` row. A host or provider prefix ends in `/`, `.`, `:` or `@` (`openrouter/`, `us.`); a word joined with `-` or `_`, as in `chat-gpt-5.4`, makes it a different model ID. Any other word or version names a different model, so `gpt-5.4-codex` does not take the `gpt-5.4` price and `gpt-6.2` does not take a `gpt-6.2-…` variant's price; such a model reports missing pricing until a catalogue lists it. When the matches include the requested model ID, entries for longer model IDs are removed from consideration. A separately priced models.dev tier, such as `gemini-3.5-flash-lite` beside `gemini-3.5-flash`, matches only a logged ID that names the tier, optionally behind a host or provider prefix, followed by nothing or by one of the suffixes above, so `models/gemini-3.5-flash-lite` and `gemini-3.5-flash-lite[1m]` use the tier price while `gemini-3.5-flash` never does. Historical schedules apply to direct model IDs and their pricing overrides when the lookup matches by identity.

## Comparing Stored and Calculated Costs

Run the same report in both modes:

```bash
ccusage claude daily --mode display --json | jq '.totals.totalCost'
ccusage claude daily --mode calculate --json | jq '.totals.totalCost'
```

In `auto` mode, `cost - storedCost` is the calculated part of a model breakdown when `storedCost` is present.

## Missing Prices

When a calculation cannot resolve a model price, its calculated cost is zero. JSON marks affected model breakdowns with `missingPricing: true` and lists the names in `totals.unpricedModels`. See [Unpriced Models](/guide/json-output#unpriced-models).

```bash
ccusage claude daily --mode calculate --json | jq '.totals.unpricedModels // []'
```

To use available online prices, run with `--no-offline`. For private or proxied models, configure a pricing override.

## Related Guides

- [JSON Output](/guide/json-output) - Stored costs, cache-write splits, and unpriced models
- [Configuration Files](/guide/config-files) - Persistent mode and pricing settings
- [Claude Code](/guide/claude/) - Data paths and source-specific options
