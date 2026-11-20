# 20 Codex-Session Baseline: Scale, Token Account, and Rework Proxies

> Language: **English** | [简体中文](20-codex-session-baseline.zh-CN.md)

> **Status: measurement record (2026-11-20).** Baseline from 615 Codex CLI rollout sessions (2026-04-01 → 2026-11-20) on two production repositories, all run **without** Loom; savings figures are arithmetic projections from Loom's levers, not a Loom A/B run. The instrument for that A/B is still missing (see [14](14-current-issues-and-blockers.md)). Companion to [19](19-real-session-baseline.md) — same method, different harness.

## 1. What was measured

Source: a Codex CLI rollout store (`sessions/YYYY/MM/DD/rollout-*.jsonl` plus `archived_sessions/`), copied to the analysis machine and queried 2026-11-20: **615 files, 1.2GB, 370,378 JSONL lines**. Workload: two production repositories on a Windows workstation — an IoT SaaS monorepo (Java backend + web frontend) and a metering/hydrology Java service — plus recurring template requests (e.g. monthly git-log summaries). No `.loom` was involved at any point. Personal paths, repository names, and account identifiers are deliberately omitted from this record.

Reproduce: a nine-pass Python scan over the rollout files (per-session totals → aggregates → tool-output sizes → interaction patterns → rework proxies). The passes lived in the session workspace (`/tmp/opencode/codex-analysis/pass1.py` … `pass9.py`) and are ephemeral; §2 records the format semantics they depend on, which is what a rewrite needs.

| Dimension | Measured |
|---|---|
| Span | 2026-04-01 → 2026-11-20, 56 active days |
| Sessions | 615 rollouts: 230 main + 385 subagent (by `session_meta.thread_source`); delegation is single-level (depth 1), subagent paths named per task |
| Model responses | 37,403 with per-response usage records |
| Token coverage | 444 sessions carry per-response records (2026-11, CLI 0.167–0.169); 163 of the remaining 171 (2026-04→06 + archived) carry session-cumulative totals; 8 sessions contribute nothing — **607 of 615 sessions in the account** |
| Billed input | **6,976.3M (6.98B)** — cached 6,722.4M (**96.4%**), genuinely new 254.0M, re-send ratio **27.5×** blended |
| Era split | new format (444, 2026-11): input 6,001.5M, cached 98.4%, re-send **60.9×**; old format (163, 2026-04→06): input 974.8M, cached 84.1%, re-send 6.3× |
| Output / reasoning | 28.75M / 10.15M |
| Avg context per response | 160,456 tokens (new format only — the old format has no per-response records) |
| Per-session re-send | median 26.4×, mean 50.4×, p90 95.2×, max 784× (new format) |
| Peak context | median 107,670 / p90 240,725 / max 462,129 (607 sessions); context windows 258,400 and 475,000 |
| Main vs subagent | main (59 instrumented): 17,296 responses (293/session), peak median 225,251, re-send 80×; subagent (385): 20,107 responses (52/session), peak median 109,824, re-send 24.8× |
| Rate-limit watermark | median 48%; ≥70% reached in 29% of sessions, ≥90% in 5% |
| Compaction | 134 events / 53 sessions; replacement record median 110KB, max 1.6MB; worst session 19× |
| Session duration | median 0.2h, p90 1.7h, max 140.8h (new format) |
| Tool calls | 48,383 total: `exec` 25,254, `shell_command` 11,937, `send_message` 4,484, `wait_agent` 2,422, `apply_patch` 1,948, `sleep` 665, `spawn_agent` 360, `request_user_input`(+async) 299, rest small |
| Tool outputs | 48,392 records (both format families), total 292.2M chars ≈ **73M tokens** (4 chars/token) |
| Human messages | 2,588 substantive (context/image payloads excluded): median 1 per session, p90 4, max 147 |

## 2. Format semantics and counting pitfalls

Worth recording because each one silently corrupts the account:

1. **The discriminator flips between record types.** `token_usage_record`, `session_meta`, `turn_context` carry no `payload.type` — the top-level `type` is the discriminator; `token_count` is the reverse (top-level `event_msg`, discriminator inside the payload). One rule for both zeroes every token aggregate.
2. **`token_usage_record.usage` is a per-response delta** (verified: Σ = `token_count.info.total_token_usage`). The old format has no such record; its totals come from the last `total_token_usage` snapshot per session.
3. **Two tool-output families.** `function_call_output` (older era) and `custom_tool_call_output` (newer era — `exec`/`shell_command` are "custom" tools); `output` may be a string **or** a list of content blocks. Counting one family undercounts the newest sessions by ~99% (largest session: 13 vs 1,462 records).
4. **`custom_tool_call.status` is always `completed`** — not a failure signal; failures must be detected from output markers.
5. **171 sessions carry no per-response records** (2026-04→06 + archived); 8 of them also lack cumulative totals. Any "total" without this caveat is wrong.
6. **`compacted` records embed the full replacement history** (median 110KB) — file size ≠ conversation size.

## 3. Workflow fit: what the plan covers and what it misses

| Observed in the sessions | Evidence | Plan item | State |
|---|---|---|---|
| Multi-agent fan-out assembled by hand | 360 `spawn_agent`, 4,484 `send_message` (median 806 chars), 386 subagent sessions, single-level, task-named paths | `loom task` dispatch + ledger + `loom top` | dispatch/ledger/top implemented; inbox pending |
| Status polling on both sides | agent: 2,542 `wait_agent`+`list_agents` calls (5% of all tool calls); human: ≈8% of messages are status queries (keyword) | `loom top` read-only watcher | implemented (2026-11-19) |
| Structured questions already native | 299 `request_user_input` calls with `header/id/question/options/label/(Recommended)` and an `answers` map — the same shape as `schemas/questions.v1.json`; 191 answers captured | `questions.json` protocol | implemented; **parity, not a gain** — same conclusion as [19](19-real-session-baseline.md) |
| Context dominated by raw tool output | per-session output tokens above the 8KB line = median **−25.7pp** of peak context (new era) / −23.8pp (old era); 19.3% of all outputs exceed 8KB; `exec`/`shell_command` median ~2.5KB, p90 ~19KB; assistant prose median only 103 chars | tool envelope (8KB/24KB/128KB + summary/address/preview, [10](10-tool-output-protocol-v0.1.md)) | implemented |
| Fixed per-request overhead | tool definitions median 41.7KB per session re-sent with history; system/developer message p90 21.5KB (instruction-file inlining) | skills bounded packet (4k main/worker, 2k judgment) | implemented; adapter mention unverified |
| Long unattended runs | median 1 human message/session (p90 4, max 147); durations median 0.2h but p90 1.7h, max 140.8h; 65 `turn_aborted` | [18](18-agent-collaboration-workflow-proposal.md) four intervention points + attention inbox | method proposed; inbox designed, not implemented |
| Manual cross-session state transfer | 59 continuation-style openings; at least one explicit "paste this prompt into a new session" hand-off carrying a hand-written continuation summary | `.loom` contract layer + hot-slice path | contract layer implemented; hot-slice path is the known gap ([18 §5.5](18-agent-collaboration-workflow-proposal.md)) |
| Compaction churn | 134 events / 53 sessions; replacement records median 110KB | slicing — one context per work item | implemented; aggregate effect unmeasured |
| Images stay expensive | `view_image`: 42 calls, median 192KB, 26.8M chars total | envelope cannot cut first sight | same as [19](19-real-session-baseline.md): cut re-reading, not first sight |

## 4. Token account: where 6.98B went and what the levers recover

The account is `Σ(context size per response)`: 37,403 × 160k average ≈ 6.0B (new era) plus 0.97B cumulative (old era). Cache hit 96.4% means **cost and quota track context size, not the 254M of new content** — every extra context token is re-sent until compaction.

| Lever | Mechanism on this data | Evidence grade |
|---|---|---|
| Tool envelope | 19.3% of 48,392 outputs exceed the 8KB default (9.2% exceed 16KB); capping at 8KB cuts **53.3%** of output volume; outputs above the 8KB line account for a median **−24~26pp** of peak context across both eras | **Arithmetic on logged sizes** (48,392 outputs); assumes summary+address+preview preserves task success and no extra re-reads — unverified |
| Step-level projection | first line + reference instead of raw history as it grows | Measured on a fixture: −46~55% per-step ([16 §8.8](16-runtime-governance-adoption-assessment.md), n=1); design target −55~65% overall ([19 §3](19-real-session-baseline.md)) — **the envelope is a subset of this, not stacked on it** |
| Slicing | fresh context per work item; the 134 compactions become 0 because history never accumulates across items | Implemented; aggregate effect unmeasured |

