# 08 Design Roadmap v0.2: In-House Orchestration Kernel + Swappable Executors

> Language: **English** | [简体中文](08-design-roadmap-v0.2.zh-CN.md)

> This version does not overturn the protocols of 01–07; it only replaces the **carrier**.  
> Trigger: the 2026-11 prompt protocol proved effective in practice (40%–70% fewer tokens per exchange), but "manually placing files + a pi plugin" is not sustainable; pi's plugins and concepts change too fast to be a long-term foundation.  
> **How to read this**: the protocol content of 01–04 still holds; the directory mapping from `.agent/` to `.loom/`, see §10. The "pi as the main path" parts of 05/06 are superseded by this document.

---

## 0. One-sentence conclusion

**Stop treating pi as the foundation, and stop treating agent chat as state.**

Turn Loom into a standalone local CLI/daemon:

- it owns state, the task graph, context compilation, acceptance, and the ledger;
- the agent is nothing but a replaceable "executor process";
- the stable contract is **process + files + JSON + exit code**, not a plugin API.

That way pi can be swapped, Codex can be swapped, OpenCode can be swapped, while Loom's state protocol, acceptance protocol, and cost ledger stay unchanged.

---

## 1. What the measurements told us

| Observation | Explanation | Decision |
|---|---|---|
| Per-exchange tokens −40%~70% | slice + short context + delivery by reference really works | **Keep the protocol, productize it** |
| Concurrency dropped, everything slower | manual dispatch, serial waiting, human review; not a problem of the protocol itself | Separate scheduling from execution, queue it; watch wall clock too, not just tokens |
| Files hand-placed in every project | no initialization / scaffolding | Generate with `loom init`; templates built into the tool |
| LLMs overwrite state files | state and workspace are not isolated, and the agent has write permission | **Single writer + read-only snapshots + outbox** |
| Uncertain whether anything is reusable after a session ends | state depends on the session and may already have been rewritten | Event log + session-free state + `loom resume/doctor` |
| pi plugins may stop working after a month | depending on someone else's plugin API | Build process-level adapters only; the core does not import pi |
| Aider only holds up for a while | shared transcript, no sub-agent isolation | Borrow the ideas, use it as a temporary adapter |

**The most important conclusion**: what you verified is that "the protocol works", not that "the pi plugin approach works". The next step is to turn the protocol into **your own tool**, instead of keep adding plugins on pi.

---

## 2. Three architectural decisions

### D1 Control plane in-house, execution plane replaceable

| Layer | Ownership | Content |
|---|---|---|
| Control plane | **In-house** | state storage, task graph, scheduling, context compilation, acceptance, budget, ledger, policy |
| Execution plane | **Replaceable** | native minimal executor, Codex, OpenCode, Aider, pi, OpenHands, goose |
| Gateway | **Optional reuse** | LiteLLM (routing) + Headroom (cache/compression) |

The control plane is where the differentiation lives and where the token savings come from; the execution plane is a commodity, use whoever is stable.

### D2 The stable contract is process-level, not plugin-level

The contract for one task dispatch:

```text
Task Packet (directory, read-only)
    ↓
executor process (separate cwd / separate context)
    ↓
code changes + outbox/receipt.json + usage.jsonl + exit code
```

It does not depend on:

- plugin hook names;
- the agent's internal session format;
- shared memory / shared transcript;
- context inheritance from a parent agent.

### D3 State is a database, not a prompt

- **Single source of truth**: append-only event log + task records.
- **Markdown views**: generated from events, not hand-written, not written by an LLM.
- **worker**: read-only snapshots + writes only `outbox/`.
- **orchestrator**: the only writer of state.
- **The state directory does not live inside the worker's worktree**; by default the worker cannot see `.loom/`.

This directly solves "LLMs overwrite state" and "a new session cannot pick up where the previous one left off".

---

## 3. Target architecture

```text
┌──────────────────────────────────────────────────────────────┐
│ Human / CLI                                                  │
│  loom init | task | dispatch | verify | status | resume      │
└──────────────────────────┬───────────────────────────────────┘
                           │
┌──────────────────────────▼───────────────────────────────────┐
│ Loom Control Plane (in-house, determinism first)             │
│  ├─ State Store: .loom/events.jsonl + tasks/                 │
│  ├─ Scheduler: ready queue / concurrency / retry / escalation│
│  ├─ Context Compiler: recompiles bounded context each call   │
│  ├─ Budget Governor: token / cost / time                     │
│  ├─ Verifier: independently reruns acceptance                │
│  └─ Ledger: requests / tasks / cost / CPVT                   │
└───────────────┬──────────────────────────────┬───────────────┘
                │ Task Packet (read-only)      │ Receipt + patch + usage
                ▼                              │
┌──────────────────────────────────────────────┴───────────────┐
│ Executor Adapters (replaceable)                              │
│  ├─ native minimal loop (later)                              │
│  ├─ codex exec --json                                        │
│  ├─ opencode run --format json / server SDK                  │
│  ├─ aider --message (temporary / A-B)                        │
│  └─ pi (optional, not the foundation)                        │
└───────────────┬──────────────────────────────────────────────┘
                │
                ▼
        git worktree (one per task)
                │
                ▼
        LiteLLM → provider
        (optional) Headroom before LiteLLM
```

### 3.1 The control plane is not a "resident chat"

C1 changes from "frontier model resident with a small context" to:

- **Deterministic code first**: scheduling, state transitions, verify, ledger, budget never call an LLM;
- **The LLM only handles ambiguity**: planning, failure judgment, acceptance that needs explanation;
- Every LLM call is a **bounded function**: the input is a compiled context, the output is JSON, and no transcript is kept.

This goes further than "shrinking the chat history": **there is no long history to resend at all**.

### 3.2 Decision ladder

