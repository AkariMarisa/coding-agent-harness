# 05 Implementation Roadmap

> Language: **English** | [简体中文](05-implementation-roadmap.zh-CN.md)

> **v0.2 revision (2026-11-04)**: the S stages have been replaced by M0 in [08-design-roadmap-v0.2.md](08-design-roadmap-v0.2.md); instead of building the pi plugin/manual files first, we first build a standalone CLI plus state ownership.  
> The acceptance thinking of P0–P4 is still reusable; stage numbering and timelines follow 08.

## 0. Principles

1. **Stop the bleeding first, then automate, then route, then parallelize.** No jumping ahead across stages.
2. **Each stage is independently rollback-able**; keep the previous version of config, prompts, and schema.
3. **Each stage has measurable exit criteria**; do not enter the next stage without meeting them.
4. **A single GPT still captures most of the gains**; multi-model is P2 business.
5. **Progress is not measured by tool count**; measure it with CPVT, first-pass verify, and cache hit.

## 1. Stage overview

| Stage | Time | Goal | New components | Exit criteria |
|---|---|---|---|---|
| S | 0–2 days | stop the bleeding with a single GPT | `.agent/` + prompt + manual subagent | 10 tasks all with receipts; main context ≤30k; baseline CPVT |
| P0 | week 1 | ledger and baseline | C11 + events + reconciliation | 100% attribution; error <5% |
| P1 | weeks 2–3 | thin runner | C1/C4/C6/C7/C13 | 20 tasks; 0 unverified merges; recoverable |
| P2 | week 4 | routing | C8 + LiteLLM | two tiers GPT + DeepSeek; worker ≥50%; CPVT −20%; no quality regression |
| P3 | weeks 5–6 | parallelism | C10 + locks + merge queue | start at 2 parallel; no lost updates; merge <5min |
| P4 | ongoing | optimization | board / A-B / tool governance | CPVT −60%; cache ≥75%; handoff ≤5% |

## 2. Stage S: stop the bleeding with a single GPT (0–2 days)

**Stage S does not install LiteLLM, does not connect DeepSeek, does not start a second proxy, does not write an SDK, and does not tune Headroom parameters.**

### S-1 Baseline freeze

- Record `pi / codex / headroom / plugin` versions into `.agent/env.lock`.
- Record the current config: `mode=cache`, port, model, reasoning level.
- Pick 5–10 recent tasks and record: request count, input, cache, output, cost, wall-clock.
- Compute the baseline CPVT (total cost / number of tasks passing acceptance).

**Deliverable**: a `runs/baseline-YYYYMMDD/` snapshot + a one-page baseline.

### S-2 State skeleton

Create:

```
.agent/
├── GOAL.md
├── spec.md
├── design.md
├── contracts/
├── tasks.json
├── tasks/
├── receipts/
├── reports/
├── PROGRESS.md
├── DECISION.md
├── budget.json
└── env.lock
```

- Fill in one real small task using the templates from 03.
- Run one pass of "can a fresh session that only reads `.agent/` recover everything".

**Exit**: a fresh session can state the goal, tasks, acceptance, and progress without depending on historical transcripts.

### S-3 Prompt protocol

- orchestrator prompt: planning/dispatch/acceptance only; no code writing; no transcript reading.
- worker prompt: one slice only; read only references; return only four lines; details go to the report.
- judgment prompt: reads only the evidence packet; outputs `fix_plan/split/reject/human`.
- Version the prompts into `prompts/`.

**Exit**: three dispatches all reliably yield four-line receipts; the parser passes.

### S-4 Manual subagent dispatch

- Dispatch with the existing `@tintinweb/pi-subagents` (or Codex subagent).
- Each worker has an independent context; still use sol (single-model stage).
- The orchestrator only receives four-line receipts; it does not receive worker transcripts.
- Each task gets its own worktree (if the existing tool supports it; otherwise an independent branch first).

**Exit**: 10 slices in a row, main session context ≤30k; no transcript pasted back.

### S-5 Tool result governance

- Tool result > 4k tokens: write to a file, return path + summary + line numbers.
- No pasting back of full JSON/HTML/logs/diff.
- Batch reads: one grep with context; read files by range.
- One read of the same file per task; repeated reads must justify themselves.
- Hard-code these rules in the worker prompt.

