# 06 Technology Selection

> Language: **English** | [简体中文](06-technology-selection.zh-CN.md)

> **v0.2 revision (2026-11-04)**: pi has been demoted from "main path" to "optional adapter"; the core is now a standalone CLI/daemon + process-level executor contract.  
> Selection conclusions follow [08-design-roadmap-v0.2.md](08-design-roadmap-v0.2.md); the tool comparison in this document and the Aider/Headroom/LiteLLM boundaries remain valid.

> The goal of this document is not to "pick the strongest tool", but to answer: **what must we write ourselves, what must we reuse, and what do we flatly refuse to touch.**  
> Tool versions and capabilities follow the prior 2026-11 discussions and local measurements; verify each item before implementation.

## 0. Selection principles

1. **Reuse the Runtime**: do not reinvent the agent loop, tool execution, sandbox, or streaming.
2. **Reuse the gateway**: Headroom for compression, LiteLLM for routing; do not build your own.
3. **Build the orchestration yourself**: state protocol, task graph, verify, budget, ledger — this is the differentiating part, and it is thin.
4. **Replaceable**: any component can be rolled back/replaced within a day; no hard-coded provider addresses.
5. **Observable**: selection must be able to get usage / errors / latency; a black box does not qualify.
6. **CPVT first**: more features is not the same as worth it; prove it with an A/B on the same task set.

## 1. Candidate comparison

| Candidate | Runtime | Multi-model | Long tasks | Sub-agent isolation | Compression/cache | Fit | Conclusion |
|---|---|---|---|---|---|---|---|
| **pi + plugins** (current) | mature | plugin/config support, needs verification | goal plugin can continue runs | subagent has its own context | pi-headroom plugin + proxy | **high** | **main path** |
| **Codex CLI** (current) | mature | subagent model override, needs verification | goal mode (mediocre experience) | has subagents | can go through Headroom | high | optional worker/fallback |
| **Claude Code + LiteLLM/CCR** | mature | strong (per-scenario routing) | supported | yes | LiteLLM guardrail / Headroom | medium-high | worth referencing, no forced migration |
| **Aider** | mature | LiteLLM underneath, architect/editor two models | weak (no goal semantics) | none (shared transcript) | can go through Headroom | medium | **A/B bypass only** |
| **DeepSeek Harness / DSH** | plugin runtime | strong | supported | yes | plugins | medium | preview, not production yet |
| **Kilo / Roo Code** | in-IDE | strong (task-level routing) | medium | yes | limited | medium | usable in the editor, not long-running CLI |
| **Build a full harness ourselves** | self-built | self-built | self-built | self-built | self-built | — | **rejected** |

### 1.1 Notes

- **pi**: already has `subagents`, `goal`, `headroom`, `context-mode` and other plugins, closest to an "off-the-shelf base". Drawbacks: fragmented plugin ecosystem and fast version drift; isolate with an Adapter.
- **Codex**: strong model capability, suited to frontier/fallback; multi-model and sub-agent config must be verified against the local version; Responses API compatibility is a risk point.
- **Claude Code + LiteLLM/CCR**: routing and compression combination is mature, but migration cost is high; if the pi ecosystem becomes unstable later, it can serve as a fallback runtime.
- **Aider**: `--architect` / `--editor-model` / `--weak-model` is the classic two-model triage; but there is no sub-agent context isolation, no goal semantics, no retrieve. Only suitable for diff generation / A-B.
- **DSH**: plugin-based and model-agnostic, but officially labeled a developer preview and the interfaces will change; not a production-critical path.
- **Kilo/Roo**: good experience inside the IDE, but not the shape of "resident CLI + long tasks + file state".

## 2. Recommended combination

```
Runtime:  pi (primary) + Codex (optional worker/fallback)
Orchestration: self-built TypeScript runner (C1/C4/C6/C7/C13)
State:    .agent/ + git worktree
Gateway:  Headroom (cache) + LiteLLM (from P2)
Ledger:   JSONL → SQLite/DuckDB (on demand)
Acceptance: the project's native test/lint/typecheck/build
Bypass:   Aider (A/B / diff generation only)
```