Every decision takes the cheapest rung first:

1. what a rule/script can do → no LLM call;
2. what a template/retrieval can do → no LLM call;
3. generation needed → worker-tier model;
4. judgment / planning / conflict handling needed → frontier-tier model.

Multiple models does not mean "one model per agent", it means "one tier per decision".

---

## 4. State ownership: solving overwrite and session dependence

### 4.1 Directory layout

```text
repo/
├── .loom/                         # state root; gitignored; invisible to workers by default
│   ├── project.json               # project id, repo, verify command, config version
│   ├── goal.md                    # written by humans; orchestrator read-only
│   ├── constraints.md             # written by humans; orchestrator read-only
│   ├── tasks/
│   │   └── T-001.json             # TaskSpec; written by orchestrator
│   ├── events.jsonl               # append-only; the single source of truth
│   ├── runs/
│   │   └── R-20261104-001/
│   │       ├── task.json          # snapshot at dispatch; read-only
│   │       ├── context/           # compiled fragments; read-only
│   │       ├── outbox/
│   │       │   ├── receipt.json   # generated by the adapter (worker self-report not trusted)
│   │       │   ├── final.md       # worker's final message, summary only
│   │       │   └── report.md      # optional, details
│   │       ├── logs/
│   │       │   ├── agent.jsonl
│   │       │   └── verify.log
│   │       └── patch.diff         # optional
│   ├── ledger/
│   ├── snapshots/                 # pre-dispatch snapshot, for restore
│   └── locks/
└── .worktrees/
    └── T-001/                     # worker cwd; code only
```

> `.loom/` sits at the repository root and is added to `.gitignore`, so worktrees created by `git worktree add` will not contain it. The worker's cwd is `.worktrees/T-001`, which cannot see the state by default.  
> When centralized storage is needed, `LOOM_HOME=~/.local/share/loom/projects/<id>` can be used; the protocol does not change.

### 4.2 Write permission table

| Path | Owner | Worker permission | Purpose |
|---|---|---|---|
| `.loom/events.jsonl` | orchestrator | none | the single source of truth |
| `.loom/tasks/*.json` | orchestrator | read-only snapshot | task graph |
| `.loom/goal.md` | human | read-only snapshot | goal |
| `.loom/runs/*/context/` | orchestrator | read-only | compiled context |
| `.loom/runs/*/outbox/receipt.json` | adapter | none | normalized receipt |
| `.loom/runs/*/outbox/final.md` | worker | write | final message, summary only |
| `.loom/runs/*/outbox/report.md` | worker | write | details |
| `.worktrees/T-001/**` | worker | write | code |
| `.loom/ledger/**` | orchestrator | none | ledger |

### 4.3 Anti-overwrite rules

1. **The worker gets no write path into `.loom/`**; the Task Packet is a copy / read-only mount.
2. **Protected paths**: `.loom/**`, `.git/**`, `goal.md`, `constraints.md`. Hashes are compared before and after dispatch; a detected change → reject the receipt, restore the snapshot, record an event.
3. **Single writer**: only the orchestrator may append events, change task status, and generate Markdown views.
4. **Snapshots**: snapshot `.loom/` before every dispatch; `loom restore <run>` rolls back.
5. **Checks**: `loom doctor` verifies schema, hashes, orphan runs, expired locks.
6. **Strong isolation (optional)**: mount only the worktree as RW and `.loom` as RO inside a container; or run the worker as a separate user.

### 4.4 Session independence

- No session id is stored in state; a run is identified only by `task_id + run_id`.
- After a process crash, `loom resume` replays from `events.jsonl` and continues the `pending/running` tasks.
- New shell, new machine, new session: as long as you have the repository and `.loom/`, you can run `loom status` / `loom resume`.
- **Acceptance test**: after dispatch, `kill -9`, restart, and `loom resume` must be able to continue from the last event; a worker writing to `.loom` must be rejected.

### 4.5 How goals/constraints get changed

- Humans use `loom goal edit` / `loom constraint edit`, with schema validation and version numbers.
- The worker's prompt states explicitly: **do not write progress/goal/decision**; progress is generated from events.
- If a worker genuinely needs to propose a goal change, it writes `outbox/proposal.md`, and the orchestrator applies it after human confirmation.

---

## 5. No long session: every call is a bounded function

### 5.1 Old model vs new model

| | Old: chat-style orchestrator | New: state-machine orchestrator |
|---|---|---|
| History | conversation history keeps growing | no conversation history kept |
| Input each turn | the full history is resent | only the compiled current context is sent |
| Recovery | depends on the session | depends on the event log |
| Cost shape | quadratic growth with the number of turns | bounded per call |
| Human intervention | interrupting the chat | `loom` commands / approvals |

### 5.2 Main loop

```text
while there is a ready task:
    packet = context.compile(task, state, budget)
    result = adapter.dispatch(packet)
    receipt = validate(result)
    evidence = verify(task, receipt)
    state.append(TaskVerified | TaskFailed | NeedsHuman)
```

- every step is recoverable;
- the adapter is a child process and can be killed on timeout;
- the orchestrator does not wait on chat, and is not blocked in some UI;
- tasks can run in the background, and `loom status` can be checked at any time.

### 5.3 The event log is not a prompt

The event log is for recovery and audit; it is **not stuffed into the model directly**. The model only sees:

- a short version of the goal/constraints;
- the current task;
- a one-line summary of each dependency task;
- relevant code snippets;
- the previous failure packet (only on retry);
- the output schema.

`StateSummary` is generated by deterministic code, targeting < 2k tokens. That keeps the source of truth while avoiding treating history as context.

---

## 6. Context Compiler: the real moat

### 6.1 Inputs

