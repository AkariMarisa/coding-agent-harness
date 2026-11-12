# 01 Problems, Requirements, and Acceptance

> Language: **English** | [简体中文](01-problems-and-requirements.zh-CN.md)

## 0. Document Purpose

Define the problems Loom is to solve, the system boundary, functional/non-functional requirements, and the acceptance criteria. The architecture, protocols, and roadmap that follow all defer to the **FR/NFR numbering** in this document, so that "lively discussion with no tracking of delivery" does not happen.

## 1. Users and Scenarios

### 1.1 User Profiles

- An independent developer or the tech lead of a small team, responsible at once for software design, architecture, and front-end/back-end implementation.
- Already has a specs workflow, and is used to handing big tasks to an agent for continuous progress.
- Sensitive to token cost and the monthly bill, and wants low-priced models to take on part of the implementation work.
- The current primary environment is Windows + PowerShell, where paths may contain spaces; there are also Linux/WSL scenarios.
- Current primary toolchain:
  - `pi` 1.5.0 + `@tintinweb/pi-subagents` + `@narumitw/pi-goal` + `@casualjim/pi-headroom` + other context/memory plugins;
  - Codex CLI;
  - a resident `headroom proxy`, currently `--mode cache`;
  - primary model `gpt-6.1-sol`, reasoning level `medium`;
  - worker models such as `deepseek-v4-flash` are not yet actually connected; but in the early stage API keys for GPT, DeepSeek, Kimi, MiniMax, MiMo, etc. can be provided, to first build a multi-provider API lane.

### 1.2 Typical Workflow (As-Is)

1. The human writes specs / design documents.
2. Hand them to the main agent, turn on goal mode, and run continuously.
3. The agent reads files, edits code, runs tests, fixes errors.
4. The human corrects course, adds context, and re-accepts within the same long session.
5. The more rounds, the more expensive the history resend; sub-agents relay each other in natural language, and goals and acceptance gradually drift.

### 1.3 Target Workflow (To-Be)

1. The human writes `GOAL.md`, `spec.md`, `design.md`, defining executable acceptance.
2. The orchestrator breaks down the task graph and generates a `TaskSpec` for each slice.
3. The worker executes in an isolated worktree / isolated context, reading only the necessary references.
4. The worker runs `verify` and outputs a four-line receipt; details go to a report file.
5. The orchestrator reads only the receipt and the verify result, and decides to merge, retry, escalate, or involve a human.
6. The human looks only at exceptions, key decisions, and final acceptance, no longer "riding along" in a long session.

## 2. Measured Status Quo

The following data comes from a local Headroom snapshot provided in earlier discussions, used only to determine the direction of optimization, not as a long-term baseline. When implementing, it must be re-sampled and the measurement definition recorded.

| Metric | Snapshot Value | Notes |
|---|---:|---|
| Request count | 1,322 | Codex 1,047 (79.2%), pi 275 (20.8%) |
| Billed input (before) | 166,837,997 | Proxy measurement definition |
| Compressed input (after) | 165,664,234 | Compression removed 1,173,763 |
| Compression savings ratio | **0.70%** | 1,314 requests were compressed, 12.5% on average |
| Output tokens | 560,035 | Only the part visible to the proxy |
| `with_headroom_usd` | **$32.07** | Single-snapshot definition |
| Prefix cache discount | **$270.63** | Far larger than compression savings |
| `json_bloat` | **60,596,889** | Cumulative detected volume, not "already saved" |
| `reread` | **5,670,755** | Cumulative detected volume |
| `html_noise` | 4,052,968 | Same as above |
| `base64` | 1,184,548 | Same as above |
| `whitespace` | 1,495,138 | Same as above |
| `by_model` | only `gpt-6.1-sol` | Multi-model not connected |
| `routing` | `null` | No routing |
| Billed input / genuine new input | 154.0M / 5.94M ≈ **25.9×** | Early snapshot, showing the magnitude of resending |
| `size_floor` skips | about **71%** of units | Small results cannot be handled by the compressor |

