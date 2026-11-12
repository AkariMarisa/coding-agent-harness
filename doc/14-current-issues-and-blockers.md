# 14 Current Issues and Blockers (2026-11-06)

> Language: **English** | [简体中文](14-current-issues-and-blockers.zh-CN.md)

> This document only records what "cannot be proven / cannot be done yet", so that design completion is not mistaken for acceptance completion.  
> Related roadmap: [08-design-roadmap-v0.2.md](08-design-roadmap-v0.2.md) §9.

---

## 1. Conclusion

- The M0/M1/M2 code loop is closed and passing; M3 control-plane routing has landed.
- The real gap is not code but **environment and real traffic**: the container has no goose/OpenCode/LiteLLM, and no provider API key or bill, so the external adapter canary, native vs adapter A/B, and CPVT calibration cannot be completed yet.
- So today we can say "the routing protocol works"; we cannot say "the M3 exit criteria are met".

---

## 2. Environment blockers

| Blocker | Impact | Status / workaround |
|---|---|---|
| No `goose` in the container | cannot run a real goose adapter canary; M1/M2's "real external adapter" exit criteria are dangling | only mock/native so far; add the binary to the image or mount it |
| No `opencode` in the container | cannot run a second external adapter canary | the host has `/usr/bin/opencode`, but the container cannot see it |
| No LiteLLM service in the container | M3's LiteLLM routing, usage logs, and bill reconciliation cannot be verified in place | only design so far; routing can first be verified with native direct connection to an OpenAI-compatible endpoint |
| No provider API key | cannot run real models, real usage, real failure distributions | offline tests uniformly use the `LOOM_NATIVE_SCRIPT` scripted provider |
| No provider bill/usage export | `cost_source=provider` cannot be measured for real; costs can only be estimated from a price table | local estimates are tagged `pricing_table`, unknown prices tagged `unknown` |
| The subscription lane needs the account holder to sign in | subscription vs API quota / rate-limit comparison cannot be automated | deferred by design; does not block M0–M3 |

> The container build only guarantees Rust + git; `curl` is not in the image either. For verifications involving the network or external binaries, first extend the image or mount explicitly, then talk about "canary passed".

---

## 3. M2 unclosed items

1. **Real goose / OpenCode canary**: passing mock/native does not mean the external adapter contract passes. Once the binaries exist, re-run `loom adapter canary <kind> --live --task ...`.
2. **native vs adapter A/B**: needs the same task set, real models, identical budgets, compared by CPVT/TPVT/wall clock; no baseline data yet.
3. **Cost budget calibration**: `ModelCapability::estimate_cost` is only a local price-table estimate; real bills may include cache writes, reasoning, tiered prices, and time-band discounts — not yet reconciled.
4. **Long-term effect of failure packets**: acceptance feedback within a run and the previous run's `FailurePack` are implemented, but there is no "with failure packet vs without" comparison of CPVT / first-pass rate.

---

## 4. M3 landed and unclosed

### 4.1 Landed

- `[routing]` configuration: `roles`, `escalate_after_failures`, `max_escalations_per_task`, `fallbacks`, `cost_limits`.
- Deterministic routing: explicit `--model` > configured role→tier > the task's own tier; no LLM call to judge difficulty.
- Escalation: triggered only by the count of `task.failed` events; by default 2 worker failures escalate to frontier, at most 1 per task; writes `task.escalated`.
- Cost guardrail: dispatch is rejected when the task budget exceeds the tier's cap, or when the worst-case cost estimated from the price table exceeds the cap.
- Explainable: `loom route TASK` shows tier, model, reason, fallback, budget, estimate, and issue list; `loom route stats` counts worker share, escalation count, fallback count, and routing reasons from the event log.
- `dispatch` allows retrying `failed`/`escalated` tasks and records the decision in `TaskDispatched.data.route`.
- Same-tier fallback: switches only on availability failures (exit code 4 / timeout), following the `[routing.fallbacks]` order; cross-tier is rejected at config load time; an explicit `--model` never switches; every switch writes `task.fallback` and opens a separate run for the new attempt; `attempt` stays unchanged, so escalation thresholds are not polluted.

### 4.2 Not closed

| Item | Gap | Next step |
|---|---|---|
| worker share ≥50% | the accounting is measurable (`loom route stats`), but a real task set is missing | run role→tier on a fixed task set, check `tasks.worker_share` |
| CPVT −20% | needs real bills and a same-task-set comparison | run worker-only vs role→tier as two groups, compare by `loom ledger` |
| no quality regression | needs first-pass rate / verify pass rate / defect escape | fixed task set; record first-pass verify and manual rework |
| real fallback effectiveness | verified only on mock; real providers' 5xx / rate-limit distribution unknown | once keys are available, run A/B with fault injection or real jitter, watch `route stats.fallbacks` and false-escalation rate |
| LiteLLM canary | no service, no bill logs | after adding a LiteLLM service, verify model alias, usage pass-through, error codes |