| Part | Source | Budget (example) |
|---|---|---|
| Stable prefix | system + tool instructions + goal + constraints | 2–4k |
| Task | TaskSpec + acceptance | 1–2k |
| Dependency summaries | one line per dependency | 0.5–1k |
| Code snippets | path + symbol + line numbers + content | 8–16k |
| Failure packet | on retry only: failed command + key logs + relevant diff | 2–4k |
| Output schema | JSON Schema / receipt format | 0.5–1k |

### 6.2 Rules

1. **Hard budgets**: planner 8k, worker 24k, judge 8k (examples, tuned per model).
2. **Stable order**: system → tools → goal → constraints → task → snippets → failure → schema. Do not put timestamps or random ids into the prefix.
3. **Structured tool results**: `{summary, path, hash, range, truncated}`; the full text goes to an artifact, and the model does `read range` on demand.
4. **Read by symbol/path/line number**: whole-repository dumps are forbidden; the same file is read once per task, and a repeated read must justify itself.
5. **diff-only**: output a patch by default; full text is allowed only for new files.
6. **Deterministic summaries**: the state summary, dependency summaries, and failure summaries are all generated by code, never re-summarized by an LLM.
7. **Cache-friendly**: do not swap system/tools within a task; batch changes; reuse the prefix for the same task.
8. **Cut at the tool first**: tool output defaults to "summary + address + artifact reference"; both per-call and per-run hard caps apply; raw JSON/HTML/logs are never pasted back.
9. **Gateway as the fallback**: Headroom only handles duplication and caching at the model layer; it is not responsible for governing tool results.

### 6.3 Cost shape

Within a single task:

```text
billed_input ≈ turns × bounded_prompt
```

Across tasks:

```text
total input ≈ Σ bounded_prompt of each task
```

There is no more "history of the whole run × per turn" quadratic growth. The shorter the task, the bigger the gain; this is also why slicing matters more than compression.

---

## 7. Executor strategy: Adapter first, Native later

### 7.1 Adapter contract v1

```bash
loom-adapter dispatch \
  --packet .loom/runs/R-001 \
  --worktree .worktrees/T-001 \
  --model worker \
  --output .loom/runs/R-001/outbox
```

Conventions:

| Item | Convention |
|---|---|
| Input | Task Packet directory; no large payloads passed over stdout |
| Output | the adapter writes `outbox/receipt.json` in normalized form; the worker only writes code + `final.md`/`report.md` |
| Receipt | generated in normalized form from exit code, git diff, final message, and verify results; **worker self-report not trusted** |
| usage | `outbox/usage.jsonl` or adapter logs; missing fields marked `unknown` |
| exit code | `0` success; `2` needs a human; `3` budget/timeout; `4` adapter failure |
| Timeout | the orchestrator kills the process group and records `adapter_timeout` |
| Environment | `LOOM_TASK_ID`, `LOOM_RUN_ID`, `LOOM_MODEL`, `LOOM_MAX_TOKENS` |
| Capabilities | `streaming`, `tool_calls`, `usage`, `patch`, `resume`, `sandbox` are declared by the adapter |

**Key point**: do not let the orchestrator parse the agent's natural-language stdout to decide state; state comes only from receipt + verify.

### 7.2 First batch of adapters

| Adapter | Invocation | Fits | Risk |
|---|---|---|---|
| **goose** | `goose run --no-session -i task.md --output-format json` | **First choice**: multi-provider, headless, session-free | JSON/usage fields change between versions; needs pinning + canary |
| **OpenCode** | `opencode run --format json` or headless server SDK | Runner-up: multi-provider, structured events | v2 iterates fast; the version must be pinned |
| **Codex** | `codex exec --json` | Optional frontier worker / fallback | OpenAI-centric; heavy tooling; awkward with multiple providers |
| **Aider** | `aider --message` | diff generation, A-B, temporary stand-in | shared transcript; no sub-agent isolation |
| **pi** | existing CLI | optional, not the foundation | plugin/concept drift |
| **native** | `loom-exec` | full control of context/tool results | quality and the tool ecosystem have to be built yourself |

Keep at least **two working adapters** (goose + OpenCode, or goose + native). Run `loom doctor --adapters` as a canary once a week; one breaking must not affect the main path.

### 7.3 Native minimal executor

**When to build it**: when external adapters cannot control tool-result projection/context, or when you need complete control over the token structure.

**Scope**:

- tools: `bash`, `read` (range), `edit` (diff), `search`;
- provider: LiteLLM / OpenAI-compatible;
- observability: every tool result is written to an artifact, returning summary + path + line numbers;
- loop: max steps, no-progress detection, hard budget stop;
- out of scope: LSP, MCP marketplace, TUI, multi-tenancy, full sandbox.

**Reference**: `mini-swe-agent` (MIT, about 190 lines of agent class, bash-only, LiteLLM). It proves that "minimal loop + strong model" works; all we need to add on top of it is:

- structured observations;
- tool-result projection;
- task/acceptance protocols;
- budgets and recovery.

This is not "building a complete runtime in-house", it is "building a swappable minimal executor in-house". It follows the same contract as external adapters and can be turned off at any time.

---

## 8. Tool selection: suitability for secondary development

> Licenses and capabilities are governed by the repository `LICENSE` and the local version; the table below is the assessment as of 2026-11.