> Measurement-definition reminder: `WasteSignals` are cumulative detected volumes and never decrease; A/B can only compare the growth rate of "Δ value / Δ requests". Do not treat a cumulative value as "current waste" or "already saved".

## 3. Problem Definition

### P1 Quadratic Resend in Long Sessions

Every round resends the system prompt, tool schemas, and the entire history. The total input over T rounds is approximately:

```
BilledInput(T) = T·(P + U) + (U + A)·T(T−1)/2
```

where `P` is system + tools, `U` is the task content, and `A` is the new content added each round. The larger T is, the more dominant the quadratic term becomes. The measured 25.9× resend is its result.

**Impact**: the main bill; and switching to a cheaper model only lowers the unit price, it cannot eliminate the resend area.

### P2 Tool Result Bloat

`json_bloat` accumulates to 60.6M, 83% of all waste signals. Large amounts of raw JSON, logs, HTML, and base64 go straight into toolResult, while the compressor skips about 71% of scattered small results because of `size_floor`.

**Impact**: context fills up with low-value content, rounds increase, and resending is amplified further.

### P3 Repeated Reads

`reread` accumulates to 5.67M. The typical bad habit: `grep` once first, then open the whole file; the same file is read repeatedly across multiple tasks/rounds.

**Impact**: duplicate input; duplicated work among workers.

### P4 Sub-Agent Communication Passes by Value

The main agent pastes the parent session summary, full file contents, and the previous round of conversation to the sub-agent; the sub-agent pastes process logs back to the main agent. Every dispatch and collection is billed again.

**Impact**: multi-agent not only fails to save tokens, it introduces extra communication overhead.

### P5 Goal and Acceptance Drift

Acceptance criteria exist only in the conversation; the sub-agent understands "done" via natural language; the main agent relies on the sub-agent's own claim of "finished". There is no executable verify, and no structured receipt.

**Impact**: rework, repeated communication, false passes; `cost per successful task` is invisible.

### P6 Single Model and No Routing

`by_model` contains only `gpt-6.1-sol`, and `routing` is `null`. All tasks — including mechanical implementation, test backfill, and documentation — go through the frontier model.

**Impact**: wasted unit price; and no way to choose a model by task risk.

### P7 Compression ROI Mismatch

Compression saves only 0.70% ($2.35), while the prefix cache discount is $270.63. `--mode token` rewrites history and breaks prefix stability; trading $270 of cache benefit for a few cents of compression is a net loss.

**Impact**: continuing to tune compression parameters is the wrong direction; tool layer and context lifecycle should be governed instead.

### P8 Missing Cost/Quality Feedback Loop

There is no per-task, per-model, per-attempt ledger; only lifetime cumulative counters; there is no `cost per successful task`, and no data on whether "escalating on failure" is worth it.

**Impact**: impossible to tell whether optimization works, and impossible to decide whether to connect cheaper models.

## 4. Root Causes

| ID | Root Cause | Corresponding Problems |
|---|---|---|
| R1 | Context lifecycle = session lifecycle | P1, P3 |
| R2 | State lives in the conversation, not in files | P1, P5 |
| R3 | Agents communicate by value, not by reference | P2, P4 |
| R4 | Misaligned optimization target: compress the peak, not reduce the area | P1, P7 |
| R5 | Acceptance not executable, escalation relies on self-assessment | P5, P8 |
| R6 | Model selection is global, not task-level | P6 |
| R7 | No result governance at the tool layer (projection, truncation, caching, references) | P2, P3 |

**In one sentence**: the problem is not that the model is not cheap enough; context is treated as an unlimited resource, and state is treated as chat history.

## 5. Goals and Non-Goals

### 5.1 Goals

