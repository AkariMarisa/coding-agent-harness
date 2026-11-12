# 04 Context and Cost Model

> Language: **English** | [简体中文](04-context-and-cost-model.zh-CN.md)

## 1. Why tokens blow up

### 1.1 The basic formula

Let:

- `P` = system prompt + tool schemas (sent every turn)
- `U` = user task/context (sent every turn)
- `A` = new assistant/tool content added each turn
- `T` = number of turns

Billed input for a naive long conversation is approximately:

```
BilledInput(T) = Σ_{t=1..T} [P + U + (t−1)·(U + A)]
              = T·(P + U) + (U + A)·T(T−1)/2
```

The first term is linear, the second is **quadratic**. The larger T is, the more the quadratic term dominates.

### 1.2 Magnitude example

Assume `P+U = 20k` and `U+A = 5k`:

| Turn T | Linear term | Quadratic term | Total input | vs T=10 |
|---:|---:|---:|---:|---:|
| 10 | 200k | 225k | 425k | 1× |
| 30 | 600k | 2.18M | 2.78M | 6.5× |
| 100 | 2.0M | 24.75M | 26.75M | 63× |

Measured 154.0M / 5.94M ≈ **25.9×** resends; the magnitude matches.

### 1.3 Why slicing works

Split a large task into:

- 1 orchestrator session with small `T_main` (reads only receipts/decisions);
- N workers, each with small `T_worker` (3–8 turns) and an independent context.

Total input goes from:

```
T_total·(P+U) + (U+A)·T_total(T_total−1)/2
```

to:

```
T_main·(P_main+U_main) + (U_main+A_main)·T_main(T_main−1)/2
+ Σ_i [T_i·(P_i+U_i) + (U_i+A_i)·T_i(T_i−1)/2]
```

The key point: **workers do not share history**, so there is no cross-worker quadratic term. Each worker's `T_i` is small, flattening the quadratic term; the orchestrator's `T_main` is also small because it only appends four-line receipts.

> This is not as simple as "split the task into multiple conversations". The session is still one long task, but the **context has a short lifetime**.

## 2. Waste categories

| Category | Typical signal | Measured/magnitude | Mechanism | Countermeasure | Owner |
|---|---|---|---|---|---|
| History resend | billed/unique 25.9× | main bill | long session resends every turn | short context + file state | C1/C3 |
| Tool result bloat | `json_bloat` 60.6M | 83% waste | raw JSON/logs entering toolResult | references + projection + truncation | C2/C12 |
| Repeated reads | `reread` 5.67M | cumulative | full-file read after grep | read cache + batching + range | C2 |
| Markup noise | `html_noise` 4.05M | cumulative | web pages/logs entering context verbatim | clean/summarize/reference | C2 |
| base64 | `base64` 1.18M | cumulative | images/attachments inlined | store to file + reference | C2 |
| Whitespace | `whitespace` 1.50M | cumulative | formatted text | compact/normalize | C9 |
| Agent communication | not measured separately | possibly 5–20% | transcript paraphrase | four-line receipts + references | C5 |
| Over-compression | compression saves 0.70% | $2.35 | lossy compression breaks the cache | keep cache | C9 |
| Wrong tiering | sol only | unit-price waste | all tasks go frontier | explicit tier | C8 |
| Rework | not measured | hidden | insufficient verify / spec drift | spec-tests + verify | C6 |
| Duplicated work | not measured | hidden | overlapping parallel tasks | task-graph dedup + locks | C4/C10 |
| Ineffective retries | not measured | hidden | retries without classification | error classification + circuit breaker | C1/C7 |

### 2.1 Priorities

Ordered by "attackability × impact":

1. **History resend** (largest, most certain) → slicing + externalized state.
2. **Tool result bloat** (largest single waste signal) → references + projection.
3. **Agent communication** (new in multi-agent scenarios) → four-line receipts + no transcripts.
4. **Repeated reads** (easy to govern) → read cache + batching.
5. **Wrong tiering** (second-order) → routing.
6. **Compression parameters** (lowest ROI) → leave them alone.

