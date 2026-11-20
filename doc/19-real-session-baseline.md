# 19 Real-Session Baseline: Workflow Fit and Token Account

> Language: **English** | [简体中文](19-real-session-baseline.zh-CN.md)

> **Status: measurement record (2026-11-19).** Baseline from one 10-day OpenCode session on `tumblr-downloader` that ran **without** Loom; the savings figures are projections arithmetically derived from Loom's measured levers, not a Loom A/B run. The instrument for that A/B is still missing (see [14](14-current-issues-and-blockers.md)). Companion baseline on a different harness: [20](20-codex-session-baseline.md) (2026-11-20).

## 1. What was measured

A real repository (`tumblr-downloader`) ran a 10-day session as one growing conversation: branch review, bug fixes, real-device testing over two phones, GitHub issues, an F-Droid release, and finally UI design exploration. No `.loom` dispatch was involved after the first day.

Source: the local OpenCode store (`session_v2` and `session_message` tables), session `ses_e45599173ffeI9lc6ycANjbIKC`, queried 2026-11-19. Reproduce with:

```bash
sqlite3 ~/.local/share/opencode/opencode.db \
  "SELECT tokens_input, tokens_output, tokens_cache_read FROM session_v2 \
   WHERE id='ses_e45599173ffeI9lc6ycANjbIKC'"
```

| Dimension | Measured |
|---|---|
| Span | 2026-11-08 18:13 → 2026-11-19 00:02 (10 days) |
| Model calls | 2,739 assistant messages, 95 user messages |
| Tokens | input 51.6M, output 1.05M, reasoning 1.68M, **cache read 949M**, cost $0 (quota models) |
| Average context per call | **~346k tokens** (949M ÷ 2,739), peak ~900k |
| Context resets | 7 compactions (5 auto, 2 manual) |
| Tool calls | 3,117 total: shell 2,248 (72%), edit 492, read 276, write 47, skill 14, question 10, grep 11, rest 20 |
| Reads that were screenshots | 201 of 276, **26.8MB of base64** accumulated in context |
| Shell calls waiting on something | 975 of 2,248 (43%) contain `sleep`/`wait`/`guard` (adb loops, gradle, CI polling) |
| Daily cache-read peak | 11-14: 228M, 11-15: 203M, 11-17: 175M — the three device-testing days, **64% of the 10-day total** |
| Longest unattended stretch | 274 assistant messages over 400 minutes (median: 10 messages per user turn) |
| Idle → nudge | 81 idle events; every `继续`/`proceed` was the human waking an idle session (median gap 4.3 min, p90 650 min) |

Two subagent sessions on 11-11 (bug investigations, ~63k input each) are excluded from the totals above; they were cheap because they were short-lived.

## 2. Workflow fit: what the plan covers and what it misses