| ID | Goal | Measurement | Target Value (First Phase) |
|---|---|---|---|
| G1 | Lower the cost per verified task | `CPVT = total cost / tasks that pass verify` | **−40%** relative to baseline (within 4 weeks); −60% (within 8 weeks) |
| G2 | Bounded main-session context | orchestrator input tokens | **≤ 30k**, and does not grow with total task duration |
| G3 | Lower inter-agent communication overhead | `handoff_overhead = handoff tokens / total tokens` | **≤ 5%** |
| G4 | Preserve prefix cache benefit | `cache_hit = cached_input / input` | **≥ 75%**, and does not drop by more than 10pp |
| G5 | Acceptance is executable and auditable | share of merges with verify evidence | **100%**; 0 unverified merges |
| G6 | Let worker models take on suitable tasks | share of implementation tasks completed by worker tier | **≥ 50%** after P2, with lower CPVT and no quality regression |
| G7 | Lower token usage per verified task | `tokens_per_verified_task = billed_input / verified_tasks` | **−50%** relative to baseline (within 8 weeks) |

> G1 is the primary cost KPI, G7 is the primary token KPI. Read them together: if tokens drop but cost does not, the model/cache was used wrongly; if cost drops but tokens do not, only a cheaper model was swapped in and the waste remains.

### 5.2 Non-Goals

| ID | Non-Goal | Reason |
|---|---|---|
| N1 | Building our own Agent Runtime, sandbox, editor, TUI | Mature components are sufficient; rewriting is pure loss |
| N2 | Unattended fully-automatic large tasks | Without verify as a backstop, automation only amplifies errors |
| N3 | Using "compression rate" or "token unit price" as a core KPI | Compression saves 0.70%, cache saves $270; unit price ≠ CPVT |
| N4 | LLM-based automatic complexity routing in v1 | Not interpretable, prone to drift; first use explicit task category mapping |
| N5 | Replacing pi / Codex / Headroom | They are replaceable execution endpoints and gateways, not things to rewrite |
| N6 | Multi-tenancy, cloud SaaS, team permissions | The first phase is a single-machine/single-user workbench |
| N7 | Sending all tasks through cheap models | Cheap models only go on "short, verifiable, re-runnable" slices |

## 6. User Stories

### US1 Long-Task Resumption

As a developer, I hand `GOAL.md` and one big task to the orchestrator; it breaks them into a task graph and dispatches slice by slice. Even after dozens of slices have run, the main session context still contains only the goal, a summary of the task graph, and the most recent receipts.

**Acceptance**: after 10 slices are completed in sequence, orchestrator input ≤ 30k; no need to read worker transcripts back into the main session.

### US2 Multiple Models Each Doing Their Part

As a developer, I let frontier models do design, decomposition, and judgment; and worker models do implementation, test backfill, and mechanical refactoring. Each task explicitly specifies its tier in `TaskSpec.tier`, rather than letting the model self-assess.

**Acceptance**: `by_model` shows both frontier and worker at the same time; the verify pass rate for worker tasks is no lower than 95% of the baseline; CPVT decreases.

### US3 Sub-Agent Goal and Acceptance Alignment

As a developer, every TaskSpec a worker receives contains: input references, paths allowed to be modified, acceptance commands, budget, output contract. Workers do not chat directly; interface contracts are frozen in `contracts/`.

**Acceptance**: when the same task is dispatched twice, the two workers' acceptance criteria are exactly identical; no "verbally added requirements".

### US4 Cost Visibility

As a developer, I can see input/cache/output/cost for each task, each attempt, and each model, as well as the cost of failure retries and escalations.

**Acceptance**: the ledger can be aggregated by `run_id / task_id / tier / model / attempt`; error vs. the provider bill < 5%.

### US5 Failure Is Recoverable

As a developer, when a worker crashes, exceeds its budget, or fails verify, the system preserves the scene (worktree, logs, receipts), and recovers from the checkpoint or escalates, instead of rerunning the whole long session.

**Acceptance**: recoverable within 5 minutes after kill -9; recovery does not repeat tasks that already passed.

### US6 Humans Only Called on Exceptions

As a developer, I am called only on `ambiguous_spec`, `conflict`, consecutive verify failures, and budget overruns; the normal path does not need me to ride along.