**Envelope arithmetic (not a measurement):** median session context shrinks ≈ 24~26%; applied to the measured 6.98B billed input that is ≈ **−1.7B** over this history. Assumptions, stated so they can be falsified (aligned with [19 §3](19-real-session-baseline.md)):

1. Same workload and models; only the context lifecycle changes.
2. Images are not cut — the levers cut re-reading, not first sight; the 42 `view_image` calls stay expensive.
3. The 2,588 human messages stay; interactivity is not the target.
4. No re-read offset is counted: a spilled artifact re-read costs bounded calls and may add call count — net effect unmeasured.
5. Aggregate unverified until the native-vs-adapter A/B runs ([14](14-current-issues-and-blockers.md)); this record gives that A/B a second baseline to beat.

**What does not shrink:** the re-send multiplier itself (a property of the stateless API), image turns, and human turns. **What converts directly:** 29% of instrumented sessions reached ≥70% of their rate window — context cuts become quota headroom one for one.

## 5. Rework and accuracy: a proxy baseline, not ground truth

| Proxy | Measured | Note |
|---|---|---|
| Outputs with failure markers | **3.0%** (1,431/48,392) | error text / non-zero exit markers |
| Consecutive duplicate commands | **2.45%** (1,187/48,383) | immediate retry |
| Git rollback commands | **1** in 48,383 (`restore`/`reset --hard`/`revert`) | rework is corrected in session, never reverted |
| `turn_aborted` | 65 | human pressed escape |
| Correction-like human messages | **≤18.2%** (472/2,588) | keyword upper bound; eyeballed sample: genuine agent-rework is a minority — in this workload bug reports **are** the task, plus user self-corrections, hypotheticals, and false positives |

**Ground-truth caveat:** true rework (the same file changed back and forth) lives in the repositories' git history, which is not on the analysis machine — chat text alone can only yield proxies, and the 18.2% upper bound overstates. This is exactly what Loom's step-level attribution + `logs/churn.json` + `loom churn stats` will measure precisely once real work runs through it.

Direction of each mechanism (hypothesis, not measurement):

| Mechanism | Targets | Expectation |
|---|---|---|
| no-progress detection (implemented) | duplicate/loop class (2.45%) | direct, small quantum |
| patch-first / write guards (implemented) | self-overwriting uncommitted work | low frequency, high severity; 0 in-session rollbacks observed — rare, or unrecoverable without a guard |
| envelope + projection | post-compaction drift (134 events), long-run drift (max 140.8h sessions) | plausible; counter-risk: truncation → re-reads / missed detail, unmeasured |
| `questions.json` (implemented) | wrong-scope builds | **parity**: codex's native protocol (299 calls) already matches the schema shape; Loom's increment is persistence + orchestrator visibility |
| skills (implemented) | procedure-skipping | unverified (adapter canary pending) |

No accuracy claim about Loom is made anywhere in this record: the Loom side has no real-model run yet.

## 6. What this changes

1. **The envelope becomes the strongest-evidence token lever on real load.** In [19](19-real-session-baseline.md) projection led; here raw tool output dominates the context (median 55.7%/58.1% of peak before capping) and the 8KB line is crossed by 19.3% of calls.
2. **Priorities reconfirmed:** attention inbox + hot-slice path (§3 rows 1/2/6/7 all point there). Manual cross-session prompt-pasting is a third instance of the [18 §5.5](18-agent-collaboration-workflow-proposal.md) gap.
3. **Questions parity reconfirmed on a second harness** — the pattern stands: native Q&A is fine in direct sessions; Loom's value there is persistence and orchestrator visibility, not the pattern itself.
4. **The account gives the A/B a second baseline to beat** ([14](14-current-issues-and-blockers.md)) — and the rework proxies in §5 give it a quality axis, measurable for the first time on the Loom side via churn stats.