| Tool | License | Stable surface usable for secondary development | Worth borrowing | Role in Loom | Risk |
|---|---|---|---|---|---|
| **OpenCode** | MIT | headless server + SDK + `run --format json` | server/event/SDK layering, provider-agnostic | backup external adapter | v2 iterates fast, the API will change |
| **OpenHands SDK** | MIT | Python/TS SDK + Agent Server + sandbox | event-sourced state, workspace abstraction, sandbox | execution-plane candidate/reference | heavier, brings its own conversation model |
| **goose** | Apache-2.0 | GDK, recipes, MCP, CLI/API, headless JSON | recipes, MCP, provider abstraction, `--no-session` | **first-choice external adapter** | many concepts of its own; version must be pinned |
| **Codex CLI** | Apache-2.0 | `codex exec`, JSONL, MCP | strong frontier execution | optional frontier adapter | OpenAI-centric, heavy tooling |
| **mini-swe-agent** | MIT | ~190 lines of agent + LiteLLM | minimal loop, bash-only, linear history | **reference/starting point for the native executor** | too simple; projection/budgets must be added |
| **Aider** | Apache-2.0 | Python, LiteLLM, repo map, architect/editor | repo map, diff format, dual model | diff engine / A-B / temporary adapter | shared transcript, no isolation |
| **Crush** | FSL | Go, MCP, LSP | TUI/LSP experience | external tool | FSL, unsuitable as a distribution foundation |
| **pi** | governed by the repository | plugins | existing workflow | optional adapter | plugin API drift |
| **Claude Agent SDK** | governed by the official terms | TS/Python SDK | tool/permission model | Claude scenarios only | model lock-in |

### 8.1 Conclusions

- **Kernel**: write it yourself, keep it small; use a ports-and-adapters (hexagonal) structure.
- **Executors**: connect **goose** first; OpenCode/Codex as backups; native later. Do not make any agent's plugin API a core dependency.
- **Aider**: borrow only repo map / diff / architect-editor, and build a temporary adapter if necessary.
- **OpenHands SDK**: if you want a complete execution plane of "sandbox + event sourcing + multi-language SDK", it is the closest MIT option; but it has its own conversation/state model, so it belongs in the execution plane, not as the Loom kernel.
- **pi**: it can keep being used, but only through process invocation; no more core plugins written for it.

### 8.2 Why not "develop directly on top of OpenCode"

OpenCode's headless server + SDK is well suited to being an executor, but:

- it is a complete product, not a thin framework; its core concepts (session, message, agent) overlap with Loom's state model;
- v2 iterates very fast, and plugins/protocols will change;
- what you need is a **control plane**, not **another agent product**.

So: **use its process/HTTP contract, not its internal model.**

### 8.3 Why goose suits being the first adapter

goose is now governed by the Linux Foundation's Agentic AI Foundation, under Apache-2.0; it is not "an internal Block tool" but a neutral foundation project. What matters more for this project is that its CLI already has the capabilities an adapter needs:

| Capability | goose equivalent | Value to Loom |
|---|---|---|
| Headless execution | `goose run -i task.md` / `-t` | one process per task |
| No session persisted | `--no-session` | matches "no long session" |
| Structured output | `--output-format json` / `stream-json` | parseable by the adapter, no reliance on natural language |
| Multiple providers | `--provider` / `--model`, built-in LiteLLM provider | LiteLLM can be the single upstream |
| Tool control | `--with-builtin developer` / `--with-extension` | only the necessary tools are given |
| Turn/repetition limits | `--max-turns` / `--max-tool-repetitions` | the first gate for budget/no-progress |
| Recipes | `--recipe` + `--params` | TaskSpec/system prompts can be templated |
| Service mode | `goose serve` (ACP over HTTP/WS), `goose acp` | a resident mode can be swapped in later without changing the core |
| Embeddable | GDK | evaluate when needed, no first-phase dependency |

Suggested adapter invocation (example; the arguments are governed by the local `goose --help`):

```bash
export GOOSE_PROVIDER=litellm
export GOOSE_MODEL=worker
export LITELLM_HOST=http://127.0.0.1:4000
export LITELLM_API_KEY=dummy

goose run --no-session \
  -i .loom/runs/R-001/task.md \
  --output-format json \
  --with-builtin developer \
  --max-turns 30 \
  --max-tool-repetitions 3 \
  --quiet
```

**Key design**: goose ships with a **LiteLLM provider**; just configure `LITELLM_HOST` / `LITELLM_BASE_PATH` / `LITELLM_API_KEY`, with no need to configure GPT/DeepSeek separately. That way:

```text
Loom → goose (provider=litellm, model=worker) → LiteLLM → provider
```

goose only sees "one LiteLLM endpoint + one model alias"; the GPT/DeepSeek differences are absorbed by LiteLLM. This is exactly the shape of "replaceable executor, centralized routing". If LiteLLM is not used, goose's OpenAI-compatible custom provider works too; but prefer the built-in LiteLLM provider and skip a layer of configuration.

> **Early-stage constraint**: the API key covers GPT, DeepSeek, Kimi, MiniMax, MiMo, and others. LiteLLM starts with two tiers, GPT `frontier` + DeepSeek `worker`; other domestic models are connected gradually through A/B, and the subscription lane does not participate.

Risks and countermeasures:

- **JSON/usage fields must be verified**: they may change between versions; the adapter relies only on stable fields, marks missing ones `unknown`, and cost is governed by the LiteLLM logs.
- **Tool output can be large**: goose's JSON carries tool results; Loom does not paste them back to the next model, it only spills them to artifact + summary.
- **The developer extension is not light either**: M0 enables only the necessary tools; if the tool surface is still too heavy, evaluate the native executor.
- **Version drift**: pin versions + `loom doctor --adapters` canary; keep OpenCode/Codex as the second adapter.

### 8.4 Rust for the control plane: yes, and it fits this architecture

Conclusion first: **Rust distributes well as a binary; for this control plane of "process orchestration + file state + SQLite + JSON", Rust is a good fit.** The real cost is not distribution, it is development speed and the native executor ecosystem.

