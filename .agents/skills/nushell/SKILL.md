---
name: nushell
description: Guides ccusage Nushell scripts. Use when adding, editing, formatting, or validating .nu files under .github/scripts, apps/ccusage/scripts, or scripts, including their Nix shebangs and GitHub Actions callers.
paths:
  - '**/*.nu'
globs: '*.nu'
---

# ccusage Nushell

## Script boundaries

Executable Nushell scripts need a pinned Nix shebang and `def main`. List only
tools the script invokes. Resolve `--inputs-from` relative to the script's
directory. Imported modules have neither a shebang nor `main`.

Scripts invoked directly through `nu` by the gate workflows, commit hook, or
infra fixture suites must run with the caller's toolchain and cannot depend on
packages provided only by the script's Nix shebang. Use `$"#!($nu.current-exe)"`
for mock executables in the Linux build sandbox.

## Docs

https://www.nushell.sh/book/thinking_in_nu.html

https://www.nushell.sh/book/nushell_map_functional.html

https://www.nushell.sh/book/style_guide.html

## Style

Functional style is the default. Treat each of these as a defect to fix:

- **`mut` + `for` as an accumulator.** Use `reduce`, `each`, `where`, `group-by`, `zip`, `flatten`, `insert`/`update`, `generate`. `mut` is legitimate only for genuinely sequential state that no filter expresses.
- **String plumbing between steps.** Pass records and tables; serialize once at the boundary (`to json --raw`) and parse once on the way in (`from json`).
- **Nested `if`/`else` chains on a value's shape.** Use `match`, including list patterns and guards.
- **Shelling out for data.** Native commands over `^jq`, `^sed`, `^awk`, `^date`.
- **Bare `each` for effects.** Say so with `| ignore` when the result is unused.

Also:

- Type custom command signatures — parameter types, `--flag`, and the return type. They document intent and are checked at parse time.
- `run-external` for commands whose flags Nushell would otherwise parse; quote short flags such as `'-L'`, `'-x'`, `'-c'`, and keep arguments as lists up to the external boundary.
- `complete` when you need exit code, stdout, and stderr without throwing; `error make` rather than a sentinel return value.
- Timestamps are `datetime` and durations are `duration`; compare and subtract them directly instead of formatting to strings.
- Progress to stderr for CI scripts whose stdout is a data artifact.

## Validation

Validate with the pinned interpreter:

```sh
direnv exec . nu --ide-check 10 path/to/script.nu
direnv exec . nu -c 'help <command>'
```

`nix flake check` fails on any `--ide-check` diagnostic in a `.nu` file
(`scripts/check-nushell.nu`). `just fmt` runs `nufmt` over `*.nu` through treefmt. For behavior changes, invoke
the script through its shebang as a smoke check and run the repo check that owns
the caller.
