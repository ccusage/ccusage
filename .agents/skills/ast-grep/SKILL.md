---
name: ast-grep
description: Guides ccusage structural code searches with ast-grep. Use when finding Rust or TypeScript syntax patterns, validating migrations, or writing AST-based search commands.
---

# ast-grep

Reach for ast-grep when a search depends on syntax structure — a macro call with
particular arguments, a match arm shape, an attribute on an item. `rg` is faster
for plain text, and `reduce-similarities` owns duplicate-code detection.

`ast-grep` comes from the Nix dev shell, so prefix with `direnv exec .` when the
shell is not already active. There is no `sgconfig.yml` here: relational rules go
through `scan --inline-rules` or a throwaway `--rule` file rather than a project
rule set. Scoping to `rust`, `apps`, `docs`, or `nix` keeps large searches quick,
though a bare root search already skips gitignored trees.

Start with `run --pattern` and widen only if it under-matches. Give relational
rules `stopBy: end`, or they stop at the first node in that direction. When a
pattern does not match the shape you expected, dump the parse with
`--debug-query`.

https://ast-grep.github.io/guide/pattern-syntax.html

https://ast-grep.github.io/reference/rule.html
