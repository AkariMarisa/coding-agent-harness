# 15 Agent Runtime Governance and Engineering

> Language: **English** | [简体中文](15-agent-runtime-governance-and-engineering.zh-CN.md)

> Status: **Design draft, not implemented** (2026-11-07).
> Scope: four observed problems — write churn, question blocking and visibility of human items, skills governance, agent/harness-driven local CI.
> Prerequisite reading: [08-design-roadmap-v0.2.md](08-design-roadmap-v0.2.md) (state ownership / adapter contract), [03-core-protocols.md](03-core-protocols.md) (TaskSpec / receipt / events), [10-tool-output-protocol-v0.1.md](10-tool-output-protocol-v0.1.md) (tools and telemetry), [14-current-issues-and-blockers.md](14-current-issues-and-blockers.md) (current environment blockers).

---

## 0. Conclusion

None of these four problems can be rooted out by "switching to a stronger model"; what the control plane lacks is a **behavior protocol**:

| Problem | Essence | What the control plane must add |
|---|---|---|
| Repeated delete-and-rewrite | The rewrite itself is not expensive; the expense is that once full text enters history it is resent at every later step, and the model substitutes whole-file output for patches | step-level context recompilation, patch-first, token attribution, cost-aware stop-loss |
| A question pauses the session | A question is treated as a failure or an interactive wait rather than a recoverable return; the items that genuinely need a human are then drowned in later conversation | `questions.json` + `needs_input` state + answer/assumption protocol + a resident attention inbox |
| skills proliferation | A skill is a context cost and a supply-chain risk, not "install it and it helps" | repo profile, offline catalog, on-demand loading, promotion after validation |
| absence of local CI | Local CI is not an accessory to acceptance; it is a repo-level check surface and pre-commit gate that both agents and humans can invoke | stack detection, `.loom.ci.toml`, `loom ci`, hooks, review/attestation, skill-based checks |

Common thread: **turn agent behavior into structured artifacts that are observable, interruptible, and recoverable**; no long sessions, no interactive waits, no asking an LLM to make judgments that are deterministic.

Another thread: **items that need a human must have a resident outlet independent of chat history** — questions, approvals, operations, reviews and reminders alike; relying on "scrolling up through messages" is not enough.

Suggested implementation order:

1. **Local CI core**: make "verification" trustworthy first, so the other three directions have a basis for measurement; start with detection, `.loom.ci.toml`, sequential execution and doctor, without rushing to hooks/attestation.
2. **question protocol + attention inbox**: today `needs_input` gets recorded by verify as `task.failed`, which is a correctness defect; at the same time, pull "items a human needs to handle" out of chat history into a resident, visible inbox.
3. **churn governance**: start with native step-level context recompilation + patch-first + token attribution; intercepting writes is only the last gate. External adapters can only get coarse-grained watchdog and selection.
4. **Move local CI earlier**: pre-commit/pre-push, deterministic auto-fix, review job, attestation — keep checks that can be solved locally in front of commit/push to reduce duplicated work on server CI.
5. **skills governance**: scanning/loading/static checks can come first; CI job templates and review rules can be an early landing spot for skills; **promotion evaluation depends on real models and a fixed task set**, currently still limited by the environment blockers in [14](14-current-issues-and-blockers.md).

Explicitly not doing: globally installing skills, auto-installing skills over the network, embedding/vector retrieval, interactive waits, calling an LLM once per question, v1 nested containers.

---

## 1. Problem one: repeated delete-and-rewrite (write churn)

### 1.1 First count the token bill

The waste of churn is not on disk but in **request history**. A single "delete then rewrite" usually incurs three charges at once:

| Charge | Source | Note |
|---|---|---|
| output tokens | the model regenerates the entire file content into `tool_calls.arguments` | ~2k tokens per pass for an 8KB file |
| input token compounding | assistant messages carry the full content, and native resends `messages` in full at every step | old versions keep being charged on **every later step** |
| extra steps / retries | churn lengthens the run; acceptance failures are fed back; in severe cases a whole run is reopened | each step resends the history again |

A concrete example: the same 8KB file rewritten 5 times, assuming 20, 16, 12, 8, 4 steps remain after each:

| Item | Without governance | With step-level projection + patch |
|---|---:|---:|
| Generating full text 5 times | ~10k output tokens | ~0.5k (only a patch per pass) |
| Old versions resent in later requests | ~2k × (20+16+12+8+4) ≈ **120k input tokens** | old versions are projected away, only the current file snapshot is kept, ~2k/step |
| The 3rd rewrite hard-blocked | saves ~2k output, but the full text of the first two is still in history and still being charged | project history first, then decide whether to block |

Conclusion: **blocking the Nth file write saves only one output; the real bulk is making old content stop entering later requests.** So the first priority is "step-level context recompilation + patch-first", not intercepting file operations. Hard blocking is only suitable as the last gate, and it may induce the model to retry with a different writing style, raising tokens instead.

### 1.2 Current state (why it compounds today)

| Current state | Consequence |
|---|---|
| native's `messages` only grows; every step `provider.complete(messages.clone())` | the longer the history, the more expensive each step's input; the more rewrites, the heavier the compounding |
| assistant's `tool_calls.arguments` holds the full file content | every `write`'s full text stays in history and is resent at every later step |
| `write` does not require a prior `read`, nor limits whole-file overwrite | the model can substitute a full-text rewrite for a patch at any time |
| `edit` returns only an error on match failure | the model easily gives up on `edit` and switches to whole-file rewrite |
| only run-level usage, no step-level attribution | cannot answer "how many extra tokens did this rewrite cost" |
| no-progress only looks at `mutated` | since 2026-11-11 a new acceptance evidence signal was added (deduped (cmd,exit) counts toward progress); a rewrite still counts as "progress" and triggers no convergence |

Related implementation: `src/crates/loom-native/src/run.rs` (`messages.push` / `provider.complete`), `src/crates/loom-native/src/tools.rs` (`write` / `edit`), `src/crates/loom-store/src/telemetry.rs`.

### 1.3 Four levers for saving tokens (ranked by ROI)

#### A. Step-level context recompilation (the biggest lever)

Change native from "a long conversation" to "recompile a bounded context packet each step", which also matches the original Context Compiler principle of [08](08-design-roadmap-v0.2.md):

```text
[stable prefix] system + task + AC + current skill index
[workspace state] files changed in this run: path / sha / line count / bounded current content or preview
[progress ledger] step / tool / path / result / verify state (one or two lines per entry)
[recent interaction] the most recent 1–2 raw tool call / result
```

Rules:

- old `write` / `edit` arguments are replaced by a one-line reference: `[elided write path=src/x.rs sha=ab12 lines=240]`;
- old `read` / `search` results are replaced by `[elided read path=... range=... sha=... ref=...]`;
- **the current version** keeps one bounded snapshot (single file ≤ 8KB; larger files keep only sha + key ranges + "read on demand");
- the stable prefix stays untouched to protect the provider's prefix cache; only the dynamic tail is recompiled;
- the suggested per-step input budget matches the worker (24k); when exceeded, trim old interactions and snapshots first, never goal / AC / constraints.

This turns per-step cost from `O(all history)` into `O(stable prefix + current state + recent interactions)`. Old rewrites stop compounding, which is the main force behind token savings.

> Cost: recompilation invalidates the prefix cache of the "dynamic tail"; this needs an A/B on a real provider comparing "input saved" against "cache discount lost". The stable prefix is unchanged, so part of the cache can still hit.

#### B. Patch-first tools (directly reduce output and history size)

- add a `patch` tool (unified diff / small hunks), or force existing files through `edit`;
- `write` is only for new files, small files, or explicit `mode = "replace"`;
- cap `write` content per call (e.g. 16KB), requiring patch or chunking beyond that;
- improve `edit` match robustness (CRLF, leading/trailing whitespace, context anchors), and on failure return "please read the exact range", **not** "rewrite the whole file";
- add a line to the acceptance failure packet: "make the minimal change with edit/patch; do not regenerate the whole file."

The benefit of patch is twofold: less output, and a smaller payload entering history.

#### C. Token-aware stop-loss / restart

- record usage per step, and separately attribute "steps that contain a whole-file rewrite";
- estimate continuation cost ≈ `remaining steps × current context tokens`, restart cost ≈ `new run's stable prefix + known state + redo steps`;
- stop only when `continuation cost > restart cost + margin`; by default restart at most once to avoid restart oscillation;
- on restart use a **newly compiled short context** plus a churn summary, rather than carrying over the old run's full history.

This is "stop-loss", not the main saving mechanism; it addresses the case where the run has already entered a bad trajectory and continuing is plainly more expensive.

#### D. Prompt / routing (reduce probability of occurrence)

- the prompt states clearly: minimal diff, read before edit, do not rebuild files from memory;
- the failure packet states clearly: do not repeat the same failed change, do not rewrite the whole file;
- `loom churn stats --by model` produces per-model token waste; edit-heavy tasks avoid high-churn models or move to a stronger tier;
- these are auxiliary and cannot replace A/B.

### 1.4 Metrics: tokens first, file metrics only for diagnosis

`churn.json` records both kinds of metrics:

| Metric | Type | Definition |
|---|---|---|
| `rewrite_output_tokens` | estimated | payload of repeated whole-file writes; estimated as `bytes / 4`; multiple calls in one step are not yet broken out as `shared` |
| `history_carry_tokens` | estimated | the tokens the old payload would be resent for over the next N steps if not projected: `payload × remaining steps` |
| `avoided_input_tokens` | estimated | input tokens actually saved by step-level projection: per step `(history_bytes − projected_bytes) / 4` |
| `retry_tokens` | actual | extra consumption from churn-triggered acceptance feedback / restarts (**not wired up**, not folded into waste yet) |
| `wasted_tokens_est` | estimated | `max(0, rewrite_output_tokens + history_carry_tokens − avoided_input_tokens)` |
| `wasted_cost_usd_est` | estimated | computed from the model price table; unknown prices marked `unknown`, never guessed |
| `write_passes` / `churn_ratio` / `oscillations` | diagnostic | used to explain "why it was expensive", not used as a KPI |

- estimates uniformly use `loom-core::text::estimate_tokens`, and `churn.json` is marked `estimated: true`;
- `loom churn stats` shows token/cost on the first screen and file-operation metrics on the second;
- only then can it answer "how many tokens did DeepSeek v4.1 flash actually waste", instead of just showing "rewrote 3 times".

### 1.5 Intervention ladder (rearranged)

| Level | Trigger | Action | Where tokens are saved |
|---|---|---|---|
| L0 baseline | always | step-level context recompilation + current file snapshot + patch-first | prevents old payload compounding; reduces output |
| L1 projection | 2nd whole-file rewrite of the same file, with no verify improvement | immediately project old payload; keep only the current version; inject one status line and "use edit/patch" | every later step sends less old full text |
| L2 constraint | 3rd whole-file rewrite | reject whole-file `write` without `mode=replace`; very short error asking for patch/edit | prevents new full text from entering history |
| L3 stop-loss | churn tokens exceed the budget threshold with no verify improvement in a row | compare continue/restart cost; stop the run if needed, restart with newly compiled context | truncates remaining compounding |

Explicit: **L2 is not the main mechanism**. Its value is "preventing the next full text from entering history", not "saving this write". If L2 makes the model retry repeatedly, fall back to L1/L3.