### 2.1 Why pi is primary

- It already has subagent, goal, headroom, and context plugins;
- The plugin shape fits the existing "agent-side compression + resident proxy" topology;
- TypeScript/Node ecosystem, convenient for writing the runner and schema;
- Codex can take over on failure; no lock-in.

### 2.2 Why Codex is not the main orchestrator

- goal mode experience is mediocre (prior feedback);
- Multi-model/sub-agent config depends on the version;
- Responses API compatibility with LiteLLM/Headroom needs extra verification;
- It suits being a "strong-model worker/fallback", not the scheduling center.

### 2.3 Why Aider is not the main path

See §6.

## 3. Why not to build a complete Runtime ourselves

| Item to self-build | Effort | Existing alternative | Differentiating | Conclusion |
|---|---|---|---|---|
| Agent loop / tool execution | very large | pi / Codex / SDK | low | do not build |
| Sandbox/permissions | large | OS/container/runtime | low | do not build |
| Compression/cache | large | Headroom | low | do not build |
| Provider routing/retries | medium | LiteLLM | low | do not build |
| Streaming/tool-call protocol | large | runtime/SDK | low | do not build |
| State protocol | small | none | **high** | build |
| Task graph/scheduling | medium | none | **high** | build |
| verify/budget/escalation | small | none | **high** | build |
| Ledger/metrics | small | partial | **high** | build |

**Conclusion**: build a 1–2k-line orchestration layer to gain control of the token structure; do not touch the runtime.

## 4. Headroom capability boundaries

> The following comes from prior discussions and observations of source code/docs; different versions may vary, verify locally before implementation.

| Capability | Current state | Impact on Loom | Countermeasure |
|---|---|---|---|
| `mode` | process-start config (`cache`/`token`) | cannot switch per-request | fix `cache`; later split instances per client/port |
| Client classification | looks at `User-Agent` / `X-Client` | does not recognize `model` | no per-model attribution; use the LiteLLM ledger |
| AuthMode policy | PAYG / Subscription differ; subscription is more conservative | compression policy is global | accept it; do not expect per-model optimization |
| `size_floor` | skips small results (measured ~71% of units) | scattered JSON not compressible | projection/references at the tool layer |
| `WasteSignals` | cumulative detection count | cannot be treated as "saved" | only look at Δ/Δrequests |
| `retrieve` | has a TTL; requires the client to register the tool | Aider/Codex cannot get the text back | keep the original text in files on critical chains |
| stats | request/compression/latency diagnostics | cost measurement basis unreliable | provider/LiteLLM are authoritative |
| Compression ROI | 0.70% vs cache $270 | not worth tuning | keep cache |

### 4.1 Conclusion

Headroom is a **compression/cache layer**, not a multi-model scheduling layer. Do not try to make it switch policies by `model`; put routing in LiteLLM and context governance in Loom.

## 5. Where LiteLLM goes and what it does

### 5.1 Placement

```
agent → Headroom → LiteLLM → provider
```

### 5.2 Responsibilities

- aliases: `frontier` / `worker` / `worker-backup`;
- provider adaptation: OpenAI Chat / Responses / Anthropic Messages;
- retries / fallback / rate limiting;
- budget / key management;
- authoritative request log (for the ledger).

### 5.3 Not responsible for

- context isolation;
- task decomposition;
- compression policy (do not stack a guardrail);
- verify/goal.

### 5.4 Risks

- Different client protocols (OpenAI/Anthropic/Responses) are inconsistent in compatibility;
- Aliases may hide the real model from Headroom;
- Missing model metadata (context window / cost) causes errors;
- Mixing the two slots' protocol prefixes may be rejected.

**Countermeasure**: compatibility matrix + canary + direct-connection fallback; the ledger records alias + real model.

### 5.5 Existing plugin inventory and conflict check

