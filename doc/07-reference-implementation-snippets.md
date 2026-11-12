# 07 Reference Implementation Snippets

> Language: **English** | [简体中文](07-reference-implementation-snippets.zh-CN.md)

> These are "copy directly" skeletons, not final implementations. Field semantics follow 03; versions/parameters follow the local `--help`.

## 1. TaskSpec JSON Schema

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://loom.local/schema/task-spec-1.0.json",
  "title": "TaskSpec",
  "type": "object",
  "additionalProperties": false,
  "required": [
    "schema_version", "task_id", "goal_id", "title", "role", "tier",
    "depends_on", "workspace", "inputs", "constraints", "acceptance", "budget", "output"
  ],
  "properties": {
    "schema_version": {"const": "1.0"},
    "task_id": {"type": "string", "pattern": "^T-[0-9]{8}-[0-9]{3}$"},
    "goal_id": {"type": "string", "pattern": "^G-[A-Za-z0-9-]+$"},
    "title": {"type": "string", "minLength": 1, "maxLength": 80},
    "role": {"enum": ["survey", "spec-tests", "implement", "review", "judgment", "docs"]},
    "tier": {"enum": ["frontier", "worker"]},
    "depends_on": {"type": "array", "items": {"type": "string"}, "uniqueItems": true},
    "workspace": {
      "type": "object",
      "additionalProperties": false,
      "required": ["mode", "base_ref", "branch", "allowed_paths"],
      "properties": {
        "mode": {"enum": ["worktree", "in_place"]},
        "base_ref": {"type": "string"},
        "branch": {"type": "string"},
        "allowed_paths": {"type": "array", "items": {"type": "string"}, "minItems": 1},
        "forbidden_paths": {"type": "array", "items": {"type": "string"}}
      }
    },
    "inputs": {
      "type": "array",
      "minItems": 1,
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["path", "must_read"],
        "properties": {
          "path": {"type": "string"},
          "sha256": {"type": "string", "pattern": "^[a-f0-9]{64}$"},
          "range": {"type": "array", "items": {"type": "integer", "minimum": 1}, "minItems": 2, "maxItems": 2},
          "must_read": {"type": "boolean"}
        }
      }
    },
    "constraints": {
      "type": "object",
      "additionalProperties": false,
      "properties": {
        "max_files_touched": {"type": "integer", "minimum": 1},
        "no_new_dependencies": {"type": "boolean"},
        "style": {"type": "array", "items": {"type": "string"}},
        "notes": {"type": "string", "maxLength": 500}
      }
    },
    "acceptance": {
      "type": "array",
      "minItems": 1,
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["id", "cmd", "expect_exit", "timeout_s"],
        "properties": {
          "id": {"type": "string", "pattern": "^A[0-9]+$"},
          "cmd": {"type": "string", "minLength": 1},
          "expect_exit": {"type": "integer"},
          "timeout_s": {"type": "integer", "minimum": 1},
          "cwd": {"type": "string"},
          "network": {"type": "boolean", "default": false},
          "flaky": {"type": "boolean", "default": false}
        }
      }
    },
    "budget": {
      "type": "object",
      "additionalProperties": false,
      "required": ["max_input_tokens", "max_output_tokens", "max_steps", "max_wall_s", "max_cost_usd"],
      "properties": {
        "max_input_tokens": {"type": "integer", "minimum": 1},
        "max_output_tokens": {"type": "integer", "minimum": 1},
        "max_steps": {"type": "integer", "minimum": 1},
        "max_wall_s": {"type": "integer", "minimum": 1},
        "max_cost_usd": {"type": "number", "minimum": 0}
      }
    },
    "output": {
      "type": "object",
      "additionalProperties": false,
      "required": ["receipt", "report_path"],
      "properties": {
        "receipt": {"const": "4-line"},
        "report_path": {"type": "string"},
        "required_artifacts": {"type": "array", "items": {"type": "string"}}
      }
    },
    "context_policy": {
      "type": "object",
      "additionalProperties": false,
      "properties": {
        "max_must_read_files": {"type": "integer", "minimum": 1},
        "max_must_read_tokens": {"type": "integer", "minimum": 1},
        "allow_extra_reads": {"type": "boolean"},
        "forbid_full_repo_scan": {"type": "boolean"}
      }
    },
    "notes": {"type": "string", "maxLength": 500}
  }
}
```

## 2. Receipt JSON Schema (parsed)

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://loom.local/schema/receipt-1.0.json",
  "title": "Receipt",
  "type": "object",
  "additionalProperties": false,
  "required": ["schema_version", "task_id", "attempt", "status", "base_sha", "head_sha", "branch", "verify", "artifacts", "tokens", "report_path"],
  "properties": {
    "schema_version": {"const": "1.0"},
    "task_id": {"type": "string"},
    "attempt": {"type": "integer", "minimum": 1},
    "status": {"enum": ["done", "blocked", "needs_input", "failed", "budget_exceeded"]},
    "base_sha": {"type": "string"},
    "head_sha": {"type": "string"},
    "branch": {"type": "string"},
    "diffstat": {
      "type": "object",
      "properties": {
        "files": {"type": "integer"},
        "insertions": {"type": "integer"},
        "deletions": {"type": "integer"}
      }
    },
    "verify": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["id", "cmd", "exit", "log_path", "result"],
        "properties": {
          "id": {"type": "string"},
          "cmd": {"type": "string"},
          "exit": {"type": "integer"},
          "duration_s": {"type": "number"},
          "log_path": {"type": "string"},
          "result": {"enum": ["pass", "fail", "skip", "flaky"]}
        }
      }
    },
    "artifacts": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["path", "sha256", "bytes"],
        "properties": {
          "path": {"type": "string"},
          "sha256": {"type": "string"},
          "bytes": {"type": "integer"}
        }
      }
    },
    "tokens": {
      "type": "object",
      "required": ["input", "cached_input", "output", "cost_usd"],
      "properties": {
        "input": {"type": "integer"},
        "cached_input": {"type": "integer"},
        "output": {"type": "integer"},
        "reasoning": {"type": "integer"},
        "cost_usd": {"type": "number"}
      }
    },
    "decisions": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {"id": {"type": "string"}, "summary": {"type": "string"}}
      }
    },
    "blockers": {"type": "array", "items": {"type": "string"}},
    "next_hint": {"type": "string"},
    "report_path": {"type": "string"}
  }
}
```