**Exit**: the `Δvalue/Δrequests` growth of `json_bloat` / `reread` declines; main context no longer blows up on tool results.

### S-6 Goal caps

- Set `automaticTurns` and `noProgressTurns` in `pi-goal.json`; tune them against reality.
- Reaching the cap means stopping: output status and evidence, never continue indefinitely.
- Budget: per-task/per-run caps go into `budget.json`; check them manually at first.

**Exit**: no "run forever" incidents; every stop has an explicit reason.

### S-7 Daily ledger (manual)

- Record daily: request count, input, cache, output, cost, number of tasks passed.
- Compute CPVT; record anomalies (retries, timeouts, human intervention).
- Compare against the baseline after a week.

**Exit**: a CPVT curve covering 5 consecutive working days; the main fluctuations can be explained.

### Stage S overall exit criteria

- All 10 tasks have a TaskSpec + four-line receipt + verify evidence.
- Main session context ≤30k; no worker transcript pasted back.
- 0 unverified merges.
- Baseline CPVT computable; 5 days of data.
- No Headroom mode/parameter changes; no multi-model connection.

## 3. Stage P0: ledger and baseline (week 1)

### Goal

Turn "manual bookkeeping" into an "automatic ledger" so that CPVT is trustworthy.

### Work items

1. Define the event/ledger schema (03 §11–12).
2. Collect usage from Runtime / LiteLLM / provider; mark missing fields `unknown`.
3. Implement `loom ledger`: aggregate by run/task/tier/model/day.
4. Implement `loom reconcile`: reconcile against provider bills.
5. Implement `loom report`: generate run reports (CPVT, cache hit, resend, handoff, verify).
6. Alerts: unattributed, cost over 80%, cache drop >10pp.

### Exit criteria

- 100% of requests have `run_id` + `task_id` (or explicitly `unattributed` and <1%).
- Ledger-to-provider-bill error <5%.
- A report is generated automatically for every run.
- It can answer: which task is most expensive, which tier is most expensive, how much retries cost.

## 4. Stage P1: thin runner (weeks 2–3)

### Goal

Turn the manual stage-S process into a repeatable runner: task graph, dispatch, verify, budget, escalation, recovery.

### Work items

1. `loom init`: validate `.agent/`, generate the task-graph skeleton, record env.lock.
2. `loom plan`: frontier produces the task graph (or a human writes it, the runner validates it).
3. `loom run`: Scheduler picks ready → creates worktree → dispatch → receipt → verify → merge.
4. Verify Runner: independently re-run acceptance; evidence written to disk; flaky handling.
5. Budget Governor: pre-dispatch estimate + in-flight usage + circuit breaker.
6. Escalation: evidence packet + judgment + one escalation.
7. `loom resume`: recover from events/state; do not re-run tasks that already passed.
8. `loom status`: task graph, budget, exceptions, pending human items.

### Exit criteria

- 20 tasks run through consecutively; 0 unverified merges.
- Budget overruns can stop; recovery within 5 minutes after a restart.
- Failed tasks have an evidence packet; no more than 1 escalation.
- Human intervention rate ≤30%.
- Main session context still ≤30k.

### Explicitly out of scope

- No multi-model routing (P2).
- No parallelism (P3).
- No complex UI; the CLI is enough.
- No automatic spec edits; spec changes must be human-confirmed.

## 5. Stage P2: routing (week 4)

### Goal

Without breaking cache/compression, bring in LiteLLM and route by explicit tier; verify whether worker models actually reduce CPVT. Early API keys cover GPT plus several Chinese model providers — first validate tier routing with GPT high tier + DeepSeek low tier; add Kimi/MiniMax/MiMo gradually through A/B, which does not affect this stage's exit criteria.

### Work items

