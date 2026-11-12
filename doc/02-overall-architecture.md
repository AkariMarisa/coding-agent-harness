# 02 Overall Architecture and Technical Design

> Language: **English** | [简体中文](02-overall-architecture.zh-CN.md)

> **v0.2 revision (2026-11-04)**: The "pi main path / resident small context" of this document has been revised by [08-design-roadmap-v0.2.md](08-design-roadmap-v0.2.md).  
> The control plane is now a deterministic state machine, and the executor is now a process-level replaceable adapter; this document's layering, components, gateway, and security sections remain valid.

## 1. Design Principles

| ID | Principle | Meaning | Counter-example |
|---|---|---|---|
| P1 | State separated from conversation | State lives in `.agent/`, chat is only for control | Leaving requirements, decisions, and progress in the transcript |
| P2 | Context budgeted | Each kind of context has a hard cap; exceed it and stop/fold | Letting context grow naturally to 200k |
| P3 | By reference, not by value | Pass paths, hashes, line numbers, diffs, log paths | Pasting full file contents/JSON/HTML into messages |
| P4 | Verification-driven | verify is the sole acceptance and escalation signal | Merging because the model says "I'm done" |
| P5 | Determinism first | What a script can do is not handed to an LLM | Using an LLM to parse test results or compute cost |
| P6 | Single writer + append-only | One writer per state file; logs only append | Several agents editing `PROGRESS.md` at once |
| P7 | Cache-friendly | Stable prefix, batched changes, no model swap within a run | Changing the system prompt mid-run, reordering tools |
| P8 | Transparent gateway | Compression/routing is transparent to the agent and replaceable | Hard-coding provider addresses in the prompt |
| P9 | Measure before optimizing | No ledger, no parameter changes | Tuning compression thresholds by feel |
| P10 | Minimal in-house code | Only write orchestration, protocol, ledger; reuse the rest | Building our own runtime/sandbox/compressor |

## 2. System Boundary

**Loom is**:

- an **orchestration layer** running on top of existing agent runtimes;
- a **file-based state protocol**;
- a **thin runner** (task graph, dispatch, verify, budget, escalation, recovery);
- a **cost/quality ledger**;
- a **gateway configuration convention** (Headroom + LiteLLM).

**Loom is not**:

- not a new Agent Runtime;
- not a new compressor or router;
- not an IDE / TUI / cloud platform;
- not an autonomous system that "lets the LLM manage itself".

## 3. Logical Architecture

```
┌──────────────────────────────────────────────────────────────┐
│ Human / CLI                                                  │
│  loom run | status | approve | abort | resume | report       │
└──────────────────────────┬───────────────────────────────────┘
                           │
┌──────────────────────────▼───────────────────────────────────┐
│ C1 Orchestrator (frontier, resident small context)           │
│  · Goal / Task graph   · Scheduler   · Budget   · Escalation │
│  · Reads state files + Receipt only; never worker transcripts│
└───────┬──────────────────────────────┬───────────────────────┘
        │ TaskSpec (by reference)      │ Receipt (4 lines)
┌───────▼──────────────────┐   ┌───────▼───────────────────────┐
│ C2 Worker Runtime        │   │ C3 State Store (.agent/)      │
│  pi / Codex / SDK        │◀─▶│ GOAL/spec/design/tasks/       │
│  isolated ctx + worktree │   │ PROGRESS/DECISION/reports     │
└───────┬──────────────────┘   └───────────────────────────────┘
        │ LLM requests (native OpenAI / Anthropic format)
┌───────▼──────────────────────────────────────────────────────┐
│ C9 Context Gateway: Headroom :8787 (mode=cache)              │
│ · live-zone compression · prefix cache · retrieve · diag stats│
└───────┬──────────────────────────────────────────────────────┘
        │ OpenAI-compatible / Anthropic-compatible
┌───────▼──────────────────────────────────────────────────────┐
│ C8 Model Router: LiteLLM :4000 (from P2 on)                  │
│  · alias → provider/model · fallback · budget · canonical logs│
└───────┬───────────────────────────┬──────────────────────────┘
        │                           │
   frontier tier                worker tier
   gpt-6.1-sol medium           DeepSeek / Kimi / MiniMax / MiMo
```