## 3. Suggested `.gitignore`

```gitignore
.loom/runs/
.loom/locks/
.loom/events.jsonl
.loom/ledger/requests.jsonl
.loom/reports/*.log
.loom/escalations/*/diff.patch
.env
.env.*
```

> Human-readable state (GOAL/spec/design/tasks/receipts/reports.md/PROGRESS/DECISION) is recommended to be committed; machine logs are ignored.

## 4. LiteLLM config skeleton

```yaml
# config/litellm.yaml — illustrative; fields follow the local LiteLLM version
model_list:
  - model_name: frontier
    litellm_params:
      model: openai/gpt-6.1-sol
      api_base: https://api.openai.com/v1
      api_key: os.environ/OPENAI_API_KEY

  - model_name: worker
    litellm_params:
      model: openai/deepseek-v4-flash
      api_base: https://api.deepseek.com/v1
      api_key: os.environ/DEEPSEEK_API_KEY

  - model_name: worker-backup
    litellm_params:
      model: openai/kimi-worker
      api_base: os.environ/KIMI_API_BASE
      api_key: os.environ/KIMI_API_KEY

router_settings:
  routing_strategy: simple-shuffle
  num_retries: 2
  timeout: 600
  fallbacks:
    - {"frontier": ["frontier-backup"]}
    - {"worker": ["worker-backup"]}

litellm_settings:
  drop_params: true
  set_verbose: false

general_settings:
  master_key: os.environ/LITELLM_MASTER_KEY
```

