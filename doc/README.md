# Loom (working title)

> Language: **English** | [简体中文](README.zh-CN.md)

> A token-efficient agent harness for software design, architecture, and development  
> Core thesis: **long tasks, short contexts.**

> **v0.2 revision (2026-11-04)**: the design roadmap is governed by [08-design-roadmap-v0.2.md](08-design-roadmap-v0.2.md).  
> pi drops from "main path" to "optional adapter"; the core becomes a **standalone CLI/daemon plus a process-level executor contract**.  
> 01–04 remain valid; where 05/06 conflict with 08, 08 wins.

> **Current priority (2026-11-05)**: run the API lane end to end with **API keys** first; keys for GPT, DeepSeek, Kimi, MiniMax, MiMo are available, starting with GPT + DeepSeek as two tiers and adding the rest gradually through A/B. The subscription lane ([11](11-subscription-quota-and-agent-sdk.md)) and hybrid lane ([12](12-external-practices-comparison-and-adoption.md) §4) are **deferred** and do not block M0/M1; investing in subscription integration early does not pay off.

## 1. In One Sentence

Do not rewrite the Agent Runtime, and do not replace Headroom. Turn Loom into a standalone orchestration kernel where agents are just swappable executors:

- Move state from the conversation into `.loom/` files; the orchestrator is the single writer, and workers only write `outbox/`;
- Cut implementation into independent, short-lived worker contexts;
- Pass only **TaskSpec + a four-line receipt + file references** between agents, never transcripts;
- Use executable `verify` as the sole acceptance and escalation signal;
- Executors attach via **process + files + JSON + exit code**, with no dependency on any plugin API;
- Keep the gateway layer transparent: Headroom handles caching/compression, LiteLLM handles model routing.

The goal is not "make the model cheaper" but **make every token pay for effective work**.

## 2. Why It Is Needed Now

Measured results from prior work (2026-11, single-machine Headroom snapshot):

| Metric | Measured | Meaning |
|---|---:|---|
| Billed input / genuinely new | 154.0M / 5.94M ≈ **25.9×** | The main cause is history retransmission, not unit price |
| Compression savings across 1,322 requests | 1.17M tokens, **0.70%** | The compression layer is near its ceiling |
| Prefix cache discount | **$270.63** | Cache gains far outweigh compression |
| `json_bloat` | **60.6M** | Re-pasting tool results by value is the largest waste source |
| `reread` | **5.67M** | Repeated reads have no governance |
| `by_model` | only `gpt-6.1-sol` | Multi-model was never actually wired up |
| `routing` | `null` | No task-level routing |

Three conclusions:

1. **The main battlefield is the context lifecycle and tool results, not the compression ratio.**
2. **Multi-model is a second-order lever.** Split long sessions into short contexts first, then talk about cheap models; otherwise a cheap model that runs a few extra rounds raises `cost per successful task` instead.
3. **The only thing worth building in-house is the thin orchestration layer.** Runtime, sandbox, compression, and routing all have mature components; rewriting them is pure loss.

## 3. Document Map

