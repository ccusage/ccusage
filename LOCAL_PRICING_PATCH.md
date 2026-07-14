# Local Pricing Patch

This checkout contains a local ccusage source patch for estimated token-list
pricing. It is used by the normal `ccusage` command on this machine.

## Normal Command Path

The shell command resolves through the user wrapper to a **stable** binary
outside the git worktree (so branch switches cannot remove Grok support):

```sh
/Users/rk/bin/ccusage
-> /Users/rk/bin/tools/claude-code-tools/ccusage   # wrapper (--mode calculate)
-> ~/.local/lib/ccusage/ccusage                  # copied release binary
```

Legacy shims also point at the same wrapper:

```sh
/Users/rk/bin/.links/ccusage
/Users/rk/bin/claude-code-tools/ccusage
```

Reinstall / refresh after building a Grok-capable release:

```sh
~/bin/tools/claude-code-tools/install-ccusage-local
# or force rebuild from CCUSAGE_REPO (default: this checkout):
~/bin/tools/claude-code-tools/install-ccusage-local --rebuild
```

Override binary for one-off tests: `CCUSAGE_BIN=/path/to/ccusage ccusage ...`.

The wrapper runs the local binary with `--mode calculate` unless an explicit
`--mode` or `-m` argument is supplied. That means normal commands use calculated
token-list estimates:

```sh
ccusage pi daily --since 2026-07-07 --until 2026-07-07 --breakdown
ccusage opencode daily --since 2026-07-07 --until 2026-07-07 --breakdown
```

To view recorded/display costs instead:

```sh
ccusage --mode display pi daily --since 2026-07-07 --until 2026-07-07 --breakdown
```

The previous wrapper was backed up at:

```sh
/Users/rk/bin/claude-code-tools/ccusage.bak-20260708-192005
```

## Embedded Fixed Prices

The fixed prices are embedded in:

```sh
rust/crates/ccusage/src/pricing.rs
```

The rates are stored per token. For example, `$1.40/M` is represented as
`1.4e-6`.

Current locally added deterministic rates:

| Model keys | Input | Cached input | Output |
| --- | ---: | ---: | ---: |
| `glm-5.2`, `zai/glm-5.2` | $1.40/M | $0.26/M | $4.40/M |
| `deepseek-v4-flash`, `deepseek/deepseek-v4-flash` | $0.14/M | $0.0028/M | $0.28/M |
| `deepseek-v4-pro`, `deepseek/deepseek-v4-pro` | $0.435/M | $0.003625/M | $0.87/M |
| `grok-build-0.1` | $1.00/M | $0.20/M | $2.00/M |
| `grok-4.3` | $1.25/M | $0.20/M | $2.50/M |
| `grok-4.5`, `xai/grok-4.5`, `x-ai/grok-4.5` | $2.00/M | $0.50/M | $6.00/M |
| `grok-composer-2.5-fast` (+ `xai/` / `x-ai/` keys) | $0.20/M | $0.02/M | $1.50/M |
| `MiniMax-M2.5`, `minimax-m2.5`, `minimax/minimax-m2.5` | $0.30/M | $0.03/M | $1.20/M |

`grok-4.5` rates match OpenRouter/LiteLLM list prices for `x-ai/grok-4.5`
(2026-07-13). `grok-composer-2.5-fast` is provisional, aligned with
`xai/grok-code-fast` until a public list price is published.

The Grok Build CLI adapter lives at `rust/crates/ccusage/src/adapter/grok/`.
After pricing or adapter changes, rebuild and reinstall the stable binary:

```sh
cargo +stable build --manifest-path rust/Cargo.toml --locked -p ccusage --release
~/bin/tools/claude-code-tools/install-ccusage-local
```

## Adapter Behavior

Pi reports keep the displayed model label, such as `[pi] glm-5.2`, but pricing
also tries the raw underlying model key, such as `glm-5.2`.

Grok reports keep the displayed model label, such as `[grok] grok-4.5`, but
pricing also tries the raw model id and `xai/<model>` / `x-ai/<model>` keys.
Reasoning tokens are billed at the output rate while the displayed output column
stays as raw output tokens.

OpenCode pricing tries both raw model keys and provider-qualified keys, such as:

```text
deepseek-v4-pro
deepseek/deepseek-v4-pro
```

Config override precedence for candidate pricing is:

1. Exact displayed override key.
2. Exact raw/provider override key.
3. Built-in fixed-price entry or alias.
4. Existing models.dev/LiteLLM fallback.
5. Missing-pricing warning.

## Intentionally Warning-Only

These remain unpriced unless logs expose the deterministic underlying model:

- `openrouter/auto`
- `openrouter/fusion*`
- `fusion-*`
- `ark-code-latest`
- `antigravity-gemini-3-flash`

Qwen models were also left out of this local pass until ccusage has an explicit
non-USD or currency-conversion policy.

## Updating Prices Later

If a provider changes its public rates:

1. Edit the relevant entries in `rust/crates/ccusage/src/pricing.rs`.
2. Run focused tests:

   ```sh
   cargo +stable test --manifest-path rust/Cargo.toml --locked -p ccusage
   ```

3. Rebuild and reinstall the stable binary:

   ```sh
   cargo +stable build --manifest-path rust/Cargo.toml --locked -p ccusage --release
   ~/bin/tools/claude-code-tools/install-ccusage-local
   ```

4. Verify the normal command still points at the local binary:

   ```sh
   which ccusage
   ccusage --version
   ```

5. Smoke test pricing:

   ```sh
   ccusage pi daily --since 2026-07-07 --until 2026-07-07 --breakdown --json
   ccusage opencode daily --since 2026-07-07 --until 2026-07-07 --breakdown --json
   ccusage grok daily --since 2026-07-13 --until 2026-07-14
   ```

The wrapper points at `~/.local/lib/ccusage/ccusage`. Rebuild +
`install-ccusage-local` is required for normal `ccusage` to pick up new rates.

## Validation Evidence From This Patch

Known successful checks:

```sh
cargo +stable fmt --manifest-path rust/Cargo.toml --all
cargo +stable test --manifest-path rust/Cargo.toml --locked -p ccusage
cargo +stable build --manifest-path rust/Cargo.toml --locked -p ccusage --release
git diff --check
```

Known smoke-test results for July 7, 2026:

- `ccusage pi daily --since 2026-07-07 --until 2026-07-07 --breakdown --json`
  reports `totalCost: 5.322367962`.
- `ccusage --mode display pi daily --since 2026-07-07 --until 2026-07-07 --breakdown --json`
  reports `totalCost: 3.492302999999999`.
- `ccusage opencode daily --since 2026-07-07 --until 2026-07-07 --breakdown --json`
  reports `totalCost: 1.1722099999999998`.

`just check` depends on `nix flake check`. It was blocked until the local
`nix-daemon` was restarted. If `direnv` reports a daemon socket refusal, restart
the daemon and reload:

```sh
sudo launchctl kickstart -k system/org.nixos.nix-daemon
direnv reload
```

Repeated hook-install warnings during `direnv reload` are from this machine's
global `core.hooksPath` setting. They do not affect normal `ccusage` usage.