---

## 5. Known technical debt and risks

1. **native supports only the chat wire**: models with `wire=responses/messages` cannot go through native directly; current routing does not filter wire by adapter and may pick a model native does not support. Later, adapter capabilities must enter routing filters, or the adapter must error out explicitly.
2. **native has no streaming, no in-run retry**: a provider 5xx/timeout ends the whole run with exit code 4, and the model never gets a "retry once on another endpoint" chance; control-plane fallback can re-run with a different model, but it must open a new run and recompile the packet, which costs more than backoff-retry within the same run.
3. **fallback resets the single-run budget**: token/wall/cost are all per-run, so after a switch the new attempt gets a fresh budget. Chain length is decided by `[routing.fallbacks]` (usually 1), but the worst case is "per-tier budget × (1 + chain length)". Cross-tier never participates, so the cost cap is still held by the tier guardrail.
4. **inconsistent budget enforcement**: `TaskBudget.max_cost_usd` is currently only enforced at runtime by native; external adapters can only reconcile after the receipt.
5. **mock adapter usage is synthetic**: `cost_source=provider` is only a simulation and cannot support real CPVT/cost conclusions; it is fit only for contract tests.
6. **failure packs and verify logs**: `load_failure_pack` reads the previous run's full `verify.log`, which the Context Compiler then truncates; oversized logs are read into memory first, and long-term this should become on-demand read/preview.
7. **routing fields not in the event schema**: `TaskDispatched.data.route` is free-form JSON; do not build external tools that depend on field names before the contract stabilizes.
8. **escalation history depends on event counts**: if `events.jsonl` is hand-edited, escalation counts will be distorted; the event log remains the single source of truth.
9. **locking and merge queue exist, real data does not**: path locks, `[parallel]` caps/budgets, the `loom merge` queue, and `loom board` are implemented; `scripts/smoke-parallel.sh` measured 2-concurrency over 10 tasks with no lost updates (see [08](08-design-roadmap-v0.2.md) §9.5); but 3-way parallel, cache hit impact, and real-repo merge conflict rates still need real providers and a real task set — do not enable 3-way parallel before verifying.
10. **no version check on the price table**: `pricing_version` can be recorded but is not enforced; after a price change, old ledgers are not recomputed automatically.
11. **role→tier is opt-in**: when `[routing.roles]` is not configured, the TaskSpec's `tier` still wins and the documented matrix is not applied automatically; this avoids behavior suddenly changing for existing projects.
12. **fallback only looks at exit codes**: the criterion is `exit_code == 4 || timed_out`. Exit code 4 currently relies on adapter-contract self-discipline (native already maps provider errors to 4); if some adapter reports a provider error as 0, the control plane will not switch, and will only record a task failure that may falsely trigger escalation.
13. ~~**`needs_input` / `blocked` recorded as failures by verify**~~ (fixed 2026-11-09): added `TaskStatus::NeedsInput` + `task.needs_input`; `verify` keeps the claim status when acceptance fully passes, and `blocked` / `budget_exceeded` write `task.blocked`, no longer polluting M3's escalation counts; see [15](15-agent-runtime-governance-and-engineering.md) §2.1.
14. **no always-on outlet for human items**: questions that need a person, approvals, operations (installing tools / providing keys), reviews, and reminders are scattered across conversation history, receipts, and logs; once the user leaves the current session it is hard to know "what still needs me now". Design in [15](15-agent-runtime-governance-and-engineering.md) §2.2 (attention inbox); not implemented yet.
15. **local CI stops at task acceptance**: `loom init`/`doctor` do not generate repo-level checks; format/lint/commit message/review still wait on server CI; agents also lack a unified local feedback command. Design in [15](15-agent-runtime-governance-and-engineering.md) §4; not implemented yet.

---

## 6. Currently reproducible verification

Inside the container (`./scripts/dev.sh`):

```bash
make check
./scripts/dev.sh bash /work/scripts/smoke.sh
./scripts/dev.sh bash /work/scripts/smoke-native.sh
```

Coverage: mock contract, native tool loop, acceptance feedback, cost estimation, failure packs, routing explanation, same-tier fallback (`LOOM_MOCK_FAIL_ALIAS` makes a given alias exit 4), event replay, path lock / parallel cap / inflight budget blocking (`task.blocked`), merge rejection (in-place / not accepted). `scripts/smoke-parallel.sh` separately covers 2-concurrency dispatch + merge queue end to end (10 tasks, disposable git repo).