### 3.1 Layer Responsibilities

| Layer | Component | Responsibility | Not responsible for |
|---|---|---|---|
| Control layer | C1 Orchestrator | Goal, task graph, scheduling, budget, escalation | Writing code, reading transcripts |
| Protocol layer | C5 Handoff | TaskSpec/Receipt/Decision schema | Business logic |
| Execution layer | C2 Runtime Adapter | Calling pi/Codex, collecting usage | Task decomposition |
| State layer | C3 State Store | File read/write, schema, locks | Reasoning |
| Acceptance layer | C6 Verify Runner | Executing acceptance, collecting evidence | Deciding "what should be done" |
| Gateway layer | C9 Headroom | Compression, caching, retrieve | Model routing, task semantics |
| Routing layer | C8 LiteLLM | alias, fallback, logs, budget | Context isolation |
| Isolation layer | C10 Worktree | branch/worktree/merge | Task orchestration |
| Observability layer | C11 Ledger | Request/task/cost ledger | Compression/routing |

## 4. Deployment Topology

### 4.1 Current (As-Is)

```
pi / Codex ──▶ Headroom :8787 (cache) ──▶ provider (gpt-6.1-sol)
```

- The pi plugin compresses toolResult on the agent side, then Headroom stores the original text / forwards;
- only a single model;
- `by_model` contains only `gpt-6.1-sol`, `routing=null`.

### 4.2 Target (To-Be)

```
pi / Codex / Aider
        │ native protocol
        ▼
Headroom :8787  (mode=cache)
        │ OpenAI /v1 or Anthropic /v1/messages
        ▼
LiteLLM :4000
        ├── frontier  → gpt-6.1-sol (medium)
        └── worker    → deepseek / other domestic models
```

**Key points**:

1. **LiteLLM comes after Headroom**, not before it. Headroom is a transparent forwarding proxy and does not need to know the final provider when it receives a request; LiteLLM is responsible for alias resolution.
2. **Compression and routing are decoupled**. Headroom stays at process-level `cache`; do not try to switch `token/cache` by the `model` field in the request.
3. **Cost follows LiteLLM logs / provider bills**. Headroom's `by_model` is for diagnosis only; the alias may keep it from seeing the real model.
4. **Protocol format is decided by the client**. pi uses the OpenAI format, Claude Code uses the Anthropic format, Codex may use Responses; each kind must be verified separately.

> **First-phase provider constraint**: API keys for GPT, DeepSeek, Kimi, MiniMax, MiMo, etc. can be provided in the early stage; first let `frontier` use GPT and `worker` use DeepSeek, and connect other models gradually through A/B. The subscription lane does not participate.

### 4.3 Why Not "LiteLLM First, Headroom After"

| Option | Problem |
|---|---|
| `agent → LiteLLM → Headroom → provider` | Headroom's compression/cache serves only a single upstream; with multiple providers a Headroom setup is needed per provider, and LiteLLM's retries/fallback bypass the compression layer |
| `agent → Headroom → LiteLLM → provider` | All clients share one compression/cache layer; routing is transparent to Headroom; adding a provider only changes the LiteLLM configuration |

### 4.4 Phased Topology

| Phase | Topology | Purpose |
|---|---|---|
| S | `agent → Headroom → provider` | Single-GPT stopgap, zero architectural change |
| P0 | the above + `.agent/` ledger | Establish a baseline |
| P1 | the above + Loom runner | Automated scheduling/verify/budget |
| P2 | `agent → Headroom → LiteLLM → provider` | Multi-model routing |
| P3 | P2 + multiple worktrees / merge queue | Parallelism |
| P4 | P3 + board/A-B | Continuous optimization |

> Do not introduce LiteLLM before P2. In the single-GPT phase, first fix the context lifecycle; otherwise you only copy one model's waste onto two models.

### 4.5 Compatibility Variant

If Codex's Responses API cannot pass through LiteLLM/Headroom, use the degraded variant:

- **pi / Claude Code**: go through `Headroom → LiteLLM`;
- **Codex**: go direct via `Headroom → provider`, using only Headroom compression/cache;
- Multi-model applies only to pi workers; Codex stays on frontier.

This is not a failure but **choosing the path by client capability**; see 07 for the compatibility matrix.

## 5. Component Inventory

| ID | Component | Responsibility | In-house/Reuse | Technology Suggestion | Key Interface |
|---|---|---|---|---|---|
| C1 | Orchestrator | Goal/task graph/scheduling/budget/escalation | In-house | TypeScript + zod | `plan() / next() / decide()` |
| C2 | Runtime Adapter | Turn TaskSpec into pi/Codex calls, collect usage | Thin in-house layer | CLI subprocess / RPC | `dispatch(spec) → receipt` |
| C3 | State Store | `.agent/` read/write, schema, locks | In-house | FS + JSON/JSONL + git | `read/write/append/lock` |
| C4 | Scheduler | DAG, ready queue, concurrency, retry | In-house | In-memory + persistence | `next() / complete()` |
| C5 | Handoff Protocol | TaskSpec/Receipt/Decision schema | In-house | JSON Schema | `validate()` |
| C6 | Verify Runner | Execute acceptance, collect evidence | In-house | subprocess + timeout | `run(spec) → evidence` |
| C7 | Budget Governor | token/cost/time caps, circuit breaker | In-house | usage stream + estimation | `check() / stop()` |
| C8 | Model Router | alias → provider, fallback, logs | Reuse LiteLLM | YAML | OpenAI/Anthropic API |
| C9 | Context Gateway | compression/cache/retrieve | Reuse Headroom | proxy | HTTP |
| C10 | Workspace Isolation | worktree/branch/merge | Reuse git | `git worktree` | `create/merge/remove` |
| C11 | Ledger | Request/task/cost ledger | In-house | JSONL → SQLite/DuckDB | `record/query` |
| C12 | Prompt Library | orchestrator/worker/judgment prompts | In-house | Markdown + version number | `load(role)` |
| C13 | Human CLI | run/status/approve/abort/resume | In-house | commander | CLI |
| C14 | Aider Adapter | Side-channel A/B / diff generation | Reuse + thin wrapper | aider CLI | `dispatch(spec)` |

### 5.1 In-House Boundary

**Must be built in-house** (about 1–2k lines, possibly less in the first phase):

- state protocol and validation;
- scheduling state machine;
- verify runner;
- budget/circuit breaker;
- ledger and metrics;
- prompt templates.

**Never built in-house**:

- Agent Runtime, tool execution, sandbox;
- compressor, cache layer;
- provider routing/retry;
- IDE/TUI.

## 6. Core Flows

### 6.1 Initialization

1. The human writes `GOAL.md` / `spec.md` / `design.md`.
2. `loom init` validates the files, generates the `tasks.json` skeleton, records `env.lock`.
3. The orchestrator reads the goal and produces the initial task graph (frontier only).

### 6.2 Planning

1. The orchestrator breaks the goal into 3–10 slices.
2. Each slice states clearly: input references, allowed paths, acceptance commands, budget, dependencies.
3. The human reviews only the task graph and acceptance, not every implementation detail.

### 6.3 Dispatch

1. The Scheduler picks a ready task.
2. The State Store creates worktree + branch + task directory.
3. The Runtime Adapter starts an isolated context with the worker prompt + TaskSpec.
4. Record the `task.dispatched` event and the budget.

### 6.4 Execution

1. The worker reads the `must_read` references; when more files are needed it reads them by path.
2. The worker modifies within `allowed_paths`; it does not touch forbidden paths.
3. The worker runs acceptance; on failure it fixes first, and stops as soon as the budget is exceeded.
4. At the end it writes the report and outputs the four-line receipt.

### 6.5 Acceptance