1. Deploy LiteLLM (local/container); configure aliases `frontier` / `worker` / `worker-backup`.
2. Change the topology to `agent → Headroom → LiteLLM → provider`; keep a direct-connection rollback path.
3. Compatibility matrix (07): streaming, tool calls, usage, errors, Anthropic/OpenAI/Responses.
4. Pilot workers on 3 task types: survey, spec-tests, single-module implement.
5. Routing guardrails: no verify, no worker; unclear spec, no worker; security/cross-module default to frontier.
6. The ledger records alias + real model; aggregate CPVT by tier.
7. A/B: same task set, baseline (all frontier) vs treatment (worker pilot).

### Exit criteria

- `by_model` shows both frontier and worker; `routing` is non-empty.
- Workers take ≥50% of the pilot implementation tasks.
- CPVT down ≥20% vs P1 (roughly −40% cumulative vs the original baseline).
- First-pass verify down no more than 5pp; defect escape does not rise.
- Gateway failure can roll back to direct connection; no tool call/streaming compatibility incidents.

### Explicitly out of scope

- No LLM-based automatic complexity routing.
- No third tier of models.
- No separate Headroom per provider.
- No compression-mode switching by the `model` field.

## 6. Stage P3: parallelism and worktrees (weeks 5–6)

### Goal

Without lost updates or conflicts, first get 2 workers running stably in parallel; 3-way parallelism must be proven not to regress `WPT`/`CPVT` by A/B.

### Work items

1. Per-task independent git worktree + branch; short paths, cleanable.
2. Path locks: read/write locks, leases, TTL, owner checks.
3. Merge queue: serial, rebase, verify before merge, integration verify after merge.
4. Task-graph dedup: same input hash + same output path never runs in parallel.
5. Conflict handling: small conflicts auto-rebase; big conflicts go to `human` or the task is split.
6. Parallel budgets: total run budget + per-task budget; avoid amplified overspend from parallelism.

### Exit criteria

- 2 workers run 10 tasks in parallel; no lost updates/overwrites.
- Merge conflict rate <10%; merge duration <5 minutes.
- Integration verify 100% pass; failures can be rolled back.
- Cache hit does not drop >10pp because of parallelism.
- 3-way parallelism only enabled with A/B evidence, and file write sets fully disjoint.

### Explicitly out of scope

- No cross-machine distributed scheduling.
- No automatic resolution of semantic conflicts.
- No lock-free parallel writes to the same path.

> **Implementation status (2026-11-08)**: path locks, `[parallel] max_running` / `max_inflight_cost_usd`, the merge queue (`loom merge`: global `**` lock → verify before merge → rebase → merge → integration verify → rollback on failure), and the board (`loom board`) are in place. The exit criteria "2 workers run 10 tasks, no lost updates, merge <5 minutes" was measured passing by `scripts/smoke-parallel.sh` (10 tasks / 2 concurrent / 0 failures / 10 merges under 5 minutes). "Same-input-hash dedup" is covered by the path locks — intersecting output paths are blocked on their own; the cache-hit impact and the 3-way parallel A/B still need a real provider. See [08](08-design-roadmap-v0.2.md) §9.5.

## 7. Stage P4: optimization (ongoing)

### Work items

1. Tool result projection: JSON schema → keep only fields; logs → summary + path; HTML → body text.
2. Read cache/index: file hashes, symbol index, incremental reads.
3. Prompt tuning: shorter worker instructions, a clearer output contract.
4. Model ladder: let data decide whether to add a third tier; not gut feeling.
5. Board: CPVT, resend, cache, handoff, verify, escalation, human touch.
6. Weekly A/B; monthly reconciliation; quarterly architecture review.

### Exit criteria

- CPVT −60% vs baseline.
- TPVT −60% vs baseline.
- Cache hit ≥75% (≥80% long-term).
- Handoff overhead ≤5%.
- First-pass verify ≥70%.
- No "optimization caused quality to drop" regressions.

## 8. FR → stage matrix

