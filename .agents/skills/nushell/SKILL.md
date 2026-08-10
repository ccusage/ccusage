---
name: nushell
description: Guides ccusage Nushell scripts. Use when adding, editing, formatting, or validating .nu files under .github/scripts, apps/ccusage/scripts, or scripts, including their Nix shebangs and GitHub Actions callers.
paths:
  - '**/*.nu'
globs: '*.nu'
---

# ccusage Nushell

Nushell is a functional, structured-data language that happens to be a shell.
Write it that way: pipelines of transformations over records and tables, not bash
with different syntax. Root `AGENTS.md` says when to pick Nushell over Babashka,
Rust, or TypeScript.

Scripts live in `.github/scripts/`, `apps/ccusage/scripts/`, and `scripts/`.

## Runtime Shape

Every executable script is a `.nu` file with a Nix shebang and a `def main`; that
shebang is how CI gets an interpreter, rather than a `nushell` profile install in
the workflow. List only the tools the script directly invokes; `--inputs-from` is relative to
the script's own directory, so its depth differs per directory — copy the header
from a neighbor, such as `.github/scripts/upsert-pr-comment.nu` or
`apps/ccusage/scripts/stage-native-package.nu`.

Files that exist only to be imported as modules carry no shebang and no `main`:
`.github/scripts/pricing-lock.nu`, `apps/ccusage/scripts/native-binary.nu`.

Workflows run these scripts through the shebang, but the commit-msg hook in
`nix/git-hooks.nix` invokes `scripts/validate-commit-scope.nu` as an argument to
`nushell` instead, so that script only ever gets `nu` itself.

Calling external tools such as `gh`, `jq`, `git`, `hyperfine`, `pnpm`, `node`, or
`bun` is fine when they are the right boundary — pin them through the shebang
shell rather than a global install.

## Docs

Read before writing. The sidebar on any of these reaches the rest of the book,
plus `/commands/` and `/cookbook/`.

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

Nushell renames and breaks things between minor releases, so check constructs
against the pinned interpreter instead of trusting memory:

```sh
direnv exec . nu --ide-check 10 path/to/script.nu
direnv exec . nu -c 'help <command>'
```

`just fmt` runs `nufmt` over `*.nu` through treefmt. For behavior changes, invoke
the script through its shebang as a smoke check and run the repo check that owns
the caller.