| Observed in the session | Evidence | Plan item | State |
|---|---|---|---|
| State only recoverable by asking the model | "测试用例还剩下多少" / "待做清单" / "todo 还有什么" — 3 explicit asks; `test-plan.md` re-read 95 times; `DownloadStateManager.kt` read 13 times | Event-log replay + `loom board` | board implemented; `loom top` pending |
| Coordination assembled by hand | 2 explicit requests: "汇总一下我需要怎么配合" and "先做不需要我配合的，完成后告诉我需要提供哪些" | Attention inbox ([15](15-agent-runtime-governance-and-engineering.md) §2.2) | designed, not implemented |
| Questions already structured | 10 `question` tool calls, answers landed inline; zero pain | `questions.json` protocol (landed 2026-11-18) | serves adapter processes that exit 2; **no gain for direct sessions** |
| Context grows until it hurts, then forgets | 346k average, ~900k peak, 7 compaction rebuilds; post-compaction re-reads feed raw input | Bounded packet (8k/24k/30k by kind) + one context per work item | implemented (`loom-context`, `loom-native`) |
| Old tool results re-surface every turn | 26.8MB of screenshots + 2.23MB of redundant re-reads carried until compaction | Step-level projection | implemented; measured −46~55% per-step input at 16–17KB history ([16](16-runtime-governance-adoption-assessment.md) §8.8, n=1) |
| Long runs with no check-in | 274-message stretch; 5 nudges after idle | doc/18 four intervention points + inbox wakeup | method proposed; idle-resume itself is harness-side |
| Device-driving boilerplate burns turns | 400+ adb guard/tap/dump sequences, 43% of shell calls | Scripted acceptance (the repo's own test plan already says "device cases are scripted with adb") | partly: script the deterministic half, keep the model for judgment |

The one row where Loom adds nothing: **questions**. OpenCode's native tool already did this well in-session. Loom's `questions.json` exists for process-level adapters (exit 2 + file), not for direct chat.

## 3. Token account: where 949M went and what the levers recover

The 949M cache read is `Σ(context size per call)`: 2,739 calls × ~346k average. Three levers attack that product, each with a different evidence grade:

| Lever | Mechanism | Evidence grade |
|---|---|---|
| Bounded packet | Every run starts from ≤30k (main) / 24k (worker) / 8k (judgment) instead of a 10-day history | Implemented; budgets are code (`compiler.rs` `budget()`), not yet measured at scale |
| Slicing | Each work item gets a fresh context; 7 compaction rebuilds become 0 because history never accumulates across items | Implemented (M4 verified 2 concurrent workers); aggregate effect unmeasured |
| Projection | Old tool results compress to first line + reference as history grows | Measured: −46~55% per-step input on a 16–17KB fixture ([16](16-runtime-governance-adoption-assessment.md) §8.8, n=1, official `deepseek-flash`) |

**Projection (arithmetic on the levers, not a measurement):** if the same 2,739 calls ran with contexts bounded to ~100–150k instead of 346k average, cache read lands at **~300–400M, i.e. −55~65%**. Raw input drops further (fewer compaction rebuilds) and call count drops with it (3 status queries answered by `loom board`, redundant re-reads gone, device boilerplate scripted). Cross-check on the other side of the ledger: the one real Loom-driven task in this repository (#24, [14](14-current-issues-and-blockers.md) §9) ran 8 attempts at 12–31k input per step, an order of magnitude under this session's 346k average.

Assumptions the projection rests on, stated so they can be falsified:

1. Same workload and models; only the context lifecycle changes.
2. Screenshots the model must look at once are not cut — the levers cut **re-reading**, not first sight. The 176 image-bearing turns stay expensive.
3. The 95 human turns stay; interactivity is not the target.
4. The aggregate is unverified until the native-vs-adapter A/B runs ([14](14-current-issues-and-blockers.md)); this record gives that A/B a baseline to beat.

Why the numbers matter beyond tokens: this repository's own quota history shows 1.52M tokens ≈ 76% of a 2M quota consumed by a single task ([14](14-current-issues-and-blockers.md) §9). A 55–65% cut in context re-reads is the difference between one task per quota top-up and a working day.

## 4. Gaps this exposed

**Reactive human-in-the-loop work has no entry path.** The session's dominant shape: the human tests on a device, reports what broke, the agent fixes it, the next report arrives hours later — one item at a time, verified by a person watching in real time. The technical path exists (`loom task add` → `dispatch` → `verify` → `merge`, exactly what #24 used), but the method in [18](18-agent-collaboration-workflow-proposal.md) describes batches, so single arriving reports have no first-class way in; and the tight observe-fix-observe loop does not fit dispatch→verify→merge at all. Observed consequence: `.loom/` was initialized on 2026-11-08 15:35, its last event is the #24 merge at 17:08, the session started at 18:13, and the following 10 days (2,739 calls, 949M cache read) never touched it again. Recorded as a known gap in [18](18-agent-collaboration-workflow-proposal.md) §5.5.

**Waiting on external systems is primitive-free by choice.** CI pipelines, F-Droid builds, and app installs were polled from shell (part of the 43% waiting calls) or by the human coming back to ask. Loom has no external-event primitive and should not grow one in v1; the inbox covers Loom-side escalations only.

**Direct sessions inherit none of the levers.** Everything in the table above lives behind `loom dispatch`. An OpenCode conversation gets zero of it unless the work is routed through Loom — or the projection idea is ported to the host harness, which is outside this project's roadmap.

## 5. What this changes

Priority order after this measurement: **attention inbox first** (two explicit human requests for it in one session, and it is the only row that is designed but untouched), then **`loom top`** (kills the status-query turns), then the **hot-slice path** for reactive work ([18](18-agent-collaboration-workflow-proposal.md) §5.5). The token levers themselves are already implemented; what is missing is (a) running real work through them at scale and (b) the A/B that turns the projection in §3 into a measurement.