| FR | S | P0 | P1 | P2 | P3 | P4 |
|---|---|---|---|---|---|---|
| FR-01 externalized state | ✅ |  |  |  |  |  |
| FR-02 slicing/task graph | ✅ |  | ✅ |  |  |  |
| FR-03 TaskSpec | ✅ |  | ✅ |  |  |  |
| FR-04 four-line receipt | ✅ |  | ✅ |  |  |  |
| FR-05 delivery by reference | ✅ |  |  |  |  | ✅ |
| FR-06 no transcript relay | ✅ |  | ✅ |  |  |  |
| FR-07 Verify contract | ✅ partial |  | ✅ |  |  |  |
| FR-08 one failure escalation |  |  | ✅ |  |  |  |
| FR-09 budget/circuit breaker | ✅ partial |  | ✅ |  |  |  |
| FR-10 parallel isolation |  |  |  |  | ✅ |  |
| FR-11 explicit routing |  |  |  | ✅ |  |  |
| FR-12 gateway order/compression ownership | ✅ |  |  | ✅ |  |  |
| FR-13 cache-friendly scheduling | ✅ partial |  | ✅ |  |  | ✅ |
| FR-14 cost ledger | manual | ✅ | ✅ | ✅ | ✅ | ✅ |
| FR-15 metrics/A-B | manual | partial | partial | ✅ | ✅ | ✅ |
| FR-16 recoverable/auditable |  | partial | ✅ | ✅ | ✅ | ✅ |
| FR-17 permissions/keys/sandbox | partial |  | ✅ |  | ✅ |  |
| FR-18 extensible/versioned |  |  | partial | ✅ |  | ✅ |

## 9. Risk register

| ID | Risk | Probability | Impact | Mitigation | Trigger signal |
|---|---|---|---|---|---|
| R1 | cheap models unstable on long chains, CPVT rises instead | high | high | short slices, verify, at most one escalation; pilot on 3 task types first | first-pass <50% or rework >20% |
| R2 | state files contaminated by wrong conclusions | high | high | decision log records trade-off reasons; append-only; periodic review | later tasks repeatedly cite a wrong decision |
| R3 | over-slicing, communication/scheduling overhead eats the gains | medium | medium | 1–4 hours / ≤8 files; handoff ≤2k; monitor overhead | handoff >10% or task count explodes |
| R4 | cache invalidation, cost rises | high | high | stable prefix, append-only, no model/prompt switch within a run | cache hit <70% |
| R5 | verify coverage insufficient, false passes | high | high | spec-tests first; human review of critical tasks; mutation/contract tests | defect escape rises |
| R6 | gateway compatibility (streaming/tool/usage) | medium | high | compatibility matrix, canary, direct-connection rollback | tool call failure / missing usage |
| R7 | version drift (pi/codex/headroom/litellm) | medium | medium | lock versions, upgrade windows, compatibility tests | error rate/cost anomaly after an upgrade |
| R8 | ledger disagrees with the bill | medium | medium | provider reconciliation, source priority, keep raw rows | error >5% |
| R9 | prompt injection / dangerous commands | medium | high | allowlist, path allowlist, no network, frontier review | out-of-scope writes / network requests |
| R10 | Windows path/encoding problems | low | medium | quoting, UTF-8, short worktree paths, CI on Linux | path too long / mojibake |
| R11 | orchestration itself burns too many tokens | medium | medium | small contexts, deterministic scripts, batched operations | scheduling overhead >2% |
| R12 | humans become the bottleneck | medium | medium | escalate only anomalies; evidence packets; SLA | human touch >30% |
| R13 | parallelism causes overwrites/conflicts | medium | high | path locks, single writer, merge queue | lost updates / conflict rate >10% |
| R14 | stacked compressors lose the original text | medium | medium | single compression ownership; retrieve availability tests | cannot retrieve / prefix changes |

## 10. Open questions