## 3. Context budget

### 3.1 Budget table

| Area | Cap | Content | Over-limit strategy |
|---|---:|---|---|
| Orchestrator system | 8k | role, rules, protocol, tools | fixed; not changed per task |
| Orchestrator goal | 2k | GOAL summary + acceptance | fixed |
| Orchestrator task graph | 6k | task index + status | fold completed tasks |
| Orchestrator receipts | 4k | last 3–5 four-line receipts | archive old receipts |
| Orchestrator decisions | 6k | decision index + one line each | keep only accepted |
| Orchestrator reserve | 4k | current decision/exceptions | — |
| **Total** | **30k** | | |
| Worker spec | 2k | TaskSpec | split the task |
| Worker refs | 40k | must_read + reads on demand | cut refs / split the task |
| Worker tool results | 50k | projection / truncation / references | write to file |
| Worker output | 10k | code / diff / commands | batch |
| Worker reserve | 18k | errors / retries | — |
| **Total** | **120k** | | |
| Judgment evidence | 40k | failed log snippet + diff + decisions | truncate + paths |
| Report | no cap | human-readable details | orchestrator does not load it |

> These are **budgets**, not targets. Smaller is better; past 80% you should stop and write state, not compress silently.

### 3.2 Folding strategy

- The orchestrator keeps only the last 3–5 receipts; older ones fold into a single line of "task ID + status + evidence path".
- Decisions keep only accepted ones plus an index of superseded ones; details live in `DECISION.md`.
- The task graph keeps only unfinished plus recently completed tasks; archived tasks go to `runs/`.
- Workers do not fold history; when context exceeds the limit they stop and write `needs_input` or `budget_exceeded`.
- "Silent automatic summarization" may not replace original evidence; summaries may only append, never overwrite.

### 3.3 Read strategy

- Locate with `grep` first, then read by `range`; do not read whole files first.
- Batch-read several related files at once; do not "read one, think, read another".
- Read the same file once per task; re-lookups use line numbers/symbols.
- For large files read only signatures/types/interfaces; implementation details on demand.
- No `cat` of a whole repo/whole log; use `head/tail/grep` + references.
- Tool results past a threshold (e.g. 4k tokens) → write to a file, return path + summary + line numbers.

## 4. Cache strategy

### 4.1 Prefix order

```
[system prompt] → [tool schemas] → [GOAL summary] → [task-graph summary] → [receipts/decisions appended]
```

- The more stable it is, the further forward; the more volatile, the further back.
- Within a run do not change system / tools / goal; the task graph only appends/folds, never reorders.
- No timestamps, random IDs, or dynamic dates in the prefix.
- Tool schema order is fixed; do not reorder per request.

### 4.2 Cache invariants

| Invariant | Consequence of violation |
|---|---|
| system prompt unchanged within a run | prefix cache fully invalidated |
| tool list/order unchanged | cache key changes |
| model unchanged | cache is isolated per model |
| messages append-only; early messages never edited | cache prefix invalidated |
| temperature/parameters stable | some providers include parameters in the cache key |
| no dynamic context injected at the start | miss every turn |

### 4.3 Parallelism and cache

- The main session is serial; parallel workers do not affect the main session prefix.
- Each worker has its own prefix; do not give all workers different system versions.
- Too much concurrency on one model hits rate limits; use LiteLLM queueing/backoff, do not let workers start their own retry storms.
- **Cap concurrency at 2 first**; going past 3 requires both "file ownership fully disjoint" and "an A/B proving `WPT`/`CPVT` do not regress" at the same time. External measurements put multi-agent at roughly 4–15× the tokens of a single agent, fan-out commonly 2.6–5.9×, and not necessarily faster; our own measurements have also shown concurrency dropping and the whole run getting slower.
- Parallel tasks must have disjoint write sets; shared files such as `schema`, lock files, migrations, and dependency manifests run serially or are merged by the orchestrator.
- Subagents notify once on completion; do not poll for progress.