> **Implementation status (2026-11-09, levers A/B/C landed)**: step-level projection is implemented in
> `loom-native` — `src/crates/loom-native/src/project.rs`. Rules: when the dynamic tail (after
> system+packet) exceeds `LOOM_NATIVE_HISTORY_BYTES` (default 24KB, 0 disables) and the estimated saving
> is ≥ `LOOM_NATIVE_MIN_CLEAR` (default 2000 tokens), old assistant `write`/`edit` arguments are
> compressed into `[elided …]` references (keeping path/id/reasoning_content), old tool results
> compressed to first line + elision marker (keeping artifact paths), and the most recent
> `LOOM_NATIVE_KEEP_TURNS` (default 4) turns plus the stable prefix are kept verbatim.
> Only content is replaced, messages are never deleted (`tool_call_id` pairing can never orphan);
> `reasoning_content` is never stripped (official DeepSeek requires it back); the projection is
> deterministic and idempotent (a message changes only once when it crosses the keep boundary,
> thresholds are judged monotonically against the "full history", so it does not oscillate).
> Offline validation: 6 unit tests + `scripts/smoke-churn.sh` (EARLY output gets projected, LATE kept); `smoke.sh` / `smoke-native.sh` / `smoke-parallel.sh` regressions all green.
>
> Levers B (patch-first / write guard) and C (token attribution) landed the same day:
> - `write` defaults to `mode=create`; overwriting an existing file requires explicit `mode=replace`,
>   with a 64KB per-call content cap; repeated overwrites are hinted "prefer edit" in the observation;
> - when `edit` misses, it returns "read the exact range, copy the current text, do not rewrite the
>   whole file", no longer pushing the model toward full-text rewrite;
> - each step's usage line carries `step` / `tool_names` / `write_paths` / `rewrite_count` /
>   `history_bytes` / `projected_bytes`, with `model` and `model_id` recorded separately;
> - every native run writes `logs/churn.json` (marked `estimated: true`), and
>   `loom churn stats [--by model]` shows token/cost on the first screen, write counts on the second;
> - `scripts/smoke-native.sh` validates the churn file and the aggregation command.
>
> Real A/B (official `deepseek-flash`, same fixture, `scripts/ab-churn.sh`):
> with a full history of ~16–17KB, projection off gives 5.2–5.4k input per step, on gives 2.3–2.5k
> (−46%–55%); per-step tokens 3,906 → 2,955, per-step cost −29%. Sample n=1,
> it only proves the mechanism; numbers and methodology see `doc/16` §8.8.
>
> **Not done**: the L1–L3 intervention ladder (automatic projection exists, but the "trigger on the
> Nth rewrite" threshold actions and cost-aware restart are not wired up); `retry_tokens` is still only
> diagnosed from acceptance feedback and is not folded into `wasted_tokens_est`; the `shared` allocation for multi-call steps is not broken out.

### 1.6 External adapters

External agents own their own conversation history, so Loom cannot project step by step. The only things we can do:

- require patch-first, minimal diff, and context caps in the adapter prompt / configuration (if the adapter supports them);
- the watchdog only terminates on gross loops where "continuation cost is clearly > restart cost"; restarting an external agent may re-read the whole repo and the benefit is uncertain, so the default is conservative;
- use `churn.json` and `loom churn stats` afterwards for model/agent selection;
- to actually save tokens, prefer adapters that support short contexts / recompilation (native or a wrapped external agent).

### 1.7 Feasibility

| Part | Feasibility | Note |
|---|---|---|
| step-level context recompilation | **high** | native owns its own messages; a prototype in 2–4 days; needs A/B on quality and cache |
| patch tool / forced edit | **high** | the tool-layer change is clear; 2–3 days including tests |
| token attribution and churn.json | **high** | per-call usage already exists, just add the step dimension |
| cost-aware restart | **medium** | needs reliable remaining-cost estimates; restart oscillation is easy; default at most 1 |
| external adapter token savings | **low–medium** | history is not in Loom's hands; mainly prompt and selection |
| model difference conclusions | **low (now)** | missing real keys / billing / fixed task set; accumulate token metrics first |

### 1.8 Acceptance (when implemented in the future)

- [ ] A scripted provider rewrites the same 8KB file 5 times in a row; without projection, input compounds with step count; with projection, it is bounded.
- [ ] `churn.json` outputs `rewrite_output_tokens`, `history_carry_tokens`, `avoided_input_tokens`, `wasted_tokens_est`, marked `estimated`.
- [ ] After the 2nd rewrite the old payload is projected; the 3rd `write` without `mode=replace` is rejected with a short error; after L3 triggers, the restart carries only a churn summary.
- [ ] The same change made with a patch vs. a whole-file `write` shows a quantifiable gap in output tokens.
- [ ] Acceptance still passes after projection/restart, and restarts cannot go on forever.

---

## 2. Problem two: a question pauses the session, and human items are drowned by history

### 2.1 Current state (this is a correctness defect + a visibility defect)

> **Implementation status (2026-11-09, correctness defect fixed)**: `TaskStatus::NeedsInput` +
> `task.needs_input` event wired in; `loom verify` now keeps the adapter's claim status when
> acceptance fully passes (`needs_input` / `blocked` / `budget_exceeded` are no longer written as
> `task.failed`), `dispatch` accepts `needs_input` for re-dispatch, and the board gained a
> `needs_input` column. M3 escalation counts are therefore no longer polluted by questions. The
> visibility part (attention inbox) is still not done, see §2.2.

Current flow (before the fix):

1. adapter exit code `2`, receipt status `needs_input`;
2. `loom verify` only treats `ReceiptStatus::Done` as `claim_done`;
3. so `needs_input` / `blocked` both end up as `task.failed` (`code = claim_not_done`);
4. M3 escalation only counts `task.failed` events → **an agent asking a question gets recorded as a failure, and can even trigger a worker→frontier escalation**;
5. `dispatch` only accepts `pending/ready/failed/escalated`, with no "answered, continue" state;
6. an external agent actually waiting on stdin hangs until timeout, burning wall clock.

The second problem is **visibility**: even if a question is correctly recorded as `needs_input`, it is still only an event or one `questions.json`; once the user leaves the current session, or moves on to ask other questions, the reminder sinks into history. And questions are not the only thing that genuinely needs human involvement:

| Items needing a human | Example | Current problem |
|---|---|---|
| answer | "Is the migration compatible with the old schema?" | `questions.json` only covers agent-initiated questions, and has no resident entry point |
| approval | changing AC, cross-tier escalation, raising budget, irreversible migration, merge/push | scattered across failure packets, receipts, logs; no unified todo |
| operation | installing `goose`, providing an API key, starting a service, running a migration | `doctor` only outputs when invoked; the user does not know unless they look |
| review | high-risk diff, red baseline, skill promotion, review finding | no "awaiting my confirmation" queue |
| reminder | a task waiting for an answer times out, budget approaches its cap, a lock is about to expire | only possible via polling or memory |

So the problem is not just "a question pauses the session" but that **the control plane lacks a human-items outlet independent of chat history**. This outlet must carry questions, approvals, operations, reviews and reminders alike, and stay visible at a fixed place in the workspace.

Related implementation: `src/crates/loom-cli/src/verify_cmd.rs` (`claim_done` / `TaskFailed`), `src/crates/loom-adapter/src/mock.rs` (`NeedsInput`), `src/crates/loom-core/src/task.rs` (no `NeedsInput` status), `src/crates/loom-cli/src/doctor_cmd.rs` (outputs only when explicitly invoked).

### 2.2 Human-items inbox (attention inbox)

Goal: at any time, a single command answers "what needs my attention now", and it stays displayed in a fixed corner of the harness instead of relying on the user to scroll back through the conversation. Items **do not only include questions**, but also approvals, operations, reviews and reminders.

It is not a chat tool, nor a ticketing system: an item is "a structured todo + evidence references + actionable commands", with no message threads and no discussion.

#### Item types

| `kind` | Meaning | Typical source | Closure condition |
|---|---|---|---|
| `question` | the agent explicitly asks, waiting for a human or upstream answer | `questions.json` | `task.answered` or an explicit answer |
| `approval` | needs explicit approve/reject | changing AC, cross-tier escalation, raising budget, irreversible migration, merge/push | explicit approve/reject |
| `operation` | a human must perform an action | missing tool, missing API key, service not started, migration needs running | probe passes or explicit completion |
| `review` | needs human judgment | high-risk diff, red baseline, skill promotion, review finding | explicit review conclusion |
| `reminder` | time/threshold reminder | answer timeout, budget near cap, lock about to expire | condition cleared or time passed |

`severity` has only three levels:

- `blocking`: the related work has stopped and cannot proceed without handling (e.g. `needs_input`, missing a critical tool);
- `soon`: work can continue, but should be handled before the next commit/dispatch (e.g. a non-blocking question, red baseline, review suggestion);
- `info`: only appears in the full list, not in the fixed corner, not counted as a todo.

#### Data model

`.loom/inbox.json` is a **view**, generated by event replay; the source of truth remains `events.jsonl`. Do not hand-edit it.

```json
{
  "v": 1,
  "generated_at": "2026-11-07T12:00:00Z",
  "counts": {"blocking": 1, "soon": 2, "info": 3},
  "items": [
    {
      "id": "ATT-20261107-003",
      "kind": "operation",
      "source": "env",
      "severity": "blocking",
      "status": "open",
      "title": "goose is missing in the container; the adapter canary cannot run",
      "detail": "goose not found in PATH; extend the image or mount the binary first",
      "refs": ["doc/14-current-issues-and-blockers.md#2", "artifact:adapter-canary-3"],
      "task_id": null,
      "run_id": null,
      "dedupe_key": "env:missing-tool:goose",
      "occurrences": 2,
      "created_at": "2026-11-07T09:00:00Z",
      "last_seen_at": "2026-11-07T12:00:00Z",
      "defer_until": null,
      "expires_at": null,
      "actions": [
        {"id": "show", "label": "Show install steps", "cmd": "loom doctor --explain goose"},
        {"id": "ack", "label": "Got it", "cmd": "loom inbox ack ATT-20261107-003"}
      ]
    }
  ]
}
```

Constraints:

- `title` ≤ 120 characters, `detail` ≤ 2KB; `refs` holds only path/event/artifact references, never full text or logs;
- `source`: `task | ci | skills | env | git | human`;
- `dedupe_key` is unique among open items; a repeated occurrence of the same problem only increments `occurrences` and `last_seen_at`, never floods the list;
- `actions[].cmd` is a **suggested command**, shown as text only, never auto-executed, never shell-evaluated;
- secrets, tokens, and full provider responses are forbidden inside items;
- resolved items stay in the view for 7 days for later review; events are kept forever.

#### Lifecycle and events

| Event | `data` | Effect |
|---|---|---|
| `attention.opened` | `item_id`, `kind`, `source`, `severity`, `dedupe_key`, `title`, `refs`, `task_id?`, `run_id?` | creates an item; if the same `dedupe_key` is still open, only `occurrences` accumulates |
| `attention.updated` | `item_id`, `changes` (`severity` / `detail` / `defer_until` / `occurrences`) | updates the item without changing its identity |
| `attention.resolved` | `item_id`, `by`, `reason`, `assumption_id?` | closes the item |
| `attention.expired` | `item_id`, `policy` | closes by expiry; only reversible/reminder kinds are allowed, irreversible `blocking` items may not silently expire |

States: `open → acked → resolved`, with side paths `deferred` / `expired`. `acked` only means "seen"; **it does not close the item and does not lower the `blocking` count**; `defer` is only allowed for `soon`/`info`, so blocking items cannot be lost by "handle it later".

Linkage between task events and items:

| Trigger | Item |
|---|---|
| `task.needs_input` | one `question` per blocking question; non-blocking questions open a `soon` |
| `task.answered` | automatically resolve the corresponding `question` |
| `task.blocked` | open an `operation` or `approval`, carrying the `reason` |
| `verify` finds a red baseline / CI failure | open a `review` (accept the baseline? how to fix?), with the log artifact attached |
| `loom doctor` missing tool / missing key | open an `operation` |
| routing cost guard rejects a dispatch | open an `approval` (raise budget, switch tier, narrow scope) |
| a skill reaches promotion conditions | open a `review` |
| consecutive `task.failed` on the same task | open a `review` (escalate, change AC, split the task) |

The control plane writes `attention.*` in step with task events. The two are not duplicated state: task events decide task state, `attention.*` decides "what a human needs to see".

#### Where the fixed corner lives

`.loom/inbox.json` is a stable view read by every front end (CLI, TUI, desktop, editor plugin); the "corner" is not bound to any one UI. v1 ships the CLI footer and statusline first; later interfaces reuse the same view and build no separate state.

| Outlet | Content | Constraints |
|---|---|---|
| footer of every human command | `[loom] 1 blocked · 2 pending → loom inbox` | writes **stderr**; TTY only; does not pollute `--json` stdout; auto-off for non-TTY/CI |
| `loom status` | counts + top 3 titles | shares the same view as the footer |
| `loom inbox` | the full list, sorted by `blocking → soon → info`, then by time | supports `--json`, `--kind`, `--task`, `--all` |
| `loom inbox --line` | one line, for shell prompt / tmux / starship / statusline polling | read-only, no side effects, fixed overhead; `[!] 1 blocking · 2 pending` |
| `loom inbox --check` | exit code `2` when any `blocking` exists | for scripts/hooks; does not print long output |
| TUI / desktop (later) | a clickable list of the same view | no separate state |

Additional rules:

- `--no-inbox` or `LOOM_NO_INBOX=1` turns off the footer; `loom inbox` itself no longer prints a footer;
- the footer shows a single line: the title of the highest-priority item, truncated; ANSI injection is forbidden;
- **the inbox is not injected into model context by default**: this channel is for humans. If the agent genuinely needs to know that someone is waiting, only inject counts (`[attention] include_in_packet = "counts"`), still under the context budget;
- the fixed corner is not a modal popup and does not block command execution; `blocking` only affects `--check` and task dispatch policy, and never interrupts the user by force;
- priority: `blocking` > `soon` > most recently updated; items on the same task are displayed aggregated but keep independent `id`s.

Configuration example:

```toml
[attention]
footer = "tty"              # tty | always | off
include_in_packet = "off"   # off | counts
dedupe_cooldown_s = 600
retain_resolved_days = 7
```

#### Noise reduction

- dedup: `dedupe_key` (e.g. `task:T-...:question:Q1`, `env:missing-tool:goose`, `ci:clippy:<tree>`);
- aggregation: several `soon` items on the same task count once in the footer and expand in the full list;
- cooldown: repeated reminders with the same `dedupe_key` are not counted again within 10 minutes by default;
- expiry: reversible questions with a default may expire and adopt the default (writing an assumption); irreversible/security/budget questions never expire automatically;
- closure: auto-resolve when the condition clears; when it cannot be judged automatically, an explicit `loom inbox resolve <id> --reason ...` is required;
- no "read means gone": `acked` items stay in the list, so one click does not make the user forget.

#### Parent/child agents and multiple tasks

- a subagent's question still returns as `questions.json` + exit 2; the control plane opens an item keyed by `task_id`;
- the parent agent answers on behalf when it can, and resolves; when it cannot, it floats the item up — the user sees one inbox and does not have to inspect each conversation;
- the same normalized question appearing in multiple subtasks merges into one item, with `refs` listing every `task_id`;
- the parent agent does not wait and does not sleep; the subtask stays `needs_input` while the parent continues with other tasks.

#### Security

- item text comes from agents/the repo and is treated as **untrusted input**: escape control characters, bound length, `refs` must not follow symlinks out of the repo;
- `actions[].cmd` is displayed only, never auto-executed; it takes effect only after the user runs it explicitly;
- item body text is never concatenated into a shell command; `loom inbox resolve --note` only writes to a file;
- if a TUI/desktop is built later, action buttons must go through the same command layer; calling internal functions directly to bypass validation is not allowed.

### 2.3 Principles

1. **A question is a return value, not a pause.** The process exits and the state lands on disk; after the answer, a new run continues — no session is kept alive.
2. **Questions must be structured.** "There is a question mark in the last paragraph of natural language" is not accepted as a protocol.
3. **Graded authorization.** What can be settled inside the spec/AC/constraints is answered directly; reversible items use a default plus an assumption record; irreversible/security/scope/budget questions must be answered by a human.
4. **Questions are not counted as failures.** `needs_input` is a normal state, not `failed`.
5. **Batch, dedup, TTL.** One answer should cover as many questions as possible; expiry uses a default or an explicit block.
6. **Parent and child agents do not wait on each other.** The child hands questions up as a return value; the parent answers what it can and floats the rest up, while continuing independent tasks.
7. **One outlet for human items.** Questions, approvals, operations, reviews and reminders all go into the attention inbox; a single chat message or a single log entry is not enough.
8. **Resident but low-noise.** footer/statusline stays visible; dedup, aggregation, `ack`/`defer` control the noise; but `blocking` never disappears because it was "read", and is not injected into model context by default.

### 2.4 Contract

When an adapter needs input:

- write `outbox/questions.json`;
- write `needs_input` on the `final.md` status line;
- exit code `2`;
- the process is started with `stdin = /dev/null` (any interactive prompt already in flight gets immediate EOF instead of hanging).

`questions.json` v1:

```json
{
  "v": 1,
  "task_id": "T-20261107-002",
  "run_id": "R-20261107-001",
  "questions": [
    {
      "id": "Q1",
      "question": "Must the migration script stay compatible with the old schema?",
      "why_now": "The migration rewrites the users table; the compatibility branch affects the rollback plan",
      "blocking": true,
      "scope": "design",
      "options": [
        {"id": "A", "label": "Stay compatible: dual-write one version", "consequence": "one extra migration, safe rollback"},
        {"id": "B", "label": "New schema only", "consequence": "small diff, old clients break"}
      ],
      "recommended": "A",
      "default": "A",
      "reversible": false,
      "evidence": ["src/db/migrate.rs:40-90", "artifacts/read-3"],
      "ttl_s": 86400
    }
  ]
}
```

Field constraints:

- `scope`: `implementation | design | contract | security | budget | scope`;
- a question with `blocking=false` can proceed on a default value and need not stop;
- `evidence` holds only path/artifact references, never full text;
- at most 3 blocking questions per task; beyond that, merge them or split the task.

### 2.5 States and events

Suggested additions:

| Event | data | State change |
|---|---|---|
| `task.needs_input` | `question_ids`, `blocking`, `scopes` | `receipt → needs_input` |
| `task.answered` | `question_ids`, `by`, `source`, `assumption_ids` | `needs_input → ready` |

Add `TaskStatus::NeedsInput`. Do not reuse `blocked`: `blocked` means dependency/lock/environment blocking, `needs_input` means waiting for a human or upstream answer; mixing the two distorts both statistics and escalation.

When writing `task.needs_input` / `task.answered`, the control plane writes `attention.opened` / `attention.resolved` in step (§2.2), so a question changes not only task state but also enters the resident inbox.

The `verify` result matrix must change together:

| Receipt status | acceptance | Result |
|---|---|---|
| `done` | all pass | `task.passed` |
| `done` | some fail | `task.failed` (`verify_fail`) |
| `needs_input` | any | `task.needs_input`, **not `task.failed`** |
| `blocked` | any | `task.blocked`, **not `task.failed`** |
| `budget_exceeded` | any | `budget.exceeded` + `task.failed` (or `blocked` per policy) |
| `failed` | any | `task.failed` |

This is the **existing behavior that most needs fixing first** in this design: questions/blocks must not pollute escalation counts.

### 2.6 Answer protocol

```text
subagent / worker asks a question
        │  exit 2 + questions.json
        ▼
control plane: task.needs_input (not counted as failed)
        │
        ├─ decidable within spec/AC/constraints ──► auto-answer (citing evidence)
        ├─ reversible and allowed by policy ──────► adopt default + record assumption
        ├─ needs design judgment ─────────────────► bounded answer from judgment/frontier
        └─ irreversible/security/scope/budget ────► human inbox
```

Authorization matrix:

| `scope` | worker answers itself | parent/sibling judgment | human required |
|---|---|---|---|
| `implementation`, reversible | default/policy | may override | no |
| `design`, reversible | no | yes, must cite the spec | no |
| `contract` / AC semantics | no | may only propose candidates | yes when changing AC/goals |
| `security` / data deletion / irreversible migration | no | no | yes |
| `budget` / cross-tier / widening scope | no | no | yes |

Answers land in `outbox/answers.json` (or control-plane `.loom/answers/<task>.json`):

```json
{
  "v": 1,
  "task_id": "T-20261107-002",
  "answers": [
    {
      "question_id": "Q1",
      "choice": "A",
      "by": "human|judgment|policy|spec",
      "rationale": "Keep the rollback path",
      "assumption_id": "A-001"
    }
  ]
}
```

Next `dispatch` / `resume`:

- inject the **summary** of `questions.json` + `answers.json` into the packet;
- open a new run, with `parent_run_id` pointing at the run that asked;
- `attempt` does not change (asking is not a failure), and `question_round` is recorded separately;
- the failure packet carries only unanswered questions, never answered ones.

`loom answer` writes `answers.json` and, at the same time, `task.answered` and `attention.resolved`, so the corresponding inbox item disappears immediately after answering; `--choice` also allows batch-answering non-blocking questions straight from `recommended`/`default`.

### 2.7 Assumption ledger

For questions answered by "reversible + continue on a default", write `.loom/assumptions.jsonl`:

```json
{"v":1,"id":"A-001","task_id":"T-...","run_id":"R-...","assumption":"keep old-schema compatibility","source":"policy","reversible":true,"verify_hint":"run the A3 rollback test after migrating"}
```

review / verify can check whether an assumption was overturned by later evidence. That way "continue first" does not become "guess quietly".

### 2.8 Parent and child agents

- The subagent has no "question channel", only a return value: `questions.json` + exit 2.
- The parent agent's `[questions]` policy decides whether it may answer on behalf: `allowed_scopes`, `max_auto_answers`, `max_question_tokens`.
- The parent agent **does not sleep and does not wait**: the subtask enters `needs_input` while the parent continues tasks that do not depend on it; tasks that do depend on it stay `pending`.
- Dedup: hash by `task_id + normalized_question`; the same question appearing in multiple subtasks merges into one inbox entry.
- Budget: the judgment call that answers a question is accounted separately, with a `max_cost_usd`; answers must not be allowed to eat through the task budget.

### 2.9 Fallback for external adapters

- State it in the adapter prompt: "Do not ask interactively; write `outbox/questions.json` and exit 2."
- Launch arguments uniformly use `stdin = null`.
- If the process still hangs: the watchdog terminates after "no tokens, no file changes, no log growth" persists for N seconds, reads the last agent message, synthesizes one `Q?` (`confidence=low`), marks `needs_input`, and opens a `blocking` attention item instead of `failed`.
- If the adapter exits 0 but the content is plainly a question with no diff: do not auto-classify it as `needs_input` (easy to misjudge); let verify fail, and hint in `next_hint` that "if input is needed, use questions.json + exit 2". Only enable the heuristic when the adapter explicitly declares `supports_questions`.

### 2.10 Feasibility

| Part | Feasibility | Note |
|---|---|---|
| control-plane states/events/answers | **high** | pure files and events, can be done before any real model |
| attention inbox view + footer/`--line` | **high** | event replay + read-only view; no model, no network dependency |
| native `ask` tool | **high** | writes questions.json + returns exit 2; testable with a scripted provider |
| external adapters honoring the protocol | **medium** | depends on whether the agent obeys; needs a canary and a fallback |
| auto-answer quality | **medium** | authorization boundaries must be explicit; better to float up than to answer badly |
| parent/child multi-agent | **medium** | no concurrency today; the protocol can be defined under sequential execution and parallelized in M4 |
| auto-closing "operations/approvals" | **low–medium** | probe-based ones (missing tool) can close automatically; judgment still needs a human |

Conclusion: **fix verify's `needs_input` misjudgment first, build `attention.*` + `loom inbox` at the same time, then `questions.json` and `loom answer`; add a canary for external-adapter compliance once the binaries are in place.**

Visibility can be completed independently of any real model: even with no agent asking questions, `doctor`/`ci`/routing guards can immediately produce `operation`/`approval`/`review` items.

---

## 3. Problem three: skills governance

### 3.1 The problem

A large number of skills are unvalidated; global installation means:

- name+description sit in context permanently, and a few dozen of them eat 25k–40k tokens;
- unaudited scripts, network access, license and prompt-injection risks;
- the model oscillating between conflicting skills;
- no way to answer "did this skill make CPVT better or worse?"

Take the layering of [Recommending Agent Skill installation from repo characteristics](https://yb.tencent.com/s/DsY6aNRnbq6v) as a reference: **Scanner (zero-LLM scan) → Catalog (offline index) → Resolver (scoring) → Installer (project-level install) → Runtime (progressive disclosure)**. This layering matches Loom's "externalized state + delivery by reference + hard context budget", so adopt it, with two tightenings:

1. **no global installation** — install at project level only, with a lock written;
2. **no embedding retrieval and no auto-install over the network in v1**, consistent with the R0/R1 boundary of [09](09-code-search-and-knowledge-base.md).

The first real use case for skills is not "install a pile of capabilities" but the review/check templates of local CI (§4.9): the repo profile decides what to recommend, the lock guarantees reproducibility, static checks bound permissions, and the L2 evaluation decides whether something can move from advisory to gate.

### 3.2 Repo profile

`loom skill scan` outputs `stack-profile.json` (deterministic, no LLM):

```json
{
  "v": 1,
  "root": "/work",
  "git_head": "365bc01",
  "lang": ["rust"],
  "frameworks": [],
  "infra": ["podman", "make"],
  "tests": ["cargo-test"],
  "toolchain": {"rust": "1.99.1", "container": "podman"},
  "evidence": {
    "rust": ["Cargo.toml", "src/Cargo.toml"],
    "container": ["Containerfile", "scripts/dev.sh"],
    "make": ["Makefile"]
  },
  "hash": "sha256:..."
}
```

Rules:

- every tag must point to an evidence file; tags without evidence do not appear;
- cache key = `git_head` + mtime of key files; no rescan on every session;
- the scan is read-only and never modifies the repo;
- this is a "profile", not an "install recommendation".

### 3.3 Catalog and metadata

A local catalog (`skills.catalog.json`, an offline cache, incrementally refreshed from trusted sources on a schedule):

```json
{
  "v": 1,
  "generated_at": "2026-11-07T00:00:00Z",
  "skills": [
    {
      "id": "rust-minimal-diff",
      "version": "0.2.0",
      "source": "internal/skills",
      "commit": "abc123",
      "sha256": "...",
      "license": "MIT",
      "tags": ["rust", "edit"],
      "triggers": {"lang": ["rust"], "globs": ["**/*.rs"], "roles": ["implement"]},
      "tokens": 900,
      "requires": {"tools": ["read", "edit"], "network": false},
      "permissions": {"shell": false, "write": ["src/**"]},
      "eval": {"tasks": 40, "first_pass_delta": 0.06, "cpvt_delta": -0.11},
      "status": "core|project|experimental|blocked",
      "min_loom": "0.2"
    }
  ]
}
```

Meaning of `status`:

| Status | Meaning | Loaded by default |
|---|---|---|
| `core` | passed L2 evaluation, broadly applicable, small | may be default per role |
| `project` | project-specific, reviewed | that project only |
| `experimental` | unvalidated or with an insufficient evaluation sample | only with an explicit `--skill` |
| `blocked` | security/license/conflict | never |

### 3.4 Resolution and scoring

`loom skill recommend --task T-...`:

```text
score = tag_match * 3
      + evidence_match * 2
      + role_match * 2
      + trust * 2
      - token_cost * 0.02
      - conflict_penalty
      - already_loaded_penalty
```

Constraints:

- at most 5 suggestions, at most 2 auto-loaded;
- below the threshold only `suggest` is shown, nothing is loaded;
- user `always_load` / `never_load` overrides;
- output must carry `why` (which tag matched, which evidence file);
- no embeddings; keywords/tags/globs are enough for v1.

### 3.5 Install and lock

- write only into a project-level directory: `.loom/skills/<id>/` (or in-repo `skills/` — pick one; `.loom/skills` is suggested so code search is not polluted);
- `skills.lock` records `id/version/source/commit/sha256/license/installed_at`;
- `loom skill add/remove/update/verify`;
- global installation is forbidden; auto-running install scripts is forbidden;
- static checks run before install, and install is refused on failure;
- rollback = restore the previous version from the lock.

### 3.6 Runtime loading

**Progressive disclosure**:

1. the packet carries only the skill index: `id | one-line trigger condition | token estimate`;
2. the model or control plane reads the body with `loom skill show <id>`;
3. before the body enters context it passes the Context Compiler's hard budget: ≤4KB per skill, ≤8KB for all skills;
4. when over budget, trim skill bodies first, never goal/AC/constraints;
5. a skill may not modify TaskSpec/AC/budget; violations are refused at load time.

### 3.7 Validation and promotion

| Level | Check | What it proves |
|---|---|---|
| L0 static | frontmatter, schema, size, links, license allowlist, script/network permissions, prompt-injection patterns | structurally valid, no obvious risk |
| L1 sandbox | run scripts in a temp dir, network off, `HOME` cleared, read-only mounts | it does not immediately break the environment |
| L2 evaluation | A/B on a fixed task set: loaded vs not loaded, comparing first-pass verify, CPVT, TPVT, wall clock | whether it is actually useful |
| L3 production | real task telemetry: load rate, task outcomes, human rework | whether it is worth keeping long-term |

Promotion rules:

- `experimental → project`: L1 passes + human review over at least 10 tasks;
- `project → core`: L2 passes (≥ 20 tasks, first-pass rate not down, CPVT not up) + license/security re-review;
- sustained negative returns in L3 → demote or `blocked`;
- any skill with script/network permissions must be human-reviewed and cannot be promoted automatically.

**Preloaded policy**: v1 does not preload domain skills; the core prompts already live in `loom-context`'s versioned prompt library. At most 2–3 "meta skills" are preloaded (e.g. minimal diff, verify-first), and they must be small, measurable, and switchable off.

### 3.8 Security and supply chain

- license allowlist (MIT/Apache-2.0/BSD; others by human review);
- verify `sha256` plus a pinned commit; floating branches are forbidden;
- no network and no shell by default; declare permissions explicitly when needed;
- prompt-injection scan: external links, hidden instructions, `ignore previous`, patterns that read secrets;
- skill bodies are treated as **untrusted input** and cannot override system constraints;
- uninstall/disable takes effect immediately and does not affect run records already produced.

### 3.9 Observability

`loom skill stats`:

- per skill: how often it was recommended / loaded / actually referenced;
- first-pass rate, CPVT, TPVT, wall clock after loading;
- comparison against the not-loaded baseline;
- conflicts: alert when contradictory skills are loaded for the same task.

### 3.10 Feasibility

| Part | Feasibility | Blocked by |
|---|---|---|
| repo scan / profile | **high** | none |
| local catalog / lock / resolver | **high** | none |
| static checks / permission model | **high** | none |
| L2 evaluation / promotion | **low (now)** | missing real model, fixed task set, billing |
| networked catalog sync | **medium** | needs curation and a trusted source; not in v1 |
| embedding retrieval | **not doing** | conflicts with the 09 boundary, benefit unproven |

Conclusion: **do "scan + local catalog + on-demand loading + static checks + lock" first; put evaluation and the networked catalog later.**

---

## 4. Problem four: initialization and agent/harness-driven local CI

### 4.1 Reframing

Local CI **is not merely acceptance of the agent's work**. It is first the project's own repo-level check surface: move formatting, lint, type checking, unit tests, code review, commit-message checks and other locally completable actions forward, so that both agents and humans can resolve them before `git commit` / `git push`, reducing duplicated work on server CI.

**What it applies to**: this section describes the **target project** that Loom manages, not Loom's own source repository. `loom ci` always runs at the project root that `--root` points to, and the configuration belongs to that project. Using the same mechanism in the Loom repository is dogfooding; `./scripts/dev.sh` and the Rust workspace are just one example of a runner for Loom itself, not a product default, and must not be written into the generator's generic rules.

Three layers to keep apart:

| Layer | Responsible for | Authority |
|---|---|---|
| TaskSpec acceptance | whether this task reached the agreed goal | `loom verify`, the task-level authority |
| repo local CI | whether this repo is currently healthy and can be committed/pushed | `loom ci run`, the local gate and feedback |
| server CI | cross-platform, integration, release, security and compliance | the server, the final authority |

The goal is not "replace the server with local", but:

- deterministic fast checks run locally, and failures feed back to the agent or human immediately;
- format/lint/review problems are resolved before commit/push;
- locally produced verifiable reports/attestation, with the server re-running only the jobs that must execute in a trusted environment;
- agents get feedback through the same command set, without each project re-guessing "how does checking work here";
- skills provide reusable, verifiable check templates and review rules instead of scattered scripts.

### 4.2 Current state

`loom init` only:

- creates `.loom/{tasks,runs,ledger,snapshots,locks}`;
- writes the default `config.toml` and the contract-layer guide `.loom/README.md` (no more goal/constraints placeholder templates; those are drafted by `loom goal` / `loom constraints`);
- migrates the root `.gitignore` to a whitelist (`/.loom/*` plus negations, contract layer tracked, state layer ignored).

`loom doctor` only checks `.loom`, git, config, writability. It does not detect the stack, does not check the toolchain, does not generate acceptance, does not provide local CI. The result:

- every project hand-writes acceptance;
- the agent does not know "what the normal checks for this repo are";
- a missing `pnpm`/`cargo`/`just` in the environment fails only at verify time;
- there is no baseline concept, so an agent may treat "tests that were already red" as its own breakage;
- format/commit-message and similar checks can only run on the server: slow feedback, wasted quota;
- code review depends on a human remembering to do it, and the agent has no structured review entry point either.

### 4.3 Division of labor between local and server

| Stage | Local by default | Server should still do |
|---|---|---|
| pre-commit | format, lint (safe auto-fixable fixes), type check, commit message, secret/debug-leftover scan, diff rules | secret/license/dependency audit re-checks in a trusted environment |
| pre-push | unit tests, build, affected-scope checks, advisory review, baseline comparison | platform matrix, integration/e2e, concurrency/performance, release builds |
| before task completion | the agent runs the `fast` profile and feeds failures back as observations | not applicable (verify remains the AC authority) |
| release/merge | generate local report and attestation | signing, artifacts, deployment, compliance and the final gate |

Principles:

- **deterministic checks first**: anything a single command can decide must not call an LLM;
- **review starts advisory**: LLM/agent review gives findings by default and does not block directly; consider promoting to a gate only after accumulating a false-positive rate;
- **local evidence is not a trust boundary**: v1 does not skip server CI based on a local attestation; attestation serves only as evidence and measurement. Once signed/trusted runners exist, that is still only opened to deterministic jobs with `local_sufficient = true`; security, release and signing jobs always run on the server;
- **no automatic commit/push**: local CI may fix files and generate reports, but commit/push must be triggered by a human or an explicit policy.

### 4.4 Initialization design

`loom init` gains **detection + generation + report**, while honoring "never overwrite user files":

1. scan the repo and output `.loom/stack-profile.json` (shared with §3.2; a runtime view, not committed);
2. generate `.loom.ci.toml` at the target project root (skip if it exists; `--force` only overwrites files Loom generated). This is **project policy** and belongs in git; do not put it under `.loom/` — `.loom/` is runtime state and is ignored as a whole, so worktrees, teammates and servers cannot see it. Personal overrides can come later as `.loom/ci.local.toml` (ignored), not in v1;
3. generate an acceptance template (written as comments in `goal.md` or as `.loom/acceptance.toml`, without modifying the TaskSpec directly);
4. optionally install git hooks (`--hooks pre-commit,pre-push`, not installed by default);
5. optionally generate a thin wrapper for server CI (`--ci github`, not generated by default): attestation verification + server-only jobs only, not a copy of every local command;
6. print the `doctor` report: detected stack, suggested jobs, missing tools, installable skills, baseline status.

`loom init --dry-run` only prints the plan and writes nothing.

### 4.5 `.loom.ci.toml`

The example uses Rust because Loom itself is Rust; it looks like "this repo's CI", but the fields are universal across target projects. The generator must not hard-code `cargo` or `./scripts/dev.sh` as defaults.

```toml
version = 1

[profiles.pre-commit]
description = "pre-commit quick gate"
jobs = ["fmt-fix", "commit-msg"]

[profiles.pre-push]
description = "full pre-push gate"
jobs = ["fmt-check", "clippy", "test", "review"]

[profiles.fast]
description = "quick check before agent finish"
jobs = ["fmt-check", "clippy"]

[profiles.full]
description = "full verification"
jobs = ["fmt-check", "clippy", "test", "build", "review"]

[jobs.fmt-fix]
kind = "command"
cmd = ["cargo", "fmt", "--all", "--", "--check"]
fix_cmd = ["cargo", "fmt", "--all"]
safe_fix = true
timeout_s = 60
required_tools = ["cargo"]

[jobs.fmt-check]
kind = "command"
cmd = ["cargo", "fmt", "--all", "--", "--check"]
timeout_s = 60
required_tools = ["cargo"]

[jobs.clippy]
kind = "command"
cmd = ["cargo", "clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]
timeout_s = 600
required_tools = ["cargo"]

[jobs.test]
kind = "command"
cmd = ["cargo", "test", "--workspace"]
timeout_s = 900
required_tools = ["cargo"]
needs = ["clippy"]

[jobs.build]
kind = "command"
cmd = ["cargo", "build", "--workspace"]
timeout_s = 600
required_tools = ["cargo"]
needs = ["test"]

[jobs.commit-msg]
kind = "command"
cmd = ["loom", "ci", "check-commit-msg", "--file", "{commit_msg_file}"]
timeout_s = 10

[jobs.review]
kind = "review"
diff = "staged"                  # staged | head | range
rules = ["no-debug-prints", "no-secrets", "minimal-diff"]
advisory = true
max_tokens = 6000
timeout_s = 120
local_sufficient = false

[jobs.skill-review]
kind = "skill"
skill = "rust-review"
diff = "staged"
advisory = true
max_tokens = 8000
timeout_s = 120
```

Constraints:

- `cmd` uses an argv array to avoid shell quoting and injection; when a shell is genuinely needed, declare `shell = "..."` explicitly;
- `timeout_s` is required; `required_tools` is checked before running, and a missing tool produces an actionable error directly;
- `needs` expresses sequential dependencies only, no parallelism (before M4);
- `kind = "command"` is the default; `review` invokes a bounded agent review; `skill` loads a validated skill from §3;
- `fix_cmd` only allows deterministic, reversible fixes; `safe_fix = true` must be explicit; show the diff after running, never commit automatically;
- `advisory = true` means findings are suggestions/attention items only and do not block the commit; promotion to a gate requires evaluation;
- `local_sufficient = true` only means the job may be referenced by the server under trusted conditions; **v1 does not skip server jobs on that basis**. Default false, always false for security/release kinds; skipping is only discussed once signed/trusted runners exist;
- `env` is allowed, but secrets such as `LOOM_API_KEY` / `OPENAI_API_KEY` are cleared by default;
- logs go to `.loom/ci/<run_id>/<job>.log`, truncated to the 8KB/24KB budget before being returned to the agent, full text spilled to an artifact.

> Note on field shape: the example in this section is the full design, using `[jobs.<id>]` + `cmd`, and exists only to explain field semantics. [16](16-runtime-governance-adoption-assessment.md) §7.3 has locked the v1 implementation subset: `[[jobs]]` + `argv` + `[runner]`, field names `fix_argv`/`fix_safe`, no `kind`/`env`/`network`, and no review/skill jobs. Implement per 16 §7.3; do not support both.

Global configuration (`.loom/config.toml`):

```toml
[ci]
default_profile = "fast"
preflight = "off"           # off | fast
hooks_fix_safe = true       # run safe_fix in pre-commit first, then re-run the checks
hooks_fail_open = true      # whether to let the commit through when loom is not on PATH
attestation = true
sign = "none"               # none | ed25519
```

### 4.6 Commands

```bash
loom init --dry-run                       # just show the detection result
loom ci plan                              # detect and print suggested jobs + evidence + recommended skills
loom ci plan --write                      # write .loom.ci.toml (does not overwrite existing)
loom ci run --profile pre-commit          # pre-commit gate
loom ci run --profile pre-push --changed  # pre-push checks (affected-only later)
loom ci run --profile fast --fix          # run only safe_fix jobs, show the diff
loom ci install-hooks --profile pre-commit,pre-push
loom ci remove-hooks
loom ci doctor                            # toolchain, versions, missing commands, baseline, hook status
loom ci report --last                     # summary of the latest report (token-bounded)
loom ci verify-attest <file>              # verify a local report/attestation without re-running
```

`loom ci run` output:

```json
{
  "v": 1,
  "profile": "pre-commit",
  "ok": false,
  "tree": "git-tree-sha",
  "tool_versions": {"cargo": "1.99.1"},
  "jobs": [
    {"id": "fmt-fix", "kind": "command", "exit": 0, "duration_ms": 120, "fixed": true, "log": ".loom/ci/R-1/fmt-fix.log"},
    {"id": "commit-msg", "kind": "command", "exit": 1, "duration_ms": 5, "log": ".loom/ci/R-1/commit-msg.log"}
  ],
  "failed": ["commit-msg"],
  "report": ".loom/ci/R-1/report.json",
  "attestation": ".loom/ci/R-1/attestation.json"
}
```

The report/attestation records at least: `tree`, the command argv, tool versions, start/end times, exit codes, log hash, `local_sufficient`, and whether hooks actually ran / why they were skipped. `--no-verify` cannot be observed by hooks and is not recorded. Signing is optional (`sign = "none|ed25519"`), but **unsigned does not mean trustworthy**.

### 4.7 git hooks

- installed only explicitly by `loom ci install-hooks`; `.git/hooks` and the global `core.hooksPath` are never changed silently;
- writes `pre-commit` / `pre-push` carrying a `# loom-ci-managed` marker;
- an existing non-Loom hook: refused by default; `--force` first backs it up as `<hook>.bak.<timestamp>` and then installs;
- pre-commit runs the `pre-commit` profile by default, target ≤ 30 seconds; pre-push runs the `pre-push` profile by default, target ≤ 5 minutes; timeouts/long jobs are left to the server;
- with `hooks_fix_safe = true`, pre-commit first runs the fixes for `safe_fix` jobs and re-runs the checks; if the working tree changed, the hook exits `1` and asks for a fresh `git add`, **never staging or committing automatically**;
- a hook failure blocks the commit/push; `git commit --no-verify` still works. **Hooks cannot observe `--no-verify`**, so no bypass recording is promised; `loom ci report` records only what actually ran, and the server is the final check;
- when `loom` is not on PATH, default is fail-open with a warning (so a fresh clone is not broken), but it is recorded; configurable with `hooks_fail_open = false`;
- hooks call only the public CLI and never import internal crates directly.

### 4.8 agent / harness loop

```text
agent changes code
   │
   ├─ loom ci run --profile fast
   │     ├─ pass ──► continue / finish
   │     └─ fail ──► failure summary fed back as a tool observation
   │                  ├─ deterministic error: agent fixes it, re-run (bounded)
   │                  ├─ safe fix: loom ci run --fix, show the diff
   │                  └─ review/judgment kind: open an attention review item
   ▼
loom verify (TaskSpec acceptance, still the task authority)
```

- **native hint**: the system prompt states "run `loom ci run --profile fast` before finish"; failure output is truncated to the tool budget before being fed back; it does not replace verify;
- **baseline preflight**: `[ci] preflight = "fast" | "off"`, default `off`; when enabled, `loom dispatch` runs the fast profile before dispatching and refuses if red, hinting "fix the baseline first or add `--allow-red-baseline`";
- **review job**: deterministic rules run first (debug leftovers, secret patterns, oversized diffs, stale generated artifacts), then an optional agent call; findings are written as structured `findings`, go to attention `review` by default, and never modify code directly;
- **acceptance templates**: `loom init` can generate candidate AC from `.loom.ci.toml` (e.g. `cargo test --workspace`); a human confirms before it is written into the TaskSpec — it never takes effect automatically;
- **server consumption**: v1 only treats `loom ci verify-attest` as evidence/measurement and does not skip server jobs based on a local attestation; security/release/signing, cross-platform matrices and e2e always re-run. Skipping is only opened to deterministic jobs with `local_sufficient = true` after signed or trusted runners exist.

### 4.9 Connection to skills

Local CI is an early landing spot for putting skills to use, but a skill must not become a backdoor that bypasses checks:

| Usage | Note |
|---|---|
| `loom ci plan` recommends skills | based on the §3.2 repo profile, recommends `rust-review`, `conventional-commit`, `secret-scan`, etc.; outputs `why` and evidence |
| `kind = "skill"` job | loads only installed, statically checked skills that are in `skills.lock`; body subject to the 4KB/8KB budget |
| review rule templates | a skill supplies checklists/rules, but deterministic rules still come first; the LLM only judges and explains |
| local fix suggestions | a skill may suggest a patch, but it must land as a diff through the `edit`/`patch` tool; it may not run arbitrary scripts directly |
| promotion | an advisory review's false-positive rate, adoption rate, and CPVT impact must pass the §3.7 L2 evaluation before a gate is considered |

Hard constraints:

- a skill **cannot** turn off or lower the `local_sufficient` or `advisory` of an existing job;
- a skill **cannot** declare arbitrary shell commands that do not appear in `.loom.ci.toml`; if a command is needed it goes through `kind = "command"` and accepts the same timeouts/permissions/static checks;
- a skill **cannot** modify TaskSpec/AC/budget;
- no network and no shell is the default; when needed it is declared explicitly in `.loom.ci.toml`, not decided by the skill itself;
- installing/updating a skill must update the lock; CI runs never use floating branches.

### 4.10 Stack detection

| Evidence | Suggested jobs | Kept on the server |
|---|---|---|
| `Cargo.toml` | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace`, `cargo build --workspace` | cross-platform/MSRV matrix, integration tests |
| `package.json` + lock | pick `pnpm/yarn/npm` from the lock; `lint`, `typecheck`, `test`, `build` (only scripts that exist) | e2e, browser matrix, release builds |
| `pyproject.toml` | `ruff check`, `mypy`/`pyright`, `pytest` (per config) | multiple Python versions, integration |
| `go.mod` | `gofmt -l`, `go vet ./...`, `go test ./...` | race, cross-platform, integration |
| `Makefile` / `justfile` | adopt existing `check`/`test`/`lint` targets first; do not reinvent | depends on content |
| `.github/workflows/*.yml` | extract recognizable commands as **evidence** only, never force-parse YAML; when unsure, suggest and do not write config | the original workflow stays |
| `rust-toolchain.toml` / `.nvmrc` / `.python-version` | record version requirements; doctor checks whether the current version matches | matrices may cover it |
| `Containerfile` / `compose.yaml` | recorded as environment evidence; v1 does not nest containers automatically | containerized integration |

When detection finds nothing: write only a comment template, never guess commands. Same for review rules: only generic rules (debug leftovers, secret patterns, oversized diffs), no domain rules invented from guesses.

### 4.11 Environment and security

- v1 executes in the **current environment** and starts no nested containers; a missing tool inside a container is reported explicitly, never silently skipped;
- commands come from repo configuration and are treated as trusted; still clear provider secrets, bound output, and set timeouts;
- no network by default; a job that needs the network sets `network = true` explicitly;
- write operations are confined to the repo; symlinks are never followed outside it;
- `--fix` only applies to jobs with `safe_fix = true`; the diff is kept after running, with no automatic commit/push;
- local reports may contain code snippets and logs: manage them as artifacts, and run secret-pattern checks/truncation before writing;
- git hooks are installed explicitly, uninstallable, and backed up; global git config is never changed;
- an attestation is only evidence that "some environment ran these commands", not a security boundary; untrusted origins get no skipping;
- no full replacement of existing CI: v1 only adds a local entry point and evidence, with no server skipping; GitHub Actions and the like continue to exist independently.

### 4.12 Observability

`loom ci stats`:

- run counts, pass rates, P50/P95 wall clock per local profile;
- `safe_fix` trigger count, re-run count after agent fixes, first-pass rate;
- how often hooks actually ran/failed/were skipped (`--no-verify` cannot be observed and is not counted);
- review findings count, adoption/rejection/false-positive rate;
- failures caught locally and the estimated server re-run time saved (v1 only estimates; real savings are counted once server skipping is enabled);
- attention items created by CI failures and how long they took to handle.

The objective function is **reducing duplicated work on server CI and shortening the feedback loop**, not simply running more local commands.

### 4.13 Feasibility

| Part | Feasibility | Note |
|---|---|---|
| detection + generating `.loom.ci.toml` | **high** | rules are clear, `--dry-run` can validate first |
| sequential execution + log budget | **high** | reuses the existing tool output protocol |
| pre-commit/pre-push hooks | **high** | pure shell/CLI wrapper; mind backups and fail-open |
| safe fix + diff display | **high** | deterministic commands only; no auto-commit |
| local report/attestation | **medium** | format and tree verification are easy; the trust model needs to be explicit |
| review job (agent) | **medium** | quality/false-positive rate needs a real task set; advisory by default |
| skill-based review/checks | **medium** | depends on §3's lock/static checks; cannot bypass gates |
| baseline preflight | **medium** | red baselines are common in old repos, default should be off |
| environment consistency | **medium** | container/version drift; v1 detects only, does not repair |
| monorepo / multi-stack | **medium** | waiting on the M5 workspace design; v1 assumes a single stack at the root |
| server skipping based on attestation | **low (not in v1)** | local files can be forged; needs signed/trusted runners, deferred |
| nested containers | **low** | current dev already uses podman/docker, nesting is complex; deferred |

Conclusion: **do detection + `.loom.ci.toml` + sequential `loom ci run` + doctor first; then hooks, safe fix, report/attestation; review/skill jobs start advisory. Parallelism, caching, containerization and affected-only are deferred.**

### 4.14 Acceptance (when implemented in the future)

- [ ] `loom init` detects the stack in the target project (e.g. a temporary Rust/Node/Python repo; Loom's own dogfood is verified separately) and generates `.loom.ci.toml` without overwriting existing files.
- [ ] `loom ci run --profile pre-commit` fails on a format error, and `--fix` only fixes `safe_fix` jobs while showing the diff.
- [ ] With `hooks_fix_safe = true`, pre-commit auto-fixes safe items and re-runs; if the tree changed it asks for a fresh `git add`, never staging/committing automatically.
- [ ] `loom ci install-hooks` is idempotent; it refuses when a non-Loom hook exists, and `--force` backs up first; no promise of recording `--no-verify`.
- [ ] The report/attestation contains tree, argv, tool versions, exit codes; verification passes for the same tree and fails for a different tree.
- [ ] v1 does not skip server jobs using local attestations; only after signed/trusted runners exist may deterministic jobs with `local_sufficient = true` be skipped, and security/release/matrix jobs never.
- [ ] The review job has token/time limits, is advisory by default, produces structured findings, and has no network.
- [ ] `kind = "skill"` can only load skills that are in the lock and passed static checks, and cannot turn off a gate.
- [ ] CI failures open attention `review`/`operation` items; they auto-resolve after a success on the same tree.
- [ ] `loom verify` behavior is unchanged and still treats only TaskSpec acceptance as the task authority.

---

## 5. Cross-cutting contract changes

All of these directions add files/commands, but share one set of boundaries. Per `AGENTS.md` §4, every event/field change must update types, schema, docs and tests together.

| Direction | New artifact | New events | Config | CLI |
|---|---|---|---|---|
| churn | `logs/churn.json` (new schema) | `task.churn` (only on intervention) | `[guardrails.churn]` | `loom churn stats` |
| attention | `.loom/inbox.json` (read-only view) | `attention.opened`, `attention.updated`, `attention.resolved`, `attention.expired` | `[attention]` | `loom inbox [--line/--check]`, `loom inbox ack/defer/resolve` |
| questions | `outbox/questions.json`, `answers.json`, `assumptions.jsonl` | `task.needs_input`, `task.answered` | `[questions]` | `loom answer` |
| skills | `stack-profile.json`, `skills.catalog.json`, `skills.lock` | none (skill loading is not task state) | `[skills]` | `loom skill scan/recommend/add/show/verify/stats` |
| CI | `.loom.ci.toml` (tracked), `.loom/ci/<run>/{report,attestation}.json` (runtime) | `attention.*` (on failure/pending review) | `[ci]` | `loom ci plan/run/doctor/install-hooks/report/verify-attest/stats` |

Shared constraints:

- **state is still replayed from events**: churn/questions/attention artifacts are evidence or views, not the source of truth; state changes must be written as events.
- **no new interactive commands**: `loom inbox/answer` are batch operations and never enter a wait loop.
- **the fixed corner does not pollute machine output**: the footer writes stderr only and only on a TTY; `--json` stdout stays pure JSON; off by default for non-TTY/CI.
- **exit code semantics unchanged**: `0` success, `2` needs human/blocking, `3` budget/timeout, `4` adapter failure; `churn_guard` / `needs_input` are both `2`, distinguished by the event's `data.code`.
- **hard context budgets unchanged**: churn summaries, questions, and skill bodies are all subject to 8KB/24KB/128KB and the Context Compiler; attention does not enter model context by default.
- **local CI is not a trust boundary**: v1 does not skip server jobs based on a local attestation; only once signed/trusted runners exist is skipping for deterministic `local_sufficient` jobs discussed; security/release/signing always run on the server.
- **testable without a real model**: the attention/questions/CI core and churn projection can all be covered with mock/native scripted providers; the model part of review jobs is marked advisory separately.
- **churn's KPI is `wasted_tokens_est` / `wasted_cost_usd_est`, not write counts**: `write_passes` / `churn_ratio` only explain causes; L2 interception is not the main saving mechanism.
- **churn optimizes `messages` projection and tool shape** (patch/edit) and does not change the provider's wire protocol.

---

## 6. Priority and milestone suggestions

| Order | Direction | Why first | Depends on | Rough effort |
|---|---|---|---|---|
| P0 | CI core: init detection + `.loom.ci.toml` + sequential `loom ci run` + doctor | gives "local checks" a single entry point first; later measurement and hooks both lean on it | none | 5–7 person-days |
| P0 | question protocol + attention inbox | fixes the `needs_input → task.failed` correctness defect; also solves human items being drowned by history | none | 4–5 person-days |
| P1 | move CI earlier: pre-commit/pre-push hooks + safe fix + report | blocks format/lint/commit-message problems before commit, directly reducing duplicated server work | CI core | 3–4 person-days |
| P1 | churn: native step-level context recompilation + patch-first + token attribution | cut the input compounding of old full text first, talk interception later; metrics must precede policy | none | 4–6 person-days |
| P1 | churn watchdog (external adapters) | only gross-loop stop-loss and a basis for selection | adapter binaries must exist in the container | 2–3 person-days |
| P2 | CI review/skill job + attestation evidence | brings review and reusable check templates in; advisory first; server skipping deferred | move CI earlier + skills | 3–5 person-days |
| P2 | skills: scan + local catalog + lock + on-demand loading | stand the structure up first, avoid global install; CI is the first use case | CI/task set | 5–8 person-days |
| P2 | skills: L2 evaluation and promotion | proves a skill is actually useful; review advisory→gate depends on it too | real model + fixed task set + billing | deferred |
| P3 | embedding/networked catalog | benefit unproven | — | not doing |

Notes:

- there is no concurrency/locking today; P2's "parent and child agents do not wait" only defines the protocol and does not implement parallel scheduling; that comes in M4.
- the real goose/OpenCode canary is still blocked on binaries and API keys, see [14](14-current-issues-and-blockers.md).
- churn optimizes `messages` projection and tool shape and does not change the provider request format; external adapters can only observe the worktree and cannot be projected step by step.
- the attention inbox does not depend on a real model and should be built together with the question protocol; otherwise even with `needs_input` fixed correctly, the user still cannot find the todos.
- moving CI earlier and server skipping are two steps: get it running locally first, then talk attestation; do not skip server checks before there is a trustworthy tree/tool-version record.

---

## 7. What we will not do

- No global skill installation, no "more installed is better".
- No auto-install over the network, no auto-running skill scripts.
- No embedding/vector store, no MCP cross-project knowledge base.
- No interactive question waiting; a question must be a file + exit code.
- No calling an LLM once per question to classify it; deterministic policy first.
- Do not build the attention inbox into a chat/ticketing system, and no resident daemon; v1 is events + view + read-only polling only.
- Do not auto-execute `actions[].cmd` from the inbox; do not auto-close `blocking` items, and do not pass `acked` off as `resolved`.
- Do not silently install git hooks or modify global git config; no promise of recording `--no-verify`, with the server's final gate as the backstop.
- Do not use local CI to replace server security/release/signing gates; never trust an unsigned or unknown-origin attestation.
- Do not let a skill define arbitrary CI shell commands or turn off existing gates; commands must appear in `.loom.ci.toml` and accept the same checks.
- No nested-container CI (v1).
- Do not attribute churn directly to a model; accumulate per-model measurement first.
- Do not automatically modify AC/goals to "make the task pass"; AC changes must go through judgment/humans.

---

## 8. Open questions

1. Should `needs_input` use a new event or reuse `task.blocked`'s `code`?
   - Leaning new event + new state, so blocking statistics and escalation counts stop being distorted.
2. When churn exceeds the limit, default to `blocked` or `failed`?
   - Leaning `blocked`; `[guardrails.churn]` explicitly chooses whether it counts toward escalation.
3. Does `attempt` increment when a question is answered?
   - Leaning no; new run + `question_round`, avoiding the "question = retry" semantics.
4. Who maintains the skills catalog's trusted source and license policy?
   - Needs team curation; v1 only supports local catalogs and manual import.
5. Priority between `.loom.ci.toml` and an existing `Makefile`/CI?
   - Leaning "adopt existing commands", with `.loom.ci.toml` only aggregating and wrapping timeouts/logs.
6. Is baseline preflight on or off by default?
   - Leaning off by default, on after the project stabilizes, to avoid adoption friction.
7. Do parent/child agent questions need a separate "question budget"?
   - Needed; otherwise answers may eat the task budget — account separately with a cap.
8. Which form should attention's fixed corner take first in the CLI?
   - Leaning "stderr footer per command on TTY + `loom inbox --line` for the statusline"; TUI/desktop deferred, so we do not build a UI nobody looks at first.
9. Should `attention.*` be fully explicit events, or only for non-task sources, with task sources derived by the projector?
   - Leaning writing task events as usual while also writing `attention.*` explicitly, keeping the two dimensions of "task state" and "what a human sees"; but duplication must be prevented, so the projector needs to be idempotent.
10. Should pre-commit fail-open or fail-closed when `loom` is missing?
    - Leaning fail-open + warning + record, so a fresh clone can still commit; the server still does the final gate.
11. Which CI jobs may be marked `local_sufficient = true`?
    - Leaning only deterministic, low-environment-dependency items like format/lint/unit tests; security, release, signing, cross-platform matrices are always false.
12. What threshold promotes a review job from advisory to gate?
    - Needs false-positive rate, adoption rate, human review and fixed-task-set data; we have none today, so advisory first.
13. Do CI job templates provided by a skill need a separate signature/curation source?
    - Leaning yes; at least lock + static check + project-level install, never referencing a floating branch directly.

---

## 9. Minimal acceptance checklist (when implemented in the future)

- [ ] `loom ci plan` detects the stack in the target project (a Rust workspace when Loom dogfoods itself) and outputs jobs with evidence.
- [ ] `loom init` does not overwrite an existing `.loom.ci.toml` / workflow / hook; `--dry-run` writes nothing.
- [ ] With `cargo`/`pnpm` missing, `loom ci doctor` gives an actionable error instead of silently skipping.
- [ ] `loom ci run --profile pre-commit` fails on a format error; `--fix` only fixes `safe_fix` jobs and shows the diff, with no automatic commit.
- [ ] When `hooks_fix_safe` is in effect, pre-commit auto-fixes safe items and re-runs; if the tree changed it asks for a fresh `git add`, never staging/committing.
- [ ] `loom ci install-hooks` is idempotent; it refuses when a non-Loom hook exists, and `--force` backs up first; no promise of recording `--no-verify`.
- [ ] The report/attestation contains tree, argv, tool versions, exit codes; verification passes for the same tree and fails for a different tree; v1 does not skip server jobs on that basis.
- [ ] The review job has token/time limits, is advisory by default, produces structured findings, and has no network; `kind=skill` can only load skills in the lock that passed static checks.
- [ ] A `needs_input` receipt after `loom verify` has status `needs_input`, **produces no `task.failed`**, and does not raise the escalation count.
- [ ] `task.needs_input` automatically opens a `question` attention item; `loom answer` resolves it automatically.
- [ ] `loom doctor` opens an `operation` item when a tool is missing; `loom ci` opens a `review`/`operation` item on failure; items auto-resolve when the condition clears.
- [ ] On a TTY, human commands print one footer line on stderr; `--json` stdout contains no footer; nothing is printed for non-TTY/CI; `LOOM_NO_INBOX=1` turns it off.
- [ ] `loom inbox` shows both question and operation items; `loom inbox --check` exits `2` when any `blocking` exists; `ack` does not close an item, `defer` applies only to `soon/info`.
- [ ] A scripted provider calls the `ask` tool → `questions.json` → exit 2 → visible in `loom inbox` → `loom answer` → a new run receives the answer and passes verify.
- [ ] A scripted provider rewrites the same 8KB file 5 times in a row; without projection, input compounds with step count; with projection, it is bounded.
- [ ] `churn.json` outputs `rewrite_output_tokens`, `history_carry_tokens`, `avoided_input_tokens`, `wasted_tokens_est`, marked `estimated`.
- [ ] After the 2nd rewrite the old payload is projected; the 3rd `write` without `mode=replace` is rejected with a short error; after L3 triggers, the restart carries only a churn summary.
- [ ] The same change made with a patch vs. a whole-file `write` shows a quantifiable gap in output tokens.
- [ ] `loom churn stats --by model` prints token/cost on the first screen and only `write_passes` / `churn_ratio` on the second (validated with scripted data first).
- [ ] Every tag output by `loom skill scan` has an evidence file; no evidence, no tag.
- [ ] `loom skill add` writes only the project-level directory and `skills.lock`; a skill that fails static checks is refused.
- [ ] When a skill is loaded, only the index line is injected; the body is read on demand via `loom skill show`, under the 4KB/8KB budget.
- [ ] Every new event is synced to the `loom-core` types, `schemas/event.v1.json`, `doc/03` §11.2, and at least one test.

---

## 10. Adoption assessment

For the integration points in the current code, contract changes, phased person-days and risks, plus the three designs that must be corrected first (`.loom.ci.toml` placement and ownership, attestation authorization, `verify`'s claim/`needs_input` semantics), see [16-runtime-governance-adoption-assessment.md](16-runtime-governance-adoption-assessment.md).