**Not covered**: real goose/OpenCode, real provider usage, LiteLLM, the subscription lane, real CPVT/TPVT/WPT baselines.

With `.env.local` filled in (see `.env.example`) there are two more live-chain scripts, both with hard token caps:

```bash
./scripts/dev.sh bash /work/scripts/live-smoke.sh     # real model dispatch→verify, ≤20k tokens throughout
./scripts/dev.sh bash /work/scripts/relay-probe.sh    # probe across sessions which wire a model accepts tool history on
./scripts/dev.sh bash /work/scripts/ab-parallel.sh --arm 1|2|3   # parallel A/B single arm (≤20k each)
./scripts/dev.sh bash /work/scripts/ab-parallel.sh --report      # three-arm comparison table
```

Four live chains verified on 2026-11-08 (all passed live dispatch→verify):

| Model | endpoint | wire | Time | Notes |
|---|---|---|---|---|
| `deepseek-flash` | relay | chat | 14s | thinking output can reach ~9k tokens; per-task output budget needs headroom |
| `deepseek-flash` | official `api.deepseek.com/v1` | chat | 9s | cache hit 89%; usage accounting normal |
| `mimo-v2.6-pro` | relay | messages | 50s | cleanest cache fields (`cache_read` read directly) |
| `gpt-6.1-sol` | relay | chat | 21–25s | the relay is occasionally slow (up to 87s for a single step); wall budget already relaxed to 600s |

Accounting warning: **the relay chat's `input_tokens` clearly undercounts** (the official endpoint reports ~15k for the same task, the relay ~4k), and `reasoning_tokens` also runs small; CPVT/cache comparisons can only be done within the same provider — token counts are not directly comparable across providers. The relay's messages/responses paths still reject tool history (raw probe 403/10082), but chat works, so usage is unaffected. `mimo-v2.6-pro` now goes through the gpt relay address (`LOOM_GPT_API_BASE` + `model_id = "mimo-v2.6-pro"`).

**Parallel A/B (2026-11-08, `ab-parallel.sh`, official `deepseek-flash`, 3 tasks/arm, ≤20k tokens per arm)**:

| max_running | dispatch wall | max/task | input | output | tokens | cache% | cost_usd | CPVT |
|---|---|---|---|---|---|---|---|---|
|1 |13s |5s |11537 |1355 |12892 |48% |0.001716 |0.000572 |
|2 |10s |6s |11664 |1383 |13047 |88% |0.001055 |**0.000352** |
|3 |**6s** |6s |11846 |1793 |13639 |89% |0.001291 |0.000430 |