### 4.4 Metrics

- `cache_hit = cached_input_tokens / input_tokens`
- Target ≥ 75%; alert on a drop > 10pp.
- Do not treat cached tokens as free; bill them at the cached price.
- Record cache savings separately; do not add them to compression savings.

## 5. Compression strategy

### 5.1 Current conclusion

Keep Headroom at `mode=cache` and do not touch the compression parameters. Reasons:

- compression saves only 0.70% ($2.35) against a cache discount of $270.63;
- `token` mode rewrites history and breaks prefix stability;
- `size_floor` skips about 71% of small results; lowering the threshold buys little and adds latency/risk;
- compression only helps full-price new input; rewriting an already-cached prefix triggers a full prefill and may cost more the more you compress;
- the real waste is at the tool layer and in history resend, not in the live-zone compression ratio.

### 5.2 ROI formula

Whether compression is worth it is not a question of compression ratio; it is a question of:

```
CompressionSaving ≈ tokens_removed × p_in × (1 − cache_hit)
CacheLoss        ≈ prefix_tokens × p_in × Δcache_hit
Net              = CompressionSaving − CacheLoss − extra latency/failure cost
```

Only tune compression when `Net > 0` and that holds sustainably. Right now `CacheLoss` far exceeds `CompressionSaving`.

### 5.3 Compression ownership

| Path | Compressor | Rule |
|---|---|---|
| pi + plugin | pi-headroom plugin + Headroom proxy | treat as one controlled chain; no LiteLLM guardrail stacked on top |
| Codex | Headroom proxy | do not enable the pi plugin |
| Aider | Headroom proxy (cache) | do not rely on retrieve |
| Claude Code | Headroom proxy (cache) | no other compressor stacked on top |

**Hard rule**: one request may have only one "compression decision-maker". Stacking multiple compressors leads to:

- original text lost, no way to retrieve;
- prefix changes, cache invalidation;
- debugging difficulty, unclear ownership.

### 5.4 When it may be touched

Consider it only when all conditions hold:

1. cache hit stably ≥ 75%;
2. tool-layer governance is done (references/projection/truncation);
3. an A/B proves CPVT drops;
4. a retrieve/rollback path exists;
5. canary only on non-critical paths.

Otherwise: **do not touch it**.

## 6. Routing strategy

### 6.1 Two model tiers

| tier | Tasks | Model examples | Context | Call frequency |
|---|---|---|---|---|
| frontier | goal decomposition, design, judgment, critical review | gpt-6.1-sol @ medium | small (≤30k) | low |
| worker | survey, spec-tests, implement, mechanical refactoring, docs | DeepSeek / Kimi / MiniMax / MiMo (start with 1 primary + 1 fallback) | medium (≤120k) | high |

> **Early-stage constraint**: our API keys cover GPT and several Chinese model providers. First validate tier routing with GPT `frontier` + DeepSeek `worker`; add Kimi/MiniMax/MiMo gradually through A/B. The subscription lane comes later and does not affect M0–M3.

### 6.2 Task category matrix

| Task | tier | Rationale | Exception |
|---|---|---|---|
| Goal/scope decomposition | frontier | needs judgment and trade-offs | — |
| Architecture/interface contracts | frontier | mistakes are expensive | a worker may draft, frontier reviews |
| Test specs | worker | verifiable, re-runnable | security/payment-critical reviewed by frontier |
| Single-module implementation | worker | short, verifiable | cross-module/concurrency/security-critical → frontier |
| Mechanical refactoring/formatting | worker | low judgment | — |
| Docs | worker | low risk | external-contract docs reviewed by frontier |
| verify-failure judgment | frontier | needs diagnosis | — |
| Final review | frontier | gatekeeping | low-risk may be sampled |

**Spec work is decision work**: implementers start from a fresh context, and anything not written in the brief has to be rediscovered or guessed; when the spec is wrong, the error multiplies by the task count. Therefore:

- specs / interface contracts / acceptance criteria are finalized by frontier; cheap models only draft candidates, and those must pass deterministic verify or human sign-off;
- long-context models fit "full-repo scan → structured candidates"; they do not produce the final spec directly;
- scan results do not enter every turn's context; only numbered, short, stable ACs go into the TaskSpec;
- ACs must at least cover the happy path, failure paths, permissions/empty states/duplicate requests, and non-goal protection;
- acceptance evidence must map back to AC numbers.

### 6.3 Routing guardrails

- Tasks without deterministic verify may not go to worker.
- Tasks with unclear specs (`ambiguous_spec`) may not go to worker.
- Cross-module/security/concurrency/data migrations default to frontier.
- Two worker failures → frontier judgment; workers may not "escalate" on their own.
- Same-tier fallback is fine; cross-tier escalation must go through judgment.
- Every routing change is recorded in `DECISION.md`.

> Implementation status (2026-11-06): same-tier fallback is in place — `[routing.fallbacks]` switches in order only on availability failures (exit code 4 / timeout) and writes `task.fallback`; cross-tier aliases fail outright at config-load time. Routing changes are currently recorded in `events.jsonl` (`task.dispatched.data.route` / `task.escalated` / `task.fallback`); there is **no** separate `DECISION.md`.

### 6.4 Gains and risks

| Gain | Risk | Validation |
|---|---|---|
| lower unit price | cheap models need more turns/rework, CPVT rises instead | A/B on the same task set |
| cheap workers in parallel | quality drops, review cost rises | first-pass verify + defect escape |
| less frontier context | decomposition quality drops | task-graph quality / rework rate |

> Conclusion: multi-model is not "plug it in and save". It must be validated with CPVT + quality guardrails; if workers need >2× the turns or the rework rate rises noticeably, it is not worth it.

### 6.5 Model capability bits (no hard-coded model names)

Models iterate fast; forced thinking, `reasoning_content` pass-back, model ID retirements, and shifts in cache pricing windows all rot a config with hard-coded model names quickly. Record capability bits at selection time:

| Capability bit | Values | Impact |
|---|---|---|
| `tier` | `frontier` / `worker` | task category → tier routing; start with two tiers |
| `thinking_mode` | `off` / `optional` / `forced` | models with forced thinking do not enter multi-turn tool chains unless there is a passthrough channel |
| `reasoning_passthrough` | `required` / `optional` | `reasoning_content` must be passed back verbatim; the compressor may not rewrite it |
| `wire` | `chat` / `responses` / `messages` | decides whether gateway, compressor, and billing cover it |
| `context_window` / `max_output` | numeric | slicing and budgets |
| `input_usd_per_mtok` / `cached_input_usd_per_mtok` / `cache_write_usd_per_mtok` / `output_usd_per_mtok` | numeric | the main term of real cost; cache prices recorded separately from list prices |
| `model_id` | fixed version or date | a change requires re-running the acceptance baseline |
| `pricing_version` | string | price-source version, traceable at reconciliation |

Rules:

- the execution layer prefers models that **can turn thinking off, speak a clean protocol, and do a stateless single call**;
- start with **1 primary + 1 fallback**, expand after A/B by task type; do not onboard four vendors at once;
- aliases must map to a real model + capability bits; the ledger records both the alias and the real model;
- config placement and check commands are in [06](06-technology-selection.md) §5.6; leave prices empty when unknown, never fill the ledger with guessed values.

## 7. Bookkeeping and metrics

### 7.1 Definitions