| Dimension | Rust | Python | TypeScript |
|---|---|---|---|
| Distribution | **Best**: single static binary | needs Python/uv | needs Node/Bun |
| Development speed | slower | **fastest** | fast |
| Process/signal/timeout | **best** | good | good |
| JSON Schema | serde + schemars | pydantic | zod |
| SQLite | rusqlite/sqlx | built-in sqlite3 | better-sqlite3 |
| LLM/agent ecosystem | small | **largest** (LiteLLM/mini-swe-agent/OpenHands) | large (OpenCode/pi) |
| native executor | more work | **easiest** (mini-swe-agent) | moderate |
| Long-term maintenance | **strong** (no runtime dependency) | medium | medium (dependency drift) |

Recommendations:

- **Use Rust for the control plane**: `clap` + `serde` + `rusqlite` + `tokio::process` + `tracing`; shell out to git first, do not rush into `git2`.
- **Do not write the native executor in M0/M1**: use the goose adapter first. The native executor waits until M2; implement it as a separate process `loom-native`, and the control plane does not import executor code.
- **If Rust proficiency is mediocre**: Python will be more steady, because you will be changing prompt/context logic often; but Python's "single-binary distribution" requires accepting the uv/pyinstaller compromise.
- **Go is not recommended**: you have already ruled it out; and what this project really needs is "state machine + process management", which both Rust and Python can do, with no extra advantage for Go.

In one sentence: **Rust suits a control plane that will not change every day; Python suits agent logic that changes every day.** You can write the core in Rust and keep the native executor as a replaceable separate process, so the two are not locked together.

---

## 9. Roadmap v0.2

> The original S stage was "manually placing files + manual dispatch". Now that you have verified the protocol, the goal of the S stage becomes "productizing the manual process".  
> No more building a pi plugin first; build the standalone CLI first.

| Milestone | Time | Deliverable | Exit criteria |
|---|---|---|---|
| **M0 Protocol productization** | 2–3 days | `loom init/task/dispatch/verify/status/resume/doctor`; state ownership; 1 adapter | 1 real task end-to-end; recoverable after restart; worker writes to `.loom` rejected |
| **M1 Context and ledger** | 1 week | Context Compiler, event log, usage, CPVT, second adapter | main context ≤30k; 0 unattributed; adapter canary passes |
| **M2 Native executor** | 1–2 weeks | minimal loop, tool-result projection, budgets, prompt library | worker tasks pass; per-task tokens drop; fallback adapter available |
| **M3 Routing** | 1 week | LiteLLM, role→tier, escalation, cost guardrails; starting with GPT + DeepSeek | worker ≥50%; CPVT −20%; no quality regression |
| **M4 Parallelism** | 1–2 weeks | worktrees, merge queue, locks, board | **starting at 2 parallel**; no lost updates; merges <5min; 3-way parallel needs A/B proof that `WPT`/`CPVT` do not regress |
| **M5 Multi-repo workspace** (pending scheduling) | 1 week | manifest member repos, `loom workspace *`, cross-member same-name-branch worktrees | see [13](13-multi-repo-workspace-and-multi-client.md) §11 P0–P3; zero migration for single-member tasks |

### 9.1 What M0 does specifically

1. `loom init`: detect the repo, create `.loom/`, write `project.json`, detect the verify command, record tool versions.
2. `loom task add`: create a TaskSpec from JSON/interactive input; validate the schema.
3. `loom dispatch T-001 --adapter goose`: create the worktree, snapshot the state, compile the packet, start goose.
4. `loom verify T-001`: rerun acceptance in a clean worktree; write evidence.
5. `loom status`: task graph, budget, anomalies, pending human items.
6. `loom resume`: recover from the event log.
7. `loom doctor`: schema, hashes, orphan runs, expired locks, adapter canary.
8. Write 3 tests: worker writes to `.loom` rejected; recovery after `kill -9`; a new shell can read the state.

**M0 does not call an LLM for planning**. Tasks are written by humans; validate the state/dispatch/acceptance loop first. Planning is an amplifier for M1/M2, not the first step.

### 9.2 What M1 does specifically

- Context Compiler: stable prefix, snippet selection, failure packet, budgets.
- Event log: JSONL as the source of truth, SQLite as the index (optional).
- Ledger: parse adapter usage + LiteLLM logs; CPVT/TPVT; reconciliation.
- Prompt library: versioned, rendered at dispatch time; no static prompts copied into the project.
- Adapter contract tests: schema, exit code, timeout, usage, empty output.
- A second adapter (OpenCode or native) as the fallback.

> **Implementation status (2026-11-06)**: all of the above have landed as the two crates `loom-context` / `loom-adapter` and the `loom context/prompt/adapter/dispatch/verify/resume` commands; the mock adapter end-to-end path (dispatch → verify → event replay → canary) passes. Remaining: real goose/OpenCode canary (needs the binaries), on-site reconciliation of LiteLLM logs.

### 9.3 What M2 does specifically

- native executor: bash/read/edit/search;
- observation: large results go to an artifact, returning summary + path + line numbers;
- budgets: pre-estimation + live usage + hard stop;
- no-progress: stop after N consecutive turns with no diff/no verify progress;
- A/B: the same task set, native vs adapter, compared by CPVT/TPVT/wall clock.

