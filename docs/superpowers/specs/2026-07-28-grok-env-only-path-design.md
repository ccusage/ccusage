# Grok Env-Only Path Discovery Design

## Context

The Grok adapter currently exposes an explicit path through `--grok-path`,
`grokPath`, and internal command/config plumbing. Carrying that value into every
report path also led to a change where a bare `ccusage` invocation is parsed as
an explicit unified daily command instead of retaining the existing
`command: None` representation.

Other adapters do not establish one universal path-configuration pattern. Pi
supports an explicit focused-command path, while Gemini discovers its data
directory from an environment variable or a default location. Grok only needs
one data root and does not need a second ccusage-specific override mechanism.

## Decision

The Grok adapter will resolve its home directory using this precedence:

1. A non-empty `GROK_HOME` environment variable.
2. The default `~/.grok` directory.

There will be no `--grok-path` CLI option, `grokPath` configuration property,
`CCUSAGE_GROK_PATH` variable, or internal Grok path argument. Focused Grok
reports and all unified report entry points will use the same adapter-owned
discovery logic.

The `grok` configuration namespace remains available for shared report options,
such as JSON or offline behavior, just as other agent namespaces are.

## Data Flow

For `ccusage grok`, explicit `ccusage all` commands, bare `ccusage`, and unified
session reports, command handling selects the Grok adapter without supplying a
path. The adapter reads `GROK_HOME`; after rejecting an empty value, it falls
back to `~/.grok`, then loads the adapter's expected session data beneath that
root.

Because no path value needs to be injected into the default command, bare
`ccusage` parsing returns `command: None` as it did before Grok support. The
existing application entry point remains responsible for interpreting that as
the unified daily report.

## Required Changes

- Remove `--grok-path` and its help text and snapshots.
- Remove `grokPath` from configuration types, generated schema, examples, and
  tests.
- Remove the Grok path field from agent command arguments and from focused and
  unified loader plumbing.
- Remove any remaining `CCUSAGE_GROK_PATH` handling or regression references.
- Keep only `GROK_HOME` and `~/.grok` resolution in the Grok adapter.
- Restore the pre-feature bare-command parser representation.

## Validation

Tests will verify that:

- a non-empty `GROK_HOME` selects that directory;
- an unset or empty `GROK_HOME` falls back to `~/.grok`;
- focused, explicit unified, bare unified daily, and unified session reports can
  load Grok data through the same discovery path;
- bare parsing again yields no explicit command;
- generated configuration schema, CLI help, and snapshots contain neither
  `grokPath` nor `--grok-path`;
- existing workspace formatting, tests, and lint checks continue to pass.

Environment-mutating tests must use the repository's environment guard pattern
so their state is restored and they do not race with unrelated tests.

## Documentation Impact

User-facing documentation will describe `GROK_HOME` and the `~/.grok` default.
Any mention of `grokPath` or `--grok-path` introduced by this feature will be
removed. The generated configuration schema will be regenerated from the
updated configuration types.

## Out of Scope

- Changing Pi, Gemini, or other adapters' path behavior.
- Introducing a generalized path configuration abstraction.
- Supporting multiple Grok roots.
- Adding another Grok-specific environment variable.
- Refactoring unrelated unified report or configuration behavior.

## Compatibility

The Grok feature has not been merged or released, so removing its provisional
path option and configuration property does not require a deprecation period or
backward-compatibility shim.
