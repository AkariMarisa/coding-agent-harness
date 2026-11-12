# 16 Runtime Governance Adoption Assessment

> Language: **English** | [简体中文](16-runtime-governance-adoption-assessment.zh-CN.md)

> Status: **Assessment draft v2, no implementation code changed** (2026-11-07).
> Scope: the four directions — attention inbox / question protocol, local CI, churn, skills.
> Method: first verify the facts in the current code, then compare against mature external solutions, and finally give trade-offs, contracts, tests and phasing. The "should" statements in design drafts are not accepted as feasibility evidence.
> Prerequisite reading: [15-agent-runtime-governance-and-engineering.md](15-agent-runtime-governance-and-engineering.md), [14-current-issues-and-blockers.md](14-current-issues-and-blockers.md), [10-tool-output-protocol-v0.1.md](10-tool-output-protocol-v0.1.md).

---

## 0. Conclusion

All four directions can land within the current architecture, but they should not be pushed flat in 15's P0/P1 order. The most expensive problems today are not missing features but three semantic errors that will distort every later measurement:

1. `verify` only treats `ReceiptStatus::Done` as done, so `needs_input`, `blocked` and `budget_exceeded` all end up written as `task.failed(code=claim_not_done)`. An agent asking a question is recorded as a failure, and a worker→frontier escalation may be triggered. See `src/crates/loom-cli/src/verify_cmd.rs:108`, `:216`.
2. After reading `outbox/receipt.json`, `verify` overwrites the same path with the acceptance receipt. The original claim is lost, and afterwards there is no way to answer "was the worker asking, blocked, or out of budget at the time". See `verify_cmd.rs:181`.
3. Local CI configuration must not go into `.loom/`: that directory is now whitelist-layered (only the contract layer is committed, see `AGENTS.md` §6), but CI configuration is valuable visible from the root — humans and CI tools only look for configuration at the root, and worktrees, teammates and servers all need to see it. It must be called `.loom.ci.toml`, live at the target project root and be tracked by git; `.loom/ci/` holds only run logs and reports.

The most important conclusion from the external solutions is not "which framework to copy" but three boundaries:

- **A question is a return value, not a pause.** LangGraph, the OpenAI Agents SDK and Temporal all persist state first and resume from external events; none of them keeps a process/chat window alive to wait for a human. Loom's existing "process + files + JSON + exit code" is the right direction; what is missing is states, events and views.
- **Context optimization is not "delete as much as possible".** Chroma's context rot shows that long inputs degrade consistently; Anthropic's context editing shows that clearing tool results invalidates the prompt cache. Projection needs a threshold and must not clear at every step.
- **A skill is an intervention, not a configuration.** SkillsBench's average gain comes with large variance; in SWE-Skills-Bench, 92 of 98 skills had no statistically significant effect and a few were harmful. v1 should default to advisory, enable per project, and evaluate with paired designs rather than "more installed is better".

Recommended order:

| Order | Work | Depends on | Rough person-days | Exit criteria |
|---|---|---|---:|---|
| 0 | fix `verify` states and the claim snapshot | none | 2–3 | `needs_input`/`blocked`/`budget_exceeded` no longer written as `task.failed`; repeated verify does not lose the claim |
| 1 | minimal attention inbox + footer | 0 | 3–4 | every human command shows todos on a TTY; `--json` is not polluted; `--check` is scriptable |
| 2 | local CI core | none (can run parallel to 1) | 5–7 | `.loom.ci.toml` parses, executes and diagnoses; failure summaries are bounded |
| 3 | question protocol | 0, 1 | 3–4 | a scripted provider can complete ask → exit 2 → inbox → answer → new run → verify |
| 4 | CI hooks / safe fix / report | 2 | 3–4 | hooks are idempotent, fail-open, never stage automatically; same-tree evidence is verifiable |
| 5 | churn step-level projection + write guard | 0, measurements | 5–7 | input is bounded in rewrite scenarios; tool_call pairing is correct; A/B does not regress |
| 6 | skills lock + static checks | none | 1–2 | the lock is reproducible; description budgets and trigger boundaries are checkable |
| later | L2 evaluation, server skipping, act/workflow simulation, review/skill jobs | real model/signing/server data | deferred | see §11 |

If only one thing can be done: fix `verify`. If two: add attention. If three: add the local CI core. The question protocol, churn and skills all build on the state, visibility and measurement these three provide.

---

## 1. Assessment baseline: current code facts

The following facts come from the current worktree, not from design assumptions. Line numbers move with the code; cite by symbol name.

| Fact | Location | Direct impact |
|---|---|---|
| `EventType` has 19 event kinds, `schemas/event.v1.json` uses an `enum` allowlist | `src/crates/loom-core/src/event.rs:10`, `schemas/event.v1.json:14` | adding `task.needs_input`, `task.answered`, `attention.*` requires syncing the types, `as_str`, `FromStr`, `validate`, schema and tests |
| `Event::validate`'s `needs_task` is an explicit list | `event.rs:199` | `attention.*` are project-level items and must not be forced to carry a `task_id`; do not add them to this list |
| `TaskStatus` has 11 variants and no `needs_input` | `src/crates/loom-core/src/task.rs:54` | `status_after_event`, `FromStr`, `is_open`/`is_terminal` all need extending |
| `status_after_event(current, event)` does not receive event data | `task.rs:128` | `task.receipt` cannot be changed to "decide state from data"; explicit events must be appended in dispatch/verify |
| `ReceiptStatus` already has `done`/`blocked`/`needs_input`/`failed`/`budget_exceeded` | `src/crates/loom-core/src/receipt.rs:10` | the receipt layer can already express asking and blocking; what is missing is control-plane states and flow |
| `verify` only treats `ReceiptStatus::Done` as `claim_done` | `verify_cmd.rs:108` | `needs_input`/`blocked`/`budget_exceeded` get misjudged |
| `verify` overwrites `outbox/receipt.json` with the acceptance receipt | `verify_cmd.rs:181`, `src/crates/loom-store/src/run.rs:114` | the adapter's original claim is untraceable after verify |
| `verify` writes `task.failed` on failure, with codes only `verify_fail` / `claim_not_done` | `verify_cmd.rs:216` | state semantics are flattened, routing and statistics cannot tell failure types apart |
| `dispatch` only accepts `pending/ready/failed/escalated` | `src/crates/loom-cli/src/dispatch_cmd.rs:55` | there is no recovery entry for `needs_input`; after an answer it must return to `ready` |
| `dispatch` writes `task.receipt` unconditionally after the adapter returns | `dispatch_cmd.rs:331` | even when the receipt is `needs_input`, the task state first becomes `receipt` |
| `route`'s failure history only counts `task.failed` | `src/crates/loom-cli/src/route_cmd.rs:236` | as long as verify keeps mis-writing failed, questions trigger escalations |
| `Config` top level only has `#[serde(default)]` and no `deny_unknown_fields` (**fixed 2026-11-11**: `Config`/`ToolsConfig`/`Limits` all deny now, `loom config set` validates wholesale before writing and rolls back on failure) | `src/crates/loom-core/src/config.rs:12` | unknown sections in old configs go from "silently ignored" to "explicitly reported" (intentional behavior change, see Conclusion 3) |
| `read_events` reads and parses the whole JSONL every time, and a corrupt line errors immediately | `src/crates/loom-store/src/events.rs:36` | a footer calling it per command would amplify the cost; a view cache is needed |
| `Project::init` builds the `.loom/` skeleton, `config.toml`, `README.md`, and migrates the root `.gitignore` to a whitelist (no more goal/constraints placeholder templates) | `src/crates/loom-store/src/project.rs:67` | CI bootstrap has no landing spot; the state layer is ignored by default |
| no `ci` / `inbox` / `skill` module or command exists in the repo | `grep src/crates schemas` | all three directions start from zero, not a few-line edit |
| native calls `provider.complete(messages.clone())` every step and `messages` only grows | `src/crates/loom-native/src/run.rs:217` | churn's input compounding is a certainty |
| native `write` can overwrite an existing file directly, with no `mode` or `expected_sha256` | `src/crates/loom-native/src/tools.rs:230` | whole-file rewrites have no tool-layer constraint (a `mode` gate was added 2026-11-09; `expected_sha256` is still missing) |
| native `edit` is `old`/`new` search-and-replace with `replace_all` support | `tools.rs:197` | patch-first has half a foundation; the precise re-read guidance after a failure was missing (added 2026-11-09) |
| native `parse_finish` accepts `needs_input` with exit code 2 | `run.rs:414` | but it does not generate `questions.json`, so a question has only a state and no answerable content |
| `UsageRecord` has no step dimension | `src/crates/loom-core/src/usage.rs:92` | churn attribution needs optional fields or a separate step log (optional `step` and attribution fields added 2026-11-09) |
| event/receipt/usage schemas are all `additionalProperties: false` | `schemas/event.v1.json:6`, `schemas/receipt.v1.json:6`, `schemas/usage-record.v1.json:8` | any new field must be synced into the schema or the validator rejects it |
| five project-level skills already exist, but Loom has no skill runtime | `.opencode/skills/*/SKILL.md` | v1 should not build a second loader; do the lock and static checks first |

