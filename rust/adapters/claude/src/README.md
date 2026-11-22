# Claude Code Source

Default data directories:

- `~/.config/claude/projects/`
- `~/.claude/projects/`

`CLAUDE_CONFIG_DIR` can specify one path or comma-separated multiple paths. Data from valid directories is combined.

File shape:

```text
projects/{project}/{sessionId}/{file}.jsonl
projects/{project}/{sessionId}.jsonl
```

`projects/` is scanned recursively, so both nested session directories and legacy flat JSONL files can be loaded.

Sidechain entries:

- Claude Code may write `isSidechain: true` entries for isolated sidechain
  conversations such as `/btw` `aside_question` logs under `subagents/`. See
  the Claude Code
  [side questions documentation](https://code.claude.com/docs/en/interactive-mode#side-questions-with-btw).
- These files can replay parent conversation messages with the same message ID
  but a different request ID, including the parent cache-read usage.
- ccusage keeps the parent entry and drops the replayed sidechain copy when at
  least one duplicate carries `isSidechain: true`. Distinct sidechain responses
  with their own message IDs are still counted.
- Sidechain replay matching is scoped to the effective session ID and does not
  require matching timestamps for any report.
- This behavior fixes the overcounting reported in
  [#913](https://github.com/ccusage/ccusage/issues/913).
- When `requestId` is missing, all reports deduplicate by message ID, effective
  session ID, and timestamp. Repeated writes at the same timestamp still
  collapse; distinct timestamps stay separate, except that content blocks of one
  response collapse across timestamps because Claude Code writes one line per
  content block. A later block belongs to an earlier one when its `parentUuid`
  chain reaches it through non-usage lines and tool results for that response's
  own `tool_use` blocks, and both report the same input and cache tokens. The
  message ID's spelling plays no part. All reports keep gateway responses that
  reuse a message ID in different sessions separate.
- When `requestId` is present, all reports deduplicate matching message and
  request IDs across sessions because Claude Code can copy one response into
  multiple session transcripts.

The term `session` has two meanings in this codebase:

- Session report grouping uses project directories.
- For nested files, session reports derive `sessionId` from the session directory name.
- True Claude Code session ID may also appear in each JSONL entry's `sessionId` field.

Malformed JSONL lines are skipped during parsing.

Advisor entries:

- Advisor usage is stored in `message.usage.iterations` records whose type is
  `advisor_message`. AgentProgress wrappers carry the same assistant message
  under `data.message.message`, including its advisor iterations.
- ccusage counts each advisor record separately under the record's own model.
  The top-level usage remains attributed only to the main model, and other
  iteration types are not added again.
- Advisor iterations do not include a precomputed `costUSD`, so `auto` and
  `calculate` modes price their tokens separately while `display` mode reports
  zero cost for them.

All reports parse direct assistant records first, then AgentProgress records under
`data.message.message`. Duplicate selection prefers main-thread usage, then the
larger token total, then the speed tier (Fast, Standard, unspecified, in that order)
and cache-write TTL detail, then the copy
that recorded a `costUSD`. The priced cost never decides, so the survivor does not
depend on `--mode` or pricing.

Explicit null values in the record fields ccusage reads (`sessionId`, `requestId`,
`version`, `costUSD`, `isApiErrorMessage`, message `id` and `model`, and usage
`speed`, `cache_read_input_tokens` and `cache_creation_input_tokens`) reject that
record quietly; so does such a null in an advisor iteration that is read. Missing
optional fields remain valid. Unread members, including `cwd`, nested tool-result
metadata and non-advisor iterations, do not impose a null contract. An advisor
iteration that cannot be decoded for other reasons produces an invalid-data diagnostic while the
parent usage and valid sibling iterations remain counted.