1. The Verify Runner reruns acceptance in a clean environment (it does not trust the worker's own account).
2. Compare exit code, logs, artifact hashes.
3. Pass → `passed`; failure → `failed`, entering retry/escalation.

### 6.6 Merge

1. The merge queue processes `passed` tasks serially.
2. Before merging, check whether the base has drifted; if it has, rebase + rerun verify.
3. After merging, run the integration verify; on failure, roll back the merge commit and mark it.
4. Update `PROGRESS.md`, `DECISION.md` (if there was a decision).

### 6.7 Escalation

1. Retry once in the same tier; still failing → produce an evidence package.
2. Frontier `judgment` reads the evidence package and outputs `fix_plan / split / reject / human`.
3. `fix_plan` → new worker task (with the plan and evidence references); `split` → split the task; `human` → stop.
4. Each task escalates at most once; each goal has a cap on total escalations.

### 6.8 Recovery

1. `loom resume` reads the events and task states.
2. Rebuild the worktrees of unfinished tasks; tasks that already passed are not rerun.
3. Tasks without a complete receipt are marked `unknown`, the scene is preserved, awaiting a human or redispach.

## 7. Task State Machine

```
pending ──▶ ready ──▶ running ──▶ verifying ──▶ passed ──▶ merged
              │          │            │
              │          │            ├──▶ failed ──▶ retrying ──▶ running
              │          │            │                 └──▶ escalated ──▶ passed/failed/human
              │          └──▶ blocked / needs_input / budget_exceeded / timeout
              └──▶ cancelled
```

| State | Meaning | Allowed Next State | Trigger |
|---|---|---|---|
| `pending` | Created, dependencies unmet | `ready` / `cancelled` | Dependencies complete |
| `ready` | Dispatchable | `running` / `cancelled` | Scheduler |
| `running` | Worker executing | `verifying` / `blocked` / `needs_input` / `timeout` / `budget_exceeded` | worker receipt/timeout |
| `verifying` | Independent rerun of acceptance | `passed` / `failed` | Verify Runner |
| `failed` | Acceptance failed | `retrying` / `escalated` / `human` | Retry policy |
| `retrying` | Same-tier retry | `running` | Budget allows |
| `escalated` | Escalated to judgment | `running` / `passed` / `human` | judgment output |
| `passed` | Acceptance passed | `merged` | Merge queue |
| `merged` | Merged | — | Integration verify passed |
| `blocked` | Path conflict/environment problem | `ready` / `human` | Lock/environment recovered |
| `needs_input` | Spec unclear | `ready` / `human` | Spec supplemented |
| `budget_exceeded` | Budget exceeded | `human` / `cancelled` | Budget governor |
| `timeout` | Timed out | `retrying` / `human` | Wall clock |
| `cancelled` | Cancelled by human/goal change | — | CLI/goal change |

**Hard rules**:

- `passed` must come from the Verify Runner, never from the worker's own account.
- `merged` must go through the merge queue + integration verify.
- `budget_exceeded` / `timeout` do not retry automatically unless a human approves.
- `escalated` at most once per task.

## 8. Concurrency Model

### 8.1 Basic Model

- The Orchestrator runs a single-threaded event loop; all state changes are serial.
- Worker concurrency defaults to **2**, capped at **3** (first phase).
- The same path allows only one writer at a time; reads can be shared.
- The merge queue is serial; during a merge, dispatch of new write tasks is paused (or only read-only tasks are dispatched).

### 8.2 Parallelism Benefits and Risks

| Benefit | Risk | Counter-measure |
|---|---|---|
| Shorter wall-clock time | Path conflicts | `allowed_paths` + path lock |
| Isolated contexts | Duplicated work | Task graph dedup + input hashes |
| Isolated cache prefixes | Cache hit drop | Serial main session; each worker its own prefix |
| Failure isolation | Merge conflicts | Merge queue + rebase + integration verify |

### 8.3 Cache and Parallelism

- Prefix cache hits on the "prefix"; different workers have different system/task prefixes and do not affect each other.
- Main-session calls must be serial and the prefix stable; do not let the main session change its prompt frequently amid parallel tasks.
- Concurrent requests to the same model may trigger rate limiting; LiteLLM handles retry/queueing, and workers only see the final result.

## 9. Data Persistence

### 9.1 Storage Choices

| Data | Format | Tracked in repo | Reason |
|---|---|---|---|
| Goal/spec/design | Markdown | Yes | Human-readable, reviewable, diffable |
| Task graph | JSON | Yes | Machine-readable, validatable |
| TaskSpec | JSON (one file per task) | Yes | Auditable, replayable |
| Receipt | JSON + four-line text | Yes | Machine-parsed + human-readable |
| Events | JSONL | No (ignored) | Append-only, replayable |
| Ledger | JSONL → SQLite/DuckDB | No (ignored) | High-frequency writes, queryable |
| Reports | Markdown | Yes | Human-readable, loaded on demand |
| verify logs | Text | No (ignored) | Large, referenced by path |
| Locks | JSON | No (ignored) | Transient state |
| Keys | Environment/secret manager | No | Security |

### 9.2 Why No Database in v1

- State files must be human-readable, reviewable, and git-diffable;
- The first-phase task volume is dozens per day, not millions;
- JSONL is enough for events and the ledger;
- Once queries get complex enough that JSONL no longer suffices, introduce SQLite/DuckDB while keeping the same schema.

### 9.3 Consistency and Locks

- One writer per file; the writer declares via `.agent/locks/<path>.lock`.
- Locks have `owner / lease_until / mode`; expired locks can be reclaimed.
- File writes use "temp file + rename" for atomic replacement.
- Events and the ledger are append-only; history is never modified.
- Every write operation records an `event` for replay.

### 9.4 Schema Version

- All JSON has `schema_version` (semver).
- Migration scripts live in `tools/migrate/`; at least one minor version is compatible.
- On read, validate the version first, then the schema; reject invalid data outright, do not "guess".

## 10. Model Routing Design

### 10.1 Tiers

The first phase keeps only two tiers, to avoid over-design:

| tier | Use | Example models | Constraints |
|---|---|---|---|
| `frontier` | Goal decomposition, architecture/design, judgment, critical review | gpt-6.1-sol @ medium | Small context, few calls, decisions only |
| `worker` | survey, spec-tests, implementation, mechanical refactoring, documentation | DeepSeek / Kimi / MiniMax / MiMo (first 1 primary + 1 fallback) | Must be short slices, with verify, re-runnable |

> Model names are placeholders; they map through LiteLLM aliases. Adding a third tier must first prove its benefit with CPVT.
> **Early API keys can cover several vendors**: first validate with GPT `frontier` + DeepSeek `worker`; connect Kimi/MiniMax/MiMo etc. gradually through A/B, not all at once.

### 10.2 Task Category → Tier

| Task Category | Default tier | Exceptions |
|---|---|---|
| goal / spec decomposition | frontier | — |
| Architecture, interface contracts, data models | frontier | — |
| Test specs (spec-tests) | worker | Critical security/payment logic reviewed by frontier |
| Implementation (single module, single interface) | worker | Cross-module refactoring, concurrency/security-critical → frontier |
| Mechanical refactoring, formatting, documentation | worker | — |
| judgment on verify failure | frontier | — |
| Final pre-merge review | frontier | Low risk may be sampled |

### 10.3 Routing Rules

1. `TaskSpec.tier` is **required**; no default inference, reject outright.
2. Unknown tiers are rejected; "default frontier" is not allowed.
3. Provider failure allows only **same-tier fallback**; silent escalation to frontier is not allowed.
4. Worker tasks must have a deterministic verify; otherwise they are not allowed on worker.
5. A task escalates at most once; escalation is triggered by verify, not by model self-assessment.
6. Routing changes (alias, fallback, prices) must go through a configuration review and be recorded in `DECISION.md`.

### 10.4 Alias Design

```yaml
# Illustrative; the actual implementation follows the LiteLLM version
model_list:
  - model_name: frontier
    litellm_params:
      model: openai/gpt-6.1-sol
  - model_name: worker
    litellm_params:
      model: openai/deepseek-v4-flash
  - model_name: worker-backup
    litellm_params:
      model: openai/glm-5.3-flash
```

- The agent only knows `frontier` / `worker`, not providers.
- Alias changes do not affect TaskSpec.
- The cost ledger records the alias + the real model (from LiteLLM logs).

### 10.5 Multiple Anthropic-Format Models

- The client keeps the native Anthropic Messages format; LiteLLM does the conversion.
- Slot names (such as `default / think / background / longContext`) map to aliases and are not parsed by Headroom.
- Do not try to switch compression mode by the `model` field in Headroom; mode is process-level.
- If some provider does not support the Anthropic format, go through LiteLLM's OpenAI-compatible endpoint, and the client points `ANTHROPIC_BASE_URL` at the gateway's compatible entry; tool calls / streaming / usage must be verified item by item.

## 11. Context Gateway Design (Headroom)

### 11.1 Positioning

Headroom is a **transparent compression/cache gateway**, not a router and not a task orchestrator. It solves "live-zone too large within a single request"; it does not solve "history resend" or "tool results passed by value".

### 11.2 Current Configuration Principles

- Keep `mode=cache`; do not switch to `token`.
- Do not lower `HEADROOM_MIN_TOKENS` / `SHELL_RESULT_MIN_CHARS` in pursuit of a compression rate.
- Do not enable lossy compression in exchange for cache stability.
- Do not stack LiteLLM's Headroom guardrail; one request has exactly one compression attribution.
- Do not switch mode by the `model` field; if two modes are ever needed, split at the process boundary by client/port.

### 11.3 Compression Attribution

| Client Path | Compression Attribution | Forbidden |
|---|---|---|
| pi + pi-headroom plugin | Plugin compresses + Headroom proxy stores original/forwards | Stacking LiteLLM guardrail on top |
| Codex | Headroom proxy | Enabling the pi plugin |
| Aider side channel | Headroom proxy (cache) | Depending on `headroom_retrieve` |
| Claude Code | Headroom proxy (cache) | Stacking another compressor on top |

### 11.4 Capability Boundaries

- `mode` is process-start configuration and cannot be switched per-request.
- Client classification looks at `User-Agent` / `X-Client`, not at `model`.
- Subscription state may trigger `lossless-only` / `live_zone_only`, making compression more conservative.
- `size_floor` skips many small results (measured about 71% of units).
- `WasteSignals` are cumulative detected volumes, not savings.
- `retrieve` has a TTL; only clients that registered a tool (pi) can use it; if Aider/Codex do not register, they cannot retrieve the original text.
- Compression benefit is far smaller than cache benefit; do not focus optimization here.

### 11.5 Measurement Definitions

- Headroom stats: for diagnosis (request count, compression rate, waste signals, latency).
- LiteLLM logs / provider bills: authoritative for cost.
- Runtime usage: task-level estimation, used for budget circuit breaking.
- Reconcile all three; investigate differences > 5%.

## 12. Security and Isolation

### 12.1 Workspace Isolation

- One git worktree + branch per write task; no direct edits in the main working tree.
- Worktree paths go in short directories (Windows long paths); keep them until merge/archive after the task ends.
- Merges are serial; check base drift before merging.

### 12.2 Paths and Commands

- `allowed_paths` / `forbidden_paths` are enforced by the runner, not left to the prompt's goodwill.
- Command allowlist: tests, lint, typecheck, build, read-only git; dangerous commands are refused.
- Network denied by default; verify runs that need the network declare and record it explicitly.
- All commands have timeouts; output is truncated before being written to the log file.

### 12.3 Keys and Privacy

- Keys are read only from environment variables / secret managers; never written to `.agent/`, prompts, or logs.
- Log redaction: API keys, tokens, cookies, connection strings.
- Local-first; no code/logs uploaded to third parties by default (except the model provider itself).

### 12.4 Prompt Injection

- Repository content, issues, web pages, and tool output are all treated as **untrusted input**.
- Verify commands may come only from the TaskSpec, never invented on the spot by the worker.
- The worker must not change `allowed_paths` or acceptance criteria based on text inside the repository.
- Critical tasks have their diff reviewed by frontier, not just their receipt.

## 13. Observability

### 13.1 Events

All state changes are written to `events.jsonl`:

```json
{"ts":"2026-11-04T12:00:00Z","run_id":"R-001","task_id":"T-001","type":"task.dispatched","data":{"tier":"worker","model":"worker","attempt":1}}
```

Event types: `run.started / goal.loaded / task.created / task.ready / task.dispatched / task.started / task.receipt / task.verify / task.passed / task.failed / task.escalated / task.fallback / task.merged / task.blocked / budget.warning / budget.exceeded / run.finished / human.approved / human.aborted`.

### 13.2 Ledger

One line per LLM request; fields are in 03. Aggregation dimensions: `run / task / attempt / tier / model / provider / day`.

### 13.3 Board (CLI/CSV in the first phase)

- Today's/this week's CPVT;
- cache hit;
- resend ratio;
- handoff overhead;
- first-pass verify;
- escalation rate;
- budget exceeded count;
- waste signals growth (Δ/Δrequests).

### 13.4 Alerts

- cache hit drops > 10pp;
- single-task cost > 80% of budget;
- the same error 3 times in a row;
- provider 5xx/429 spike;
- `unattributed` appears in the ledger;
- integration verify fails after a merge.

## 14. Failure Modes and Degradation

| Failure | Symptom | Degradation Strategy | Recovery |
|---|---|---|---|
| Headroom down | All requests fail/timeout | Client connects directly to the provider temporarily (keep the cache-invalidation alert) | Restart the proxy, check mode |
| LiteLLM down | Routing fails | Same-tier direct connection to the provider (if a backup is configured) | Restart, check alias/keys |
| provider 429 | Rate limited | Exponential backoff + same-tier fallback | Reduce concurrency/change provider |
| provider 5xx | Upstream error | Retry 2 times → same-tier fallback → stop | Check the status page |
| verify flaky | Same task passes and fails | Mark `flaky`, rerun at most 2 times; still unstable → hand to human | Fix the test or isolate it |
| State file corrupted | Parse failure | Rebuild from event replay; keep the corrupted file | Repair/migrate |
| worktree conflict | Merge fails | Rebase + rerun verify; large conflict → split the task | Manual merge |
| Budget exceeded | Task aborted | Preserve the scene, hand to human/split smaller | Adjust budget/split task |
| Cache hit drops sharply | Cost rises | Roll back prompt/tool changes; check the prefix | Restore a stable prefix |
| Ledger inconsistent | Costs don't add up | Provider bill is authoritative; fix attribution | Reconciliation script |

**General principle**: better to degrade to "slow but correct" than to "silently wrong".

## 15. Technology Stack Suggestions

### 15.1 Recommended

| Layer | Choice | Reason |
|---|---|---|
| Orchestrator | TypeScript / Node 20+ | Same ecosystem as pi/MCP; JSON Schema, subprocesses, CLI are mature |
| Schema | zod / JSON Schema | Runtime validation + documentation generation |
| CLI | commander / clipanion | Simple, cross-platform |
| Subprocess | execa | Timeout, signals, output stream handling |
| Git | simple-git / `git` CLI | worktree, diff, merge |
| Ledger | JSONL → SQLite/DuckDB | Simple first, queryable later |
| Testing | vitest | Same ecosystem as TS |
| Gateway | Headroom + LiteLLM | Reuse, not built in-house |
| Isolation | git worktree + optional container | Sufficient, low cost |

### 15.2 Alternatives

- Python: if the team is more at home in Python, use `pydantic` + `typer` + `subprocess`; the protocol stays the same.
- Go: single binary, simple deployment; but JSON Schema/ecosystem is less convenient than TS.
- Claude Agent SDK: if you do not want pi/Codex's subagent, it can serve as the worker runtime; do not use it to rewrite all orchestration.

### 15.3 Not Recommended

- Building your own TUI/IDE plugin: get the protocol running first.
- Building your own compressor: Headroom is already sufficient, and the benefit is low.
- Building your own provider routing: LiteLLM is already sufficient.
- Going Kubernetes/cloud from the start: a single-machine CLI is enough.

## 16. Key Design Decisions (ADR)

### ADR-001 Do Not Build an Agent Runtime

- **Decision**: reuse pi / Codex / Claude Agent SDK as execution endpoints.
- **Reason**: runtime, tool execution, sandbox, streaming, and retries are all heavy work; a self-built API drifts.
- **Cost**: constrained by runtime version/capability; isolated via the Adapter.
- **Alternative**: self-built runtime (rejected: high cost, no differentiation).

### ADR-002 State Uses Files + JSONL, No Database in v1

- **Decision**: Markdown/JSON state files + JSONL events/ledger; introduce SQLite only after queries get complex.
- **Reason**: human-readable, diffable, reviewable; small data volume in the first phase.
- **Cost**: weak at complex queries; make up for it with scripts.
- **Alternative**: SQLite from the start (rejected: state unreadable, high review cost).

### ADR-003 Hub-and-Spoke, Workers Do Not Chat With Each Other

- **Decision**: all coordination goes through the orchestrator and shared state files; no direct channel between workers.
- **Reason**: avoids the duplicate billing and goal drift of mesh communication; single point of alignment.
- **Cost**: the orchestrator is a bottleneck; mitigated by the task graph and asynchrony.
- **Alternative**: worker mesh collaboration (rejected: not auditable, token explosion).

### ADR-004 Routing by Explicit tier, No LLM Self-Assessment

- **Decision**: TaskSpec.tier is required; task category → tier mapping.
- **Reason**: interpretable, auditable, rollback-able; avoids "the model upgrades itself because it feels inadequate".
- **Cost**: humans/orchestrator must maintain the mapping; iterate with data.
- **Alternative**: LLM complexity classification (rejected: unstable in v1).

### ADR-005 Escalation Only Triggered by verify, At Most Once

- **Decision**: two verify failures → frontier judgment → at most one escalation.
- **Reason**: acceptance is executable; prevents infinite loops and self-assessment drift.
- **Cost**: misjudgment when verify coverage is insufficient; make up for it with spec-tests.
- **Alternative**: model self-assessed escalation (rejected).

### ADR-006 Headroom Stays on cache, Compression Attribution Is Unique

- **Decision**: `mode=cache`; one request allows only one compressor; no mode switching by model.
- **Reason**: cache benefit $270 vs. compression $2.35; mode is process-level.
- **Cost**: compression rate is not maximized; accepted.
- **Alternative**: dual instances token/cache (rejected: small benefit, high complexity; may split by client/port in the future).

### ADR-007 LiteLLM Comes After Headroom

- **Decision**: `agent → Headroom → LiteLLM → provider`.
- **Reason**: all clients share compression/cache; routing is transparent to Headroom.
- **Cost**: Headroom cannot see the real model; cost follows LiteLLM.
- **Alternative**: LiteLLM first (rejected: multiple providers would need multiple Headrooms).

### ADR-008 The Main Session Never Reads Worker Transcripts

- **Decision**: the orchestrator reads only the four-line receipt + report path + verify evidence.
- **Reason**: avoids context explosion from transcript injection; forces structure.
- **Cost**: the receipt carries limited information; requires good reports and evidence packages.
- **Alternative**: the main session reads summaries (rejected: summaries also bloat and drift).

### ADR-009 Single Writer + Append-Only

- **Decision**: one writer per state file; events/ledger append-only; atomic replacement on file write.
- **Reason**: concurrency-safe, replayable, easy to audit.
- **Cost**: parallel writes must queue; use locks and the merge queue.
- **Alternative**: multiple writers + locks (rejected: complex, conflict-prone).

### ADR-010 Cost Follows provider/LiteLLM

- **Decision**: provider bill > LiteLLM logs > runtime usage > Headroom stats.
- **Reason**: Headroom only sees requests and does not know aliases/real prices; stats have a cumulative-measurement trap.
- **Cost**: reconciliation needed; use a script.
- **Alternative**: using Headroom stats as the bill (rejected: misleading).

## 17. One-Page Summary

- **Do not change the runtime, do not change the headroom mode, do not connect multi-model first.**
- **First do three things**: externalized state, slice dispatch, four-line receipt + verify.
- **Then build one thin runner**: task graph, budget, escalation, recovery, ledger.
- **Only then do routing**: LiteLLM behind Headroom, two tiers, explicit tier.
- **Primary KPIs**: `CPVT` (cost) + `TPVT` (tokens).