**Notes**:

- `frontier-backup` must be defined as well; the example omits it.
- Early API keys cover GPT plus several Chinese model providers: keep `frontier` + DeepSeek `worker` first; pick one of Kimi/MiniMax/MiMo etc. as `worker-backup` and add the rest later through A/B, avoiding turning everything on at once.
- Model names/prices/context windows follow the provider docs; LiteLLM needs model metadata.
- If the client uses Anthropic Messages, confirm LiteLLM's `/v1/messages` compatibility.
- If Codex uses the Responses API, confirm `/v1/responses` compatibility; if unsupported, degrade per 06 §7.3.

## 5. Headroom startup skeleton

### 5.1 PowerShell

```powershell
headroom proxy `
  --port 8787 `
  --mode cache `
  --openai-api-url "http://127.0.0.1:4000" `
  --anthropic-api-url "http://127.0.0.1:4000"
```

### 5.2 bash

```bash
headroom proxy \
  --port 8787 \
  --mode cache \
  --openai-api-url "http://127.0.0.1:4000" \
  --anthropic-api-url "http://127.0.0.1:4000"
```

**Items to verify**:

- whether `--openai-api-url` / `--anthropic-api-url` exist in the current version;
- the behavior of `provider-type=auto`;
- whether the Anthropic format is forwarded through `/v1/messages`;
- the `retrieve` TTL and how tools are registered;
- whether subscription mode triggers `lossless-only`.

> Before P2, the downstream URL points at the provider, not at LiteLLM.

## 6. Client environment variables

```bash
# pi / OpenAI compatible
export OPENAI_BASE_URL="http://127.0.0.1:8787/v1"
export OPENAI_API_KEY="dummy"   # Headroom passes through; the real key lives at the gateway/upstream

# Anthropic compatible
export ANTHROPIC_BASE_URL="http://127.0.0.1:8787"
export ANTHROPIC_API_KEY="dummy"

# LiteLLM (if the client connects directly)
export LITELLM_BASE_URL="http://127.0.0.1:4000/v1"
export LITELLM_MASTER_KEY="..."
```

> Never let real provider keys enter the agent process; the gateway holds them. If the runtime must connect directly, use a least-privilege key and cap the budget.

## 7. Verify Runner (Node skeleton)

```js
// tools/verify.mjs — read the TaskSpec, run acceptance, write evidence
import { readFile, mkdir, writeFile } from "node:fs/promises";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import path from "node:path";

const specPath = process.argv[2];
if (!specPath) throw new Error("usage: node tools/verify.mjs <task-spec.json>");

const spec = JSON.parse(await readFile(specPath, "utf8"));
const cwd = process.cwd();
const logDir = path.join(".agent", "reports");
await mkdir(logDir, { recursive: true });

const results = [];
for (const a of spec.acceptance) {
  const started = Date.now();
  const logPath = path.join(logDir, `${spec.task_id}.${a.id}.log`);
  const out = [];
  const child = spawn(a.cmd, { cwd: a.cwd ?? cwd, shell: true, env: { ...process.env, CI: "1" } });
  child.stdout.on("data", d => out.push(d));
  child.stderr.on("data", d => out.push(d));
  const timeout = setTimeout(() => child.kill("SIGKILL"), a.timeout_s * 1000);
  const code = await new Promise(resolve => child.on("close", resolve));
  clearTimeout(timeout);
  const buf = Buffer.concat(out);
  await writeFile(logPath, buf);
  const result = code === a.expect_exit ? "pass" : "fail";
  results.push({
    id: a.id, cmd: a.cmd, exit: code, duration_s: (Date.now() - started) / 1000,
    log_path: logPath, log_sha256: createHash("sha256").update(buf).digest("hex"),
    result
  });
}

const allPass = results.every(r => r.result === "pass");
await writeFile(
  path.join(logDir, `${spec.task_id}.verify.json`),
  JSON.stringify({ task_id: spec.task_id, all_pass: allPass, results }, null, 2)
);
process.exit(allPass ? 0 : 1);
```