**Acceptance**: human touch rate ≤ 30% (first phase), and every intervention has a clear pending question and an evidence package.

## 7. Functional Requirements

Priority: **M** = Must (required in the first phase); **S** = Should; **C** = Could.  
Phase: **S** = single-GPT stopgap; **P0** = ledger; **P1** = thin runner; **P2** = routing; **P3** = parallelism; **P4** = optimization.

| FR | Priority | Main Phase |
|---|---|---|
| FR-01 Externalized state | M | S |
| FR-02 Slice/task graph | M | S/P1 |
| FR-03 Typed dispatch | M | S/P1 |
| FR-04 Four-line receipt | M | S/P1 |
| FR-05 Delivery by reference | M | S/P4 |
| FR-06 No transcript relay | M | S/P1 |
| FR-07 Verify contract | M | S/P1 |
| FR-08 Escalate on failure once | M | P1 |
| FR-09 Budget/circuit breaker | M | S/P1 |
| FR-10 Parallel isolation | S | P3 |
| FR-11 Explicit routing | S | P2 |
| FR-12 Gateway ordering/compression attribution | M | S/P2 |
| FR-13 Cache-friendly scheduling | M | S/P1 |
| FR-14 Cost ledger | M | P0 |
| FR-15 Metrics/A-B | S | P0–P4 |
| FR-16 Recoverable/auditable | S | P1 |
| FR-17 Permissions/keys/sandbox | M | S/P1 |
| FR-18 Extensible/versioned | C | P2/P4 |

### FR-01 Externalized State and Single Source of Truth

**Requirement**: goals, specs, design, task graph, progress, decisions, receipts, and reports must live in files under `.agent/`; chat is not used as state storage.

**Acceptance**:
- A new session recovers the task by reading `.agent/` alone, without depending on historical transcripts.
- Every file has a schema version and a single writer.
- `GOAL.md`, `spec.md`, `design.md`, `PROGRESS.md`, `DECISION.md`, `tasks.json` — none can be missing.

### FR-02 Task Slices and Task Graph

**Requirement**: a large task must be split into independently verifiable slices; slices have explicit dependencies between them, forming a DAG.

**Acceptance**:
- Every TaskSpec has `depends_on`, `inputs`, `allowed_paths`, `acceptance`, `budget`.
- Default slice granularity is 1–4 hours, ≤ 8 files; exceeding this requires further splitting.
- The task graph is visualizable/queryable and can list ready / running / blocked.

### FR-03 Typed Dispatch (TaskSpec)

**Requirement**: the orchestrator may dispatch only through structured TaskSpecs; natural-language "while you're at it, do X too" is forbidden.

**Acceptance**:
- TaskSpecs are validated by JSON Schema; unknown fields are rejected.
- `tier` is required; `acceptance` has at least 1 entry; `budget` is required.
- Dispatch content ≤ 2k tokens (not counting the referenced files themselves).

### FR-04 Four-Line Receipt

**Requirement**: at the end, the worker outputs only a fixed four-line receipt; details go to a report file.

**Acceptance**:
- The four lines are respectively `status / branch / verify / report`; the parser rejects extra lines.
- Receipt ≤ 100 tokens; contains no diff, file contents, logs, or chain of thought.
- Status enum: `done / blocked / needs_input / failed / budget_exceeded`.

### FR-05 Delivery by Reference

**Requirement**: across boundaries only paths, sha256, line numbers, diffs, and log paths are passed; no full text, no raw JSON/HTML/base64, no transcript.

**Acceptance**:
- The TaskSpec's `inputs` is a list of references, not content.
- The Receipt's `artifacts` is path + hash + byte count.
- Tool results return "written to <path>, N bytes" by default, rather than pasting content back.

### FR-06 No Transcript Relay / Worker Mesh Communication

**Requirement**: workers do not communicate directly; all coordination goes through the orchestrator and shared state files. Interface contracts are frozen in `contracts/`, and changes go through design tasks.

