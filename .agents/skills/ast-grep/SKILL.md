---
name: ast-grep
description: Guides ccusage structural code searches with ast-grep. Use when finding Rust or TypeScript syntax patterns, validating migrations, or writing AST-based search commands.
---

# ast-grep

Use `ast-grep` for syntax and `rg` for text. Use the Nix dev-shell binary.
Start with `run --pattern`; use `--debug-query` to inspect unmatched syntax.
Use `scan --inline-rules` or a temporary `--rule` file for a one-off rule.
Use `stopBy: end` when the relation must search beyond the next node.
Use `reduce-similarities` for duplicate Rust code.

https://ast-grep.github.io/guide/pattern-syntax.html

https://ast-grep.github.io/reference/rule.html