> **Implementation status (2026-11-06)**: `loom-native` has landed as a separate process and can be invoked directly with `loom dispatch --adapter native`. The current implementation runs an OpenAI-compatible `/chat/completions` tool loop with tools `read/search/edit/write/bash/finish`; long output is written to `.loom/runs/<run>/outbox/artifacts` and only a projected summary is sent back; token/wall-clock/step/cost budgets, no-progress, acceptance pre-run, and `final.md`/`report.md`/`usage.jsonl` are implemented.
>
> **no-progress definition (evidenced 2026-11-11)**: progress is "a file change **or** a new acceptance result". A bash call that runs the task's acceptance command counts as progress the first time the (cmd, exit) pair appears, repeated reads of the same check do not count; pure inspection loops still trip the `LOOM_NATIVE_MAX_IDLE` breaker (default 6 steps). This corresponds to §5's "stop after no diff/no verify progress", fixing a regression where the implementation only looked at diffs.
>
> Acceptance failures are fed back within the same native run: the failed command, exit code, and truncated output (3KB per command, 8KB for the whole packet) go back to the model as the `finish` tool result or a user message, by default up to 2 rounds (`LOOM_NATIVE_MAX_VERIFY_ROUNDS`, minimum 1); a retry resets no-progress, and exhausting the rounds marks it `failed`. `loom dispatch` also reads the previous run's failed receipt and `verify.log` to build a `FailurePack` and injects it into the Context Compiler.
>
> Cost: provider cost takes priority (`cost_source=provider`); otherwise the `ModelCapability` price table of the dispatch model is used for estimation (`pricing_table`); if the price is unknown, `cost_usd` is omitted and marked `unknown`. Estimated cost counts toward the running budget and `TaskBudget.max_cost_usd`, and exceeding the limit stops with `budget_exceeded`/exit 3. Offline tests use `LOOM_NATIVE_SCRIPT` as a scripted provider, and `scripts/smoke-native.sh` passes end-to-end. Remaining: real-model A/B, calibration of the cost budget against real bills, real goose/OpenCode canary.
>
> **Wire support (2026-11-08)**: the native implementation supports two wires: `chat` (OpenAI `/chat/completions`, default) and `messages` (Anthropic `/messages`, `x-api-key`+`anthropic-version` headers, `tool_use`/`tool_result` content blocks, thinking blocks dropped, `cache_read` counted as cached), selected via `[[models]] wire` through `LOOM_MODEL_CAPABILITY`; `responses` is not implemented and `wire = "responses"` fails immediately. The reason for choosing messages is in [14](14-current-issues-and-blockers.md) §2: at the time the relay's chat path did not accept tool history, which was later proven to be a format defect in **our** chat wire; after the fix the relay chat retested with 200 (the relay was wrongly blamed); mimo still uses messages (the cleanest cache fields). Real links: `gpt-6.1-sol` (chat), `mimo-v2.6-pro` (messages, via the gpt relay), and official `deepseek-flash` (chat) each completed a live dispatch→verify (`scripts/live-smoke.sh`, ≤20k tokens throughout); `scripts/relay-probe.sh` probes with multiple sessions to find which wire a model accepts tool history on; `LOOM_NATIVE_DEBUG=1` writes each request/response to the run log (no credentials in the body). Also fixed: `default_prompt_role` now selects the prompt by task role (escalating to frontier no longer gets the orchestrator prompt), a duplicated system section in the packet is no longer sent twice (about 600 tokens saved per step), and the provider retries 3 times on 403/408/429/5xx and transport errors (1s/2s backoff).
>
> **Two chat wire format fixes (2026-11-08, found during joint debugging with the official endpoint)**: (1) `tool_calls` must be serialized in the OpenAI nested format `{id, type:"function", function:{name, arguments}}`; previously the flat internal structure was sent directly, and the official endpoint returned 422 `messages[N]: missing field type`; the relay's 403 had the same root cause; (2) `reasoning_content` is parsed and passed back verbatim with the assistant message, which is mandatory in official thinking mode (`The reasoning_content in the thinking mode must be passed back`), whereas it was previously parsed and dropped. After the fixes, official `deepseek-flash` returned 200 on the first probe, and a full task passed in 9 seconds (89% cache hits); gpt/mimo regressions are unchanged. `ChatMessage` therefore gained a `reasoning_content` field (wire-neutral; not sent on the messages wire).
>
> **Per-model endpoint (2026-11-08)**: `[[models]]` gained `base_url` and `key_env`: `base_url` overrides the global `LOOM_API_BASE`, `key_env` stores only the environment variable name (the value always stays in `.env.local`), native resolves it by the selected alias, and dispatch validates that the variable exists before creating the run. Resolution order: model config → environment → OpenAI default; when the key is missing the error names the variable directly. `scripts/dev.sh` passes through all `*_API_KEY`. Live verification: in a single `live-smoke.sh` run, two aliases each with their own relay (mimo on relay A, gpt on relay B) both passed. The configuration in `scripts/live-smoke.sh` is the example.

### 9.4 What M3 does specifically

- Deterministic routing: `role → tier`, explicit `--model` override, the task's own tier as the fallback;
- Escalation: escalate only after a verify failure, worker failures default to moving to frontier after 2 failures, at most 1 escalation per task;
- Cost guardrails: per-task cost cap at tier level + worst-case estimate before dispatch, reject the dispatch when exceeded;
- Explainable: `loom route TASK` outputs the selected model, tier, reason, fallback, budget, and estimate;
- LiteLLM as an optional gateway: the model alias is fixed by the control plane, and LiteLLM absorbs provider differences.