| Doc | Content | Readers |
|---|---|---|
| [01-problems-and-requirements.md](01-problems-and-requirements.md) | Background, pain points, goals/non-goals, FR/NFR, acceptance principles | Product/tech leads |
| [02-overall-architecture.md](02-overall-architecture.md) | Design principles, layers, components, topology, state machines, concurrency, security | Architects/implementers |
| [03-core-protocols.md](03-core-protocols.md) | `.agent/` layout, TaskSpec, Receipt, Verify, escalation, budgets, events | Implementers/agents |
| [04-context-and-cost-model.md](04-context-and-cost-model.md) | Token formulas, waste taxonomy, context budgets, cache/compression/routing, metrics | Architects/operations |
| [05-implementation-roadmap.md](05-implementation-roadmap.md) | S/P0–P4 phases, exit criteria, risks, open questions, operations manual | Project leads |
| [06-technology-selection.md](06-technology-selection.md) | pi/Codex/Aider/LiteLLM/Headroom/DSH comparisons and boundaries | Architects/selection |
| [07-reference-implementation-snippets.md](07-reference-implementation-snippets.md) | Config, schemas, prompts, scripts, compatibility checklists | Implementers |
| [08-design-roadmap-v0.2.md](08-design-roadmap-v0.2.md) | **v0.2 revision**: own orchestration kernel, state ownership, adapter contract, M0–M4 roadmap | Architects/implementers |
| [09-code-search-and-knowledge-base.md](09-code-search-and-knowledge-base.md) | **Evaluation notes**: code index vs knowledge base, R0–R3 tiers, freshness and A/B | Architects/implementers |
| [10-tool-output-protocol-v0.1.md](10-tool-output-protocol-v0.1.md) | **Protocol draft**: envelope, short keys, three-tier limits, artifact, telemetry, Headroom integration | Implementers |
| [11-subscription-quota-and-agent-sdk.md](11-subscription-quota-and-agent-sdk.md) | **Research notes**: subscription vs API, Codex/Claude SDK/CLI boundaries, gateway compatibility, implementation options | Architects/implementers |
| [12-external-practices-comparison-and-adoption.md](12-external-practices-comparison-and-adoption.md) | **Comparison notes**: external harness/multi-model/subscription practices, adopted concurrency, capability bits, hybrid lane, observability definitions | Architects/implementers |
| [13-multi-repo-workspace-and-multi-client.md](13-multi-repo-workspace-and-multi-client.md) | **Design draft (not implemented)**: monorepo comparison, meta control plane, manifest member repos, same-name-branch aggregate worktrees, cross-client task splitting | Architects/implementers |
| [14-current-issues-and-blockers.md](14-current-issues-and-blockers.md) | **Status record**: environment blockers, unclosed M2/M3 items, tech debt and next steps | Project leads/implementers |
| [15-agent-runtime-governance-and-engineering.md](15-agent-runtime-governance-and-engineering.md) | **Design draft (not implemented)**: write churn, question blocking and the attention inbox, skills governance, agent/harness-driven local CI | Architects/implementers |
| [16-runtime-governance-adoption-assessment.md](16-runtime-governance-adoption-assessment.md) | **Assessment draft (no code changes)**: integration points based on existing code, contract changes, phasing, person-days and risks; covers `.loom.ci.toml` ownership, target project vs dogfooding, attestation, verify semantics fixes | Project leads/implementers |
| [17-workspace-dev-environment-isolation.md](17-workspace-dev-environment-isolation.md) | **Direction record (not designed)**: per-workspace environment isolation on three platforms, A+B layers without virtualization, reusing existing declaration files, demoting containers to an optional profile; not a general environment manager | Project leads/implementers |
| [18-agent-collaboration-workflow-proposal.md](18-agent-collaboration-workflow-proposal.md) | **Method proposal (not implemented)**: surviving principles from sixty years of process history, a list of failed ceremonies, direction/batch/work-item three-tier rhythm, four human intervention points, a six-command onboarding path; companion `loom plan` | Project leads/newcomers |
| [adr/README.md](adr/README.md) | **ADR index**: irreversible-decision records with a Model assumptions field; format, numbering and supersede rules | Architects/implementers |
| [`.opencode/skill-pilot/README.md`](../.opencode/skill-pilot/README.md) | **Skill pilot**: project-level skill inventory, trigger scope, fixed token cost, provenance and licensing | Implementers/maintainers |

**Recommended reading order**: README → 01 → 02 → 03 → 04 → 05 → 06 → 07 → 08 → 09 → 10. 11/12 are later research and can be read last; 13 is the multi-repo workspace design draft, read it when doing multi-client work; 15 is the runtime governance design draft and 16 its adoption assessment, read 15 → 16 when dealing with agent behavior/engineering; if you read only one: **08**.

## 4. Eight Design Conclusions

1. **Externalized state**: the event log and task records in `.loom/` are the single source of truth; Markdown is only a generated view, and chat is just a control channel.
2. **Slice dispatch**: the control plane only plans, dispatches, and accepts; implementation happens in a separate worktree + separate context and is torn down when done.
3. **Delivery by reference**: across boundaries pass only paths, hashes, line numbers, diffs, and verify log paths — never full text, never raw JSON/HTML, never transcripts.
4. **Verification-driven**: `verify` is the sole acceptance and escalation signal; no model self-assessment of "this task is hard", no escalation on self-assessment.
5. **Transparent gateway**: compression and routing are decoupled; Headroom stays in `cache` mode, LiteLLM sits behind Headroom; cost is judged by provider/LiteLLM bills, Headroom stats are diagnostics only.
6. **Swappable executors**: the stable contract is process + files + JSON + exit code; no agent's plugin API becomes a core dependency.
7. **Slim the tool side first**: tool output defaults to "summary + address + artifact reference" with per-call and per-run hard caps; tokens trimmed at the tool boundary are not left for Headroom. Retrieval v1 introduces no embeddings, see [09](09-code-search-and-knowledge-base.md).
8. **Auth/billing on separate planes**: API and subscription are two lanes; the subscription does not assume it can pass transparently through Headroom/LiteLLM and does not silently fall back to API, see [11](11-subscription-quota-and-agent-sdk.md).

## 5. Non-Goals