**Notes**:

- This is a minimal skeleton; production needs an allowlist, no-network enforcement, resource limits, and log redaction.
- `shell: true` carries injection risk; in production use a `cmd + args` array, or a sandbox.
- The timeout only kills the child process; production must kill the process group.
- Do not trust the worker's verify output; the runner re-runs it.

## 8. Four-line receipt parsing (TypeScript skeleton)

```ts
export type ReceiptStatus = "done" | "blocked" | "needs_input" | "failed" | "budget_exceeded";

export function parseReceipt(text: string): {
  status: ReceiptStatus; branch: string; sha: string; verify: string; log: string; report: string;
} {
  const lines = text.replace(/\r/g, "").trim().split("\n");
  if (lines.length !== 4) throw new Error(`receipt must be 4 lines, got ${lines.length}`);

  const get = (prefix: string) => {
    const line = lines.find(l => l.startsWith(prefix));
    if (!line) throw new Error(`missing ${prefix}`);
    return line.slice(prefix.length).trim();
  };

  const status = get("status: ") as ReceiptStatus;
  if (!["done", "blocked", "needs_input", "failed", "budget_exceeded"].includes(status)) {
    throw new Error(`bad status: ${status}`);
  }

  const branchRaw = get("branch: ");
  const at = branchRaw.lastIndexOf("@");
  if (at < 0) throw new Error("branch must be <branch>@<sha>");

  const verifyRaw = get("verify: ");
  const [verifyPart, logPart] = verifyRaw.split(";").map(s => s.trim());
  const log = logPart?.replace(/^log=/, "");
  if (!log) throw new Error("verify must include log=<path>");

  return {
    status,
    branch: branchRaw.slice(0, at),
    sha: branchRaw.slice(at + 1),
    verify: verifyPart,
    log,
    report: get("report: ")
  };
}
```

## 9. `pi-goal` cap example

```json
{
  "continuationLimits": {
    "automaticTurns": 25,
    "noProgressTurns": 3
  },
  "rpc": { "enabled": false }
}
```

> Field names follow the local `@narumitw/pi-goal` version; the token budget is not a money cap — the money cap belongs to Loom's budgeter.

## 10. Orchestrator Prompt Template

```markdown
你是 Loom 的 orchestrator。你只做四件事：
1. 维护 GOAL、任务图与验收标准；
2. 为 ready 任务生成 TaskSpec 并派发；
3. 读取四行回执与 verify 结果；
4. 决定合并、重试、升级或找人。

硬规则：
- 你不写代码，不读 worker transcript，不回贴文件内容/日志/diff。
- 任何实施任务必须有可执行的 acceptance；没有就先生成 spec-tests 任务。
- 跨边界只传路径、sha256、行号、diff 路径、verify 日志路径。
- 只有 verify 失败、规格冲突、预算超限才升级；每个任务最多升级一次。
- 主上下文只保留：GOAL 摘要、任务图摘要、最近 3 条回执、决策索引。
- 上下文超过 30k 时，停止并折叠旧回执；不要静默摘要证据。
- 预算超限立即停止，输出状态与证据路径。
- 不确定时输出 needs_input，不要猜。

每次输出只允许以下之一：
A. 一个 TaskSpec（JSON）；
B. 一个状态更新（任务 ID + 新状态 + 证据路径）；
C. 一个升级请求（证据包路径 + 待决问题，≤200 字）；
D. 一个 run 总结（≤300 字 + 指标）。

不要输出解释性长文、不要复述需求、不要贴代码。
```

## 11. Worker Prompt Template