| Metric | Formula | Notes |
|---|---|---|
| `billed_input` | Σ input_tokens | billed input |
| `cached_input` | Σ cached_input_tokens | the cached portion |
| `unique_new` | tokens of new content (estimate / bill difference) | truly new |
| `resend_ratio` | `(billed_input − unique_new) / billed_input` | share of history resend |
| `context_amplification` | `billed_input / unique_new` | amplification factor |
| `cache_hit` | `cached_input / billed_input` | cache hit rate |
| `handoff_overhead` | `handoff_tokens / total_tokens` | communication overhead |
| `CPVT` | `total_cost / verified_tasks` | **primary cost KPI** |
| `TPVT` | `billed_input / verified_tasks` | **primary token KPI** |
| `first_pass_verify` | tasks passing first verify / total tasks | quality |
| `escalation_rate` | escalated tasks / total tasks | escalation abuse |
| `budget_exceeded_rate` | over-budget tasks / total tasks | budget sanity |
| `human_touch_rate` | tasks needing human intervention / total tasks | degree of automation |
| `rework_rate` | reworked tasks / total tasks | spec/acceptance quality |

### 7.2 Targets

| Metric | Baseline | First-phase target | Long-term target |
|---|---:|---:|---:|
| CPVT | TBD | −40% | −60% |
| TPVT | TBD | −50% | −60% |
| resend_ratio | ~96% | −20pp | −40pp |
| cache_hit | ~75% | ≥75% | ≥80% |
| handoff_overhead | TBD | ≤5% | ≤3% |
| first_pass_verify | TBD | no decline | ≥70% |
| escalation_rate | TBD | ≤20% | ≤10% |
| human_touch_rate | TBD | ≤30% | ≤15% |

> Baselines must be resampled with the same task set and the same measurement basis; do not just apply this document's historical snapshots.

### 7.3 Cost measurement basis

Priority:

```
provider bills > LiteLLM logs > Runtime usage > Headroom stats
```

- `with_headroom_usd` is only for single-snapshot comparisons; do not mix it with lifetime cumulative figures.
- Record `provider_cache_discount_usd` separately; do not add it to compression savings.
- `WasteSignals` is a cumulative detection count, not "already saved"; A/B looks only at the growth rate `Δvalue / Δrequests`.
- The attribution for `reread_compressed` is not in place, so it cannot serve as a criterion for compression failure; `share_of_saved_percent` can be distorted by a wrong denominator and is disabled.
- For client attribution look at `source` in `agent_usage.agents[]`; `requests.by_client` may be empty.
- Each run generates `cost_report.json`, aggregated by tier/model/task.
- Reconcile against the provider bill monthly; investigate attribution when the error is >5%.

### 7.4 A/B design

1. Pick 10–20 tasks of the same type; fix spec/verify.
2. Randomize into two groups: baseline / treatment; or run the same task twice (order randomized).
3. Record CPVT, first-pass verify, escalation, human touch, defect escape.
4. Use paired comparisons (per-task differences), not lifetime cumulatives.
5. At least 1 week or 20 tasks; write the conclusion into `DECISION.md`.
6. If treatment CPVT drops but quality drops, count it as a failure.

### 7.5 Cost model example

Assume (example only):

- frontier: $10/M input, $30/M output;
- worker: $1/M input, $2/M output;
- one task: 100k input, 10k output.

| Option | Input cost | Output cost | Single run | If it passes in 1 run | If it passes in 3 runs |
|---|---:|---:|---:|---:|---:|
| frontier | $1.00 | $0.30 | $1.30 | $1.30 | $3.90 |
| worker | $0.10 | $0.02 | $0.12 | $0.12 | $0.36 |

But a worker may:

- need more turns (longer context);
- need frontier review (+$0.20–0.50);
- need rework/human intervention (hidden cost).

So the criterion is **CPVT**, not unit price. It is only worth it if a worker passing in 3 runs + review is still below frontier passing in 1 run.

## 8. Conclusions

1. **Cut the area first, then the unit price.** History resend is the largest, most certain target.
2. **Tool-layer governance is worth more than compression tuning.** json_bloat 60.6M is the largest single item.
3. **Multi-agent must be hub-and-spoke.** Mesh communication eats the saved tokens right back.
4. **verify is the shared anchor of cost and quality.** Without verify, cheap models only manufacture rework.
5. **Cache before compression.** $270 vs $2.35 — do not get it backwards.
6. **Only CPVT counts.** Unit price, compression ratio, and cache rate are explanatory variables.