**Acceptance**:
- No worker → worker message channel exists in code/configuration.
- Two workers' implementations of the same interface follow `contracts/`, not chat alignment.
- The orchestrator does not inject one worker's transcript into another worker.

### FR-07 Verify Contract

**Requirement**: every implementation task must have an executable, non-interactive, repeatable acceptance command; the verify result is the sole basis for passing.

**Acceptance**:
- `acceptance[]` contains `cmd / expect_exit / timeout_s`.
- Commands run at the worktree root, output is written to a log file, and the receipt carries only the path.
- Exit code 0 = pass; non-0 = fail; timeout = fail.
- Code tasks without verify cannot be dispatched; exploration tasks are accepted with "artifact + reference", and must not merge code.

### FR-08 Escalate on Failure Once

**Requirement**: on a verify failure, retry once in the same tier first; on a second failure, escalate to the frontier `judgment`; judgment outputs `fix_plan / split / reject / human`; each task escalates at most once.

**Acceptance**:
- Escalation is triggered only by verify failure, `ambiguous_spec`, and `conflict`, not by model self-assessment.
- The escalation evidence package contains: TaskSpec, Receipt, diff, verify log, relevant decisions, file references.
- Still failing after escalation → `human`, no infinite loop.

### FR-09 Budget and Circuit Breaker

**Requirement**: every task and every run has token / cost / step / wall-clock caps; on exceeding a cap, stop immediately and preserve the scene.

**Acceptance**:
- On exceeding a cap the receipt is `status=budget_exceeded`; no automatic retry.
- At run level there is a total budget and a "no progress rounds" circuit breaker.
- Budget checks take effect both before dispatch (estimation) and during execution (usage stream).

### FR-10 Parallel Isolation and Merge Queue

**Requirement**: parallelizable tasks run in isolated git worktrees / branches; the same path allows only one writer at a time; merges are serialized.

**Acceptance**:
- Every task has `workspace.branch` and `allowed_paths`.
- Path locks have owner, lease, TTL; on conflict the task enters `blocked` and is not forcibly overwritten.
- verify must pass before merging; an integration verify runs once after merging.

### FR-11 Explicit Model Tier Routing

**Requirement**: tasks choose a model tier explicitly via `tier`; the gateway routes by alias to a concrete provider/model; implicit escalation is forbidden.

**Acceptance**:
- At least two tiers: `frontier` / `worker`; aliases are defined in the LiteLLM configuration.
- Unknown tiers are rejected at dispatch; provider failure allows only same-tier fallback.
- `by_model` shows both tiers; `routing` is no longer empty.

### FR-12 Gateway Ordering and Unique Compression Attribution

**Requirement**: the request path is `agent → Headroom → LiteLLM → provider`; a single request allows only one compression attribution; the compression mode stays process-level `cache`, with no per-request switching.

**Acceptance**:
- Headroom's downstream URL points at LiteLLM; the agent configuration does not need to know the provider.
- pi plugin compression and LiteLLM guardrail do not stack; the pi plugin is not enabled on the Codex path.
- Designs that switch `token/cache` by the `model` field are not enabled; if both modes are ever needed, split only at the process boundary by client/port.

### FR-13 Cache-Friendly Scheduling

**Requirement**: the main session prefix is stable and append-only; no mid-run changes to the system prompt, no tool reordering, no mid-run model swap; batch and merge changes.

**Acceptance**:
- The orchestrator's system + tools + goal sit in the prefix and are unchanged within a run.
- Receipts are append-only; old receipts are folded into a summary without modifying the original text.
- `cache_hit ≥ 75%`; a drop of more than 10pp triggers an alert and rollback.

### FR-14 Cost Ledger and Attribution

**Requirement**: every request records run / task / attempt / tier / model / input / cached / output / reasoning / cost; the provider bill or LiteLLM logs are authoritative.

**Acceptance**:
- The ledger can be aggregated on any dimension; requests without a `task_id` go to `unattributed` and raise an alert.
- Estimated cost is within 5% of the provider bill.
- Headroom stats are for diagnosis only, not a basis for billing.

