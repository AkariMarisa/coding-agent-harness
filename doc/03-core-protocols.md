# 03 Core Protocols

> Language: **English** | [简体中文](03-core-protocols.zh-CN.md)

> This document is Loom's "interface contract". Implementations may switch language or runtime, but protocol IDs and field semantics must stay stable. All **cross-boundary** JSON carries a `schema_version`, and all cross-boundary content must be validatable.

## 1. `.agent/` Directory Layout

```
.agent/
├── GOAL.md                 # goals, non-goals, overall acceptance, budget
├── spec.md                 # user requirements, scenarios, boundaries
├── design.md               # architecture, interfaces, data models, trade-offs
├── contracts/              # OpenAPI / JSON Schema / TS types, frozen interfaces
├── tasks.json              # task graph index (lightweight)
├── tasks/
│   └── T-20261104-001.json # a single TaskSpec
├── receipts/
│   └── T-20261104-001.json # structured receipt
├── reports/
│   ├── T-20261104-001.md   # human-readable report (details)
│   └── T-20261104-001.verify.log
├── PROGRESS.md             # append-only progress
├── DECISION.md             # decision log (ADR)
├── budget.json             # run/task default budget
├── env.lock                # version pinning
├── escalations/            # escalation evidence packages
├── locks/                  # path/file locks (transient)
├── runs/                   # per-run snapshots (ignored)
├── events.jsonl            # event stream (ignored)
└── ledger/
    └── requests.jsonl      # request ledger (ignored, append-only)
```

### 1.1 Committed vs. Ignored

> **Implementation status (`loom init`)**: the actual directory name is `.loom/`, and the root `.gitignore` uses a **whitelist** — the contract layer is tracked (`README.md`, `goal.md`, `constraints.md`, `specs/`), everything else is ignored (`/.loom/*` plus negations; new state is ignored by default). In the table below, the "commit" items such as `tasks/` and `receipts/` are design goals; the current implementation still places them in the state layer; see later milestones for auditability.

| Path | git | Reason |
|---|---|---|
| `GOAL.md` / `spec.md` / `design.md` | Commit | Human-readable, reviewable |
| `contracts/` | Commit | Interface contract, shared by workers |
| `tasks.json` / `tasks/` | Commit | Auditable, replayable |
| `receipts/` / `reports/*.md` | Commit | Acceptance evidence |
| `PROGRESS.md` / `DECISION.md` | Commit | Decisions and progress |
| `budget.json` / `env.lock` | Commit | Reproducible |
| `runs/` / `locks/` / `*.log` | Ignore | Large/transient |
| `events.jsonl` / `ledger/requests.jsonl` | Ignore | High-frequency writes; archive when needed |
| Keys | Forbidden | Security |

## 2. Human-Readable File Templates

### 2.1 `GOAL.md`