- Do not build an Agent Runtime, sandbox, IDE, or TUI in-house.
- Do not pursue fully unattended large tasks; v1 allows human intervention at exception points.
- Do not treat "compression ratio" as the core KPI; the core KPI is "cost per verified task".
- Do not do "LLM automatically judges task complexity and routes" in v1; only an explicit task-category → model-tier mapping.
- Do not replace pi / Codex / Headroom; they are swappable executors and gateways.
- No multi-tenancy, cloud SaaS, or team permission system (optional later).

## 6. What Needs Building (Minimal Set)

| Category | Content | Build/reuse | Notes |
|---|---|---|---|
| State protocol | `.loom/` directory, event log, TaskSpec, Receipt, Decision | **build** | Single source of truth; orchestrator single writer; see 03/08 |
| Orchestrator | Task graph, scheduling, budgets, escalation, recovery | **build** | Deterministic first, thin layer; see 02/08 |
| Context compilation | Recompile a bounded context on every call | **build** | The core of token savings; see 08 §6 |
| Tool I/O budgets | Limits, structure, spilling to disk, dedup | **build** | Before gateway compression; see 09 §8 |
| Acceptance runner | Run acceptance, collect evidence, handle flaky | **build** | Independent re-run; never trust the worker's own report |
| Ledger | Requests/tasks/costs/metrics | **build** | JSONL source of truth + SQLite index; see 03/04 |
| Prompt library | orchestrator / worker / judgment / review | **build** | Versioned, rendered at dispatch; see 07 |
| Runtime / executor | goose / OpenCode / native / pi / Aider | **reuse/swappable** | Process-level adapter; first integration is goose |
| Compression/cache | Headroom (cache mode) | **reuse** | No tuning, no stacked compressors |
| Routing | LiteLLM (from M3) | **reuse** | Sits behind Headroom |
| Isolation | git worktree + branch | **reuse** | One per task; M4 parallelism |
| Bypass | Aider | **optional** | Only for A/B / diff generation |

**No need to build**: a full Agent Runtime, sandbox, IDE/TUI, compressors, provider routing, multi-tenant platform.  
**Optional to build**: a minimal native executor (bash/read/edit/search) for full control over tool results and context; it is not a full runtime.

## 7. Minimal Loop (M0, 2–3 days)

1. `loom init` creates the `.loom/` skeleton and fills in a real small task's goal/constraints.
2. `loom task add` creates 1 slice; a human writes the acceptance command.
3. `loom dispatch T-001 --adapter goose` dispatches; the worker only writes code + a final message, and the adapter normalizes the receipt.
4. `loom verify T-001` independently re-runs acceptance; on pass an event is recorded.
5. `loom status` for the board; after `kill -9`, `loom resume` continues.
6. Record one CPVT/TPVT/WPT; repeat 10 times before talking about auto-planning, routing, or parallelism.

If this loop runs, most of the token savings are already captured; `context compiler`, `router`, and parallelism are amplifiers that come later.

## 8. Current Status

- Version: v0.2 (2026-11-04)
- Basis: prior requirements discussion v0.4 + local measured snapshots + prompt protocol measurements + current toolchain
- Focus of this version: from "pi plugin + hand-written files" to "standalone CLI + state ownership + swappable adapters"; see [08-design-roadmap-v0.2.md](08-design-roadmap-v0.2.md)
- M1 mostly landed: `loom task add/list/show/ready/graph`, `loom event list`, `loom context compile`, `loom prompt list/show`, `loom adapter list/canary`, `loom dispatch`, `loom verify`, `loom resume`, `loom usage ingest`; model capability bits in [06](06-technology-selection.md) §5.6, request ledger in [03](03-core-protocols.md) §12, schemas in [`schemas/`](../schemas/) (task-spec / event / receipt / context-manifest / usage-record / adapter-usage)
- M2 started: the `loom-native` standalone executor is wired into `--adapter native`, with an OpenAI-compatible tool loop, result projection, token/wall-clock/step/cost budgets, acceptance feedback, failure-packet injection, no-progress detection, and a scripted provider; see [08](08-design-roadmap-v0.2.md) §9.3
- M3 control-plane routing: `[routing]` + `loom route`, with role→tier, verify-failure escalation, tier cost guards, and pre-dispatch estimation; real-model A/B and the LiteLLM canary still await the environment; see [08](08-design-roadmap-v0.2.md) §9.4
- Current blockers and unclosed items: see [14-current-issues-and-blockers.md](14-current-issues-and-blockers.md)
- Items pending local verification: see the "open questions" section of [05-implementation-roadmap.md](05-implementation-roadmap.md) and the "compatibility checklist" of [07-reference-implementation-snippets.md](07-reference-implementation-snippets.md)
- Model names, ports, and CLI arguments in the docs are **examples/placeholders**; verify against local `--version`, `--help`, and official docs before relying on them.
