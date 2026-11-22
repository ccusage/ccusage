---
name: code-comments
description: Applies the ccusage comment rule when writing, reviewing, or deleting code comments in any repository language, including source files, configuration, workflows, and build scripts.
---

# Code Comments

- Default to no comment. Keep a comment only for a fact a reader cannot derive
  from the code and would otherwise undo by mistake.
- Use `// NOTE(YYYY-MM-DD[, <agent CLI> <version>][, #<issue or PR>]): <fact>. <why>.`
  with the language's line-comment prefix (`#` for Nushell, shell, Nix, TOML,
  YAML and just, `;` for Babashka). Date the fact when it was found or verified.
  Name the agent CLI and version used to verify any claim about its log format.
  Include the issue or PR when one exists. The source must be clear.
- Allow `// SAFETY:` immediately above a Rust unsafe block. Allow `///` on
  `pub` items that another crate uses as API and on the schema structs and their
  fields in `config_schema.rs`. Allow `//!` at crate roots only.
- No comments after code on the same line. No TODO, FIXME, HACK or XXX markers.
  Put change history and reasons for config entries or small diffs in commits.
- Exempt shebangs and tool directives such as `#!`, `# shellcheck`, `// biome-ignore`,
  `# yaml-language-server` and Cargo, rustfmt and Clippy configuration syntax.
  Give `@ts-expect-error` a dated reason.
- Exempt Renovate version tags after 40-hex SHA-pinned `uses:` entries in YAML under `.github/`.
- Exempt just recipe docs shown by `just --list`, Nushell `def` help and parameter
  comments, licensing headers, generated files, test fixture data and golden
  snapshots.
