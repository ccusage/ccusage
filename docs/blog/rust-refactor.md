---
title: 'ccusage is now 2.5× faster with 2.7× less memory'
description: 'Two weeks and 175 pull requests later, every ccusage report runs in well under half the time and memory it used to, and the pricing is more accurate. Here is what changed and how we measured it.'
date: 2026-11-22
image: /blog/rust-refactor-og.png
---

# ccusage is now 2.5× faster with 2.7× less memory

_November 22, 2026 · ryoppippi_

![ccusage blog: rewritten for speed, 2.5× faster with 2.7× less memory. 0.39× wall time and 0.37× peak memory on Linux, across 175 pull requests](/blog/rust-refactor-og.png)

Over the last two weeks, ccusage went through a deep rework of its Rust core: 175 pull requests that touched parsing, pricing, threading, the allocator, the build, and the way we test all of it. The result: every report is faster and uses much less memory than in v20.0.51, and costs are more accurate for many models.

These changes ship in the next release, v20.1.0. This post covers the numbers, where the speed came from, what changed in your reports, and how we made sure nothing changed that shouldn't have.

## The numbers

We compared the final build against v20.0.51 (pre-refactor `main`). Both are the shipped release builds, measured on generated fixtures on clean Linux and macOS machines. The headline is the geometric mean over nine commands; lower is better.

|                      | Wall time | CPU time | Peak memory |
| -------------------- | --------: | -------: | ----------: |
| Linux x64 (32 vCPU)  | **0.39×** |    0.49× |   **0.37×** |
| macOS arm64 (12 CPU) | **0.42×** |    0.55× |   **0.41×** |

On a maintainer's real logs, reports take 0.45× the wall time and 0.43× the peak memory. We measured that locally on copies of the logs, and only ratios left the machine.

Per command, against v20.0.51:

| Command                          | Linux wall | Linux memory | macOS wall | macOS memory |
| -------------------------------- | ---------: | -----------: | ---------: | -----------: |
| `ccusage claude daily`           |      0.44× |        0.26× |      0.44× |        0.28× |
| `ccusage claude session`         |      0.58× |        0.52× |      0.47× |        0.51× |
| `ccusage claude blocks`          |      0.67× |        0.55× |      0.53× |        0.52× |
| `ccusage claude daily --since …` |      0.32× |        0.25× |      0.48× |        0.34× |
| `ccusage codex daily`            |      0.35× |        0.37× |      0.44× |        0.33× |
| `ccusage codex session`          |      0.37× |        0.38× |      0.45× |        0.33× |
| `ccusage daily` (all sources)    |      0.38× |        0.47× |      0.47× |        0.38× |
| `ccusage opencode daily`         |      0.29× |        0.32× |      0.29× |        0.54× |
| `ccusage opencode session`       |      0.27× |        0.32× |      0.29× |        0.54× |

## Where the speed came from

There was no single trick. The gains add up from many small, measured changes:

- **Parsing on every core.** Codex logs are now parsed in parallel on every core up to 16 ([#2481](https://github.com/ccusage/ccusage/pull/2481)). That makes Codex reports up to 39% faster on 2–12-core machines.
- **Reading less.** `--since` skips Claude logs that cannot contain matching usage, even when their modification times are stale ([#2432](https://github.com/ccusage/ccusage/pull/2432), [#2462](https://github.com/ccusage/ccusage/pull/2462)).
- **No lock contention in SQLite.** OpenCode reports were spending CPU on SQLite's heap-accounting mutex and the system allocator. Turning off the unused accounting and routing SQLite's allocations through mimalloc cut their CPU time by 46–47% on Linux ([#2486](https://github.com/ccusage/ccusage/pull/2486)).
- **Less memory from the allocator.** On Linux, stopping the allocator from asking for transparent huge pages cut daily-report peak memory by 27–29% ([#2482](https://github.com/ccusage/ccusage/pull/2482)). macOS now uses mimalloc too ([#2450](https://github.com/ccusage/ccusage/pull/2450)).
- **Lazy pricing.** The embedded pricing catalog stays compressed until a lookup needs it ([#2491](https://github.com/ccusage/ccusage/pull/2491)), which cut the instructions of small reports by 37%.
- **A faster build.** Release binaries are now built with profile-guided optimization on Linux and Apple silicon ([#2457](https://github.com/ccusage/ccusage/pull/2457)). Our own code and every dependency except SQLite are compiled at the highest optimization level ([#2458](https://github.com/ccusage/ccusage/pull/2458), [#2518](https://github.com/ccusage/ccusage/pull/2518)).

## The trade-off: a bigger download

Speed came first, and it cost some binary size. The npm download for Linux x64 grew by 0.90 MB (gzip), and the macOS arm64 download by 0.91 MB. Most of that is profile-guided optimization and the higher optimization level. We measured each of those levers on its own and kept only the ones that paid for their size. For example, building SQLite for speed would have added another 0.3 MB for at most 3% on one command, so we left it out.

| Binary                         | v20.0.51 |     Now |   Change |
| ------------------------------ | -------: | ------: | -------: |
| Linux x64                      |  5.32 MB | 7.16 MB |     +34% |
| Linux x64, gzip (npm download) |  2.68 MB | 3.57 MB | +0.90 MB |
| macOS arm64                    |  3.88 MB | 5.71 MB |     +47% |
| macOS arm64, gzip              |  2.27 MB | 3.18 MB | +0.91 MB |

## More accurate costs

Some reported costs change in this release. That is intended: each change fixes a case where ccusage billed a model differently from its vendor.

- Gemini is billed as Google's own vendor, with its dated price changes. Vertex is billed as a separate host.
- Prompts over 200K tokens are billed as whole requests for Anthropic, Gemini, and xAI, including your `pricingOverrides`.
- A host's own listing (Bedrock, Azure, and others) is used before the vendor's list price.
- A vendor's free model used through a paid host is billed as a marked estimate (`estimatedPricing` / `estimatedModels` in [JSON output](/guide/json-output)) instead of $0.
- Model IDs match the pricing catalog regardless of letter case, including overrides.
- DeepSeek V4 follows the vendor's current rates and cutoff.
- Tokens that no priced category explains are no longer billed or counted as output.

See [Cost Calculation Modes](/guide/cost-modes) for how ccusage picks a price.

## Other changes you may notice

- A configured `project` now filters every Claude report, not only `daily`.
- [`weekly`](/guide/weekly-reports) is available for every agent.
- Codex `service_tier = "ultrafast"` in `config.toml` is honored for usage that records no tier.
- SQLite-backed reports (Kilo, Hermes, OpenCode, and others) no longer crash intermittently on Apple silicon when several databases open at once ([#2520](https://github.com/ccusage/ccusage/pull/2520)).

## How we checked that nothing else changed

A refactor this large is only useful if the reports stay right. Before calling it done, we compared the final build with v20.0.51 on:

- 816 golden report cases covering every agent, every report, and every major option (JSON, tables, date windows, cost modes, time zones, sort order);
- 43 synthetic cases for edge cases like `--last` around today and DeepSeek's dated price change;
- 205 cases on a maintainer's real logs, run on local copies, with only aggregate ratios kept.

Every difference was traced to the single pull request that introduced it, and every one matched that pull request's stated intent. There were zero unexplained changes.

Along the way, every pull request ran a performance comparison in CI, and the larger levers were measured on dedicated Linux and macOS machines before they landed.

## What's next

- The [summary pull request](https://github.com/ccusage/ccusage/pull/2521) has the full per-command numbers and methodology.
- A few things were measured and deferred: Codex parsing stops scaling past about 22 threads on very large machines, and profile-guided builds are not yet byte-for-byte reproducible.

If a report looks slower or a cost looks wrong after you upgrade, please [open an issue](https://github.com/ccusage/ccusage/issues). The tables above are what we expect, and we'd like to know if your logs disagree.