> **Implementation status (2026-11-06)**: control-plane routing has landed. `.loom/config.toml` gained `[routing]`: `roles` (role→tier), `escalate_after_failures`, `max_escalations_per_task`, `fallbacks`, `cost_limits`. `loom route TASK` and `loom dispatch` share the same `RoutingConfig::plan`; an explicit `--model` wins, and the task tier is used when no role is configured. Escalation reads only the count of `task.failed` events and writes `task.escalated` before dispatch; `TaskDispatched` records the route reason, tier, and fallback. The cost guardrail rejects the dispatch when `budget.max_cost_usd` exceeds the tier cap, or when the worst-case cost estimated from the price table exceeds the cap. `loom route stats` aggregates from the event log the tier of each task's last dispatch, the worker share, escalation counts, and routing reasons, for checking the M3 exit criteria.
>
> **Same-tier fallback is wired up (2026-11-06)**: `[routing.fallbacks]` is configured per tier; on config load it validates that the alias exists, that the alias's tier equals the key (cross-tier fails immediately, cross-tier can only go through escalation), that it is tool-loop-safe, and that there are no duplicates. On dispatch, only an **availability failure** (adapter exit code 4, or a timeout) switches; `budget_exceeded` / `needs_input` / `blocked` do not switch; switching models cannot undo budget that is already burned, it only doubles the cost. Every switch writes `task.fallback` (`run_id` = the new attempt's run, `parent_run_id` = the failed run, `attempt` unchanged) and opens a **separate run** for the new attempt, so receipt, usage, and failure packet do not overwrite each other, and the previous attempt's failure is injected into the next attempt's packet as a FailurePack. An explicit `--model` pins the alias: `RouteDecision.fallback_enabled = false`, no switching. `scripts/smoke.sh` covers the full chain of "primary model exit 4 → fallback succeeds → verify passes" via `LOOM_MOCK_FAIL_ALIAS`.
>
> Not finished: worker share ≥50%, CPVT −20%, and no quality regression still need A/B with real models and real bills. LiteLLM keeps only the integration design; there is no LiteLLM service in the container and no on-site canary has been run.

### 9.5 M4 parallelism: implementation status (2026-11-08, complete)

- **Path locks**: `.loom/locks/<sha256(path)[..12]>.lock`, fields and rules in [03](03-core-protocols.md) §13. `loom dispatch` / `loom verify` take locks per `workspace.allowed_paths` before executing; the guard is RAII and only releases paths it wrote itself (nested guards do not mis-release each other); a vanished owner process is reclaimed immediately, and an expired lease with a living owner is reported as stuck by `loom doctor`.
- **Parallel cap and budget**: `[parallel] max_running` (default 2) counts "tasks holding a live lock"; `[parallel] max_inflight_cost_usd` (default 0 = unlimited) caps the sum of `budget.max_cost_usd` over concurrent tasks. Over the limit writes `task.blocked(code=parallel_cap / inflight_budget)` respectively, and the task can be retried.
- **Merge queue**: `loom merge <task>` accepts only `passed` tasks (or a retry after a previous `merge_conflict`/`integration_verify` block). The flow = a global `**` lock (all writes to task dispatch paused during the merge) → re-verify before merging → clean tree check → `git rebase` (on conflict, abort and `task.blocked(code=merge_conflict)`) → `git merge` → integration verify (rerun acceptance in the base checkout; on failure `reset --hard` to roll back and `task.blocked(code=integration_verify)`) → `task.merged(merge_sha, base_before, integration_verify)` → clean up the worktree and branch (a failure only warns). In-place tasks are rejected (the changes are already on the base branch).
- **Board**: `loom board` (`--format json|pretty`) lists all tasks by state, and shows running/max and lock holders.
- **Concurrency correctness (no lost updates)**: `events.jsonl` / `requests.jsonl` / `seen.jsonl` / `tools.jsonl` were changed to a single `write_all` (two processes interleaving a half line would corrupt the JSONL); `tasks.json` temp files carry a pid suffix; `new_run_id` reserves the directory with an atomic `mkdir`, so concurrent dispatches in the same second no longer share a run id.
- **Measured**: `scripts/smoke-parallel.sh` runs **10 tasks with 2-way concurrent dispatch** in a throwaway git repository using native scripting (sampling `loom board` to confirm 2 running at once), and after all verify pass it merges serially: **10 `task.merged`, 0 `task.failed`, 0 `task.blocked`, 10 files with no loss, all locks released, worktrees and branches all cleaned up, 10 merges in <5 minutes**; along the way an external `**` lock was used to check the merge queue's global gate. `scripts/smoke.sh` covers lock conflicts, the parallel cap, the inflight budget, and rejection of in-place/unverified merges.
- **Task-graph dedup**: the "same input hash + same output path do not run in parallel" rule from `doc/05` §6 is covered by the path locks; intersecting output paths are blocked by construction; tasks with the same input but disjoint outputs are duplicated effort, and input-hash dedup waits for a real task set.
- **A/B measured (2026-11-08, `scripts/ab-parallel.sh`, official `deepseek-flash`, 3 tasks/arm, ≤20k tokens per arm)**: dispatch wall for `max_running=1/2/3` was **13s / 10s / 6s** (3-way parallel is 54% faster than serial), tokens **12892 / 13047 / 13639** (+5.8%, no regression), cache hits **48% / 88% / 89%** (arm1 was a cold-cache first run), CPVT **$0.000572 / $0.000352 / $0.000430** (parallel went down instead of up: caching squeezed out the spend on missed inputs). The price table is `scripts/model-prices.sh` (official list prices as of 2026-11-08, deepseek off-peak, peak 2x; gpt $2/$0.10/$10), `--report` computes cost/CPVT offline, and the price fields in the config are generated from the same source. The sample is small (3 tasks, single round, single model), and the CPVT baseline awaits multi-round expansion. **Still not done**: merge-conflict-rate statistics on a real repository, repeated validation across multiple models/rounds.

### 9.6 Not doing for now

- a complete Agent Runtime, IDE, TUI, cloud platform;
- a plugin marketplace;
- LLM self-assessed complexity routing;
- automatic compression/automatic summarization (state summaries must be deterministic);
- multiple agents chatting with each other;
- unattended large tasks.

---

## 10. Differences from the older documents

| Old assumption | v0.2 | Reason |
|---|---|---|
| pi is the main path | pi is just an optional adapter | plugin API drift |
| S stage manually placing files | M0 productizes it with a CLI | manual work is not sustainable |
| orchestrator resident with a small context | every call is bounded, no transcript kept | eliminates resending history |
| `.agent/` writable by anyone | `.loom/` single writer + outbox | LLM overwrite |
| worker writes a four-line receipt | adapter generates it in normalized form from diff/verify/usage | self-report not trusted |
| JSONL ledger | JSONL source of truth + SQLite index | query/concurrency |
| runner only at P1 | minimal runner in M0 | turn the protocol into a tool first |
| Aider as a side path | still a side path, usable temporarily as an adapter | low cost, no bet placed |
| multiple models at P2 | M3 | stop the bleeding first, then route |