The plugins currently in `pi list` need their responsibilities confirmed one by one to avoid overlapping with Loom's state/communication/compression layers. The table below is a **to-verify checklist**, not a conclusion:

| Plugin | Possible responsibility | Potential conflict with Loom | Suggestion |
|---|---|---|---|
| `pi-cache-optimizer` | cache optimization / prefix stability | if it rewrites the prefix automatically it breaks the cache | keep, but monitor cache hit; forbid mid-run rewrites of system/tools |
| `context-mode` | context management / automatic compression | may conflict with "externalized state / no silent summaries" | turn off auto-compression for the orchestrator; workers stop when over budget |
| `pi-hermes-memory` | long-term memory | may duplicate/contaminate `.agent/` state | auxiliary only; `.agent/` remains the source of truth |
| `pi-web-access` | web scraping | HTML/large results entering context directly | force "body extraction + file reference + truncation" |
| `@ff-labs/pi-fff` | search / file location | oversized results, repeated reads | use for batch locating; read by range; results by reference |
| `pi-intercom` | agent-to-agent communication | **conflicts with hub-and-spoke**, easy to produce mesh communication | allow only orchestrator↔worker; forbid worker-to-worker chat |
| `ponytail` | unknown (needs verification) | uncertain | decide whether to keep after the inventory |
| `@tintinweb/pi-subagents` | sub-agent dispatch | may inject the parent context by default | configure it to "pass only the TaskSpec"; disable transcript inheritance (if possible) |
| `@narumitw/pi-goal` | continuation | may run forever | set `automaticTurns` / `noProgressTurns` |
| `@casualjim/pi-headroom` | agent-side compression | stacks with proxy/guardrail | keep exactly one compression owner per chain |

**Principle**: plugins may be kept, but it must be explicit "who is the source of truth, who is the compressor, who is responsible for communication". Where responsibilities overlap, turn off automatic behavior first and use Loom's explicit protocol instead.

### 5.6 Model capability bits

Model names and prices change fast; hard-coding model names in config rots quickly. Record capability bits at selection time; aliases map only to "real model + capability bits":

| Capability bit | Values | Selection impact |
|---|---|---|
| `tier` | `frontier` / `worker` | task category → tier routing; start with two tiers |
| `thinking_mode` | `off` / `optional` / `forced` | models with forced thinking do not enter multi-turn tool chains unless they pass through completely |
| `reasoning_passthrough` | `required` / `optional` | `reasoning_content` must be passed back verbatim; the compressor may not rewrite it |
| `wire` | `chat` / `responses` / `messages` | decides whether Headroom / LiteLLM / billing cover it |
| `context_window` / `max_output` | numeric | slicing, budgets, suitability for long-context scanning |
| `input_usd_per_mtok` / `cached_input_usd_per_mtok` / `cache_write_usd_per_mtok` / `output_usd_per_mtok` | numeric | the main term of real cost; cache prices recorded separately, more important than list prices |
| `tool_calls` / `streaming` | boolean | adapter capability declaration; treat missing as unknown |
| `model_id` | fixed version or date | the acceptance baseline must be re-run after a change/retirement |
| `pricing_version` | string | price-source version; reconciliation can trace which price list was used |

Config example (prices are placeholders only; the provider bill is authoritative before implementation):

```toml
[[models]]
alias = "worker"
provider = "deepseek"
model_id = "deepseek-v4-flash"
tier = "worker"
wire = "chat"
base_url = "https://api.deepseek.com/v1"   # optional: per-model endpoint switch
key_env = "DEEPSEEK_API_KEY"      # optional: env var name holding this model's key (value never in config)
thinking_mode = "off"
reasoning_passthrough = "optional"
context_window = 128000
max_output = 8192
tool_calls = true
streaming = true
input_usd_per_mtok = 0.27
cached_input_usd_per_mtok = 0.07
output_usd_per_mtok = 1.10
pricing_version = "2026-11-04"

[[models]]
alias = "frontier"
provider = "openai"
model_id = "gpt-6.1-sol"
tier = "frontier"
wire = "responses"
thinking_mode = "optional"
reasoning_passthrough = "optional"
```