Conclusion: wall shortens nearly linearly (3-way parallel is 54% faster than serial), tokens only +5.8% (no regression), and **CPVT drops instead of rising** (arm2 is 38% below arm1 — cache hits going from 48%→88% crushed the cost of missed inputs; arm3 has 32% more output and is slightly above arm2, still 25% below arm1). Small sample (3 tasks, single round, single model); conclusions are limited to the same provider. The price table lives in `scripts/model-prices.sh` (official list prices 2026-11-08: `deepseek-flash` takes off-peak $0.15/$0.003/$0.60, peak hours 2x; `gpt-6.1-sol` $2/$0.10/$10; relay multipliers deepseek 0.09x, gpt 0.32x recorded in the same file's comments), `--report` computes cost/CPVT offline from it, and `[[models]]` config also generates price fields from the same file.

---

## 7. Suggested order

1. Extend the container image: `goose`, `opencode` (or mount them), `curl`, optional LiteLLM.
2. Get GPT + DeepSeek API keys, first run a native-only A/B on a fixed task set to confirm routing and cost accounting.
3. Then run the real goose/OpenCode canary, and only then claim the M1/M2 exit criteria are complete.
4. 2-way parallel is ready (locks + merge queue + measured with `scripts/smoke-parallel.sh`); 3-way parallel still needs A/B proof that `WPT`/`CPVT` do not regress before enabling.
5. Do the attention correctness fixes and inbox in `doc/15` first (fix bugs + satisfy visibility), then CI core and the question protocol; churn, CI hooks/review, and skills assessment are deferred. Phasing and person-days in [16](16-runtime-governance-adoption-assessment.md).

Same-tier fallback and `loom route stats` are complete and no longer on the todo list.

---

## 8. New findings from 2026-11-07 (design drafts)

The following items have recorded designs but are **not implemented**; details in [15-agent-runtime-governance-and-engineering.md](15-agent-runtime-governance-and-engineering.md):

1. **churn**: step-level projection + patch-first/write guard + step-level token attribution are implemented; still missing are L1–L3 tiered intervention and cost-aware restart (automatic projection only triggers on a byte threshold, not on "Nth rewrite").
2. **blocking on questions and visibility of human items**: `needs_input` is no longer recorded as `task.failed`; but external agents can still hang waiting on stdin, and questions/approvals/operations/reviews still lack a persistent attention inbox.
3. **skills governance**: an unverified skill cannot be installed globally; needs repo profiling, an offline catalog, on-demand loading, static checks, and promotion after evaluation; CI review/check templates are its early landing spot.
4. **local CI is absent**: `loom init` only creates a skeleton; it does not probe the tech stack, nor generate acceptance/CI; checks like format, lint, review, and commit messages that could be resolved locally still depend on the server, and agents lack a unified local feedback loop.
5. **the adoption assessment is complete**: integration points based on the current code, contract changes, phased person-days, and risks are in [16-runtime-governance-adoption-assessment.md](16-runtime-governance-adoption-assessment.md). It confirms three designs that must be fixed first: the CI config should be named `.loom.ci.toml`, live at the target project root, and be version-controlled (it is project policy, not `.loom/` runtime state); local attestation cannot authorize the server to skip CI in v1; `verify` must first stop overwriting claim receipts and stop writing `needs_input`/`blocked`/`budget_exceeded` as `task.failed`.

## 9. First battle on a real repo (2026-11-08, tumblr-downloader / issue #24)

**Result**: the full chain closed successfully. `loom init` → task card (the issue's original text + the orchestrator's troubleshooting map) → native dispatch (official `deepseek-flash`) → verify → `loom merge` merged into local `loom/integration` (**32.5s**, including pre-merge acceptance and integration acceptance, **0 conflicts**, worktree/branch/locks all cleaned up) → `loom board` shows merged. Code changes: `DownloadStateManager` +80 / tests +81, `fix #24` waiting on the integration branch for human review (not pushed to the remote, per convention).

**Cost and lessons (#24 total: 8 attempts, 1.52M tokens, about 76% of the 2M quota)**:

| Cause of death | Symptom | Fix |
|---|---|---|
| budget calibration error ×2 | 60k/250k input hit the compounding effect of "resending full history every step" (12–31k/step) | budget by steps × per-step cost; calibrate with measured usage via `loom receipt` |
| no-progress watchdog false kill | the default 6 steps only recognized file writes: verify-only runs without writes got blocked (measured 2026-11-11: T1 round 2, bash×7 + one write rejected by the guard) | **now evidence-based** (acceptance execution counts as progress, (cmd,exit) dedup, see `run.rs`); investigative tasks can still tune `LOOM_NATIVE_MAX_IDLE` |
| steps cap | 12/6 steps hit the ceiling, and `max_steps exhausted` recorded as `budget_exceeded` (exit 3) | give wrap-up tasks a "run tests → commit → finish" card |
| model waste | repeated `git diff`/re-reading the same file took 5 steps; `bash` errored 5 times on the `command` parameter name | cards forbid diff; bash accepts the `command` alias; `read` accepts absolute paths inside the workspace |
| card missing "must commit" | merge only looks at branch commits, not the working tree | wrap-up cards state `git add -A && git commit` explicitly |

**A real product bug dug up along the way (fixed with #24)**: the 5-second throttling of progress persistence swallowed the completion snapshot; changes arriving during an in-flight save were silently dropped (`persistDirty` backfill added at the end of `finally`); completion events lost through the static `progressListener` slot were never reconciled on disk afterwards (`CompletedMediaStore` reconciliation fix). 2 regression tests; all 154 unit tests green.

**Environment (added to `dev.sh`, all mounted at the same paths, rootless podman with no ownership issues)**: `LOOM_TARGET_REPO` / `LOOM_ANDROID_SDK` / `LOOM_JAVA_HOME` / `LOOM_GRADLE_HOME` / `LOOM_MAVEN_HOME` (HOME points at their parent directory); Robolectric dependencies are made offline via `.loom/robolectric-init.gradle` (`maven.repo.local`), acceptance A1 passes `--init-script`; JDK21+SDK34+Gradle caches are all shared with the host.

**Left over**: `#25` (edit queue) and `#22` (profile batch download) are large features; with the current quota remaining it is not worth starting them — wait for quota top-up. The churn projection (step-level context recompilation) has landed and was measured with official deepseek (when full history is 16–17KB, per-step input drops 46%–55%, see [16](16-runtime-governance-adoption-assessment.md) §8.8) and can serve as the cost baseline after work starts.
