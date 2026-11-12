# 11-Subscription Quota and Agent SDK Integration

> Language: **English** | [简体中文](11-subscription-quota-and-agent-sdk.zh-CN.md)

> **Research notes, 2026-11-05. Status: pending local verification.**  
> This document supplements [08](08-design-roadmap-v0.2.md) and [10](10-tool-output-protocol-v0.1.md); it does not change the core contract of "process + files + JSON + exit code".  
> Conclusion: **subscription is a separate authentication / billing / rate-limiting lane, not a cheap mirror of the API.** Do not assume it passes through Headroom / LiteLLM transparently.

> **Priority update (2026-11-05): deferred.** For now, get the API lane working with API keys (keys for GPT + DeepSeek/Kimi/MiniMax/MiMo etc. can all be provided); M0/M1 will not implement subscription, nor require installing/signing in to codex / claude. This document is kept as follow-up design research and does not block the main line; investing in subscription integration early does not pay off.

---

## 1. Problem

The current design assumes "API key + per-token billing":

```text
agent → Headroom → LiteLLM → provider (api.openai.com / api.anthropic.com)
```

But subscriptions like ChatGPT Plus/Pro and Claude Pro/Max:

- are not billed per token; they limit by **seat + usage window**;
- use OAuth / local sign-in state, not an `sk-...` API key;
- have endpoints, request headers, and available models that differ from the public API;
- usually return `429` or a quota error when over the limit, rather than letting the bill keep growing;
- multiple concurrent workers compete directly for the same seat's quota.

So three questions need answering:

1. Can a subscription be invoked programmatically through an Agent SDK / CLI?
2. Can subscription traffic keep going through Headroom / LiteLLM, preserving unified logging and budgets?
3. How must Loom's task, ledger, scheduling, and resume protocols change so that "subscription quota" is not misrecorded as "API spend"?

---

## 2. First clarify a few easily confused names

| Name | What it is | Subscription usable | What it means for Loom |
|---|---|---|---|
| **OpenAI Agents SDK / Agents API** | OpenAI's platform-side agent orchestration API, billed per API token | No, uses API key | Not a subscription entry point |
| **Codex CLI** | OpenAI's official coding agent CLI | **Yes**, after `codex login` it uses the ChatGPT plan | Preferred subscription adapter |
| **Codex SDK** (`@openai/codex-sdk` / `openai-codex`) | Official SDK wrapping the local Codex runtime / sign-in state | **Yes**, reuses Codex's file-backed sign-in | Alternative when structured events are needed |
| **Codex app-server** | Codex's local JSON-RPC service | Yes, reuses Codex sign-in state | Candidate for a future resident adapter |
| **Claude Code CLI** | Anthropic's official coding agent CLI | **Yes**, after `claude login` it uses Pro/Max etc. plans | Preferred subscription adapter |
| **Claude Agent SDK** | Python/TS SDK that runs the Claude Code binary internally | **Restricted by official terms**: third-party products are not allowed to offer claude.ai sign-in/quota without approval | In subscription scenarios prefer Claude Code CLI; do not use the Agent SDK directly |
| **LiteLLM `chatgpt/` provider** | LiteLLM's ChatGPT subscription provider | Yes, but the proxy itself holds one account | Suitable for a single seat; not a BYO multi-seat solution |
| **LiteLLM client OAuth forwarding** | The client puts the OAuth token in `Authorization` and the gateway key in `x-litellm-api-key` | Yes, keeps each developer's own subscription | Candidate for a multi-seat gateway; needs verification |
| **Headroom** | Compression / caching proxy | Does not understand subscriptions itself | Might work, but OAuth headers and streaming responses must be verified |

**Key correction**: what the user meant by "Agent SDK consuming subscription quota" corresponds on the OpenAI side to **Codex SDK / Codex CLI**, not the API-side Agents SDK; on the Anthropic side it corresponds to **Claude Code CLI's headless mode**, not to arbitrary third parties using the Claude Agent SDK.