```markdown
你是 Loom 的 worker。你只完成 TaskSpec 描述的一个切片。

开始：
- 先读 inputs 中 must_read=true 的引用；大文件按 range 读。
- 需要更多文件时，按路径自读；不要让 orchestrator 粘贴内容。
- 读 contracts/ 里的接口契约；不要自行改契约。

执行：
- 只改 allowed_paths；绝不碰 forbidden_paths。
- 保持现有风格；不新增依赖（除非 TaskSpec 允许）。
- 先写/补测试，再实现；运行 acceptance 中的命令。
- 失败时先自己修；超过预算或无法推进就停。
- 不要输出完整 diff、文件内容、原始日志；写到 report 文件。

结束：
- 只输出四行回执，顺序固定：
  status: done|blocked|needs_input|failed|budget_exceeded
  branch: <branch>@<sha>
  verify: A1=pass,A2=fail; log=<path>
  report: <path>
- 不要有前言、后记、代码块、空行。
- 详细信息写 report；回执不超过 100 tokens。

禁止：
- 修改 TaskSpec、验收标准、allowed_paths；
- 运行 TaskSpec 之外的破坏性命令；
- 访问网络（除非 TaskSpec 显式允许）；
- 把其他 worker 的日志/transcript 当上下文。
```

## 12. Judgment Prompt Template

```markdown
你是 Loom 的 judgment。你只读证据包，不做实现。

输入：
- TaskSpec、最近回执、diff、verify 日志、相关决策、文件引用。
- 不要读完整 transcript；不要猜缺失证据。

判断（四选一）：
- fix_plan：有明确修复计划；给出 ≤5 步，每步可执行/可验证。
- split：任务太大/太模糊；给出拆分建议（≤5 个切片）。
- reject：方案不可行/需求冲突；说明原因与需要谁决策。
- human：证据不足/风险高；列出待决问题（≤3 个）。

输出 JSON：
{
  "decision": "fix_plan|split|reject|human",
  "plan": ["..."],
  "refs": ["..."],
  "max_attempts": 1,
  "human_required": false,
  "evidence_gaps": ["..."]
}

约束：
- 只引用证据包里的内容；不要引入新事实。
- 不要因为“模型弱”就建议升级；只根据 verify 证据。
- 输出 ≤300 字（不含 JSON 字段名）。
```

## 13. Review Prompt Template

```markdown
你是 Loom 的 reviewer。只审查 diff 与证据，不重写代码。

检查：
1. 是否满足 spec/contracts 的验收场景；
2. 是否越权修改 allowed_paths 之外的路径；
3. 是否有安全/并发/数据迁移风险；
4. 测试是否覆盖失败路径；
5. 是否有未记录的决策。

输出：
- verdict: pass | request_changes | human
- reasons: ≤5 条，每条引用文件/行号/证据路径
- required_changes: ≤5 条，可执行
- 不贴完整 diff；只引用路径/行号。
```

## 14. Stats fetch commands

### 14.1 PowerShell

```powershell
$s = Invoke-RestMethod http://127.0.0.1:8787/stats

# client attribution (exact)
$s.agent_usage.coverage | Format-List *
$s.agent_usage.agents | Select-Object agent,source,requests,savings_percent | Format-Table

# model/routing (should change once multi-model is connected)
$s.requests.by_model
$s.routing

# cost and waste signals
@{ req=$s.summary.api_requests
   cost=$s.summary.cost.with_headroom_usd
   cache=$s.summary.cost.provider_cache_discount_usd
   json=$s.waste_signals.json_bloat
   reread=$s.waste_signals.reread } | Format-List
```

### 14.2 bash

```bash
curl -s http://127.0.0.1:8787/stats \
  | jq '{requests: .summary.api_requests,
         cost: .summary.cost.with_headroom_usd,
         cache: .summary.cost.provider_cache_discount_usd,
         models: .requests.by_model,
         routing: .routing,
         waste: .waste_signals}'
```

### 14.3 Measurement reminders

