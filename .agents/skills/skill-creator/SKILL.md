---
name: skill-creator
description: Guides repo-local skill creation and updates. Use when adding or editing .agents/skills, agent instructions, SKILL.md frontmatter, description trigger quality, references layout, skill scripts, or skill file line budgets.
---

# Skill Creator

Edit `.agents/skills` and let the Nix dev shell generate `.claude/skills`.

## Frontmatter

Require `name` and a third-person `description` with concrete triggers.
Aim for 20–35 words. Add `paths` or `globs` for file routing when relevant.

## Body

Record decisions, boundaries, repo-specific commands, and required validation.
Keep each rule in one skill and route to it by name. Refer to code, tests, and
external docs directly. Leave out examples, rationale, history, and facts an
agent can derive from the code or command help.

Use `references/` for conditional material and link each file directly from
`SKILL.md` with its trigger. Use `scripts/` for deterministic repeated work.
Keep `SKILL.md` under 160 lines and each reference under 180 lines.