### 10.1 `.agent/` → `.loom/` migration mapping

| Old `.agent/` | New `.loom/` | Note |
|---|---|---|
| `GOAL.md` | `goal.md` | written by humans; orchestrator read-only |
| `spec.md` / `design.md` | `tasks/T-*.json` + `runs/*/context/` | split into tasks and compiled context |
| `PROGRESS.md` | generated view (not written to the source of truth) | no longer hand-written / LLM-written |
| `DECISION.md` | `events.jsonl` + generated view | auditable, replayable |
| `receipts/` | `runs/*/outbox/receipt.json` | independent per dispatch |
| `reports/` | `runs/*/outbox/report.md` | independent per dispatch |
| `budget.json` | `project.json` + task budgets | project-level + task-level |
| `env.lock` | `project.json` / `env.lock` | version pinning |

`loom import .agent` can read the old directory and generate the `.loom/` skeleton; after the import the old directory is kept read-only, so migration does not mean loss.

---

## 11. Metrics: do not look only at tokens

| Metric | Definition | Purpose |
|---|---|---|
| **CPVT** | total cost / tasks that passed acceptance | core cost metric |
| **TPVT** | billed tokens / tasks that passed acceptance | core token metric |
| **WPT** | wall clock / tasks that passed acceptance | explains "tokens dropped but it got slower" |
| **first-pass verify** | first-time verify pass rate | quality |
| **human intervention** | human interventions per task | degree of automation |
| **cache hit** | cached tokens / input tokens | gateway health |
| **handoff ratio** | tokens handed across agents / total tokens | communication waste |

The "per-exchange −40%~70%, but everything got slower" you observed is very likely because:

- after slicing tasks, the number of calls went up;
- the time spent on manual dispatch/waiting/copying files was not counted;
- concurrency dropped, so wall clock got longer.

So M0/M1 must record **CPVT, TPVT, and WPT** at the same time; otherwise you will misjudge "saved tokens but did not save time/money".

---

## 12. Risks and mitigations

| Risk | Mitigation |
|---|---|
| adapter output/schema changes | contract tests + canary + two adapters + pinned versions |
| native executor quality insufficient | only take worker tasks; frontier fallback; A/B |
| state corruption | event log + snapshot + `doctor` + `restore` |
| missing cost attribution | adapter usage + LiteLLM logs + reconciliation; missing marked `unknown` |
| concurrency makes it slower | dispatch made asynchronous; queue; measure WPT; get serial working first, then parallel |
| the tools themselves get heavy | keep the core small; UI deferred to M4; do not build a runtime in-house |
| scope creep | every milestone has exit criteria; no criteria met, no moving to the next stage |

---

## 13. Decided and open

### 13.1 Decided

| Item | Decision | Note |
|---|---|---|
| State directory | `.loom/` | `loom import .agent` provided for migration; old directory kept read-only |
| First adapter | **goose** | Apache-2.0, Linux Foundation, headless JSON, multi-provider |
| Control-plane language | **Rust** | single-binary distribution; keep the core thin; native executor not locked in at M0 |
| Executor boundary | process + files + JSON + exit code | no agent's plugin API used as the core |
| Routing | direct connection first, LiteLLM at M3 | goose points at LiteLLM's OpenAI-compatible endpoint |
| UI | CLI only through M3 | TUI/Web is an optional layer after M4 |
| Code retrieval | v1 introduces no embedding/vector store; R2 uses BM25/FTS5 | client performance and distribution size first; see 09 |
| Tool I/O | wrapper-enforced budget + spill to artifact + dedup | takes priority over gateway compression; see 09 §8 |
| Knowledge base sharing | this project only by default; users can explicitly enable cross-project | avoids default leakage and index bloat; see 09 |
| Tool output protocol | `items` short keys + 8/24/128KB three-tier budget + telemetry | human views are converted at the tool layer and cost no tokens; see 10 |
| MCP | not in M0 | CLI + internal API first; Headroom via proxy |
| Output localization | Chinese by default, switchable to English/Japanese | paths/symbols/enums not translated; see 10 §6 |

### 13.2 Open / needs spike

1. **Rust proficiency and maintenance cost**: if actual iteration slows down noticeably, fall back to Python; Go is not recommended.
2. **goose JSON usage fields**: do 1 spike in M0 to confirm the token/cost fields; if missing, the LiteLLM logs are authoritative.
3. **Whether goose's developer extension is too heavy**: try the minimal `--with-builtin` set; if the tool surface is still too large, go to the native executor in M2.
4. **Target platform**: Linux only, or Windows support as well.
   - Windows affects worktree long paths, process groups, file locks, and the shell.
5. **Where the native executor lives**: inside the Rust main binary, or as a separate process (Python/Go/Rust).
   - Recommend a separate process first, to avoid locking it to the M0 control plane.
6. **Scale and language coverage of the code index**: whether R1 is worth doing and whether to introduce zoekt; see [09-code-search-and-knowledge-base.md](09-code-search-and-knowledge-base.md) §14.2.
   - Decided: v1 introduces no embedding; M0 only does R0 + tool budgets + retrieval traces.

---

## 14. One-sentence summary

**Demote "how the agent chats" to an execution detail, and promote "how tasks are defined, compiled, accepted, and accounted" to the product.**

That way you are neither held hostage by pi's plugin API, nor cut off from the new ideas of mainstream agents/harnesses: new ideas come in first as an adapter or an internal module, and enter the core only after they are proven effective.
