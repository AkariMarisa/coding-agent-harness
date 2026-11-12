# 12 External Practices Comparison and Adoption (2026-11)

> Language: **English** | [简体中文](12-external-practices-comparison-and-adoption.zh-CN.md)

> **Source**: two Yuanbao conversations from 2026-11-05 (<https://yb.tencent.com/s/xDULZPSc4rGJ>, <https://yb.tencent.com/s/ayaF5zkD04ow>).
> The content is AI-generated + second-hand compilation; model names, prices, field names, and statistical accounting may all be stale or wrong; this document only extracts what has **incremental value for Loom**, and everything must be verified locally before acting on it.
> Subscription conclusions that overlap with [11](11-subscription-quota-and-agent-sdk.md) are not repeated here; this document focuses on multi-model, concurrency, observability accounting, and the hybrid lane.

---

## 0. One-line conclusion

Both sources reach the same conclusion as Loom: **the main causes of bills are history re-sending and tool-result bloat, not compression ratio; do not build a full harness yourself; thin orchestration + externalized state + four-line receipt + verify escalation is the right direction.**

There are five real increments:

1. Parallel sub-agents are not "the more the better" — **start at 2 concurrency**;
2. Choosing models means recording **capability bits** (thinking, protocol, cache price), not just writing down the model name;
3. The **hybrid lane** where subscription decides and API executes is cheaper than "wrapping the whole chain in a gateway";
4. Headroom's `WasteSignals` / `agent_usage` / `mode` have clear **observability and configuration boundaries**;
5. Spec is **decision work**, not document work, and cannot be handed to a "long-context but factually weak" model for direct finalization.

**Current constraints (2026-11-05)**: early on, **API keys** for GPT, DeepSeek, Kimi, MiniMax, MiMo etc. can be provided, so v1 can start with a multi-provider API lane: GPT `frontier` + DeepSeek `worker` first, other domestic models joined gradually by A/B. What is genuinely deferred is **subscription plan integration** and this document's hybrid lane; they do not block M0–M2; investing in subscription integration early does not pay off.

---

## 1. Already covered, no changes needed

| External view | Our equivalent |
|---|---|
| Long-session cost is quadratic; 20 rounds is about 52× the first round | [04](04-context-and-cost-model.md) §1.1 |
| A slice is not "more conversations", it is "short context" | [04](04-context-and-cost-model.md) §1.3, [08](08-design-roadmap-v0.2.md) |
| Compression saves only 0.70%; cache discount $270.63 | [04](04-context-and-cost-model.md) §5 |
| Tool results returned by reference, not pasted back by value | [04](04-context-and-cost-model.md) §2, [10](10-tool-output-protocol-v0.1.md) |
| Four-line receipt + file status | [03](03-core-protocols.md), [04](04-context-and-cost-model.md) §3.2, [08](08-design-roadmap-v0.2.md) |
| Escalate only when verify fails | [04](04-context-and-cost-model.md) §6.3 |
| Aider can compose directly, but loses retrieve, has no goal, no isolation | [06](06-technology-selection.md) §6 |
| Headroom before LiteLLM; compression and routing decoupled | [06](06-technology-selection.md) §5 |
| Subscription is a separate lane; no silent API fallback | [11](11-subscription-quota-and-agent-sdk.md) |

> These do not need to be "absorbed" again; what is needed is landing them per the docs and validating with real measurements.

---

## 2. Adoption: concurrency starts at 2, not 3

Public data from the external material:

- a single agent uses about 4× the tokens of a single-turn conversation, multi-agent about 15×;
- Claude Code sub-agent fan-out measured 513k input tokens vs 121k sequential, about 4.2×; across model families 2.6–5.9×, and not any faster;
- the Codex team's experience: more than 2 parallel sub-agents is "almost certainly burning tokens without improving quality";
- the parent context usually does not swallow the child's full transcript; the overhead comes from **each sub-agent being a separate agent loop**, each re-reading the system prompt and tool table.

Our own measurements have also shown "prompt protocol lowers per-exchange tokens, but subagent concurrency drops and everything gets slower". The two agree.

### 2.1 Rules

- **M4 changes from "3 parallel" to "start with 2 parallel"**;
- 3 parallel requires all of: file ownership fully disjoint, shared files already serialized, and A/B proof that `WPT` and `CPVT` do not regress;
- notify once when a sub-agent finishes; **do not poll for progress**;
- the write sets of parallel tasks must be disjoint; shared files such as `schema`, lock files, migrations, and dependency manifests are merged serially by the orchestrator;
- tighten the concurrency cap further by `credential_lane` / `quota_pool`, not just by process count.

### 2.2 Where it lands

- [08](08-design-roadmap-v0.2.md) M4 exit criteria;
- [04](04-context-and-cost-model.md) §4.3;
- TaskSpec's `paths` / write-set declaration (M1/M4).

---

## 3. Adoption: record model capability bits, not just model names

The external material shows that after 2026-10 model iteration is fast: forced thinking, `reasoning_content` must be passed back, model IDs hard-deprecated, cache prices varying by time of day. Hard-coding model names in the scheduler will rot quickly.

### 3.1 Capability bits

| Capability bit | Values | Effect |
|---|---|---|
| `tier` | `frontier` / `worker` | task category → tier routing; start with two tiers |
| `thinking_mode` | `off` / `optional` / `forced` | models with forced thinking do not go into multi-turn tool chains unless there is a passthrough channel |
| `reasoning_passthrough` | `required` / `optional` | `reasoning_content` must be passed back verbatim; the compressor must not rewrite it |
| `wire` | `chat` / `responses` / `messages` | determines whether gateway, compressor, and billing cover it |
| `context_window` / `max_output` | numeric | slicing and budgets |
| `input_usd_per_mtok` / `cached_input_usd_per_mtok` / `cache_write_usd_per_mtok` / `output_usd_per_mtok` | numeric | the main component of real cost; cache price recorded separately from list price |
| `model_id` | pinned version or date | changes require re-running the acceptance baseline |

### 3.2 Rules

- the execution plane prefers models that **can turn thinking off, have a clean protocol, and can be called statelessly in one shot**;
- models with forced thinking are used only for single-turn, stateless, re-runnable tasks;
- do not connect four cheap model families at once; start with **1 primary + 1 fallback**, expand after A/B by task type;
- model ID deprecation, price time-band changes, and cache price changes all go into version management;
- model aliases must map to **the real model + capability bits**; the ledger records both the alias and the real model.

### 3.3 Where it lands

- [04](04-context-and-cost-model.md) §6.5;
- [06](06-technology-selection.md) §5.6;
- [11](11-subscription-quota-and-agent-sdk.md) §5.1 (`credential_lane` gains capability bits).

---

## 4. Adoption (deferred): hybrid lane (subscription decides + API executes)

> **Priority: deferred.** For now we use API keys with multiple providers and do not implement the subscription control plane; this section only keeps the shape, to be evaluated once subscription is available.

The external material's judgment of "harness → goose → LiteLLM/Headroom → Agents SDK → Codex" is: structurally more expensive, because the credential system splits the chain in half, each extra agent adds a context and a cache, and compression never reaches the final request.

This agrees with [11](11-subscription-quota-and-agent-sdk.md), but adds a useful shape:

```text
[control plane / decision plane]
  Codex / Claude subscription (direct official client, short stable prefix)
        │  TaskSpec + acceptance criteria
        ▼
[execution plane]
  cheap API model → Headroom (cache) → LiteLLM → provider
        │  four-line receipt + file references
        ▼
[acceptance plane]
  local verify / cheap model; returns structured results only
```

### 4.1 Rules

- do **not** insert LiteLLM in the subscription lane; translation of Responses' reasoning items and custom tool types loses structure;
- use `Headroom → LiteLLM` only on the execution plane, which is exactly where compression and routing belong;
- the two planes exchange only **TaskSpec + four-line receipt + file references**; the execution plane's transcript does not flow back to the control plane;
- the control plane keeps only goals, constraints, decisions, and receipt indexes; it does not read full diffs/logs;
- the ledger records `api_equiv_usd`, `allowance_burn`, `provider_cache_discount_usd` separately, without adding them together;
- the hybrid lane is deferred and does not change the current "API keys with multiple providers" main line; do it only when subscription is actually available.

### 4.2 Where it lands

- [11](11-subscription-quota-and-agent-sdk.md) §6.4;
- [02](02-overall-architecture.md) topology diagram (hybrid lane to be added later).

---

## 5. Adoption: Headroom observability accounting

The external material lists several easy pitfalls:

| Symptom | Correct accounting |
|---|---|
| `WasteSignals` is large | it is **cumulative detected volume**, not savings; A/B only looks at `Δvalue / Δrequests` |
| `reread_compressed: 0` | the attribution logic is not landed; cannot be used as a criterion that "compression is broken" |
| `share_of_saved_percent` >100% | the denominator may be wrong; disable it |
| `agent_usage` has `codex` / `pi-extension` | may come from `aggregate fallback`; exact attribution comes from `agents[]`'s `source` |
| `requests.by_client` | may be empty; cannot be the sole attribution source |
| `mode` | process-level configuration; cannot be switched per request or per model |
| client classification | look at `User-Agent` / `X-Client`, **not at the model** |
| subscription-state compression | when `AuthMode=Subscription` it is more conservative: lossless-only / live_zone_only / lower `max_lossy_ratio` |

Conclusion:

- Headroom stays on `mode=cache`; do not switch modes per model;
- if two modes are genuinely needed, split instances at the process boundary by **client / port**;
- record `provider_cache_discount_usd` separately; do not add it to compression savings;
- subscription requests cannot use LiteLLM's `$0` spend for budgeting.

Where it lands: [10](10-tool-output-protocol-v0.1.md) §8.1, [04](04-context-and-cost-model.md) §7.3.

---

## 6. Adoption: spec is decision work

A key judgment from the external material: implementers start from a fresh context, so whatever the brief does not say must be rediscovered or guessed; **when the spec is wrong, the error multiplies by the number of tasks**, and a cheap model will faithfully implement the error.

### 6.1 Rules

- `spec` / interface contracts / acceptance criteria are finalized by a frontier model; cheap models may only draft candidates, which must pass deterministic verify or human sign-off;
- long-context models (e.g. Gemini Flash class) are suited to "whole-repo scan → structured candidates", **not to directly producing the final spec**;
- scan results do not enter every turn's context; only numbered, short, stable ACs go into the TaskSpec;
- ACs must at least cover: normal path, failure path, permission/empty-state/duplicate-request, and non-goal protection;
- acceptance evidence must map back to AC numbers.

### 6.2 Where it lands

- [04](04-context-and-cost-model.md) §6.2;
- [08](08-design-roadmap-v0.2.md) M1 prompt library, M2 native executor.

---

## 7. Adoption: Stage-S bleeding-stop checklist (works even with a single model)

Without changing the architecture and without multi-model, first turn "long sessions" into "long tasks + short context":

1. the main session only slices tasks, dispatches, and accepts; it does not write code directly;
2. each implementation slice gets its own context;
3. sub-agents end with a fixed four-line receipt and write everything else to files;
4. tool results are returned by reference (`wrote 300 lines to ...`), not pasted back in full;
5. batch reads: one grep with context, not "grep first, then open the file";
6. state files live in `.loom/`, not in chat history;
7. triple caps on goal / step / token;
8. acceptance stays in the lower layer and returns only structured results.

Acceptance: run the same task twice and compare only `CPVT` and `cache_hit`. First prove that "single GPT + context isolation" works, then talk about multi-model.

> These actions match M0/M1 in [08](08-design-roadmap-v0.2.md); the difference is that they do not require a full CLI first; they can serve as a manual bleeding-stop checklist.

---

## 8. Not adopted / needs caution

| View | Judgment |
|---|---|
| Dual instances of `token` / `cache` split by model | **Retracted**: `mode` is process-level, and Headroom sits before LiteLLM so it cannot see the final model; split instances only by client/port |
| Tagging "corrected" attribution via `X-Client` | no gain when attribution is already exact; the Subscription stealth state may require not changing UA / not adding headers — verify first |
| Treating Gemini as the final spec author | weak factual reliability, verbose, long-context surcharge; scan-candidate use only |
| Connecting four cheap model families at once | coordination tax is high; start with 1 primary + 1 fallback |
| Forced-thinking models in multi-turn tool chains | missing `reasoning_content` may cause 400; unless a full passthrough channel is built |
| Forwarding subscription OAuth tokens through a third-party proxy | terms / risk-control exposure; not in v1 |
| Over-compressing tool output | can trigger re-fetch loops, saving tokens while raising the bill; verify with A/B |
| Treating Headroom stats as the bill | diagnostics only; provider/LiteLLM is authoritative |
| Using a model's self-assessment "this is hard" to trigger escalation | escalate only when verify fails |
| Using LLM self-assessed complexity for automatic routing | not before M3; fix tier + explicit rules first |

---

## 9. Impact on the roadmap

| Milestone | Change |
|---|---|
| **M0** | Unchanged; write the Stage-S checklist into the docs as the manual bleeding-stop path |
| **M1** | TaskSpec / ledger gains `capability`; prompt library gains AC templates; telemetry gains observability accounting; no subscription dimensions implemented |
| **M2** | Native executor: receipt protocol, reference returns, batched reads, budget; model capability catalog (GPT + DeepSeek first) |
| **M3** | Multi-provider tier routing: GPT frontier + DeepSeek worker first; Kimi/MiniMax/MiMo expanded by A/B; 1 primary + 1 fallback |
| **M4** | Start at 2 parallel; file ownership; 3 parallel requires A/B proof |
| **Later** | Subscription lane, hybrid lane; does not block M0–M4 |

---

## 10. To be verified

1. Headroom native `/v1/responses` compression ratio and header pass-through for Codex subscription; whether `requires_openai_auth` / `ChatGPT-Account-ID` are required.
2. Whether the LiteLLM guardrail really only covers `/v1/chat/completions` and `/v1/messages`, not Responses.
3. Whether `X-Client` is forwarded on the Subscription chain, and whether it breaks upstream authentication.
4. The effect of `reasoning_content` passthrough for forced-thinking models on the cache prefix.
5. `WPT` / `CPVT` for 2 parallel vs 3 parallel; file ownership conflict rate.
6. The effect of model version deprecation, price time bands, and cache price changes on `CPVT`; the model ID pinning strategy.
7. Local reproduction of "over-compression causes a re-fetch loop": relationship between the truncation threshold and `reread` growth rate.

> Items 1–3 depend on the subscription lane and will be verified once it is enabled; they do not block the current API-key multi-provider stage. Items 5–7 belong to the main line.

---

## 11. References

- Yuanbao conversation 1: multi-model agent workflow optimization, pi plugin, Stage-S configuration, global prompt (2026-11-05, AI-generated).
- Yuanbao conversation 2: Codex direct vs multi-layer harness, subscription quota, cheap-model selection (2026-11-05, AI-generated).
- This repository: [04](04-context-and-cost-model.md), [06](06-technology-selection.md), [08](08-design-roadmap-v0.2.md), [10](10-tool-output-protocol-v0.1.md), [11](11-subscription-quota-and-agent-sdk.md).

> External model names, prices, leaderboards, and field names are only leads; rely on local `--help`, provider bills, and A/B results.