Check commands:

```bash
loom capability list
loom capability show worker
loom capability check
```

Leave prices empty when they are unknown; never guess a price so the ledger "looks good"; the ledger records `cost_source=unknown` and `loom ledger`'s `cpvt` returns `n/a`. `loom capability check` reports "forced thinking without reasoning passthrough" as an error and missing prices as a warning.

Selection rules:

- the execution layer prefers models that **can turn thinking off, speak a clean protocol, and do a stateless single call**;
- start with **1 primary + 1 fallback**, expand after A/B by task type, do not onboard four vendors at once;
- long-context models only do "full-repo scan → structured candidates"; they never finalize a spec;
- the ledger records alias, real model, and capability bits together, so reconciliation never relies on an alias.

## 6. Aider assessment

### 6.1 Can it compose directly?

**Yes, but it only covers half the job.** Aider runs on LiteLLM underneath and can point at Headroom/LiteLLM through a compatible endpoint:

```bash
export OPENAI_API_BASE=http://127.0.0.1:8787/v1
export OPENAI_API_KEY=dummy
aider --model openai/frontier \
      --architect \
      --editor-model openai/worker \
      --weak-model openai/worker
```

`headroom wrap aider` also works, but its lifecycle is bound to the wrapper and it does not fit a resident-proxy scenario; a resident Headroom + environment variables is preferred.

### 6.2 What it can solve

- `--architect` / `--editor-model`: the expensive model produces the plan, the cheap model produces the diff;
- fast diff generation, single-file/small-scope refactoring;
- as an A/B control: testing "can a cheap model write the right diff".

### 6.3 Hard boundaries

| Limitation | Consequence | Mitigation |
|---|---|---|
| no `headroom_retrieve` | compressed-away original text cannot be fetched back | use `mode=cache`; write failure logs to a file |
| no sub-agent context isolation | architect/editor share a transcript, history resend remains | short tasks only; not for long runs |
| no goal semantics | no "continue until verifiable completion" | wrap it with an external runner |
| `--auto-test` is just one more fix round after failure | no no-progress governance | external budget / circuit breaker |
| in-session cost readings are not the bill | cost misjudgment | use the LiteLLM/provider ledger |
| the two slots must share a protocol prefix | mixing `anthropic/` + `openai/` may be rejected | go through a unified OpenAI-compatible alias |
| model metadata required | errors with `Unknown context window size and costs` | configure `.aider.model.metadata.json` |

### 6.4 Conclusion

- **Not the main path**;
- Use as an **A/B bypass** and **diff generator**;
- If used, it must be wrapped by a runner (read slice → `aider --message` → verify → escalate → write state), about 100 lines of script;
- Do not expect it to solve the token-area problem.

## 7. Alternatives and exit strategy

### 7.1 If the pi ecosystem becomes unstable

- Switch to Codex as the main runtime; the runner stays unchanged behind the Adapter.
- Or switch to Claude Agent SDK / Claude Code + LiteLLM; the protocol stays unchanged.
- Cost: rewrite the Adapter, not the orchestration/state/ledger.

### 7.2 If Headroom fails

- Clients connect directly to the provider temporarily;
- Keep `DIRECT_*` env vars/config;
- Alert when cache hit dies;
- Switch back after recovery.

### 7.3 If LiteLLM is incompatible

- Degrade per client: pi goes through LiteLLM, Codex connects directly;
- Or use LiteLLM only for worker routing and connect frontier directly;
- Worst case, fall back to a single model and keep the context-governance gains.

### 7.4 If worker model quality falls short

- Go back to all-frontier;
- Keep slicing/receipts/verify/ledger (those gains are independent of the model);
- Use CPVT to decide whether it is worth trying again.

### 7.5 If the Aider bypass yields nothing

- Delete it outright; the main path is unaffected.

## 8. Version locking and upgrades

### 8.1 `.agent/env.lock`