- `WasteSignals` is a cumulative detection count and never goes down; A/B looks at `Δvalue / Δrequests`.
- `share_of_saved_percent` can exceed 100% because of a wrong denominator; do not use it.
- The `reread_compressed` attribution is not in place; do not use it as a criterion.
- `by_model` may be affected by alias/client classification; cost follows LiteLLM/provider.
- `with_headroom_usd` is a single-snapshot measure; do not mix it with lifetime cumulative figures.

## 15. Compatibility checklist (must run before P2)

### 15.1 Protocol/features

- [ ] OpenAI Chat Completions: plain chat, streaming, tool calls, parallel tool calls
- [ ] OpenAI Responses: plain, streaming, tool calls (Codex path)
- [ ] Anthropic Messages: plain, streaming, tool use, system prompt
- [ ] Large tool results (>100k) without truncation/crash
- [ ] usage fields: input / output / cached / reasoning
- [ ] Errors: 400 / 401 / 429 / 5xx passthrough and retry
- [ ] fallback: same-tier switching does not change semantics
- [ ] Model alias: context window / cost metadata correct
- [ ] Images/PDFs (if used)
- [ ] Prompt cache headers/fields preserved
- [ ] Latency: extra p95 <100ms (excluding model time)
- [ ] Keys never leak into logs/responses

### 15.2 End-to-end

- [ ] `agent → Headroom → provider` (stage S)
- [ ] `agent → Headroom → LiteLLM → provider` (P2)
- [ ] Headroom dies → fall back to direct connection
- [ ] LiteLLM dies → direct/same-tier fallback
- [ ] 429 → backoff → success
- [ ] Compression policy behaves as expected in subscription/PAYG modes
- [ ] retrieve available (pi path); explicitly unavailable for Aider/Codex
- [ ] Ledger records alias + real model + cached tokens

## 16. Minimal CLI command surface

```bash
loom init                 # validate .loom/, generate the skeleton
loom task add --file f.json | --json '...'   # validate the TaskSpec and register it
loom task list|show|ready|graph              # task graph (status replayed from events)
loom context compile T-001 [--kind main|worker|judgment] [--budget-tokens N]
loom prompt list|show <role>                 # versioned prompt library
loom adapter list|canary <mock|native|goose|opencode> [--live --task T-001]
loom dispatch T-001 --adapter mock|native|goose|opencode [--in-place] [--model alias]
loom verify T-001 [--attempt N]              # independently re-run acceptance, write receipt.json
loom status | resume [--dispatch --adapter mock]
loom event list [--task T-001] [--type task.passed] [--limit N]
loom usage record|ingest                     # ledger writes
loom ledger [--by ...] [--passed-tasks N] [--wall-seconds S]
loom doctor | capability list|show|check | tool * | artifact
```

Schema: [`schemas/task-spec.v1.json`](../schemas/task-spec.v1.json), [`schemas/event.v1.json`](../schemas/event.v1.json), [`schemas/receipt.v1.json`](../schemas/receipt.v1.json), [`schemas/context-manifest.v1.json`](../schemas/context-manifest.v1.json), [`schemas/usage-record.v1.json`](../schemas/usage-record.v1.json), [`schemas/adapter-usage.v1.json`](../schemas/adapter-usage.v1.json).

## 17. Anti-pattern quick reference

| If you are thinking... | Stop, do this instead |
|---|---|
| "summarize the parent session for the child agent" | give the TaskSpec + file references |
| "have the child agent paste the diff back" | receipt + diff path |
| "let it judge for itself whether this is hard" | trigger escalation from verify results |
| "turn the compression ratio up first" | govern tool results first |
| "onboard four Chinese model vendors at once" | 1 primary + 1 fallback first, expand by A/B |
| "spin up a few more agents in parallel" | path locks/merge queue first |
| "use an LLM to estimate token cost" | use the LiteLLM/provider ledger |
| "this task has no tests, just do it first" | write spec-tests/acceptance first |
| "paste the whole log in here" | write the file + path + line numbers |
| "cache hit is low, switch to token mode" | keep cache, investigate prefix stability |