### FR-15 Metrics, A/B, and Board

**Requirement**: provide metrics such as CPVT, resend ratio, cache hit, handoff overhead, context amplification, first-pass verify, escalation rate; support A/B on the same task set.

**Acceptance**:
- A metrics snapshot is generated at the end of each run.
- A/B uses the same task set, the same baseline, paired comparison; lifetime cumulative values are not compared directly.
- Quality guardrails (defect escape / rework) are read together with cost.

### FR-16 Recoverable, Auditable, Replayable

**Requirement**: a run's events, task states, receipts, verify logs, and ledger are replayable; after a crash, recovery is possible from the last stable point.

**Acceptance**:
- `events.jsonl` is append-only; every state change has an event.
- `loom resume` does not repeat tasks that already passed; worktrees can be rebuilt.
- Any historical task can answer "who, in what context, with what model, ran what command, with what result".

### FR-17 Permissions, Keys, and Sandbox

**Requirement**: workers can modify only `allowed_paths`; commands have an allowlist / timeout; keys never enter state files; repository content is treated as untrusted input.

**Acceptance**:
- Keys are read only from environment variables / secret managers; logs are redacted.
- Network is forbidden by default (unless the TaskSpec explicitly allows it); dangerous commands are refused.
- Every task has an audit record: command, path, exit code, duration.

### FR-18 Extensibility and Version Compatibility

**Requirement**: Runtime, models, verify types, and compressors are all replaceable; schemas and adapters carry version numbers; upgrades have a compatibility window.

**Acceptance**:
- The Runtime Adapter interface is stable; replacing pi/Codex does not change the orchestrator.
- `.agent/env.lock` records pi / codex / headroom / litellm / plugin versions.
- Schema migration scripts work; at least one minor version is compatible.

## 8. Non-Functional Requirements

| ID | Requirement | Target | Verification Method |
|---|---|---|---|
| NFR-01 | Bounded main context | orchestrator input ≤ 30k tokens | Ledger/request logs |
| NFR-02 | Minimal receipt | ≤ 100 tokens, fixed 4 lines | Parser validation |
| NFR-03 | Minimal dispatch | TaskSpec ≤ 2k tokens (excluding references) | Estimator |
| NFR-04 | Low orchestration overhead | scheduling/validation overhead < 2% of total tokens | Ledger |
| NFR-05 | Low gateway latency | extra p95 < 100ms | Load test/proxy logs |
| NFR-06 | Stable cache | cache hit ≥ 75%, no drop > 10pp | Request logs |
| NFR-07 | Recoverable | recovery ≤ 5 minutes after kill | Fault drills |
| NFR-08 | Idempotent | replaying the same TaskSpec produces no duplicate merges | Event replay |
| NFR-09 | Cross-platform | Windows PowerShell + Linux/WSL | CI + local machine |
| NFR-10 | Local-first | never leaves the machine by default; keys never enter the repository | Audit |
| NFR-11 | Observable | every task has complete lifecycle events | Event replay |
| NFR-12 | Version-controllable | key component versions pinned | `.agent/env.lock` |

## 9. Constraints and Assumptions

### 9.1 Environment Constraints

- The primary environment is Windows + PowerShell, paths may contain spaces; all scripts must be quoted and handle UTF-8.
- The resident Headroom process already exists; the pi plugin compresses first on the agent side, and the proxy then stores the original text / forwards; do not write "start the proxy" into the plugin lifecycle.
- Only a single GPT is in use now; multi-model is a later phase and must not block the first-phase stopgap.
- Network may go through a proxy; provider compatibility and streaming/tool calls need to be verified item by item.

### 9.2 Technical Assumptions (to be verified locally)