```json
{
  "schema_version": "1.0",
  "recorded_at": "2026-11-04T00:00:00Z",
  "runtime": {"pi": "1.5.0", "codex": "x.y.z"},
  "plugins": {
    "@tintinweb/pi-subagents": "x.y.z",
    "@narumitw/pi-goal": "x.y.z",
    "@casualjim/pi-headroom": "x.y.z"
  },
  "gateway": {"headroom": "x.y.z", "litellm": "x.y.z"},
  "models": {"frontier": "gpt-6.1-sol@medium", "worker": "deepseek-worker"},
  "pricing": "pricing@2026-11-01"
}
```

> Early API keys cover GPT plus several Chinese model providers: use DeepSeek for `worker` first, swap in Kimi/MiniMax/MiMo etc. through A/B or as fallbacks, and re-run the baseline.

### 8.2 Upgrade process

1. Record current versions + baseline.
2. Read the changelog; mark breaking changes.
3. Run the compatibility checklist (07).
4. Canary on small tasks (1–2).
5. Observe for 1 day: CPVT, cache hit, error rate, tool call success rate.
6. Full rollout; keep a rollback package.

### 8.3 Forbidden

- Upgrading runtime/gateway mid-run in production.
- Upgrading multiple critical components at once.
- Upgrading with no baseline and no canary.
- Upgrading without updating `env.lock`.

## 9. FAQ (direct answers)

### Q1 Should LiteLLM sit between the agent and Headroom?

No. The order is `agent → Headroom → LiteLLM → provider`. Headroom is the transparent compression/cache layer; LiteLLM sits after it and does alias routing. Reversed, every provider would need its own Headroom setup, and LiteLLM's retries/fallback would bypass the compression layer.

### Q2 Several Anthropic-format models — how to switch token/cache per model?

**Do not switch, and you cannot.** `mode` is process-start config, not per-request; Headroom sits in front of LiteLLM and cannot see the final model anyway. Keep everything on `cache` and hand slot-name mapping to LiteLLM. If both modes are ever truly needed, it can only be done by splitting instances at the process boundary per client/port.

### Q3 We only use a single GPT today — can we cut cost first?

Yes, and we should. Stage S captures most of the gains without multi-model: externalized state, slicing, four-line receipts, delivery by reference, verify. Multi-model is a P2 amplifier, not a stop-the-bleeding remedy.

### Q4 Can Aider be used directly with LiteLLM + Headroom?

It can compose, but only fits bypass/A-B/diff generation. It has no `retrieve`, no sub-agent context isolation, no goal semantics, and shares a transcript; do not make it a long-running main path.

### Q5 Should we build a complete harness ourselves?

No. Build only the thin orchestration layer (state protocol, task graph, verify, budget, ledger); use off-the-shelf components for runtime, compression, routing, and sandbox. A complete harness is heavy work with no differentiation.

### Q6 Why might cheap models end up more expensive?

Because the billing shape is `turns × context`. If a cheap model takes a few more tries, reads a few more files, and does rework, its `cost per successful task` can exceed frontier passing once. It must be validated with CPVT + quality guardrails, not by unit price alone.

## 10. Selection conclusions

| Question | Conclusion |
|---|---|
| Build a complete harness ourselves? | **No**; build a thin orchestration layer (1–2k lines) |
| Main runtime? | **pi**; Codex as optional worker/fallback |
| Compression? | **Headroom**, keep `cache`; no tuning |
| Routing? | **LiteLLM**, placed after Headroom; ship in P2 |
| Aider usable? | **It can compose, but bypass/A-B only**; not the main path |
| How to split multi-model? | **Two tiers + explicit tier**; no LLM self-evaluation routing |
| Where does state live? | **`.agent/` files**; chat is not state |
| Where does cost come from? | **provider/LiteLLM**; Headroom stats are diagnostics only |
| Core KPIs? | **CPVT** (cost) + **TPVT** (tokens) |

**In one line**: treat "swapping models" as a second-order optimization and "context lifetime + tool results + acceptance protocol" as a first-order one. The former saves money; the latter stops the bleeding.