From these facts follows a set of hard constraints:

1. **Fix states before building views.** Otherwise the inbox visualizes the wrong state and only makes the error more visible.
2. **Build the claim snapshot before the question.** Otherwise after answering there is no proof of "what was asked at the time".
3. **The footer must not replay all events per command.** It needs a `.loom/inbox.json` view and `size/mtime/sha256` invalidation checks.
4. **CI bootstrap must not go into `loom-store`.** `loom-store` must not depend on `loom-ci`; the CLI calls `loom_ci::bootstrap`, and `Project::init` keeps only building the skeleton.
5. **The write guard is a behavior change.** The tool schema, tests and error messages must all change together; old calls must not be rejected silently.
6. **New fields are optional by default.** Breaking changes bump to v2; consumers ignore unknown fields.

---

## 2. External solutions compared

### 2.1 What can and cannot be borrowed

| Source | Key facts | Adopted | Rejected / boundary |
|---|---|---|---|
| [LangGraph interrupts](https://docs.langchain.com/oss/javascript/langgraph/interrupts) | interrupts need a checkpointer; resume uses the same `thread_id` + `Command(resume=...)`; nodes re-run from the start, so side effects before the interrupt must be idempotent | "a question is a return value"; stable question IDs; idempotent answers; resume with a new run, no kept process | do not introduce checkpointer/thread as primary state. Loom's state is `events.jsonl` + files + exit code |
| [OpenAI Agents SDK HITL](https://openai.github.io/openai-agents-python/human_in_the_loop/) | approvals surface as `interruptions`; `RunState` serializes and resumes; deserialization does not authenticate a snapshot; approval is not exactly-once | explicit pending items; answer by ID; answers stored separately from the snapshot; an untrusted local file cannot authorize | do not treat `RunState` as the source of truth; no in-process pause/resume |
| [Temporal HITL](https://docs.temporal.io/ai-cookbook/human-in-the-loop-python) | Signal + `wait_condition`; durable timers; waiting hours or days costs no compute | long waits are a normal state; TTL/expiry expressed through events and views; no resident daemon | do not introduce a workflow engine; no long-polling process |
| [Anthropic Agent Skills](https://platform.claude.com/docs/en/agents-and-tools/agent-skills/overview) | L1 metadata is resident; L2 `SKILL.md` loads on trigger, recommended <5k tokens; L3 resources read on demand | advertise only the index; read the body on demand; hard budget per skill / total skills | no global install; do not treat a large bundle as a default capability |
| [Aider edit formats](https://aider.chat/docs/more/edit-formats.html) | whole is simple but emits the whole file; diff/udiff emit only changes; different models suit different formats; the leaderboard counts "format correctness" separately | `edit` primary, `write` as fallback; measure format failure rate separately; weak models may use whole | do not assume every model can stably emit diffs; do not patch for patching's sake |
| [Chroma context rot](https://research.trychroma.com/context-rot) | 18 models degrade consistently as input grows, possibly before the nominal window; distractors amplify the problem | bounded history; project old payloads; treat "the window is big enough" as an anti-pattern | do not substitute "1M context" for context governance |
| [Anthropic context editing](https://platform.claude.com/docs/en/build-with-claude/context-editing) | clearing tool results invalidates the cached prefix; `clear_at_least` ensures clearing enough to be worth breaking the cache | set a projection threshold; keep the stable prefix untouched; estimate cache invalidation cost | do not clear at every step; do not count only half of the "input saved" |
| [pre-commit](https://pre-commit.com/) / [lefthook](https://lefthook.dev/) | hooks are a local convenience layer; pre-commit stashes unstaged changes, and recovery in partial-stage/conflict scenarios is complex; lefthook also has to deal with existing hooks and `core.hooksPath` | write thin hooks ourselves; explicit install; fail-open; no hook as a security boundary | do not introduce a hook-framework dependency; never modify global git config |
| [nektos/act](https://nektosact.com/usage/runners.html) / [Dagger](https://dagger.io/learn/run-github-actions-locally) | act runs workflow YAML, but the default runner image and GitHub semantics are incomplete; Dagger argues for extracting steps into portable commands that the workflow only calls | `.loom.ci.toml` as the portable command source; server workflows call the same commands | v1 does not simulate GitHub Actions; no parsing of arbitrary workflow YAML |
| [SLSA verifying artifacts](https://slsa.dev/spec/v1.2/verifying-artifacts) | unsigned provenance cannot be a root of trust; L2 needs signatures, L3 needs a trusted build platform | v1 only produces evidence; `verify-attest` only checks consistency | no authorizing server CI skipping from a local attestation |
| [OpenAI apply_patch](https://developers.openai.com/api/docs/guides/tools-apply-patch) | V4A structured patch, suited to multi-file/mechanical changes; applied by the application side | a candidate after `edit`; measure format failure rate first | not introduced by default in v1; never let the model run arbitrary patch shell |
| [SkillsBench](https://arxiv.org/abs/2602.12670) | 87 tasks, 18 configurations; curated skills average 33.9%→50.5% (+16.6pp), but gains across configurations range +4.1 to +25.7pp; focused ≤3 modules beat a big bundle | small, focused, switchable-off skills; paired evaluation | no preloaded all-in-one skill pack |
| [SWE-Skills-Bench](https://arxiv.org/abs/2603.15401) | 98 skills, 3920 paired evaluations; 92/98 with no statistically significant effect; average 76.6%→76.9%; a few harmful skills override explicit task instructions (down to -17.0pp) | advisory/opt-in by default; paired evaluation; harmful skills can be disabled immediately | do not treat "a skill is installed" as a capability gain; do not let a skill override the TaskSpec |

### 2.2 Three external lessons

**First, a long wait is a state problem, not a process problem.** LangGraph uses a checkpointer, OpenAI uses `RunState`, Temporal uses event history and durable timers, and none of them keeps an agent process interactive. Loom's `exit 2 + questions.json` direction is right; what is missing is writing "who is waiting, on what, who answered, and what is next" into events and views. Answers must be idempotent: resubmitting the same answer for the same `question_id` must not produce two state transitions.

**Second, context governance must account for the cache.** Chroma shows that long contexts themselves degrade the model; Anthropic shows that clearing tool results breaks the prefix cache. Together, the right strategy is not "recompile every step" but "project only past the threshold, and clear at least enough in one go to be worth breaking the cache". That directly determines §8's `min_clear_tokens` and "keep the stable prefix untouched".

**Third, skills need paired evaluation, not static review.** SkillsBench's average gain hides large variance; SWE-Skills-Bench shows most skills have no significant effect and a few are harmful. v1 can do locks, static checks and trigger boundaries, but cannot claim "skills raise capability"; the L2 evaluation must wait for a real model, a fixed task set and billing.

---

## 3. Overall trade-offs and measurement

### 3.1 What to do, what not to do

| Direction | v1 does | v1 does not do | Stop condition |
|---|---|---|---|
| state/receipt | claim snapshot; `needs_input` state; state mapping; schema drift tests | do not change the adapter output path; do not rename `receipt.json` | if state mapping still produces `task.failed`, do not move on to attention |
| attention | events + `.loom/inbox.json` view + TTY footer + `--line`/`--check` | no TUI, no desktop, no resident daemon, no chat/ticket threads | if there are no blocking items within a week, invest no more UI |
| question | `ask` tool, `questions.json`, `.loom/answers/`, `loom answer`, packet injection | no interactive waiting; no automatic LLM answers in v1 | if first-pass rate does not improve after answers, do not widen automatic answering |
| local CI | `loom-ci` crate, `.loom.ci.toml`, plan/run/doctor, log budget, report | no server skipping, no act simulation, no nested containers, no review/skill jobs | if local CI cannot reduce server re-runs or shorten feedback, do not build hooks/review |
| churn | step dimension, threshold projection, write guard, churn statistics | no per-step cache clearing, no hard blocking, no soft restart, no default `apply_patch` | if A/B first-pass rate drops more than 2pp, turn projection off |
| skills | the existing project-level pilot, `skills.lock`, static checks, description budgets | no second runtime, no global install, no L2 promotion | if paired evaluation shows no significant gain, downgrade to explicit invocation |

### 3.2 Measurement definitions

The optimization target is **total cost per successful task**, not "how many checks ran locally" or "how many rewrites".

| Metric | Definition | Use |
|---|---|---|
| `CPVT` | total cost / tasks passed | primary cost metric; must be read together with first-pass rate |
| `TPVT` | total tokens / tasks passed | approximation when there is no billing; mark `estimated` |
| `WPT` | wall clock per task, at least P50/P95 | feedback speed; the main benefit of local CI |
| `first_pass_rate` | share passing verify on the first try | quality guardrail |
| `blocked_age` | time from open to resolved for an attention item | whether visibility really reduces waiting |
| `time_to_first_ack` | when a human first sees/acknowledges | whether the footer is useful |
| `server_reruns_avoided` | jobs/minutes not re-run on the server after a local catch | benefit of local CI; v1 can only estimate |
| `wasted_tokens_est` | rewrite output + history carry + retries, estimated | churn's primary metric; mark `estimated` |
| `cache_hit_ratio` | `cached_input_tokens / input_tokens` | whether projection breaks the cache |
| `skill_delta` | paired pass rate / CPVT delta on the same task set | decides whether a skill is kept |

Anti-patterns: using `write_passes`, `churn_ratio`, job count or skill count as the primary KPI. They can only explain causes, never prove benefit.

---

## 4. A. State and receipt correctness (fix first)

### 4.1 Current flow

```text
adapter exits 2 / final.md declares needs_input
  -> normalize produces ReceiptStatus::NeedsInput
  -> dispatch writes task.receipt (state receipt)
  -> verify reads the claim, claim_done=false
  -> acceptance still runs; whatever the result, writes task.failed(code=claim_not_done)
  -> route's history counts this task.failed toward the escalation threshold
  -> the user can only see this question in conversation history / logs
```

Three independent defects: wrong state, lost evidence, missing visibility. The first and second must be fixed before attention.

### 4.2 Claim snapshot

The adapter contract stays unchanged: it still writes `outbox/receipt.json`. `verify` snapshots before overwriting:

1. Read `outbox/receipt.json`.
2. If `.loom/runs/<id>/logs/claim.json` does not exist, copy the original receipt bytes there; if it exists, read the claim from `logs/claim.json`.
3. Validate the claim; a parse failure is treated as `claim_invalid`, never guessed.
4. By default run acceptance only for `Done`; skip `NeedsInput`/`Blocked`/`BudgetExceeded`. `--run-verify` can force the run, but it still may not be marked `passed`.
5. Write the final `outbox/receipt.json` with an added optional field `claim_status`, and record the same value in the `task.verify` event.
6. A repeated verify takes the original claim from `logs/claim.json` and does not treat the previous verify receipt as the claim.

Moving the verify receipt to `verify.json` while keeping the original `receipt.json` is not recommended: it changes the assumptions dispatch/status/canary make about `receipt.json`, and the migration cost exceeds the benefit.

### 4.3 State mapping

| Claim status | Acceptance | Events written | Task state | Exit code |
|---|---|---|---|---|
| `Done` | all pass | `task.verify` + `task.passed` | `passed` | 0 |
| `Done` | some fail | `task.verify` + `task.failed(code=verify_fail)` | `failed` | 1 |
| `NeedsInput` | not run by default | `task.needs_input` + `attention.opened` | `needs_input` | 2 |
| `Blocked` | not run by default | `task.blocked(code=blocked)` + `attention.opened` | `blocked` | 2 |
| `BudgetExceeded` | not run | `budget.exceeded` + `task.blocked(code=budget_exceeded)` + `attention.opened` | `blocked` | 3 |
| `Failed` | not run | `task.failed(code=claim_failed)` | `failed` | 1 |
| no claim / parse failure | not run | `task.failed(code=claim_missing/claim_invalid)` | `failed` | 1 |

Notes:

- `needs_input` is not a failure and is not counted in escalation history; `route` only looks at `task.failed`, so there is no false escalation.
- `blocked` is not a failure either and is not counted in escalation; it means dependency, environment or budget blocking. Distinguish the cause with `data.code`.
- `budget_exceeded` writes a `budget.exceeded` event and uses `blocked` for the task state, not `task.failed`. If later statistics need a separate state, bump `TaskStatus` then; do not mix them in v1.
- The `status` of the `verify` receipt reflects the final state and no longer always writes `Done`/`Failed`.
- Exit codes `0/1/2/3` are new semantics for `loom verify`; `main` still maps internal errors to 4. It is not a global CLI convention and must not be mixed with the other two:

| Command family | 0 | 1 | 2 | 3 | 4 |
|---|---|---|---|---|---|
| `loom verify` (this design) | passed | acceptance or claim failure | `needs_input` / `blocked` | budget/timeout | internal error |
| `loom tool` ([10](10-tool-output-protocol-v0.1.md) §5) | success | — | bad arguments | timeout/budget | internal error |
| adapter ([08](08-design-roadmap-v0.2.md) §7.1) | success | — | needs human | budget/timeout | adapter fault |

This must be stated in `loom verify --help`, the CLI docs and the tests at the same time, so callers do not mistake `needs_input` for an argument error.

### 4.4 `TaskStatus::NeedsInput`

Suggested semantics:

- `satisfies_dependency() == false`;
- `is_terminal() == false`;
- `is_open() == true`, so `loom status`/`loom resume` can see tasks that are "still waiting for an answer";
- the `dispatch` gate does not pass `needs_input`; only when `loom answer` has answered all blocking questions and writes `task.ready` does it become dispatchable again;
- `resume --dispatch` dispatches only `ready` and never runs off a `needs_input`; the pretty output hints "questions awaiting answer".

Also add `Blocked` to `is_open()`. Today `Blocked` is neither open nor terminal, so `resume` cannot see it; this is an existing display gap that should be fixed together with `needs_input`.

### 4.5 Events and schema

New:

- `task.needs_input`: `data.question_ids`, `data.blocking`, `data.scopes`;
- `task.answered`: see §6;
- `attention.opened` / `attention.updated` / `attention.resolved` / `attention.expired`: see §5.

Places to sync:

1. `EventType`'s enum, `as_str`, `FromStr`;
2. `Event::validate`'s `needs_task`;
3. `schemas/event.v1.json`'s `enum`;
4. the event table in `doc/03` §11.2;
5. at least one event-replay test.

Add a schema drift test: read `schemas/event.v1.json`'s `enum` and compare it with the string set of `EventType::all()`. There is no such protection today, so a new event easily only gets the Rust type updated.

### 4.6 Offline tests

No real model needed:

- a `NeedsInput` claim after verify has state `needs_input` and produces no `task.failed`;
- `Blocked`/`BudgetExceeded` no longer write `claim_not_done`;
- a repeated verify takes the claim from `logs/claim.json` and the original bytes are unchanged;
- `route history` does not raise the failure count for `needs_input`;
- the event schema drift test points at the missing event when it fails;
- `--run-verify` runs acceptance but does not mark `needs_input` as `passed`.

### 4.7 Risks

| Risk | Mitigation |
|---|---|
| an old adapter does not write `questions.json` | still enter `needs_input`, and open an `operation` item at the same time; never guess the question |
| exit code changes affect scripts | add 2/3 only in `verify` first, with docs and tests in sync; other commands unchanged |
| the claim snapshot is hand-edited | `logs/claim.json` is only evidence; the source of truth is still the event log; `verify-attest` does not treat it as a root of trust |
| adding `Blocked` to `is_open` changes resume display | display only, no change to dispatch; pin the behavior with a test |

---

## 5. B. Attention inbox

### 5.1 Events are the source of truth, the view is a cache

`attention.*` events are written into `events.jsonl`; `.loom/inbox.json` is read-only and rebuildable, never hand-edited.

| Event | Required data | Effect |
|---|---|---|
| `attention.opened` | `item_id`, `kind`, `source`, `severity`, `dedupe_key`, `title` | creates an item; if the same `dedupe_key` is still open, only `occurrences` accumulates |
| `attention.updated` | `item_id`, `changes` | updates `severity`/`detail`/`defer_until`/`occurrences` without changing identity |
| `attention.resolved` | `item_id`, `by`, `reason` | closes the item |
| `attention.expired` | `item_id`, `policy`, `reason` | only reversible/reminder kinds may expire; blocking may not silently expire |

`attention.*` does not require `task_id`: a missing tool, a missing key, or the first CI onboarding are all project-level items. Do not add them to `Event::validate`'s `needs_task`.

### 5.2 Item model

| Field | Constraint |
|---|---|
| `id` | `A-` + the first 8 characters of `sha256(kind + "\0" + dedupe_key)`; deterministic, stable under replay |
| `kind` | `question` / `approval` / `operation` / `review` / `reminder` |
| `severity` | `blocking` / `soon` / `info` |
| `status` | `open` / `acked` / `deferred` / `resolved` / `expired` |
| `title` | ≤120 characters, control characters and ANSI forbidden |
| `summary` | ≤2KB; long text spills to `.loom/attention/<id>.md`, the event stores only path/hash |
| `source` | `task` / `ci` / `env` / `git` / `human` / `agent` |
| `task_id` / `run_id` | nullable |
| `dedupe_key` | unique among open items; e.g. `task:T-001:question:Q1`, `env:missing-tool:goose`, `ci:clippy:<tree>` |
| `actions` | at most 8, text suggestions only, never auto-executed, never shell-evaluated |
| `created_at_ms` / `updated_at_ms` / `expires_at_ms` | lifecycle |
| `resolved_by` / `resolved_reason` | audit |

Principles:

- `acked` only means "seen"; it does not close the item and does not lower the `blocking` count;
- `defer` is allowed only for `soon`/`info`; `blocking` must not be hidden away;
- automatic resolve is only for deterministic conditions: tool installed, same-tree CI passed, question answered;
- item text comes from agents/the repo and is treated as untrusted input: truncate, escape, and never follow `refs` symlinks outside the repo.

### 5.3 View and cache

`read_events` reads everything, so running it per command for the footer amplifies the cost. View cache rules:

1. `.loom/inbox.json` stores `events_size`, `events_mtime_ms`, `events_sha256`, `generated_at_ms`, `items`, `counts`.
2. On read, `stat` first; if size/mtime/sha256 are unchanged, use the view directly.
3. If changed, do a full replay and write back atomically (temp + rename).
4. When events are truncated or hand-edited the size may shrink; a full replay is mandatory, never an incremental append.
5. A failed view write does not affect the command; the footer skips silently.
6. `loom inbox --json` / `--check` may force a replay so a dirty cache cannot affect a decision.

### 5.4 The fixed corner

v1 ships only two outlets: the TTY footer and `loom inbox --line`. It is not bound to any UI; later TUI/desktop reuse the same view.

| Outlet | Content | Constraints |
|---|---|---|
| footer of every human command | `attention: 2 blocking, 1 soon | loom inbox --line | A-7f3: ...` | stderr only; TTY only; not under `--json`; not for `loom inbox` itself; failures swallowed |
| `loom inbox --line` | one line, for statusline/tmux/starship polling | read-only, no side effects, fixed overhead |
| `loom inbox --check` | exit 2 when any `blocking` exists | for scripts/hooks, no long output printed |
| `loom inbox list/show/ack/defer/resolve` | full list and operations | `ack` does not close; `defer` applies only to soon/info |

Configuration:

```toml
[attention]
footer = "tty"              # tty | always | off
include_in_packet = "off"   # off | counts
dedupe_cooldown_s = 600
retain_resolved_days = 7
```

Footer rules:

- not printed when there are no todos, to avoid noise;
- single line ≤160 characters, taking the highest-priority item's title and truncating;
- ANSI injection forbidden; stderr only; exit codes never changed;
- off via `LOOM_NO_INBOX=1` or `--no-inbox`;
- the inbox is not injected into model context by default. When needed only counts are injected, under the hard budget.

### 5.5 First batch of sources

| Source | Kind | Trigger |
|---|---|---|
| `verify` | `question` | claim `NeedsInput` with a blocking question |
| `verify` | `operation` | claim `Blocked`, e.g. missing tool/missing key |
| `doctor` | `operation` | missing podman/docker/cargo/goose/opencode etc. |
| `ci` | `review` / `operation` | job failure; first onboarding needs human confirmation |
| `route` | `approval` | cost guard rejects, or an escalation needs confirmation |
| `human` | `reminder` | `loom inbox add`, deferred |

Even with no agent asking questions, the user sees operation items like "tool needs installing / CI needs confirmation" in the footer. That is the value of attention independent of questions.

### 5.6 Risks

| Risk | Mitigation |
|---|---|
| footer performance | view cache + size/mtime/sha256; failures silent |
| item flooding | `dedupe_key` + cooldown + `occurrences`, never re-opened |
| `acked` treated as done | pinned by docs and tests: ack does not lower the blocking count |
| user ignores the footer | `--check` for scripts; later TUI/desktop deferred |
| automatic resolve misjudges | only deterministic conditions close automatically; judgment requires explicit resolve |

---

## 6. C. Question protocol

### 6.1 Contract

When an adapter needs input:

- write `outbox/questions.json`;
- write `needs_input` on the `final.md` status line;
- exit code 2;
- the process starts with `stdin = /dev/null`, so an interactive prompt gets immediate EOF instead of hanging.

Native adds an `ask` tool, or allows `finish(status="needs_input", questions=[...])`. `ask` is preferred: one tool call can carry several questions and does not need to stuff an array into `finish`. `ask` writes `outbox/questions.json`, sets `Finish { status: "needs_input" }`, and exits 2. If `finish` declares `needs_input` without a `questions.json`, do not guess the questions; open an `operation` item pointing at the protocol.

### 6.2 `questions.json` v1

```json
{
  "v": 1,
  "task_id": "T-20261107-001",
  "run_id": "R-20261107-001",
  "questions": [
    {
      "id": "Q1",
      "text": "Must the migration script stay compatible with the old schema?",
      "kind": "choice",
      "options": [
        {"id": "A", "label": "Stay compatible: dual-write one version", "consequence": "one extra migration, safe rollback"},
        {"id": "B", "label": "New schema only", "consequence": "small diff, old clients break"}
      ],
      "blocking": true,
      "scope": "design",
      "recommended": "A",
      "default": "A",
      "reversible": false,
      "evidence": ["src/db/migrate.rs:40-90", "artifact:read-3"],
      "ttl_s": 86400
    }
  ]
}
```

Constraints:

- at most 5 questions, of which at most 3 blocking; beyond that, merge them or split the task;
- `text` ≤1KB; `options` ≤8; `evidence` ≤8 entries, paths/artifact references only;
- `id` unique within the task, `Q1`, `Q2` suggested;
- `kind`: `choice` / `text` / `confirm`;
- `scope`: `implementation` / `design` / `contract` / `security` / `budget` / `scope`;
- `ttl_s` up to 30 days; `blocking` may not expire automatically, and on expiry it becomes an attention `review`;
- arbitrary shell commands or URLs are not accepted as an action.

### 6.3 Answer storage

Answers are written to `.loom/answers/<task_id>.jsonl`, append-only, never into the outbox of an old run:

```json
{"v":1,"question_id":"Q1","answer":"A","source":"human","by":"local-user","rationale":"keep the rollback path","answered_at_ms":0,"answer_sha256":"..."}
```

Rules:

- `source`: `human` / `policy` / `judgment` / `spec`; v1 only opens `human` and an explicit `--assume`;
- resubmitting the same `answer_sha256` for the same `question_id` is idempotent and does not write state twice;
- a different answer is rejected by default; only `--replace` appends a record carrying `supersedes`; the event keeps the whole history;
- answer content never goes into an old run's outbox, so history cannot be rewritten after the fact.

### 6.4 State machine

```text
receipt -> needs_input
            |
            +-- every blocking question has an answer -> task.ready -> dispatch new run
            |
            +-- only some answered -> stays needs_input
```

- `task.answered` itself does not change state directly, otherwise multi-question cases pass too early;
- each answer writes `task.answered` and resolves the corresponding `question` item;
- when all blocking questions have answers, the command appends `task.ready`;
- non-blocking questions may proceed on a default, with `assumptions.jsonl` written at the same time.

### 6.5 attempt semantics

After an answer a new run opens and **`attempt` increments**, because this is a new execution; but `TaskDispatched.data` and the run meta record `question_round` and `reason=answer`. Escalation counting only looks at `task.failed`, not `attempt`, so "a question is not a failure" still holds. The "attempt unchanged" wording in 15 §2.6 must be corrected to this definition, so one attempt does not map to several runs.

### 6.6 packet injection

The next dispatch's Context Compiler reads the latest questions and answers and injects them as a separate section:

- only the Q/A summary of blocking questions and the unanswered questions;
- an answered question keeps one line `Q1=A` plus the rationale, without resending the full option list;
- total budget ≤4KB; when exceeded, trim non-blocking and older questions first, never AC/constraints;
- the attention inbox full text is not injected; only counts when needed.

### 6.7 Authorization matrix

| Scope | Who may answer in v1 | Note |
|---|---|---|
| `implementation`, reversible | human; may pass explicit `--assume` | assumption recorded, a review item opens on low confidence |
| `design`, reversible | human; judgment deferred | v1 does no automatic LLM answers |
| `contract` / AC semantics | human | changing AC/goals must be confirmed by a human |
| `security` / data deletion / irreversible migration | human | an assumption may not bypass this |
| `budget` / cross-tier / widening scope | human | goes through an approval item |

v1 explicitly does not call an LLM to answer questions. Deterministic policy can only handle `operation` items like "missing tool/missing key", never semantic questions.

### 6.8 Offline tests

- a scripted provider calls `ask` → `questions.json` → exit 2 → visible in `loom inbox`;
- after `loom answer` a new run receives the answer and verify passes;
- with multiple blocking questions, answering only 1 still leaves `needs_input`;
- resubmitting the same answer is idempotent; conflicting answers are rejected by default;
- with no `questions.json` no questions are guessed and an operation item is opened;
- answers do not enter the failure count: `attempt` increments but `task.failed` does not.

---

## 7. D. Local CI

### 7.1 What it applies to and authority

Local CI applies to the **target project that Loom manages**, not to Loom itself. Using the same mechanism in the Loom repository is dogfooding; `./scripts/dev.sh` is just one of Loom's runner examples and must not be hard-coded as a product default.

| Layer | Responsible for | Authority |
|---|---|---|
| TaskSpec acceptance | whether this task reached the agreed goal | `loom verify`, the task-level authority |
| repo local CI | whether this repo is currently healthy and can be committed/pushed | `loom ci run`, the local gate and feedback |
| server CI | cross-platform, integration, release, security and compliance | the server, the final authority |

v1 does not skip server jobs based on a local attestation. The benefit of local CI is fewer repeated failures, faster feedback and a single entry point for agents — not replacing the server.

### 7.2 Where the configuration lives

- `.loom.ci.toml`: target project root, **tracked**, shared by the team, inherited by worktrees, reviewable in PRs;
- `.loom/ci/`: runtime logs, report, attestation, **ignored**;
- `.loom/config.toml`: `[ci]` global preferences (default profile, hook policy), runtime configuration.

Why it must be tracked:

| Requirement | What happens without tracking |
|---|---|
| the team shares one set of checks | everyone has their own; a PR cannot review check changes |
| git worktree inheritance | worker worktrees have no configuration, `loom ci` behaves inconsistently |
| server/attestation references | the config hash is not reproducible, reports cannot match the same tree |
| an agent modifies checks | the diff is invisible, easy to turn into a hidden backdoor |

Personal overrides via `.loom/ci.local.toml` come later, and may only add personal profiles/timeouts, never turn off a team gate.

### 7.3 Locked schema

15 §4.5 still uses the `[jobs.<id>] + cmd` example, which differs from the `[[jobs]] + argv` locked here. The implementation form is governed by this document: `[[jobs]]` + `argv` + `[runner]`; 15's example is only a reference for field semantics. Do not support both.

```toml
version = 1

# Target projects may leave this empty; Loom dogfood uses the container script to avoid host cargo.
[runner]
argv = ["./scripts/dev.sh"]

[profiles]
pre-commit = ["fmt-fix", "skills-check"]
pre-push = ["fmt-check", "clippy", "test"]
fast = ["fmt-check", "clippy"]
full = ["fmt-check", "clippy", "test", "build"]

[[jobs]]
id = "fmt-check"
argv = ["cargo", "fmt", "--all", "--", "--check"]
timeout_s = 120
local_sufficient = true

[[jobs]]
id = "fmt-fix"
argv = ["cargo", "fmt", "--all", "--", "--check"]
fix_argv = ["cargo", "fmt", "--all"]
fix_safe = true
timeout_s = 120

[[jobs]]
id = "clippy"
argv = ["cargo", "clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]
timeout_s = 600
local_sufficient = true

[[jobs]]
id = "test"
argv = ["cargo", "test", "--workspace"]
needs = ["clippy"]
timeout_s = 900
local_sufficient = true

[[jobs]]
id = "skills-check"
argv = ["./scripts/check-skills.sh"]
timeout_s = 30
```

Fields:

| Field | Required | Meaning |
|---|---|---|
| `version` | yes | must be 1 |
| `runner.argv` | no | argv prefix; final command = `runner.argv + job.argv` |
| `profiles.<name>` | yes | array of job ids; executed in order |
| `jobs[].id` | yes | unique, non-empty |
| `jobs[].argv` | yes | non-empty array, never passed through `sh -c` |
| `jobs[].timeout_s` | yes | 1–3600; on timeout the process group is killed |
| `jobs[].cwd` | no | relative to the repo root, must not escape it |
| `jobs[].needs` | no | job dependencies; v1 uses them only for ordering, no parallelism |
| `jobs[].fix_argv` | no | run on `--fix`; `argv` is re-run afterwards to verify |
| `jobs[].fix_safe` | no | `--fix` allowed only when explicitly true; requires `fix_argv` at the same time |
| `jobs[].required_tools` | no | without a runner checks `argv[0]`; with a runner only the runner is checked |
| `jobs[].env_pass` | no | allowlist of variable names passed through from the host; stores names only, never values; `*_API_KEY`/`*_TOKEN`/`*_SECRET` denied by default |
| `jobs[].local_sufficient` | no | evidence marker only; v1 does not authorize server skipping |

v1 provides no `kind`, no free-form `env` key/value pairs, and no `network` field. Review/skill jobs are deferred; environment variables allow only the default allowlist plus the `env_pass` name list, with no values stored, so tokens are never written into the repo; network isolation is not claimed for v1 — jobs that need the network are a human decision.

Validation rules:

- `version == 1`;
- job ids unique and non-empty;
- `argv` non-empty, `argv[0]` non-empty;
- `cwd` does not escape the repo root;
- `timeout_s` within 1–3600;
- jobs referenced by a profile must exist;
- jobs referenced by `needs` must exist, and there are no cycles;
- `fix_safe` requires `fix_argv`;
- `env_pass` must match `^[A-Za-z_][A-Za-z0-9_]*$` and must not match `*_API_KEY`/`*_TOKEN`/`*_SECRET`/`*_PASSWORD`;
- `local_sufficient` is true only for deterministic commands;
- unknown fields error directly (`deny_unknown_fields`);
- file size limit 256KB.

### 7.4 Execution semantics

- final argv = `runner.argv + job.argv`; never through a shell, avoiding quoting and injection;
- `cwd` defaults to the repo root; every path must still be inside the root after resolution;
- `stdin = null`;
- stdout/stderr stream into `.loom/ci/<run>/<job>.log`, with only head/tail summaries kept in memory;
- timeouts use `process_group(0)` with `SIGTERM` then `SIGKILL`, so no `podman run` child processes are left behind;
- by default the environment passes only an allowlist such as `PATH`, `HOME`, `LANG`, `TERM`, `CARGO_HOME`, `RUSTUP_HOME`, with `*_API_KEY`, `*_TOKEN`, `*_SECRET` stripped; `env_pass` lists names only, never values;
- set `NO_COLOR=1`, `TERM=dumb`, `LOOM_CI=1`; strip ANSI before writing to disk;
- record argv, cwd, exit code, signal, duration, log hash; `--version` is optional and a failure does not block;
- stop at the first failure by default; `--keep-going` runs everything;
- v1 executes sequentially, with no parallelism and no affected-only.

### 7.5 Commands and exit codes

```text
loom ci plan [--profile <name>] [--json]
loom ci run  [--profile <name>] [--keep-going] [--fix] [--json]
loom ci doctor [--json]
loom ci report [--run <id>] [--json]
loom ci verify-attest <path> [--json]
loom ci install-hooks --profiles pre-commit,pre-push
loom ci remove-hooks
```

Exit codes:

- `0`: all jobs passed;
- `1`: some job failed;
- `2`: needs human handling (missing tool, missing runner, needs confirmation) — not a code failure;
- `3`: timeout/budget;
- `4`: configuration/execution error.

With `--json`, stdout is only JSON; progress and summaries go through stderr or fields.

`--fix` runs only jobs with `fix_safe = true`: run `fix_argv` first, then re-run `argv` to verify; show `git diff --stat` and the patch path; **never stage, never commit**; if the tree changed, ask for a fresh `git add`.

### 7.6 `loom init` detection

v1 does single-stack, root-level detection only, to avoid tackling monorepos too early:

| Evidence | Generated |
|---|---|
| root `Cargo.toml` or `src/Cargo.toml` (workspace) | `cargo fmt/clippy/test` |
| `scripts/dev.sh` | `runner.argv = ["./scripts/dev.sh"]` |
| `Makefile` with `check`/`test`/`fmt` | adopt existing targets, do not reinvent |
| `package.json` / `pyproject.toml` / `go.mod` | deferred, no guessing |

Generation policy:

- `--dry-run` prints what would be written and writes nothing;
- an existing `.loom.ci.toml` is kept by default with a hint;
- `--force` backs up `.loom.ci.toml.bak.<ts>` before overwriting;
- with insufficient evidence, only print suggestions and write no configuration;
- never generate a workflow automatically, never touch `.github/`;
- `Project::init` keeps building only the `.loom` skeleton; the CLI calls `loom_ci::bootstrap`, avoiding a `loom-store -> loom-ci` reverse dependency.

### 7.7 hooks

- installed only explicitly by `loom ci install-hooks`; `.git/hooks` and the global `core.hooksPath` are never changed silently;
- writes `pre-commit` / `pre-push` carrying a `# loom-managed` marker;
- an existing non-Loom hook: refused by default; `--force` first backs it up as `<hook>.bak.<timestamp>`;
- a hook only calls `loom ci run --profile ... --from-hook` and never inlines business commands;
- pre-commit target ≤30 seconds, pre-push target ≤5 minutes; long jobs go to the server;
- when `loom` is missing, the config is invalid, or the runner is absent: fail-open + warning, so a fresh clone can still commit;
- `LOOM_CI_SKIP=1` skips explicitly, writes only local diagnostics and is not treated as evidence;
- `hooks_fix_safe` defaults to **false**; when enabled, run the safe fix first and re-run the checks, exit 1 if the tree changed and ask for a fresh `git add`, never staging/committing;
- **`--no-verify` cannot be observed by hooks**, so no bypass recording is promised; only what actually ran / the skip reason is recorded;
- never modify global git config.

### 7.8 report and attestation

`report.json`:

```json
{
  "v": 1,
  "run_id": "CI-...",
  "profile": "pre-push",
  "ci_toml_sha256": "...",
  "tree": {
    "head": "abc123",
    "dirty_sha256": "...",
    "untracked_sha256": "...",
    "dirty": true
  },
  "started_at_ms": 0,
  "duration_ms": 0,
  "jobs": [
    {
      "id": "fmt",
      "argv": [],
      "cwd": ".",
      "exit": 0,
      "signal": null,
      "timed_out": false,
      "duration_ms": 0,
      "log_path": "...",
      "log_sha256": "..."
    }
  ],
  "ok": true,
  "local_sufficient": true,
  "advisory": false
}
```

Tree computation: hash of `HEAD` + `git diff --binary` + the path/size/content hashes of untracked files sorted by path. Do not run `git write-tree` for an attestation; it touches the index. Verification for the same tree must include untracked files, otherwise "it ran locally" can miss new files.

`attestation.json` is the report's digest + verifiable fields:

- v1 does not sign, `signature = "none"`;
- `loom ci verify-attest` can only prove "this file matches this tree";
- server skipping needs HMAC/trusted runners/protected branches, deferred;
- until that is done, server CI keeps running security/release/matrix.

### 7.9 Connection to attention

- CI failures open a `review` item with `dedupe_key = ci:<job>:<tree>`;
- a missing tool/runner opens an `operation`;
- after a success on the same tree, the corresponding `review` resolves automatically;
- `loom inbox --check` can warn about unhandled failures before a commit;
- job results in a report are not TaskSpec acceptance; `verify` remains the task authority.

### 7.10 Offline tests

A temp directory plus fake commands covers it:

- parsing/validation: good config, duplicate id, missing job, escaping cwd, invalid timeout, `fix_safe` without `fix_argv`, unknown fields;
- execution: a temporary `sh -c 'exit 1'`, checking exit code, log, summary;
- timeout: a `sleep` child killed, no process-group residue;
- `--fix`: a fake fix command changes a file, verify nothing is staged and the diff is visible;
- hooks: temporary git repo, idempotency, refusal of non-Loom hooks, backup, fail-open;
- report/attestation: same tree passes, different tree fails, dirty/untracked files change the hash;
- `--json` stdout is pure JSON;
- init detection: temporary Rust workspace + `scripts/dev.sh` produces the expected configuration; an existing configuration is not overwritten.

### 7.11 Risks

| Risk | Mitigation |
|---|---|
| configuration does not enter the repo | `.loom.ci.toml` at the root, tracked |
| false positive "passed locally" | tree includes untracked files; doctor checks the environment; the server still runs the final gate |
| forged attestation | evidence only; never authorizes skipping |
| hooks block a fresh clone | fail-open + warning |
| timeout residue | process-group kill; logs streamed |
| command drift | adopt existing Makefile targets; doctor hints |
| overconfident generated configuration | v1 does single-stack root detection only; no evidence, no write |
| `--no-verify` | cannot be observed; no promise of recording, server backstop |

---

## 8. E. Churn

> **Implementation status (2026-11-08)**: the §8.2 step-level projection is implemented (knobs match this
> document: `max_history_bytes` / `keep_recent_turns` / `min_clear_tokens`, env names
> `LOOM_NATIVE_HISTORY_BYTES` / `LOOM_NATIVE_KEEP_TURNS` / `LOOM_NATIVE_MIN_CLEAR`, 0 disables), see
> `doc/15` §1 implementation status and `scripts/smoke-churn.sh`. The §8.3 write guard and §8.4 token
> attribution (`churn.json` / `loom churn stats`) were **implemented on 2026-11-09**: the `write` `mode`
> gate with its 64KB cap, `edit`'s re-read guidance on failure, per-step usage attribution fields, per-run
> `logs/churn.json`, and `loom churn stats [--by model]` are all wired up; real A/B numbers are at the end
> of this section.

### 8.1 The token bill

Churn wastes not disk but request history:

| Charge | Source | Governance |
|---|---|---|
| output tokens | the model regenerates the whole file into `tool_calls.arguments` | patch-first, write guard |
| input compounding | old full text stays in `messages` and is resent at every later step | threshold projection |
| cache invalidation | projecting/clearing tool results breaks the prefix cache | stable prefix, `clear_at_least` |
| retries | acceptance feedback, whole-run reopen | failure packet, stop-loss |

An illustration: an 8KB file is about 2k tokens; the same file rewritten 5 times with 20, 16, 12, 8, 4 steps remaining afterwards means the old versions are resent for roughly `2k × (20+16+12+8+4) ≈ 120k` input tokens. This number is not measured; it only illustrates why "blocking the Nth write" saves a single output while "keeping old versions out of later requests" is the main lever.

### 8.2 Threshold projection

A deterministic projection runs before every `provider.complete`, but only when needed:

1. **The stable prefix stays untouched**: system + packet remain byte-identical to protect the prefix cache.
2. **Trigger condition**: the dynamic tail exceeds `max_history_bytes` (default 24KB) or the estimated input exceeds the cap, and at least `min_clear_tokens` (default 2000) is expected to be cleared. If it would not clear enough, do not clear — do not break the cache to save a little input.
3. **Keep the most recent K complete turns** (default 4–6), including assistant tool_calls and their tool results.
4. **Project old assistant tool_calls**:
   - `write(content=...)` → `{path, bytes, sha256, mode}`;
   - `edit(old,new)` → `{path, old_bytes, new_bytes, old_sha, new_sha}`;
   - `bash(cmd)` → a command summary + artifact path.
5. **Project old tool results**: keep one line of result + artifact path, never the full log.
6. **Never orphan a tool result**: trim by complete turn, or keep the assistant message matching the `tool_call_id` at the same time.
7. **Current working set**: append deterministic worktree state after the projected history (changed files, diffstat, most recent verify failure), never whole files.
8. **Hard budget**: when over, drop from the oldest turn until it fits.
9. **Failure packets keep only the newest**: old verify feedback is replaced, never accumulated.
10. **No model summarization**: projection is a deterministic rule; do not call an LLM again to save tokens.

Configuration:

```toml
[churn]
project = "auto"             # auto | off
max_history_bytes = 24576
keep_recent_turns = 4
min_clear_tokens = 2000
write_guard = "create"       # create | off
```

`auto` means "project only past the threshold", not "project every step".

### 8.3 patch-first and the write guard

- `edit` stays the primary path: `old` must be unique or `replace_all`; on failure it says "read the exact range first", never "rewrite the whole file";
- `write` gains `mode`:
  - default `create`: only allows creating a file that does not exist;
  - overwriting an existing file requires `mode="replace"`;
  - an overwrite without `mode` is rejected with a short error pointing at `edit`;
  - `expected_sha256` is optional and deferred for concurrency protection;
- a per-call content cap on `write` (e.g. 16KB), requiring patch or chunking beyond it;
- v1 does not introduce `apply_patch` by default. Measure the `edit` failure rate first; if it stays high, then introduce the V4A structured patch. External adapters can only be handled by prompt and selection — their history cannot be projected from Loom's side;
- the system prompt and failure packets state clearly "prefer edit, no whole-file rewrite".

**Implementation (2026-11-09)**: `write` in `tools.rs` defaults to `mode=create`; overwriting an
existing file requires `mode=replace`, otherwise it returns a short error pointing at `edit`; the
per-call content cap is 64KB; the Nth whole-file overwrite of the same file is marked `full rewrite
#N` in the observation. When `edit` misses, the error points to "read the file's 1-based range and
copy the current text, do not rewrite the whole file". `tool_schema()` and `default_system()` both
require "prefer edit". Tests cover: the mode gate, the file unchanged after a refusal, repeated
rewrite counting, over-cap refusal, and the `edit` failure hint.

### 8.4 Token attribution

- `UsageRecord` gains an optional `step`, synced into `schemas/usage-record.v1.json`; this is an incremental field and does not break old data;
- every call records `step`, input/cached/output, tool_names, history_bytes, projected_bytes, write_paths, rewrite_count;
- `logs/churn.json` aggregates:
  - `rewrite_output_tokens`: output of repeatedly writing the same file;
  - `history_carry_tokens`: estimate of the old versions being resent in later requests;
  - `avoided_input_tokens`: estimate of what projection stopped being sent;
  - `cache_invalidation_cost_est`: estimate of breaking the prefix cache (in money, not folded into tokens);
  - `wasted_tokens_est` = `max(0, rewrite_output_tokens + history_carry_tokens − avoided_input_tokens)`;
  - `wasted_cost_usd_est`: computed from the model price table, unknown marked `unknown`;
  - `write_passes` / `churn_ratio`: diagnostics, not the primary KPI;
  - all estimates are marked `estimated: true`; billing/usage wins.

`loom churn stats [--by model]` shows token/cost on the first screen and write counts on the second.

**Implementation (2026-11-09)**: `AdapterUsageRow` and `UsageRecord` gained optional `step` /
`model_id` / `tool_names` / `write_paths` / `rewrite_count` / `history_bytes` / `projected_bytes`
(synced into `schemas/adapter-usage.v1.json`, `schemas/usage-record.v1.json`). Native computes the
serialized history bytes before and after projection at each step and writes the usage row after the
tools finish, so the row can carry this step's tools and write paths; at run end it writes
`logs/churn.json`, where `rewrite_output_tokens` is estimated from the repeated payload,
`history_carry_tokens` from "written at step S × remaining steps", `avoided_input_tokens` from
`(history_bytes - projected_bytes) / 4` per step, and `cache_invalidation_cost_est` is only given
when the capability bit has `cache_write_usd_per_mtok`, otherwise 0. When the price is unknown
`wasted_cost_usd_est` is `null` (unknown), never guessed. `loom churn stats` aggregates
`.loom/runs/*/logs/churn.json`, and `--by model` groups by alias + provider id.

### 8.5 Cost-aware restart

Deferred until A/B data exists:

- trigger: the same file rewritten ≥N times, or `wasted_tokens_est` over a threshold;
- action: rebuild `messages`, carrying only system + packet + worktree summary + churn summary + the latest failure;
- limit: at most 1 per run; shares the original budget; records `task.churn` or a churn log;
- risk: losing key context degrades quality; not enabled by default before real A/B.

### 8.6 Offline tests

- a scripted provider writes the same 8KB file 5 times in a row; record the bytes actually sent in `messages` each time; without projection it grows with step count, with projection it is bounded;
- construct an over-long history and assert there is no orphan tool result after trimming;
- `write` overwrite guard: the second overwrite is refused with a short error; `edit` succeeds;
- churn statistics use synthetic usage rows; assert the fields and the `estimated` marker;
- projection does not change the system/packet prefix (byte comparison);
- acceptance still passes; at most 1 restart.

### 8.7 Risks and stop conditions

| Risk | Mitigation |
|---|---|
| quality regression | keep recent turns + worktree summary; turn projection off if A/B first-pass rate drops >2pp |
| prefix cache invalidation | stable prefix bytes unchanged; `min_clear_tokens` threshold |
| tool_call pairing errors | trim by complete turn + tests |
| misleading metrics | mark `estimated`; billing/usage wins |
| external adapters uncontrollable | watchdog/selection only, no projection promised |
| premature hard blocking | project first; the write guard only refuses overwrites without `mode` |

### 8.8 Real A/B: projection off vs on (2026-11-09)

Using the official `deepseek-flash` (wire `chat`, trustworthy usage) on the same fixture and the same
task: `src/data.txt` about 8KB, task = read → write the first line → finish. Each arm started from
the same commit in a temp repo, the only difference being the projection knobs:

- `off`: `LOOM_NATIVE_HISTORY_BYTES=0`;
- `on`: `LOOM_NATIVE_HISTORY_BYTES=4096`, `LOOM_NATIVE_KEEP_TURNS=1`,
  `LOOM_NATIVE_MIN_CLEAR=200` (more aggressive than the default 24KB / keep 4 / clear 2000, so the
  lever is visible within a 4–5 step task).

Totals:

| arm | steps | input | cached | output | total tokens | cost_usd | wall |
|---|---|---|---|---|---|---|---|
| off | 4 | 14,747 | 9,728 | 876 | 15,623 | 0.001308 | 7s |
| on | 5 | 13,875 | 9,984 | 899 | 14,774 | 0.001153 | 9s |

Per request (`history_bytes` is the full text before projection, `sent` is what was actually sent):

| step | off: full→sent / input | on: full→sent / input |
|---|---|---|
| 1 | 3,818 → 3,818 / 1,826 | 3,817 → 3,817 / 1,826 |
| 2 | 5,759 → 5,759 / 2,352 | 15,195 → 15,195 / 4,764 |
| 3 | 16,822 → 16,822 / 5,213 | 16,278 → 5,754 / 2,332 |
| 4 | 17,392 → 17,392 / 5,356 | 16,637 → 6,113 / 2,426 |
| 5 | — | 16,986 → 6,462 / 2,527 |

Conclusions (n=1, showing the mechanism only, not first-pass rate):

- with the same ~16–17KB of full history, `off` sends 5.2–5.4k input per step while `on` sends only
  2.3–2.5k, a 46%–55% cut in per-step input; `churn.json` reports
  `avoided_input_tokens=7893` (estimated as `(history−projected)/4`), the same order of magnitude as
  the ~8.4k measured over three steps, so the estimate methodology holds.
- `on` ran one extra step on less cumulative input (5 steps 13,875 input vs 4 steps 14,747);
  per-step tokens 3,906 → 2,955 (−24%), per-step cost $0.000327 → $0.000231 (−29%).
- both arms were recorded as `budget_exceeded` at the end because of the task budget
  `max_input_tokens=12000`, while acceptance itself had passed; so these numbers compare
  **per-request size**, not task success rate.
- `wasted_tokens_est=0` is the correct result: there was no repeated whole-file write this time, and
  the projection benefit shows only in `avoided_input_tokens`; write churn and projection are two
  independent levers, so `churn_ratio` alone is not enough.
- the default knobs (24KB / keep 4 / clear 2000) do not trigger in a 4-step task; in a real #24
  task at 12–31k per step the projection triggers earlier with a larger absolute benefit, but first-pass
  rate needs validation on the next round of real tasks.

Reproduce: `scripts/ab-churn.sh --arm off|on`, `--report` prints the tables above; the script carries
its own `LOOM_AB_MAX_TOKENS` cap (default 30k, summed over both arms). Note `scripts/dev.sh` only
passes through a whitelisted env, so an endpoint override must be `export`ed inside the container,
otherwise it silently falls back to the official endpoint in `.env.local`.

---

## 9. F. Skills

### 9.1 Current pilot facts

Five project-level skills already exist and are auto-discovered by OpenCode:

| ID | Purpose | Auto-advertised | Body roughly |
|---|---|---|---|
| `docs-writing` | document writing, structure, Markdown, de-AI-ifying | yes | 0.6K tokens |
| `code-quality-review` | architecture boundaries, dependencies, code smells, refactor evaluation | yes | 1.2K tokens |
| `architecture-design` | module boundaries, interfaces, seams, ADRs | yes | 0.8K tokens |
| `refactor-code` | behavior-preserving cleanup, dedup, simplification | yes | 0.8K tokens |
| `format-code` | Rust/YAML/SQL/JSON/Markdown/Shell/TOML formatting | yes | 0.6K tokens |

The five descriptions total about 300 tokens/step; a 50-step session costs about 15K tokens of fixed advertising. Bodies load only on trigger, and reference files are read on demand. Pilot notes and third-party licenses are in `.opencode/skill-pilot/`.

### 9.2 Verdict: v1 does not build a second skill runtime

Reasons:

- OpenCode already provides project-level discovery, on-demand loading and description advertising; writing `loom skill` on top would duplicate the implementation and create two sources of truth;
- there is no second external adapter today that needs Loom to load skills itself;
- the L2 evaluation depends on a real model, a fixed task set and billing, none of which exist today;
- both SWE-Skills-Bench and SkillsBench show skill benefits are highly heterogeneous; governance matters more than a loader right now.

v1 does three things only:

1. `.opencode/skills.lock` (tracked) records each skill's path, sha256, license, source and description length;
2. `scripts/check-skills.sh` performs the static checks, as the `skills-check` job of `.loom.ci.toml`;
3. trigger budgets: each `description` ≤400 characters, the five together ≤1500 characters; each must state "do not use for".

### 9.3 Static checks

`scripts/check-skills.sh` checks at least:

- every `SKILL.md` has valid frontmatter and a `name` matching its directory;
- `description` non-empty, ≤400 characters, containing the trigger condition and the "do not use for";
- body ≤5k tokens (estimated by bytes), reference file paths exist and do not escape;
- no symlinks pointing outside the repo;
- no executable scripts, network calls, or secret patterns (the current pilot has no scripts; if one appears it gets human review);
- the sha256 matches `skills.lock`;
- the license file exists, and third-party sources are recorded in `THIRD_PARTY_NOTICES.md`.

`skills.lock` example:

```json
{
  "v": 1,
  "skills": [
    {
      "id": "docs-writing",
      "path": ".opencode/skills/docs-writing/SKILL.md",
      "sha256": "...",
      "license": "MIT",
      "source": "internal adaptation",
      "description_chars": 280
    }
  ]
}
```

The lock lives under `.opencode/` and is tracked, never under `.loom/`. If external adapters learn to load skills later, a root-level `skills.lock` or a `loom skill` command can be considered then.

### 9.4 L2 evaluation protocol (deferred)

Waiting on a real model, a fixed task set and billing:

- same task set, same model, same budget, paired runs of "loaded / not loaded";
- metrics: first-pass verify, CPVT, TPVT, WPT, trigger rate, false-trigger cost;
- at least 20–30 tasks per skill; report confidence intervals of the delta;
- promotion from `experimental` to `project` only with "no quality regression and CPVT not up";
- harmful skills (overriding explicit instructions, causing regressions) are disabled immediately;
- no global install, no auto-install over the network, no embedding retrieval.

### 9.5 Risks

| Risk | Mitigation |
|---|---|
| description too broad, frequent false triggers | description budget + trigger/non-trigger examples + static checks |
| a skill overrides the TaskSpec | body treated as untrusted input; system constraints win; disable on harm |
| fixed advertising cost | the five descriptions cost ~300 tokens/step; over budget means compressing descriptions first |
| evaluation not doable | lock/static checks first, L2 deferred; no capability claims |
| a second source of truth | v1 writes no `loom skill` loader |

---

## 10. Cross-cutting contracts and tests

For any event/state/config change, complete all of these per `AGENTS.md` §4:

1. `loom-core` types and exhaustive `match`;
2. `schemas/*.json` (event enum, fields, `additionalProperties`);
3. docs: `doc/03` §11.2, `doc/10` (output/exit codes), `doc/15`, this document;
4. at least one unit/integration test;
5. CLI `--help`, exit codes, `--json` purity;
6. old-data compatibility: new fields optional + `serde(default)`; breaking changes bump to v2.

Two more anti-drift tests are suggested:

- read `schemas/event.v1.json`'s `enum` and compare it with the `EventType` string set;
- read the `required`/properties of `schemas/receipt.v1.json` / `usage-record.v1.json` and do a minimal consistency check against the Rust types (at least guarantee new fields are written into the schema).

Adding `deny_unknown_fields` to `Config` is an intentional behavior change: unknown sections in old configs go from "silently ignored" to "explicitly reported". That is a good thing, but the error must be clear in the release note/`doctor`. **Shipped (2026-11-11)**: the error comes from serde (`` `unknown field \`x\`` ``), `loom config set` validates wholesale before writing to disk and rolls back on failure, and `doctor` handles the day-to-day hint.

Test entry points:

```bash
make check
./scripts/dev.sh bash /work/scripts/smoke.sh
./scripts/dev.sh bash /work/scripts/smoke-native.sh
```

New tests should prefer temp directories, fake commands and scripted providers, without depending on the contents of the host repository.

---

## 11. Phasing and acceptance

### Phase 0: state and claim (2–3 person-days)

Scope: `TaskStatus::NeedsInput`, the claim snapshot, verify mapping, exit codes, the event schema drift test.

Acceptance:

- `needs_input` stays `needs_input` after verify and produces no `task.failed`;
- `blocked` / `budget_exceeded` are no longer written as `claim_not_done`;
- a repeated verify does not lose the claim;
- escalation counts do not increase;
- the schema drift test passes.

### Phase 1: attention visibility (3–4 person-days)

Scope: `attention.*` events, the `.loom/inbox.json` cache, `loom inbox list/show/ack/defer/resolve/--line/--check`, the TTY footer, doctor items.

Acceptance:

- after running `loom status`/`dispatch`/`verify` on a TTY, one todo line appears on stderr;
- `--json` stdout has no footer;
- nothing is printed for non-TTY/CI by default;
- `--check` exits 2 when there is a blocking item;
- ack does not close an item; only resolve does;
- a footer failure does not affect the original command's exit code.

### Phase 2: local CI core (5–7 person-days)

Scope: the `loom-ci` crate, root-level `.loom.ci.toml`, detection/generation, plan/run/doctor, runner/argv, log budget, report.

Acceptance:

- `.loom.ci.toml` is generated in a temporary target project; Loom's own dogfood using `./scripts/dev.sh` is verified separately;
- `loom ci run --profile pre-commit` fails on a format error;
- `--json` is pure JSON; logs land on disk and summaries are bounded;
- doctor reports a missing container engine/runner;
- existing configuration is not overwritten;
- no configuration is guessed without evidence.

### Phase 3: question protocol (3–4 person-days)

Scope: the native `ask` tool, `questions.json` validation, `.loom/answers/`, `loom answer`, Context Compiler injection, assumptions.

Acceptance:

- a scripted provider calls `ask` → exit 2 → visible in the inbox → answer → ready → a new run receives the answer → verify passes;
- not released until all blocking questions are answered;
- with no `questions.json` no questions are guessed;
- answers do not enter the failure count.

### Phase 4: CI hooks / safe fix / report (3–4 person-days)

Scope: install-hooks, pre-commit/pre-push, `--fix`, diff display, report/attestation, verify-attest.

Acceptance:

- hooks are idempotent, non-Loom hooks are refused, backups are made;
- `--fix` does not stage/commit and the changes are visible;
- attestation passes for the same tree and fails for a different tree;
- `--no-verify` is not faked as recorded;
- server skipping is **not in this phase**.

### Phase 5: churn (5–7 person-days)

Scope: step-level projection, usage step, churn statistics, the write guard, `edit` hardening; soft restart optional.

Acceptance:

- input is bounded in the 5-rewrite scenario;
- tool_call pairing is correct;
- churn fields are complete and marked estimated;
- acceptance does not regress;
- the prefix bytes are stable;
- projection is turned off if A/B first-pass rate drops >2pp.

### Phase 6: skills governance (1–2 person-days)

Scope: `skills.lock`, `scripts/check-skills.sh`, the CI `skills-check` job.

Acceptance:

- the lock matches the files' sha256;
- description length and trigger boundary checks pass;
- no `loom skill` runtime; L2 deferred.

### Deferred

- real goose/OpenCode canary: needs binaries inside the container;
- native vs adapter A/B, real CPVT/TPVT/WPT: needs provider keys, billing, a fixed task set;
- server skipping based on attestation: needs signed/trusted runners;
- skills L2 evaluation: needs a real model and a task set;
- act/workflow simulation, nested-container CI, review/skill jobs, parallel CI: not in v1.

### Parallelization notes

- Phase 1 and Phase 2 have essentially no file overlap and can run in parallel; with a single implementer, do 0 first, then either 1→2 or 2→1.
- Phase 3 depends on Phase 1's inbox; do not build automatic answering early.
- Phase 5 depends on Phase 0's states and usage step; do not build churn statistics while verify semantics are unfixed.
- Phase 6 is independent and can be inserted at any time, but do not grow it into a skill runtime.

---

## 12. Decisions to confirm

1. **CI configuration location**: confirm root-level `.loom.ci.toml` tracked, `.loom/ci/` ignored.
2. **attestation authorization**: confirm v1 does not skip server CI based on a local attestation, only evidence/measurement.
3. **verify exit codes**: confirm `0/1/2/3` (2 = needs_input/blocked, 3 = budget).
4. **`needs_input` state**: confirm a new standalone `TaskStatus`, not reusing `blocked + code`.
5. **attempt semantics**: confirm whether the attempt increments on a new run after an answer (recommended: increment, and record `question_round` separately).
6. **answer storage**: confirm `.loom/answers/<task_id>.jsonl` as the local content store, with events keeping only hashes/metadata.
7. **footer default**: confirm enabled on TTY by default, off via `LOOM_NO_INBOX=1`, off by default for non-TTY.
8. **CI schema**: confirm `[[jobs]] + argv + [runner]`; v1 does not support `[jobs.<id>] + cmd`, nor `kind`/`env`/`network`.
9. **hooks default**: confirm `hooks_fix_safe` defaults to false; safe fix requires an explicit opt-in.
10. **churn defaults**: confirm `write_guard = "create"` and `project = "auto"`; `apply_patch` deferred.
11. **skills v1**: confirm no `loom skill` runtime, only lock + static checks; L2 deferred.
12. **CI detection scope**: confirm v1 does Rust single-stack + `scripts/dev.sh` + Makefile only, other stacks deferred.

Until these decisions are confirmed, do not start code implementation. This document is an assessment only and changes no implementation.
