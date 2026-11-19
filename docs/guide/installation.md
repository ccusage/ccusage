# Installation

You can run ccusage directly or install it globally.

## Quick Start (Recommended)

Run ccusage directly with a package runner:

::: code-group

```bash [bunx (Recommended)]
bunx ccusage
```

```bash [pnpm]
pnpm dlx ccusage
```

```bash [npx]
npx ccusage@latest
```

```bash [pkg.pr.new preview]
bunx -p https://pkg.pr.new/ccusage@<pr-number-or-commit-sha> ccusage --offline
```

:::

## Global Installation (Optional)

You can install ccusage globally if you prefer a persistent command:

::: code-group

```bash [npm]
npm install -g ccusage
```

```bash [bun]
bun install -g ccusage
```

```bash [yarn]
yarn global add ccusage
```

```bash [pnpm]
pnpm add -g ccusage
```

:::

After global installation, run commands directly:

```bash
ccusage daily
ccusage monthly --breakdown
ccusage blocks --active
```

## Development Installation

For development or contributing to ccusage:

```bash
# Clone the repository
git clone https://github.com/ccusage/ccusage.git
cd ccusage

# Allow direnv to load the Nix dev shell
direnv allow
```

The Nix dev shell provides the pinned `pnpm`, Rust toolchain, GitHub CLI, git hooks, package tooling, and project utilities. Run project tasks with `just`:

```bash
# Format the tree
just fmt

# Run tests
just test

# Run static checks
just check

# Build the documentation site and lint the npm package manifest
just build

# Build the native CLI in release mode
cargo build --manifest-path rust/Cargo.toml --release --bin ccusage
```

You can also run the package directly from source:

```bash
just ccusage::start daily
just ccusage::start monthly --json
```

## Runtime Requirements

### Node.js

- Needed when using Node-based package runners or npm-style global installs
- The package launcher also supports Bun

### Native Binary

The npm launcher runs the native binary for your operating system and architecture. The package provides binaries for macOS, Linux, and Windows on x64 and arm64.

## Verification

After installation, verify ccusage is working:

```bash
# Check version
ccusage --version

# Run help command
ccusage --help

# Test with daily report
ccusage daily
```

## Updating

### Direct Execution (npx/bunx)

An unversioned package name can reuse a locally installed or cached copy. When the runner needs to fetch ccusage from the registry, it uses the default `latest` dist-tag. To request that dist-tag explicitly, run `npx ccusage@latest` or `bunx ccusage@latest`. For reproducible runs, specify an exact version.

### Global Installation

```bash
# Update with npm
npm update -g ccusage

# Update with bun
bun update -g ccusage
```

### Check Current Version

```bash
ccusage --version
```

## Uninstalling

### Global Installation

::: code-group

```bash [npm]
npm uninstall -g ccusage
```

```bash [bun]
bun remove -g ccusage
```

```bash [yarn]
yarn global remove ccusage
```

```bash [pnpm]
pnpm remove -g ccusage
```

:::

### Development Installation

```bash
# Remove cloned repository
rm -rf ccusage/
```

## Troubleshooting Installation

### Permission Errors

If you get permission errors during global installation:

::: code-group

```bash [npm]
# Use npx instead of global install
npx ccusage@latest

# Or configure npm to use a different directory
npm config set prefix ~/.npm-global
export PATH=~/.npm-global/bin:$PATH
```

:::

### Network Issues

If installation fails due to network issues:

```bash
# Try with different registry
npm install -g ccusage --registry https://registry.npmjs.org

# Or use bunx for offline-capable runs
bunx ccusage
```

### Version Conflicts

If you have multiple versions installed:

```bash
# Check which version is being used
which ccusage
ccusage --version

# Uninstall and reinstall
npm uninstall -g ccusage
npm install -g ccusage@latest
```

## Next Steps

After installation, check out:

- [Getting Started Guide](/guide/getting-started) - Your first usage report
- [Configuration](/guide/configuration) - Customize ccusage behavior
- [Daily Usage](/guide/daily-reports) - Understand daily usage patterns