| ID | Assumption | Verification Method |
|---|---|---|
| A1 | pi subagent supports specifying the model per task and returning usage | Check docs / test locally |
| A2 | Codex supports subagent model override | Check `--version` / configuration |
| A3 | Headroom's `--openai-api-url` / `--anthropic-api-url` can point at LiteLLM | `headroom proxy --help` + integration test |
| A4 | LiteLLM supports both OpenAI Chat/Responses and Anthropic Messages | Compatibility matrix |
| A5 | LiteLLM passes through tool calls, streaming, usage (including cached tokens) | Compatibility matrix |
| A6 | git worktree works under Windows paths/long paths | Local test |
| A7 | Worker models have an acceptable pass rate on short slices + verify | 10–20 task A/B |
| A8 | Headroom stats' `by_model` shows the alias or the real model | Sampling |

### 9.3 Hard Constraints

- Do not write provider keys into `.agent/`, prompts, or logs.
- Do not let workers run arbitrary commands not on the allowlist.
- Do not stack two independent compressors on the same request.
- Do not use model self-assessment as a basis for escalation.
- Do not read worker transcripts back into the orchestrator.

## 10. General Acceptance Rules

1. **Every FR's acceptance must be executable or observable**; writing "supported" is not enough.
2. **The primary cost KPI is CPVT, the primary token KPI is TPVT**: the former is `total cost / tasks that pass verify`, the latter is `billed input / tasks that pass verify`. Token unit price and compression rate are only explanatory variables.
3. **A/B must use the same task set, the same baseline, and paired comparison**; lifetime cumulative values must not be compared directly.
4. **Quality guardrail takes priority**: if CPVT drops but defect escape / rework rises, it counts as a failure.
5. **Cost measurement definition priority**: provider bill > LiteLLM logs > Runtime usage > Headroom stats.
6. **Every phase must be rollback-capable**: keep the previous version of configuration, prompts, schema, and a working direct provider path.

## 11. Glossary

| Term | Meaning |
|---|---|
| Orchestrator | The orchestration component; maintains goal/task graph, dispatch, acceptance, escalation, with constantly small context |
| Worker | The agent that executes a single slice; isolated worktree + isolated context |
| Slice / Task | The smallest independently verifiable unit of work |
| TaskSpec | Structured dispatch contract |
| Receipt | Four-line receipt; the only permitted "result summary" across boundaries |
| Verify | Executable acceptance command and evidence |
| Handoff | Passing across agents/contexts; only by reference is allowed |
| Frontier | The frontier model tier; design, judgment, critical review |
| Worker tier | Low/mid-price model tier; short, verifiable, re-runnable implementation |
| CPVT | Cost per Verified Task, the cost of each verified task |
| Resend ratio | The share of history resend in billed input |
| Handoff overhead | Communication tokens as a share of total tokens |
| `.agent/` | The file-based state directory; the single source of truth |

## 12. Requirements Traceability Matrix

| Problem | Root Cause | Functional Requirement | Component | Metric | Phase |
|---|---|---|---|---|---|
| P1 Quadratic resend | R1/R2 | FR-01/02/03/13 | C1/C3/C4 | CPVT, resend ratio, cache hit | S/P1 |
| P2 Tool result bloat | R3/R7 | FR-05/06 | C5/C2 | json_bloat growth, context amplification | S |
| P3 Repeated reads | R7 | FR-05/13 | C2/C12 | reread growth, cache hit | S/P4 |
| P4 Communication by value | R3 | FR-04/05/06 | C5 | handoff overhead | S |
| P5 Acceptance drift | R5 | FR-07/08 | C6/C1 | first-pass verify, escalation rate | S/P1 |
| P6 Single model | R6 | FR-11/12 | C8 | by_model, CPVT | P2 |
| P7 Compression ROI | R4 | FR-12/13 | C9 | cache hit, compression savings/cost | S |
| P8 No cost feedback loop | R5 | FR-14/15/16 | C11 | CPVT, ledger error | P0/P1 |
| Parallel conflicts | — | FR-10/17 | C10 | conflict rate, merge duration | P3 |
| Version drift | — | FR-18 | C2/C8/C9 | compatibility test pass rate | Continuous |