```markdown
# GOAL G-20261104-auth

## Goal (one sentence)
Add username/password login to the existing API, with session support and regression tests.

## Context and Value
Registration exists but login does not; the front end cannot close the authentication loop.

## Non-Goals
- No OAuth / SSO
- No password recovery
- No database schema changes (reuse the existing users table)

## Overall Acceptance (executable)
- [ ] `pnpm test` all green
- [ ] `pnpm tsc --noEmit` passes
- [ ] Manual curl login succeeds and returns a session cookie

## Constraints
- No new runtime dependencies
- Follow the existing error response format
- All changes under `src/api/auth/**`, `tests/api/auth/**`

## Budget
- Total cost ≤ $5
- Total wall clock ≤ 4 hours
- At most 12 tasks

## Decision Maker / Escalation Contact
- Decision maker: @akari
- Escalation conditions: 2 consecutive verify failures, spec conflict, budget exceeded

## Status
active
```

### 2.2 `spec.md`

```markdown
# SPEC

## User Story
As a registered user, I can log in with username/password and receive a session.

## Acceptance Scenarios
### S1 Successful login
Given a registered user
When POST /api/auth/login with correct credentials
Then 200, Set-Cookie contains the session

### S2 Wrong password
Given a registered user
When logging in with a wrong password
Then 401, error code AUTH_INVALID_CREDENTIALS

### S3 Missing field
When password is missing
Then 400, error code VALIDATION_ERROR

## Boundaries and Not Doing
- No rate limiting (a later task)
- No multi-device session management

## Open Questions
- Should the session expiry use the existing configuration or a new one?
```

### 2.3 `design.md`

```markdown
# DESIGN

## Architecture
POST /api/auth/login → AuthController → AuthService → UserRepository → SessionStore

## Interfaces
See `.agent/contracts/auth.openapi.yaml`

## Data Model
Reuse the `users` table; sessions live in the existing `sessions` table.

## Error Handling
Unified `{ code, message, requestId }`; error codes are in contracts.

## Test Strategy
- Unit: AuthService validation
- Integration: supertest against the real routes
- Regression: run all existing auth tests

## Trade-offs
- No ORM: reuse the existing repository
- No new dependencies: use the existing crypto/session libraries

## Risks
- The existing session middleware may carry implicit assumptions; read `src/middleware/session.ts` first
```

### 2.4 `PROGRESS.md`

Append-only, never rewrite history:

```markdown
# PROGRESS

| Time | Task | Status | Evidence |
|---|---|---|---|
| 2026-11-04T12:10Z | T-001 | passed | `.agent/receipts/T-001.json` |
| 2026-11-04T12:40Z | T-002 | failed → escalated | `.agent/escalations/T-002/` |
```

### 2.5 `DECISION.md`

```markdown
## D-20261104-001 Reuse the existing session store

- Date: 2026-11-04
- Status: accepted
- Context: login needs a session; a sessions table and middleware already exist
- Options:
  1. Reuse the existing store (selected)
  2. New Redis session (rejected: new dependency/operations)
- Rationale: smallest change, consistent with the existing middleware
- Impact: session TTL follows the existing configuration
- Reversibility: high
- Superseded by: none
```

## 3. Task Graph `tasks.json`

```json
{
  "schema_version": "1.0",
  "goal_id": "G-20261104-auth",
  "updated_at": "2026-11-04T12:00:00Z",
  "tasks": [
    {
      "task_id": "T-20261104-001",
      "title": "Freeze the auth API contract",
      "status": "merged",
      "tier": "frontier",
      "depends_on": [],
      "spec": ".agent/tasks/T-20261104-001.json"
    },
    {
      "task_id": "T-20261104-002",
      "title": "Implement POST /api/auth/login",
      "status": "ready",
      "tier": "worker",
      "depends_on": ["T-20261104-001"],
      "spec": ".agent/tasks/T-20261104-002.json"
    },
    {
      "task_id": "T-20261104-003",
      "title": "Add login integration tests",
      "status": "pending",
      "tier": "worker",
      "depends_on": ["T-20261104-002"],
      "spec": ".agent/tasks/T-20261104-003.json"
    }
  ]
}
```

**Rules**:

- `tasks.json` holds only the index; details live in `tasks/<id>.json`.
- `status` is maintained by the runner; humans do not edit it directly (except manual cancellation).
- `depends_on` must reference tasks within the same goal; circular dependencies are rejected.
- One `spec` file per task; paths are not reused.

## 4. TaskSpec

> External Schema: [`schemas/task-spec.v1.json`](../schemas/task-spec.v1.json); the implementation is `loom-core::TaskSpec`, written to `.loom/tasks/<id>.json`, indexed by `.loom/tasks.json` (generated by event replay, not the source of truth).

### 4.1 Fields

| Field | Type | Required | Description |
|---|---|---|---|
| `schema_version` | string | Yes | Protocol version, e.g. `1.0` |
| `task_id` | string | Yes | `T-YYYYMMDD-NNN`, globally unique |
| `goal_id` | string | Yes | Associated GOAL |
| `title` | string | Yes | One sentence, ≤ 80 characters |
| `role` | enum | Yes | `survey / spec-tests / implement / review / judgment / docs` |
| `tier` | enum | Yes | `frontier / worker` |
| `depends_on` | string[] | Yes | Dependent task IDs; may be empty |
| `workspace` | object | Yes | worktree/branch/path permissions |
| `inputs` | object[] | Yes | File references; not content |
| `constraints` | object | Yes | Style, dependencies, maximum changes |
| `acceptance` | object[] | Yes | At least 1 verify entry |
| `budget` | object | Yes | token/cost/step/wall-clock |
| `output` | object | Yes | Receipt/report/artifacts |
| `context_policy` | object | No | Read policy and caps |
| `notes` | string | No | ≤ 500 characters; must not substitute for the spec |

### 4.2 Example

```json
{
  "schema_version": "1.0",
  "task_id": "T-20261104-002",
  "goal_id": "G-20261104-auth",
  "title": "Implement POST /api/auth/login",
  "role": "implement",
  "tier": "worker",
  "depends_on": ["T-20261104-001"],
  "workspace": {
    "mode": "worktree",
    "base_ref": "main",
    "branch": "agent/T-20261104-002",
    "allowed_paths": ["src/api/auth/**", "tests/api/auth/**"],
    "forbidden_paths": ["migrations/**", "package.json", "pnpm-lock.yaml"]
  },
  "inputs": [
    {"path": ".agent/contracts/auth.openapi.yaml", "sha256": "9f2c…", "must_read": true},
    {"path": ".agent/design.md", "sha256": "a71e…", "must_read": true},
    {"path": "src/middleware/session.ts", "range": [1, 200], "must_read": true},
    {"path": "src/api/routes.ts", "range": [1, 160], "must_read": false}
  ],
  "constraints": {
    "max_files_touched": 8,
    "no_new_dependencies": true,
    "style": ["follow existing error envelope", "reuse session store"],
    "notes": "Do not touch the users table; no new ORM"
  },
  "acceptance": [
    {"id": "A1", "cmd": "pnpm vitest run tests/api/auth/login.test.ts", "expect_exit": 0, "timeout_s": 300},
    {"id": "A2", "cmd": "pnpm tsc --noEmit", "expect_exit": 0, "timeout_s": 300},
    {"id": "A3", "cmd": "pnpm lint --max-warnings=0", "expect_exit": 0, "timeout_s": 300}
  ],
  "budget": {
    "max_input_tokens": 120000,
    "max_output_tokens": 20000,
    "max_steps": 25,
    "max_wall_s": 1800,
    "max_cost_usd": 0.25
  },
  "output": {
    "receipt": "4-line",
    "report_path": ".agent/reports/T-20261104-002.md",
    "required_artifacts": ["git_diff", "verify_log"]
  },
  "context_policy": {
    "max_must_read_files": 5,
    "max_must_read_tokens": 40000,
    "allow_extra_reads": true,
    "forbid_full_repo_scan": true
  }
}
```

### 4.3 Rules

- `inputs` carries references only; across a boundary it must include `sha256`; large files use `range`.
- `must_read` defaults to ≤ 5 files / ≤ 40k tokens; exceeding it means the slice was split badly.
- `allowed_paths` is a whitelist; `forbidden_paths` has higher priority.
- `acceptance` has at least 1 entry; each entry must be non-interactive, repeatable, and have a timeout.
- `budget` is required; `max_cost_usd` may be an estimate, corrected against actual usage during execution.
- `notes` are not requirements; requirements live only in spec/design/contracts.
- The worker must not modify the TaskSpec; when a change is needed, return `needs_input` and the orchestrator rewrites it.

## 5. Receipt (Four-Line Receipt)

### 5.1 Text Format

At the end, the worker **outputs only** the following four lines, in a fixed order:

```
status: done
branch: agent/T-20261104-002@7f3c2a1
verify: A1=pass,A2=pass,A3=pass; log=.agent/reports/T-20261104-002.verify.log
report: .agent/reports/T-20261104-002.md
```

| Field | Values | Rules |
|---|---|---|
| `status` | `done / blocked / needs_input / failed / budget_exceeded` | Required; `done` does not mean verify passed, only that the worker finished |
| `branch` | `<branch>@<sha>` | Must be this task's branch; sha is HEAD |
| `verify` | `<id>=pass/fail/skip,...; log=<path>` | Write only the result and log path; do not paste the log |
| `report` | `<path>` | Human-readable details; ≤ 1 line |

### 5.2 Parsing Rules

- Exactly 4 lines; a trailing `\r` is allowed; blank lines, code blocks, preambles, and postscripts are not.
- Field names must be lowercase; the separator is `: `.
- Every acceptance id in `verify` must appear; a missing one counts as `fail`.
- The `branch` sha must match the worktree HEAD; a mismatch is rejected.
- More than 100 tokens or more than 4 lines → parse failure, task marked `failed`.
- Natural language is not parsed; if the worker wants to explain, it writes the report.

### 5.3 Structured Receipt

After parsing the four lines, the runner generates `receipts/<task_id>.json` (the implementation writes `.loom/runs/<run>/outbox/receipt.json`, which `loom verify` overwrites as the authoritative version):

> External Schema: [`schemas/receipt.v1.json`](../schemas/receipt.v1.json); the implementation is `loom-core::Receipt`.

```json
{
  "schema_version": "1.0",
  "task_id": "T-20261104-002",
  "attempt": 1,
  "status": "done",
  "base_sha": "1a2b3c4",
  "head_sha": "7f3c2a1",
  "branch": "agent/T-20261104-002",
  "diffstat": {"files": 3, "insertions": 128, "deletions": 12},
  "verify": [
    {"id": "A1", "cmd": "pnpm vitest run tests/api/auth/login.test.ts", "exit": 0, "duration_s": 14, "log_path": ".agent/reports/T-20261104-002.verify.log", "result": "pass"},
    {"id": "A2", "cmd": "pnpm tsc --noEmit", "exit": 0, "duration_s": 22, "log_path": ".agent/reports/T-20261104-002.verify.log", "result": "pass"},
    {"id": "A3", "cmd": "pnpm lint --max-warnings=0", "exit": 0, "duration_s": 9, "log_path": ".agent/reports/T-20261104-002.verify.log", "result": "pass"}
  ],
  "artifacts": [
    {"path": "src/api/auth/login.ts", "sha256": "c0ff…", "bytes": 4210},
    {"path": "tests/api/auth/login.test.ts", "sha256": "b1ad…", "bytes": 2870}
  ],
  "tokens": {
    "input": 48210,
    "cached_input": 32000,
    "output": 5120,
    "reasoning": 1800,
    "cost_usd": 0.031
  },
  "decisions": [{"id": "D-20261104-001", "summary": "Reuse the existing session store"}],
  "blockers": [],
  "next_hint": "",
  "report_path": ".agent/reports/T-20261104-002.md"
}
```

### 5.4 Status Semantics

| status | Meaning | Orchestrator action |
|---|---|---|
| `done` | The worker believes it is finished; verify has been run | Rerun verify independently; only then `passed` |
| `blocked` | Blocked by environment/dependencies/permissions | Check locks/environment; may redispatch or hand to human |
| `needs_input` | Spec unclear/additional references needed | Supplement the spec or append `inputs`, then redispatch |
| `failed` | Gave up / cannot complete | Retry once in the same tier → escalate |
| `budget_exceeded` | Aborted after exceeding the budget | No automatic retry; split smaller or hand to human |

> **Do not treat the worker's `status=done` as acceptance.** The only acceptance is the result of the runner rerunning acceptance.

## 6. Verify Contract

### 6.1 Basic Requirements

- Non-interactive: no reading stdin, no popups, no human confirmation.
- Repeatable: repeated runs on the same code/environment give the same result.
- Has a timeout: every command must carry `timeout_s`.
- Observable: stdout/stderr go to a log file; the receipt carries only the path.
- Isolated: executed at the task worktree root; no dependence on uncommitted changes in the main working tree.
- Exit code: `0 = pass`; non-0 = fail; timeout = fail.

### 6.2 Evidence

The runner records the following for each acceptance:

```json
{
  "id": "A1",
  "cmd": "pnpm vitest run tests/api/auth/login.test.ts",
  "cwd": "/worktrees/T-20261104-002",
  "exit": 0,
  "signal": null,
  "duration_s": 14,
  "timeout_s": 300,
  "log_path": ".agent/reports/T-20261104-002.verify.log",
  "log_sha256": "e3b0…",
  "git_sha": "7f3c2a1",
  "env_hash": "sha256:…",
  "result": "pass"
}
```

### 6.3 Acceptance Types

| Type | Example | Evidence |
|---|---|---|
| Unit/integration tests | `pnpm vitest run ...` | exit code + log |
| Type check | `pnpm tsc --noEmit` | exit code + log |
| Lint/format | `pnpm lint` | exit code + log |
| Build | `pnpm build` | exit code + artifact |
| Interface contract | `spectral lint openapi.yaml` | exit code |
| Performance | `node bench.js --max-p95=200` | output + threshold |
| Visual | Playwright screenshots | image + hash + diff |
| Documentation | markdownlint / link check | exit code |
| Manual acceptance | Scenarios that cannot be automated | human sign-off + evidence path |

### 6.4 Anti-Patterns

- `verify: take a manual look` — not executable, reject.
- `verify: run the relevant tests` — nondeterministic, reject.
- `verify: retry if it fails` — nondeterministic, reject.
- `verify: continue if it passes` — no command, reject.
- Letting the worker decide verify commands on the spot — reject; commands may only come from the TaskSpec.

### 6.5 Flaky Handling

- Known flaky: the TaskSpec marks `flaky: true`, rerun at most 2 times.
- Still unstable: the task goes to `human`, and the flaky evidence is recorded.
- Do not hide unstable tests behind "rerun until green"; consecutive flakiness requires a fix task.

## 7. Handoff Rules (Cross-Boundary Communication)

### 7.1 Allowed Transfers

| Content | Form | Limit |
|---|---|---|
| TaskSpec | JSON | ≤ 2k tokens (excluding references) |
| File reference | path + sha256 + range | ≤ 1 line per reference |
| diff | patch file path + stat | never paste the full text |
| verify evidence | result + log path + sha | never paste the log |
| Decision | Decision ID + one sentence + path | ≤ 100 tokens |
| report | path | never paste content |
| Interface contract | `contracts/` path | never paste full text |

### 7.2 Forbidden Transfers

- Full file contents (unless it is `must_read` and genuinely needed; use a reference so the worker reads it itself).
- Raw JSON / HTML / base64 / large logs.
- The worker's chain of thought, full transcript, previous round of conversation.
- Unverified conclusions ("should work", "I guess").
- Keys, tokens, cookies, connection strings.
- Another worker's process logs (only structured receipts/decisions are allowed).

### 7.3 Communication Budget

| Channel | Budget | On Overflow |
|---|---|---|
| orchestrator → worker | ≤ 2k tokens | Split the task / reduce references |
| worker → orchestrator | ≤ 100 tokens (four lines) | Write the report, do not paste back |
| judgment evidence package | ≤ 20k tokens | Only necessary log snippets + paths |
| Manual escalation note | ≤ 500 characters | Attach evidence paths |

### 7.4 Alignment Mechanisms

- **Single source of truth**: interfaces follow `contracts/`, requirements follow `spec.md`, design follows `design.md`.
- **Frozen interfaces**: before a cross-module task starts, there is first a contract task; contract changes go through a design task, not chat.
- **Task graph dedup**: tasks with the same input hashes + the same output paths do not run in parallel.
- **Path ownership**: only one writer per path; when collaboration is needed, split into "write the interface / write the implementation / write the tests".
- **No worker-to-worker chat**: when coordination is needed, the orchestrator creates a coordination task or updates the shared files.

## 8. Escalation Protocol

### 8.1 Trigger Conditions

Escalation is allowed only in the following situations:

1. The same task's verify fails 2 times (first run + 1 same-tier retry).
2. `status=needs_input` and the orchestrator cannot fill the gap from the existing spec/design.
3. Path/interface conflict that cannot be merged automatically.
4. Budget exceeded, but the task is valuable and a human must decide whether to raise the budget.
5. Security/compliance risk.

**Not allowed**: the model self-assessing "this one is hard", the model self-assessing "needs a stronger model", an unevidenced "there might be a problem".

> If the worker proactively returns `failed` without verify evidence, treat it as `verify_fail`: allow one same-tier retry; if there is still no evidence, go to `human`.

### 8.2 Evidence Package

```
.agent/escalations/T-20261104-002/
├── request.md          # pending question, ≤ 500 characters
├── task.json           # TaskSpec snapshot
├── receipt.json        # latest receipt
├── diff.patch          # code changes
├── verify.log          # failure log (may be truncated)
├── decisions.md        # related decision references
└── refs.json           # file references + hashes
```

### 8.3 judgment Output

```json
{
  "schema_version": "1.0",
  "escalation_id": "E-20261104-002",
  "decision": "fix_plan",
  "plan": [
    "First add type definitions for the session middleware",
    "Then reuse the existing error envelope; no new error codes"
  ],
  "refs": [".agent/design.md", "src/middleware/session.ts"],
  "max_attempts": 1,
  "human_required": false,
  "evidence_gaps": ["current value of the session TTL config is missing"]
}
```

`decision` enum:

| Value | Meaning | Follow-up |
|---|---|---|
| `fix_plan` | There is a clear fix plan | Create a worker task with the plan + evidence references |
| `split` | Task too large/too vague | Split into multiple TaskSpecs |
| `reject` | Approach infeasible / requirements conflict | Back to spec/design, involve a human if necessary |
| `human` | A human decision is needed | Stop the automated flow, send an escalation notice |

### 8.4 Limits

- Each task escalates at most 1 time.
- Each goal allows at most 3 automatic escalations; beyond that, hand to human.
- Still failing after escalation → `human`, no looping.
- All escalations are recorded in `DECISION.md` and `events.jsonl`.

## 9. Budget and Circuit Breaker

### 9.1 `budget.json`

```json
{
  "schema_version": "1.0",
  "run": {
    "max_cost_usd": 5.0,
    "max_input_tokens": 5000000,
    "max_output_tokens": 500000,
    "max_wall_s": 14400,
    "max_tasks": 40,
    "max_escalations": 3
  },
  "defaults": {
    "task": {
      "max_input_tokens": 120000,
      "max_output_tokens": 20000,
      "max_steps": 25,
      "max_wall_s": 1800,
      "max_cost_usd": 0.25
    }
  },
  "tiers": {
    "frontier": {"max_cost_usd_per_task": 1.0},
    "worker": {"max_cost_usd_per_task": 0.25}
  }
}
```

### 9.2 Enforcement Points

| Timing | Check | Action |
|---|---|---|
| Before dispatch | Estimate input/output/cost | Over the cap → refuse dispatch, hand to human / split the task |
| During execution | Cumulative usage stream | 80% → alert; 100% → stop immediately |
| After receipt | Reconcile actual usage | Over the cap → mark `budget_exceeded` |
| Run end | Total cost / task count | Generate a report; alert on overrun |

> If the runtime does not provide real-time usage: estimate from historical averages before dispatch and reserve a buffer; fall back to step count / wall clock during execution; correct against actual usage after the receipt. When real-time circuit breaking is impossible, better to conservatively underestimate the budget cap than to discover an overrun after the fact.

### 9.2.1 Definition of Task-Level `max_input_tokens`

`TaskSpec.budget.max_input_tokens` uses a **cumulative-per-run definition**: the `input_tokens` of every request across the whole run are summed and compared against the cap (the `usage.input_tokens > config.max_input_tokens` check at the end of `loom-native/src/run.rs`), **not** a per-request cap. Single-request size is governed by Context Budget (§7, worker 24k); do not mix the two.

- The repo provides no default; the task author fills it in. Measured sample: a 9-step task accumulated 89,557 with a per-step peak of 14,318 — estimating as "per-step peak × expected steps × 1.3" is fairly stable.
- After hitting the cap the claim records `budget_exceeded` → verify keeps that claim → the task status becomes `blocked` → **merge rejects it** (only `passed` is accepted), even if every acceptance passed. Handling: raise `max_input_tokens` and dispatch again (the worktree is reused; existing changes are not lost).
- Not yet added to `task add` validation: the definition has just been recorded; wait for enough samples before setting a warning threshold.

### 9.3 No-Progress Circuit Breaker

- The same task with no file changes / no verify progress for 3 consecutive rounds → stop.
- The same error appearing 3 times in a row → stop, escalate.
- The same file repeatedly reverted → stop, check for spec conflicts.
- After tripping, keep the worktree, logs, and receipt; no automatic rerun.

## 10. Error Classification

| code | Meaning | Retryable | Default Action |
|---|---|---|---|
| `verify_fail` | Acceptance command exited non-0 | Yes (1 time) | Same-tier retry → escalate |
| `timeout` | Wall-clock/command timeout | Yes (depends on cause) | Retry / split the task |
| `budget_exceeded` | Over token/cost/steps | No | Human / split smaller |
| `tool_error` | Tool crash/permission/missing dependency | Yes (1 time) | Fix environment / redispatch |
| `conflict` | Path/merge conflict | No | Human / reschedule |
| `ambiguous_spec` | Spec unclear | No | Supplement spec / escalate |
| `env_error` | Environment/version/network problem | Yes | Fix environment |
| `flaky_verify` | Unstable test | Yes (≤2 times) | Mark / fix the test |
| `security_blocked` | Command/path/key violation | No | Human audit |
| `provider_error` | 429/5xx/authentication | Yes | Backoff / same-tier fallback |
| `parse_error` | Receipt/schema invalid | Yes (1 time) | Redispatch / fix the template |
| `unknown` | Unclassified | No | Preserve the scene, human |

**Principle**: errors must be classified before entering the ledger; an `unknown` ratio > 10% means the protocol or tools need fixing.

## 11. Event Protocol

### 11.1 Event Format

> External Schema: [`schemas/event.v1.json`](../schemas/event.v1.json); the implementation is `loom-core::Event`, written to `.loom/events.jsonl` (append-only).

```json
{
  "v": 1,
  "ts_ms": 1793784062000,
  "run_id": "R-20261104-001",
  "task_id": "T-20261104-002",
  "type": "task.dispatched",
  "attempt": 1,
  "data": {"tier": "worker", "model": "worker", "worktree": "/worktrees/T-20261104-002"}
}
```

### 11.2 Event Types

| Type | Key `data` fields |
|---|---|
| `run.started` | `goal_id`, `budget` |
| `goal.loaded` | `goal_path`, `spec_path`, `design_path` |
| `task.created` | `spec_path`, `depends_on` |
| `task.ready` | `deps_done` |
| `task.dispatched` | `tier`, `model`, `worktree`, `budget` |
| `task.started` | `pid`, `started_at` |
| `task.receipt` | `status`, `branch`, `head_sha` |
| `task.verify` | `acceptance_id`, `result`, `exit`, `log_path` |
| `task.passed` | `verify_ids` |
| `task.failed` | `code`, `verify_ids` |
| `task.escalated` | `escalation_id`, `decision` |
| `task.fallback` | `from`, `to`, `tier`, `reason`, `exit`, `timed_out`, `switch` |
| `task.merged` | `merge_sha`, `integration_verify` |
| `task.blocked` | `code`, `lock_owner` (optional `lock_run`/`lock_path`/`detail`; when `code=parallel_cap` it is `running`/`max_running`) |
| `task.needs_input` | `code` (`needs_input`), `verify_ids`; means the adapter is waiting for a human answer, not a failure |
| `budget.warning` | `used`, `limit` |
| `budget.exceeded` | `used`, `limit`, `scope` |
| `human.approved` | `by`, `scope` |
| `human.aborted` | `by`, `reason` |
| `run.finished` | `tasks`, `cost`, `cpvt` |

### 11.3 Write Rules

- Append-only; never modify history.
- A failed event write does not block the main flow, but it must raise an alert.
- Every event has a `run_id`; tasks spanning runs use `parent_run_id`.
- Sensitive fields are redacted before writing.
- Task state is derived by replaying events (`loom-core::status_after_event`): `task.created→pending`, `task.ready→ready`, `task.dispatched→dispatched`, `task.started→running`, `task.receipt→receipt`, `task.passed→passed`, `task.failed→failed`, `task.blocked→blocked`, `task.needs_input→needs_input`, `task.escalated→escalated`, `task.merged→merged`, `human.aborted→cancelled`; `task.verify` / `task.fallback` / budget events do not change state. `needs_input` / `blocked` are not failures: they can be dispatched again, and they **do not count** toward M3's `task.failed` escalation count; only an acceptance failure or an explicit adapter failure writes `task.failed`. `tasks.json` is just a view of this replay result.
- `task.fallback` means a **same-tier model switch and rerun**: the `run_id` is the run of the new attempt, and `parent_run_id` points to the failed run; `attempt` does not change, so the escalation threshold (counting `task.failed`) is not polluted by the switch. Cross-tier model switches are triggered only by escalation, never by fallback.
- Dependencies are considered satisfied only after `passed` / `merged`; circular dependencies and cross-goal dependencies are rejected when the index is written.

## 12. Ledger Protocol

### 12.1 Request-Level Records

The implementation writes `.loom/ledger/requests.jsonl` (append-only; on read it filters by run/task/tier/model and recomputes aggregates instead of storing them separately).  
External Schema: [`schemas/usage-record.v1.json`](../schemas/usage-record.v1.json).

```json
{
  "v": 1,
  "ts_ms": 1793784062000,
  "run_id": "R-20261104-001",
  "task_id": "T-20261104-002",
  "attempt": 1,
  "tier": "worker",
  "model_alias": "worker",
  "model_id": "deepseek-v4-flash",
  "provider": "deepseek",
  "wire": "chat",
  "input_tokens": 48210,
  "cached_input_tokens": 32000,
  "output_tokens": 5120,
  "reasoning_tokens": 1800,
  "cost_usd": 0.031,
  "cost_source": "provider",
  "source": "litellm",
  "latency_ms": 13900,
  "status": 200,
  "step": 7,
  "tool_names": ["read", "edit"],
  "write_paths": [],
  "rewrite_count": 0,
  "history_bytes": 182400,
  "projected_bytes": 96400
}
```

`step` and the six fields after it are optional incremental fields (doc/16 §8.4):

- `step`: the 1-based step number within the run; omitted for one-shot requests;
- `tool_names` / `write_paths`: the tools the model called in this step and the paths of whole-file writes (`edit` does not count);
- `rewrite_count`: how many times this step overwrote already-written files wholesale;
- `history_bytes` / `projected_bytes`: serialized history bytes before and after projection; equality means this step did not project;
- These are **attribution and estimation** inputs; the bill still follows `input_tokens` / `cost_usd`.

### 12.2 Attribution Rules

- Every request must have `run_id` + `task_id`; missing ones go to `unattributed` and raise an alert.
- `input_tokens` is the normalized total input **including cached**; the adapter must first normalize the provider's "cache reads billed separately" definition. Rows with `cached_input_tokens > input_tokens` are rejected from the ledger.
- `source` priority: `provider > litellm > runtime > headroom > manual`.
- `cost_source` must be preserved: `provider` (the bill) takes precedence over `pricing_table` (local capability-bit estimate); a local estimate must never overwrite an existing provider cost. When `cost_usd` is non-null, `cost_source=unknown` is forbidden; when the CLI only passes `--cost-usd`, it is recorded as `manual` by default.
- `cost_usd` is computed from the provider price table; the price table version is written to `env.lock`. When the price is unknown, keep `null` + `cost_source=unknown`; do not guess.
- Cached tokens are recorded separately; do not treat cached as free, and do not apply the uncached price to cached.
- Each retry/fallback gets its own row; do not merge them into one row.
- Capability bits (`tier` / `wire` / `model_id`) are recorded with the row, so reconciliation is not done on the alias alone.
- Step-level attribution is only filled in controlled loops like native; external adapters legitimately have no `step` — do not fake it as 1. Rows with `history_bytes < projected_bytes` are rejected from the ledger.

Common commands:

```bash
loom --run R-20261104-001 usage record \
  --task T-20261104-002 --attempt 1 --tier worker --model worker \
  --input-tokens 48210 --cached-input-tokens 32000 --output-tokens 5120 \
  --reasoning-tokens 1800 --cost-usd 0.031 --cost-source provider \
  --source litellm --latency-ms 13900 --status 200

loom --run R-20261104-001 ledger --passed-tasks 10 --wall-seconds 1800

# step-level churn: read .loom/runs/*/logs/churn.json (estimate, not billing)
loom churn stats --by model --format pretty
```

`loom ledger`'s `cpvt`/`tpvt`/`wpt` all require `--passed-tasks`; `wpt` also requires `--wall-seconds`. As long as the filtered scope contains unattributed rows, unknown-cost rows, or unknown-token rows, `cpvt`/`tpvt` return `n/a`, so that an incomplete ledger does not produce a fake baseline.

### 12.3 Reconciliation

- Reconcile daily/weekly against the provider bill; investigate differences > 5%.
- Headroom stats are for diagnosis only: request count, compression rate, waste signals, latency.
- Do not run A/B on lifetime cumulative values; use "Δ value / Δ requests".
- The ledger keeps raw rows; aggregates can be recomputed.
- `loom capability check` catches capability-bit conflicts and price gaps before dispatch; do not wait until the ledger is missing costs.

## 13. Locks and Concurrency

### 13.1 Lock Files

The file name is the first 12 hex digits of `sha256(path)` + `.lock` (paths contain `/` and cannot be used directly as file names), placed in `.loom/locks/`:

```json
{
  "v": 1,
  "path": "src/api/auth",
  "mode": "write",
  "owner_task": "T-20261104-002",
  "owner_run": "R-20261104-093000-T-20261104-002",
  "owner_pid": 12345,
  "lease_until_ms": 1793785800000
}
```

`lease_until_ms` is Unix milliseconds; `owner_run` keeps a new holder of the same task (verify after dispatch) from being released by a stale guard.

### 13.2 Rules

- Write locks lock the patterns of `workspace.allowed_paths`; if the static prefixes of two patterns contain each other, it counts as a conflict (conservative over-approximation: `src/**` conflicts with `src/api/**`, `src/**` does not conflict with `migrations/**`, `**` or `*.sql` conflict with all patterns).
- Read locks are shared; write locks are exclusive. **v1 implements write locks only**; read locks come later.
- A lock can be reclaimed once its owner process disappears; if the lease expired but the owner is still alive the conflict remains, reported by `loom doctor` for manual handling.
- Failing to acquire a lock → write `task.blocked(code=lock_owner)`, never write forcibly; when the number of tasks holding locks exceeds `[parallel] max_running` → `task.blocked(code=parallel_cap)`; when the sum of concurrent tasks' `budget.max_cost_usd` exceeds `[parallel] max_inflight_cost_usd` → `task.blocked(code=inflight_budget)`. All three are retryable.
- Locks are best-effort advisory: they scan before writing, but a TOCTOU window still exists; the no-lost-update guarantee comes from per-task worktrees and the serialized merge queue.
- Locks are cleaned up when the guard releases (including the error path), and only the paths that guard wrote are released (nested guards do not release each other); leftovers are reclaimed by `loom doctor`.
- The merge queue itself holds a global write lock (the `**` pattern, conflicting with all write sets); dispatch of write tasks pauses during a merge. The `loom merge` order: global lock → rerun acceptance before merge → base clean-tree check → `git rebase` (on conflict abort and `task.blocked(code=merge_conflict)`) → `git merge` → rerun acceptance on the base checkout (on failure roll back with `reset --hard` and `task.blocked(code=integration_verify)`) → `task.merged(merge_sha, base_before, integration_verify)` → clean up the worktree and branch. In-place tasks do not enter the queue (their changes are already on the base branch).

## 14. Versions and Migration

### 14.1 Version Numbers

- Protocol version: `schema_version`, semver.
- Component versions: `.agent/env.lock`.
- Prompt versions: `prompts/<role>@<version>.md`; the version actually used is written to the event.
- Price table version: `pricing@YYYY-MM-DD`; written to the ledger.

### 14.2 Compatibility Strategy

- On read, validate `schema_version` first; incompatible data is rejected outright, never guessed.
- minor upgrades stay backward compatible; major upgrades ship migration scripts.
- Migration scripts must support dry-run and keep backups.
- Old events/ledger rows are never rewritten; the query layer does compatibility mapping.

### 14.3 Upgrade Window

- Before upgrading a key component (pi / codex / headroom / litellm / plugins):
  1. Record the current versions and baseline;
  2. Run the compatibility checklist (see 07);
  3. Canary with a small task;
  4. Observe CPVT / cache hit / error rate for 1 day;
  5. Then roll out fully.
- On a failed upgrade, roll back version + configuration; do not upgrade in the middle of a production run.

## 15. End-to-End Example

Goal: add login to the existing API.

### 15.1 Task Graph

| Task | Role | tier | Dependencies | Acceptance |
|---|---|---|---|---|
| T-001 Freeze the auth contract | spec-tests | frontier | — | OpenAPI lint + human review |
| T-002 Implement login | implement | worker | T-001 | Unit tests + tsc + lint |
| T-003 Add integration tests | spec-tests | worker | T-002 | Integration tests + tsc |
| T-004 Security review | review | frontier | T-003 | checklist + diff review |

### 15.2 Flow

1. `loom run G-20261104-auth`:
   - read GOAL/spec/design;
   - validate the task graph;
   - create the run directory and ledger.
2. T-001 dispatched to frontier:
   - read spec/design;
   - write `contracts/auth.openapi.yaml`;
   - run `spectral lint`;
   - four-line receipt.
3. The runner reruns lint; pass → merge; update PROGRESS.
4. T-002 dispatched to worker:
   - read contracts + design + the session middleware;
   - implement in the worktree;
   - run A1/A2/A3;
   - four-line receipt.
5. The runner reruns A1/A2/A3; pass → merge.
6. T-003 is similar; T-004 is a frontier review of diff + checklist.
7. Finally run the integration verify and generate the run report:
   - task count, pass rate, CPVT, cache hit, escalation count, human intervention count.

### 15.3 Failure Paths

- T-002 A1 fails:
  - same-tier retry 1 time (with the failure log path);
  - still failing → evidence package → frontier judgment;
  - judgment outputs `fix_plan` → new worker task (with the plan);
  - still failing → `human`.
- T-002 exceeds its budget:
  - stop; keep the worktree;
  - the human decides to split the task / raise the budget / abandon it.

## 16. Anti-Pattern List

| Anti-pattern | Consequence | Correct Practice |
|---|---|---|
| Pasting a whole conversation to a sub-agent | Duplicate billing, goal drift | TaskSpec + file references |
| Sub-agent pasting back the full diff/log | Main context explosion | Four-line receipt + report path |
| Treating "I'm done" as acceptance | False passes, rework | Runner reruns verify |
| Escalating because the model self-assesses "this is hard" | Escalation abuse, runaway cost | Escalate only on verify failure |
| Several agents editing the same file at once | Conflicts, overwrites | Path lock + single writer |
| Changing the system prompt/tools mid-run | Cache invalidation | Stable prefix within a run |
| Running A/B on lifetime cumulative values | Wrong conclusions | Δ value / Δ requests |
| Treating Headroom stats as the bill | Cost misjudgment | provider/LiteLLM are authoritative |
| Switching to token mode for compression rate | Lost cache, higher cost | Stay on cache |
| Dispatching a task with no verify | Cannot be accepted | Write spec-tests/acceptance first |
| Sub-agents chatting to align | Token explosion, not auditable | hub-and-spoke + contracts |
| Dispatching one big task touching 10 files at once | Context overrun, high failure rate | 1–4 hour / ≤ 8 file slices |