| ID | Question | Impact | Suggestion | Due |
|---|---|---|---|---|
| Q1 | Main runtime: pi or Codex? | adapter/toolchain | pi as orchestrator + worker; Codex as optional worker/fallback | stage S |
| Q2 | Can LiteLLM pass through OpenAI Chat/Responses and Anthropic Messages at the same time? | P2 topology | build the compatibility matrix; if unsupported, degrade per client | before P2 |
| Q3 | Does pi subagent support per-task model selection and usage pass-back? | routing/ledger | read docs + measure; if unsupported, use a CLI adapter | before P1 |
| Q4 | Current field for Codex subagent model override? | routing | follow the local `--version`/config | before P2 |
| Q5 | Does usage include cached tokens/reasoning tokens? | cost measurement basis | verify per provider; if missing, estimate and label it | P0 |
| Q6 | Is git worktree stable under long Windows paths? | parallelism | short paths + measurement; use WSL/containers if needed | before P3 |
| Q7 | Trust boundary of verify commands? | security | allowlist + no network + sandbox; do not accept ad-hoc worker commands | P1 |
| Q8 | How to reliably estimate `unique_new`? | resend metric | estimate from provider bill difference + runtime new content; document the method | P0 |
| Q9 | Which task categories suit workers? | routing | start with survey/spec-tests/single-module implement; let data decide | P2 |
| Q10 | Do we need SQLite/DuckDB? | ledger queries | JSONL first; add it when queries get complex | P4 |
| Q11 | Multi-user/remote support? | architecture | single machine first; keep the protocol extensible | not yet |
| Q12 | Is Aider worth keeping as a bypass? | selection | only for A/B/diff generation; not on the main path | after P2 |

## 11. Decision record (deltas from prior discussions)

| Version | Original plan | Correction | Reason |
|---|---|---|---|
| v0.1 | two Headroom instances (8787/cache, 8788/token), split by model | **retracted** | `mode` is process-level; splitting by model does not hold; cache gains far exceed compression |
| v0.2 | LiteLLM before Headroom | **moved after** | Headroom forwards transparently; LiteLLM handles aliases; multiple providers do not need multiple Headrooms |
| v0.2 | 6 agents / multiple model tiers | **converged to two tiers** | in practice only sol exists; prove worker gains first |
| v0.3 | `reread_compressed` as an acceptance metric | **disabled** | the attribution logic is not in place; cumulative counts are not directly comparable |
| v0.4 | connect multi-model first | **changed to stop the bleeding with a single GPT first** | history resend is the main cause; a single GPT can also lower CPVT |
| this doc | build a complete harness ourselves | **build only a thin orchestration layer ourselves** | Runtime/compression/routing already have mature components |

## 12. Migration steps

1. **Freeze the baseline**: record versions, config, and data for 5–10 tasks.
2. **Add `.agent/`**: no runtime changes, no Headroom changes; only state files and prompts.
3. **Manual dispatch**: use the existing subagent, run 10 slices; verify the four-line receipts.
4. **Add the ledger**: events/ledger/reconciliation; confirm CPVT.
5. **Add the runner**: task graph/verify/budget/escalation/recovery.
6. **Add LiteLLM**: topology becomes Headroom → LiteLLM; low-traffic canary.
7. **Add parallelism**: worktrees/locks/merge queue.
8. **Keep optimizing**: tool governance, A/B, board.

Every step is rollback-able: keep the previous config; LiteLLM failure can go direct; runner failure can go back to manual dispatch.

## 13. Operations manual

### Daily

- `loom status`: running / blocked / needs_input / budget.
- Look at exceptions: failures, over-budget, conflicts, unattributed.
- Handle only tasks that need a human; do not touch normal tasks.
- Record that day's CPVT and anomalies.

### Weekly

- Roll up CPVT, cache hit, resend, handoff, first-pass verify, escalation.
- Compare with last week; explain swings >10%.
- Review `DECISION.md`: has any wrong conclusion been inherited?
- Check waste-signal growth (Δ/Δrequests), not cumulative values.
- Decide whether to adjust task granularity/model mapping.

### Monthly

- Reconcile against the provider bill; investigate errors >5%.
- Review the risk register; update trigger signals.
- Assess whether to enter the next stage; do not skip without meeting criteria.
- Clean up worktrees/logs; archive runs.

## 14. Definition of Done (DoD)

A stage is "done" only when all of the following hold:

- Function: all acceptance for that stage's FRs passes.
- Cost: CPVT meets the stage target, or at least does not regress.
- Quality: first-pass verify / defect escape do not regress.
- Recoverable: recovery after a kill; no re-running of passed tasks.
- Auditable: every task has a TaskSpec, receipt, verify evidence, and ledger entries.
- Rollback-able: explicit rollback steps exist and have been rehearsed.
- Docs: protocol/config/prompts updated, versions recorded in `env.lock`.
