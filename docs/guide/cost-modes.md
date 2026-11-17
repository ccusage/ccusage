# Cost Modes

Use `--mode auto`, `--mode calculate`, or `--mode display` to select how a report obtains costs. The default is `auto`.

The entry rules below describe Claude Code. Auto's stored-cost preference depends on the source:

| Source                        | Auto                                                         | Calculate | Display                                     |
| ----------------------------- | ------------------------------------------------------------ | --------- | ------------------------------------------- |
| Claude Code                   | Recorded cost when present, including zero; otherwise tokens | Tokens    | Recorded cost only                          |
| Codex, Goose, Droid, Codebuff | Tokens                                                       | Tokens    | $0; these logs contain no recorded USD cost |
| Hermes                        | Positive recorded actual/estimated cost; otherwise tokens    | Tokens    | Recorded actual/estimated cost only         |
| OpenCode                      | Positive recorded cost; otherwise tokens                     | Tokens    | Recorded cost only                          |

Tokens remain in Display reports even when their cost is zero. Display does not need a pricing table to obtain recorded costs. Other sources retain their documented stored-cost policies; see their source guide.

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

The one-hour rate uses the published duration price or `cacheCreationInputTokenCostAbove1hr` override, and above 200K tokens the published long-context one-hour price. When a model publishes no cache rate, the default depends on its provider: OpenAI models use the input price for cache writes of either duration; Anthropic and other models use 1.25× for five-minute writes and 2× for one-hour writes. An absent cache-read rate defaults to 0.1× input for every family.

For pricing entries with a context threshold, the request input context (including cache reads and writes) selects the rate tier for the whole request. Other entries with above-200K rates apply the higher rate only to the portion of each token category above 200,000. Fast-mode usage applies the model fast multiplier.

Pricing comes from LiteLLM, models.dev, built-in model data, and configured [pricing overrides](/guide/config-files#pricing-overrides). Timestamp-aware historical rates use the entry timestamp where supported. `--offline` uses embedded pricing without network requests; `--no-offline` enables online pricing retrieval, including its cache.

LiteLLM and built-in model data take precedence over models.dev, which only prices models the primary sources do not. A model that models.dev marks as a separately priced tier is matched exactly, never by a fuzzy match to its base model. Explicit pricing overrides retain precedence over all sources.

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