---

## 3. Modeling differences between subscription and API

| Dimension | API (PAYG) | Subscription (Seat / Allowance) |
|---|---|---|
| Credential | API key | OAuth sign-in state, seat, workspace |
| Billing | Per token / request | Prepaid seat fee + usage window |
| Usage unit | input/output/cache token | request count, token equivalents, 5-hour/week windows, quota percentage |
| Visibility | Bills and usage are exact | Partially visible; windows and reset times are not always public |
| Concurrency | Mainly limited by RPM/TPM | Limited jointly by seat, window, concurrency |
| Over the limit | Billing can continue or the budget breaker trips | `429` / quota exhausted / wait for reset |
| Multiple workers | Scales horizontally | Workers on the same seat steal quota from each other |
| Cost accounting | US dollars | **Cannot be recorded in dollars directly**; only API-equivalent estimates + quota consumption |
| Headroom | Compression reduces the bill | Compression mainly helps context fit; it does not directly save money |
| Failure modes | budget, rate limit, network | quota window, expired sign-in, endpoint changes, `429` |

Impact on Loom:

1. `CPVT` is still useful, but the "cost" of a subscription task should be split into `api_equiv_usd` and `allowance_burn`, and must not be mixed into the real bill.
2. `WPT` must include time spent "waiting for quota reset"; otherwise the subscription route looks cheap while the actual wall clock is worse.
3. The scheduler must know **which seat, which window, current concurrency**; it cannot just round-robin by model alias.
4. When subscription quota is exhausted, **the default must not be a silent switch to the API**, otherwise it produces bills the user did not expect.

Quota shapes are still changing: some plans are moving from "requests/window" to token/credits metering, and model IDs can be hard-deprecated. Therefore:

- the ledger must record both the raw `usage` fields and Loom's own `allowance_burn`, without assuming the quota unit is stable;
- after a model ID change/deprecation, the acceptance baseline for that lane must be re-run; the old `CPVT` cannot be reused;
- `loom doctor --auth` only reports account and quota status; it does not infer "how many more tasks can run".

---

## 4. Three viable routes

### 4.1 Route A: official client adapter lane (recommended)

```text
loom-adapter-codex  → codex exec / Codex SDK  → ChatGPT subscription endpoint
loom-adapter-claude → claude -p               → Anthropic subscription endpoint
```

Approach:

- only call the official CLI / official SDK;
- reuse the existing local sign-in state; do not read, copy, or forward OAuth tokens;
- the adapter extracts usage, the final message, and the exit code from JSON/JSONL output;
- by default do not go through Headroom / LiteLLM; enable the gateway experiment explicitly only when unified logging is needed;
- one concurrency semaphore per seat; on `429`, cool down and wait for the window.

Pros: matches official usage most closely; endpoint changes are absorbed by the official client; credentials never enter Loom.
Cons: one adapter per provider; usage fields change between versions; cannot reuse LiteLLM's unified budget.
Conclusion: **the main path once subscription is available; not enabled now, API keys first.**

### 4.2 Route B: gateway-forwarded subscription (experimental)

```text
codex / claude
  → Headroom (optional)
  → LiteLLM
  → provider subscription endpoint
```

LiteLLM currently supports two kinds of subscription forwarding:

1. **Client OAuth forwarding**: the client keeps its own ChatGPT / Claude sign-in state, puts the OAuth token in `Authorization` and the LiteLLM key in `x-litellm-api-key`, and LiteLLM passes them through to upstream.
2. **Proxy-held account**: LiteLLM logs in itself with the `chatgpt/` provider + `auth.json`; all requests share one ChatGPT account and the token sent by the client is ignored.

Type 2 suits a single seat / personal proxy, not "each developer uses their own subscription"; only type 1 is a multi-seat candidate.

Risks:

- whether Headroom preserves `Authorization`, `x-litellm-api-key`, `ChatGPT-Account-Id`, `anthropic-beta` and other headers unchanged is unverified;
- Codex goes through `/v1/responses`; Headroom may only forward that path without compressing it;
- subscription endpoints may be affected by Cloudflare / risk controls / endpoint changes;
- LiteLLM often logs subscription requests as **$0**, so `max_budget` never triggers; RPM/TPM or a custom quota ledger is needed instead;
- two proxy layers enlarge the failure surface and make troubleshooting hard.

Conclusion: **a P2 experiment, not part of the v1 main path.** Consider it only after it passes compatibility testing.

### 4.3 Route C: self-built OAuth proxy / token extraction (rejected)

Includes reading `~/.codex/auth.json`, copying the Claude keychain, injecting subscription tokens into a custom OpenAI-compatible proxy, multi-account rotation, and so on.

Reasons for rejection:

- high risk of violating provider terms;
- token refresh, revocation, risk controls, and endpoint changes are all out of our control;
- once leaked, the damage lands on the user's main account;
- conflicts with Loom's security boundary of "credentials do not enter the control plane".

**Not implemented.**

---

## 5. Minimal changes to the Loom protocol

### 5.1 New dimensions, no new magic

> At this stage we only use API keys (GPT + DeepSeek/Kimi etc.), so the following subscription dimensions are not implemented yet; add them when subscription actually enters the roadmap. The API lane works with default values.

Add to `TaskSpec` / adapter configuration:

```json
{
  "model": {
    "alias": "worker",
    "provider": "openai",
    "auth_mode": "subscription",
    "credential_lane": "codex-pro",
    "allow_api_fallback": false
  }
}
```

Suggested enums:

```rust
pub enum AuthMode {
    Api,
    Subscription,
    Local,
}

pub enum BillingMode {
    PayPerToken,
    SeatAllowance,
    Free,
}
```

`CredentialLane` should contain at least:

| Field | Description |
|---|---|
| `id` | e.g. `codex-pro`, `claude-max`, `deepseek-api` |
| `provider` | `openai` / `anthropic` / `deepseek` / ... |
| `auth_mode` | `api` / `subscription` / `local` |
| `billing_mode` | `pay_per_token` / `seat_allowance` / `free` |
| `account_label` | human readable, e.g. `work-chatgpt`; do not store tokens |
| `quota_pool` | pool of entities sharing one quota, e.g. `chatgpt-pro-personal` |
| `max_concurrency` | concurrency allowed for this seat |
| `gateway` | `direct` / `litellm` / `headroom+litellm` |
| `allow_api_fallback` | whether to switch to the API when quota runs out, default `false` |

The same `credential_lane` also carries a set of **capability bits** (`tier` / `thinking_mode` / `reasoning_passthrough` / `wire` / input/cache/output prices / `model_id` etc.). Configuration definition in [06](06-technology-selection.md) §5.6, cost accounting in [04](04-context-and-cost-model.md) §6.5. The scheduler filters by capability bits, not by hard-coded model names.

### 5.2 Adapter capability declaration

An adapter should report what it supports instead of making the orchestrator guess:

```json
{
  "adapter": "codex",
  "version": "x.y.z",
  "auth_modes": ["subscription", "api"],
  "gateways": ["direct", "litellm", "headroom+litellm"],
  "usage_fields": ["input_tokens", "cached_input_tokens", "output_tokens"],
  "quota_fields": ["reset_at", "remaining_percent"],
  "supports_resume": true,
  "supports_streaming": true
}
```

Missing fields are marked `unknown`; do not let the ledger pass 0 off as a value.

### 5.3 Ledger fields

In addition to the existing `usage.jsonl` / `tools.jsonl`, task-level usage gains at least:

| Field | Description |
|---|---|
| `auth_mode` | `api` / `subscription` / `local` |
| `billing_mode` | `pay_per_token` / `seat_allowance` |
| `credential_lane` | seat / account label |
| `api_equiv_usd` | estimated at public API prices, **not the bill** |
| `allowance_units` | subscription quota consumed (provider's raw unit or an estimate) |
| `quota_window` | `5h` / `weekly` / `monthly` / `unknown` |
| `reset_at` | reset time when known |
| `wait_ms` | time spent waiting on quota / rate limits |
| `source` | `provider` / `litellm` / `adapter` / `estimated` |

Rules:

- aggregate `api_equiv_usd` separately from the real API bill;
- subscription tasks must not count toward `spend_usd`; only `api_equiv_usd` counts;
- keep `allowance_burn` alongside `api_equiv_usd`, to compare "is subscription actually cheaper";
- count `429`, expired sign-in, and endpoint errors separately; do not mix them into "task failures".

### 5.4 Scheduling and resume

- one semaphore / token bucket per `credential_lane`;
- tasks in the same `quota_pool` share a concurrency cap;
- on receiving `429` or a quota error:
  1. record `quota_exhausted`;
  2. set a cooldown from `reset_at` or exponential backoff;
  3. by default pause that lane; do not automatically switch to the API;
  4. only take `allow_api_fallback` when the user explicitly allows it.
- `loom resume` must be able to resume tasks that are "waiting for quota" instead of treating them as failures to re-run.
- Interactive use and headless automation share quota (ChatGPT / Claude may both have shared windows); the scheduler should estimate conservatively and not assume headless is an independent pool.

---

## 6. Headroom / LiteLLM compatibility assessment

### 6.1 Claude path

The shape given in LiteLLM's docs:

```text
claude → LiteLLM → api.anthropic.com
```

- Claude Code keeps and refreshes its own OAuth sign-in;
- LiteLLM does gateway authentication via `x-litellm-api-key`;
- LiteLLM forwards the client's OAuth token; logs can be tagged `used_client_oauth_token=true`;
- cost is estimated at Anthropic API list prices, but the actual bill comes from the subscription plan.

If Headroom is inserted in between:

```text
claude → Headroom → LiteLLM → api.anthropic.com
```

Must verify:

- whether `Authorization` passes through unchanged;
- whether `x-litellm-api-key` is preserved;
- whether `anthropic-beta` / `anthropic-version` are preserved;
- whether streaming SSE is not rewritten;
- whether `mode=cache` still hits the prefix cache;
- whether subscription state triggers a more conservative compression strategy.

### 6.2 Codex path

```text
codex → LiteLLM → chatgpt.com/backend-api/codex
```

LiteLLM supports two modes:

- **BYO subscription**: Codex's ChatGPT sign-in goes in `Authorization`, the LiteLLM key in `x-litellm-api-key`; LiteLLM only forwards and does not store the token;
- **Proxy account**: LiteLLM logs in itself with `chatgpt/` + `auth.json`; all requests share one account; the client token is ignored and $0 spend is recorded.

Notes:

- Codex uses the Responses API;
- Headroom has a separate **native `/v1/responses` proxy**; whether it can compress and whether it merely forwards must be verified separately; the community already has `msgs=0`, `transforms=none` reports;
- LiteLLM's Headroom guardrail rewrites `messages` at the `pre_call` stage; the compression endpoints officially listed are only `/v1/chat/completions` and `/v1/messages`, **not covering Responses**;
- subscription state may require headers/fields like `requires_openai_auth` and `ChatGPT-Account-ID`; whether Headroom preserves them unchanged is unverified;
- for an unknown model alias, Codex may send the full tool definition, inflating the prompt from about 13.5k to about 155k, **burning subscription quota fast**; a model catalog must be configured or a known Codex model name used;
- the subscription endpoint's model list differs from the public API; do not assume `gpt-*` names work universally.

### 6.3 Conclusion

| Path | v1 recommendation | Reason |
|---|---|---|
| API lane → Headroom → LiteLLM | **Keep** | Current design; billing and cache gains are clear |
| Claude Code subscription direct | **Support** | Official client, minimal assumptions |
| Codex subscription direct | **Support** | Official client, minimal assumptions |
| Claude subscription → LiteLLM | **Experimental** | Officially supported by LiteLLM; Headroom must be verified |
| Codex subscription → LiteLLM | **Experimental** | Supports BYO forwarding, but endpoints/model catalog are unstable |
| Subscription → Headroom → LiteLLM | **Off by default** | Headers and streaming unverified; fall back to direct on failure |
| Self-built OAuth proxy | **Reject** | Terms, security, and stability are all unacceptable |

**Hard rule**: the subscription lane must not silently degrade to the API lane. Degradation must be explicitly allowed by task policy and written to the event log.

### 6.4 Hybrid lane: subscription decides, API executes

For most long tasks, the cheapest shape is not "put a gateway around the whole chain" but separating the two planes:

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

Rules:

- do **not** insert LiteLLM in the subscription lane; translation of Responses' reasoning items and custom tool types loses structure;
- use `Headroom → LiteLLM` only on the execution plane, which is exactly where compression and routing belong;
- the two planes exchange only **TaskSpec + four-line receipt + file references**; the execution plane's transcript does not flow back to the control plane;
- the control plane keeps only goals, constraints, decisions, and receipt indexes; it does not read full diffs/logs;
- the ledger records `api_equiv_usd`, `allowance_burn`, `provider_cache_discount_usd` separately, without adding them together;
- the hybrid lane starts as an M3 experiment; it does not change v1's two independent paths of "subscription direct + API direct".

---

## 7. Local verification plan

**Local status 2026-11-05** (not signed in; P0–P4 all deferred; for now use API keys: GPT + DeepSeek/Kimi/MiniMax/MiMo etc.):

| Client | Recommended install method | Notes |
|---|---|---|
| `codex` | Official repo: `sudo pacman -S openai-codex` | Current repo version 0.166.0-1; do not overwrite `~/.local/bin` again with release packages or install scripts, or they will shadow the system package |
| `claude` | AUR: `yay -S claude-code` (or `ya-claude-code`) | `claude-code` pins the official binary's sha256; `ya-claude-code` additionally verifies against Anthropic's signature manifest |

Note: the official binary temporarily installed last round has been removed; going forward everything goes through the system package manager to avoid version drift between `~/.local/bin` and `/usr/bin`.

Region and network:

- `claude.ai` and `downloads.claude.ai` return a region-unavailable page on direct connection from this machine; AUR builds also fetch the binary from `downloads.claude.ai` and need a proxy (e.g. `https_proxy=http://127.0.0.1:10809`).
- Codex installs directly from the official repo; whether the sign-in endpoint is reachable must be verified on the actual network.
- Sign-in must be performed by the account holder; Loom does not hold accounts on behalf of users and does not read tokens.

Sign-in commands (for the account holder to run):

```bash
# OpenAI: device-code flow, prints URL + code
codex login --device-auth

# Anthropic: default Claude subscription
claude auth login
```

### P0: Confirm subscription sign-in and headless output

```bash
# OpenAI
codex login status
codex exec --json "print hello" > /tmp/codex-hello.json

# Anthropic
claude auth status
claude -p --output-format json "print hello" > /tmp/claude-hello.json
```

Record:

- sign-in method (ChatGPT / API key / Claude subscription / API key);
- whether the JSON contains usage, model name, session id;
- whether quota / rate-limit fields appear;
- whether the sign-in state is visible inside the container (prefer mounting `CODEX_HOME`, do not copy tokens).

### P1: Direct subscription lane

- complete one small read-only task with `codex exec`;
- complete the same kind of task with `claude -p`;
- record wall clock, usage, whether `429` triggered;
- confirm no API bill was produced (provider console / subscription usage page).

### P2: LiteLLM forwarding

Claude:

```bash
export ANTHROPIC_BASE_URL="http://127.0.0.1:4000"
export ANTHROPIC_CUSTOM_HEADERS="x-litellm-api-key: Bearer sk-<virtual-key>"
claude
```

Codex:

```toml
# ~/.codex/config.toml
model = "gpt-6.1-sol"
model_provider = "litellm"

[model_providers.litellm]
name = "LiteLLM"
base_url = "http://127.0.0.1:4000/v1"
env_key = "LITELLM_API_KEY"
wire_api = "responses"
```

BYO subscription also needs LiteLLM's client OAuth forwarding configuration; follow the docs for LiteLLM's current version and confirm that `used_client_oauth_token=true` or an equivalent field appears in the request log.

### P3: Insert Headroom

```text
claude → Headroom → LiteLLM → provider
codex  → Headroom → LiteLLM → provider
```

Check item by item:

- whether `Authorization` / `x-litellm-api-key` / `ChatGPT-Account-Id` / `anthropic-beta` reach LiteLLM;
- whether streaming responses are complete;
- whether the prefix cache still hits under `mode=cache`;
- whether Codex `/v1/responses` gets compressed (`msgs`, `transforms`, `tokens_saved`);
- whether a one-click fallback to direct connection works on failure.

### P4: Quota and scheduling

- run N small tasks in a row; record when `429` appears, reset time, available concurrency;
- compare subscription lane vs API lane on `CPVT`, `TPVT`, `WPT`;
- compute `api_equiv_usd` to confirm "is subscription actually cheaper than the API";
- verify that no API bill is produced when `allow_api_fallback=false`.

### Acceptance criteria

1. `loom doctor --auth` reports the sign-in status of each lane without reading/printing tokens.
2. One real task is completed through the subscription lane, the ledger is tagged `auth_mode=subscription`, and no API spend is produced.
3. `429` / expired sign-in enter cooldown and recover instead of being treated as ordinary task failures.
4. The subscription lane does not silently switch to the API by default; it switches only when explicitly allowed.
5. When the Headroom experiment fails, the direct path still works; no core protocol change is needed.

---

## 8. Impact on the roadmap

| Milestone | Change |
|---|---|
| **M0** | No change; keep API key + goose, do not install/sign in to subscription clients. |
| **M1** | Only the API lane's usage/ledger/`capability`; `auth_mode`, `billing_mode`, `credential_lane` are not landed yet — add them when subscription enters the roadmap. |
| **M2** | No subscription adapter yet; continue the API lane's adapter / native executor. |
| **M3** | If subscription demand actually exists by then, evaluate `loom-adapter-codex` / `loom-adapter-claude` and per-lane concurrency semaphores. |
| **M4** | Finish 2-way parallelism first; the subscription gateway (LiteLLM OAuth forwarding), hybrid lane, and multi-seat scheduling are all deferred. |

Explicitly not doing:

- do not write subscription tokens into `.loom/`, logs, or artifacts;
- no OAuth reverse proxy;
- no multi-account rotation;
- do not treat subscription `api_equiv_usd` as a real bill;
- do not rewrite Loom's task protocol for the sake of subscriptions.

---

## 9. Open questions

> All of the following depend on the subscription lane; they do not block the main line before subscription is enabled. For now we use API keys with multiple providers.

1. **Does OpenAI currently allow third-party tools to use Codex OAuth?** The official Codex CLI/SDK clearly supports ChatGPT sign-in; third-party clients (e.g. OpenClaw) have practice, but the official public terms still need item-by-item confirmation. Loom will only use official clients for now, to reduce risk.
2. **Will the Claude Agent SDK's subscription policy change again?** The official docs currently state clearly that "third parties may not offer claude.ai sign-in/quota without approval", so subscription scenarios use Claude Code CLI, not the Agent SDK.
3. **Does Headroom preserve subscription auth headers?** Unverified; this is a blocker for P3.
4. **Will Headroom compress Codex `/v1/responses`?** Community reports lean toward "no compression"; a local A/B is needed.
5. **Ledger accounting for LiteLLM subscription requests**: Claude can be estimated at API list prices; ChatGPT's `chatgpt/` provider may record $0. Loom must record `allowance_burn` independently itself.
6. **Quota windows and reset times**: providers do not necessarily give stable fields; when unknown, only conservative backoff is possible.
7. **Authentication inside the container**: how to mount `CODEX_HOME` / keyring / the Claude credential directory read-only needs a security review; do not copy tokens into images or the repository.
8. **Shared quota between interactive and headless**: Codex and ChatGPT Work may share; Claude's 5-hour/week window may also be shared with interactive use. Scheduling must be conservative.

---

## 10. Conclusion

1. **The current main line is API keys with multiple providers**: early on we can use GPT, DeepSeek, Kimi, MiniMax, MiMo etc. keys; first validate context, acceptance, and the ledger with GPT frontier + DeepSeek worker; subscription is not a prerequisite.
2. **Subscription is a separate lane**, not a Headroom mode and not a LiteLLM alias; enable it only once we have accounts and the main line is stable.
3. **Subscription goes only through official clients**: Codex CLI/SDK, Claude Code CLI; no OAuth token extraction, no Claude Agent SDK for subscription.
4. **Protocol dimensions are deferred**: `auth_mode` / `billing_mode` / `credential_lane` / `allow_api_fallback` and friends are added when subscription enters the roadmap; for now only `capability` and the API lane ledger remain.
5. **Headroom keeps serving the API lane**; the subscription lane is direct by default, and gateway forwarding and the hybrid lane are both deferred.
6. **LiteLLM can serve as a later experimental gateway**: it supports client OAuth forwarding for Claude and Codex, but headers, streaming, the model catalog, and the quota ledger must all be verified; do not rely on its `$0` spend for budgeting.
7. **Never silently fall back to the API**; quota exhaustion is a recoverable state, not a failure to re-run.

In one line: **get the "auth/billing plane" modeled right before talking about saving money with subscriptions; otherwise you are only hiding invisible quota burn inside the logs.**

---

## 11. References

> The following were publicly available when researching on 2026-11-05; versions and terms change — rely on official docs and local `--help` before acting.

- Claude Agent SDK overview (third-party claude.ai sign-in restriction): <https://code.claude.com/docs/en/agent-sdk/overview>
- Claude Code headless / `-p`: <https://code.claude.com/docs/en/headless>
- LiteLLM: Use your Claude subscription with Claude Code: <https://docs.litellm.ai/docs/tutorials/claude_code_max_subscription>
- LiteLLM: ChatGPT Subscription (`chatgpt/` provider): <https://docs.litellm.ai/docs/providers/chatgpt>
- LiteLLM: Connect Codex CLI to LiteLLM: <https://docs.litellm.ai/docs/proxy/client_setup/codex_cli>
- LiteLLM: Auto Router with Codex and your ChatGPT subscription (client OAuth forwarding, dual credentials): <https://docs.litellm.ai/docs/tutorials/codex_chatgpt_auto_router>
- OpenAI Codex SDK: <https://developers.openai.com/codex/codex-sdk>
- OpenAI Codex SDK (ChatGPT Learn mirror): <https://learn.chatgpt.com/docs/codex-sdk>
- OpenAI Codex authentication (sign-in cache, `CODEX_HOME`, credential store): <https://learn.chatgpt.com/docs/auth>
- OpenAI Help: Using Codex with your ChatGPT plan: <https://help.openai.com/en/articles/11369540-using-codex-with-your-chatgpt-plan>
- Headroom repository and proxy docs: <https://github.com/headroomlabs-ai/headroom>
- Headroom Codex `/v1/responses` routing/compression issue (community report): <https://github.com/headroomlabs-ai/headroom/issues/425>
