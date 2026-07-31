---
name: docs
description: Routes ccusage documentation impact work. Use when code or behavior changes affect README files, docs guides, VitePress navigation, screenshots, schema docs, or user-facing commands/options, and when auditing whether a change needs docs at all.
---

# ccusage Docs

Documentation impact is decided by what a user can observe, not by which
directory changed. Internal refactors, test-only changes, and skill maintenance
need nothing; anything that changes what a user can run, see, or configure needs
a pass over every surface, not only the guide page you started from:

- root `README.md`
- `apps/ccusage/README.md`
- the relevant `docs/guide/` pages and their cross-links
- VitePress navigation in `docs/.vitepress/`

Then read the local conventions before writing:

- `docs/README.md` - site structure, schema-copy behavior, `just docs::*` recipes.
- `docs/AGENTS.md` - screenshot placement and alt text, cross-linking, lint escapes.
- `apps/ccusage/AGENTS.md` - before touching README content that ships to npm.
