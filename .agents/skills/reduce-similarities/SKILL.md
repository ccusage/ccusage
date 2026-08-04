---
name: reduce-similarities
description: Detects duplicated Rust code with the dev-shell similarity-rs CLI. Use when reviewing .rs files for repeated functions, impl methods, or parallel struct and enum definitions, or before extracting a shared helper.
argument-hint: '[path] [--threshold 0.85] [--print]'
allowed-tools: Bash(similarity-rs *) Read Grep Glob
paths: '**/*.rs'
---

# Rust Code Similarity Detection

`similarity-rs` comes from the Nix dev shell (`similarity` in
`nix/dev-shell.nix`); prefix it with `direnv exec .` from outside the shell.
TypeScript duplication belongs to the `typescript` and `ast-grep` skills.

Run it over `$ARGUMENTS`, or over `.` when none was given. Functions and type
definitions are separate passes, so a default run silently misses parallel
structs and enums:

```bash
similarity-rs . --threshold 0.85 --min-lines 5
similarity-rs . --threshold 0.85 --experimental-types
```

`--print` shows the matching snippets and `similarity-rs --help` has the rest;
`--skip-test` is worth adding when fixture-shaped test functions drown out the
real findings.

## Triage

A score is a starting point, not a verdict. Usually worth refactoring:

- 100% matches: extract a shared function or generic.
- 95-100% across different types: generic function with trait bounds.
- Duplicate impl methods on several types: trait with default implementations.
- 85-95% match arms or error handling: shared helper or macro.
- Parallel structs with identical fields: shared base or generic struct.

Usually left alone: short `new()` constructors, simple `From`/`Into` impls, and
anything a derive would generate.

Report each surviving candidate as a concrete before/after shape, not as a list
of scores.
