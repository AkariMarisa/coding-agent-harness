# 11-订阅额度与 Agent SDK 对接

> **2026-11-05 调研笔记。状态：待本机验证。**  
> 本文补充 [08](08-设计路线-v0.2.md) 与 [10](10-工具输出协议-v0.1.md)，不改变“进程 + 文件 + JSON + exit code”的核心契约。  
> 结论：**订阅是一条独立的认证 / 计费 / 限流 lane，不是 API 的便宜镜像。** 不能假设它能透明穿过 Headroom / LiteLLM。

> **优先级更新（2026-11-05）：后置。** 当前先用 API key 跑通 API lane（GPT + DeepSeek/Kimi/MiniMax/MiMo 等 key 均可提供）；M0/M1 不实现订阅，也不要求安装/登录 codex / claude。本文保留为后续设计调研，不阻塞主线；前期投入订阅对接不划算。

---

## 1. 问题

现有方案默认“API key + 按 token 计费”：

```text
agent → Headroom → LiteLLM → provider（api.openai.com / api.anthropic.com）
```

但 ChatGPT Plus/Pro、Claude Pro/Max 这类订阅：

- 不按 token 出账，而是按 **席位 + 用量窗口** 限制；
- 使用 OAuth / 本机登录态，不是 `sk-...` API key；
- 端点、请求头、可用模型与公开 API 不完全相同；
- 超额时通常返回 `429` 或额度错误，而不是账单继续增长；
- 多 worker 并发会直接争抢同一个席位的额度。

因此需要回答三个问题：

1. 订阅能不能通过 Agent SDK / CLI 被程序化调用？
2. 订阅流量能不能继续走 Headroom / LiteLLM，保留统一日志与预算？
3. Loom 的任务、账本、调度、恢复协议要怎么改，才不会把“订阅额度”误记成“API 费用”？

---

## 2. 先澄清几个容易混淆的名字

| 名字 | 是什么 | 能否用订阅 | 对 Loom 的意义 |
|---|---|---|---|
| **OpenAI Agents SDK / Agents API** | OpenAI 平台侧的 agent 编排 API，按 API token 计费 | 否，走 API key | 不是订阅入口 |
| **Codex CLI** | OpenAI 官方编码 agent CLI | **是**，`codex login` 后使用 ChatGPT 计划 | 首选订阅 adapter |
| **Codex SDK**（`@openai/codex-sdk` / `openai-codex`） | 官方 SDK，包装本机 Codex 运行时 / 登录态 | **可以**，复用 Codex 的 file-backed sign-in | 需要结构化事件时的备选 |
| **Codex app-server** | Codex 的本地 JSON-RPC 服务 | 可以，复用 Codex 登录态 | 未来常驻 adapter 的候选 |
| **Claude Code CLI** | Anthropic 官方编码 agent CLI | **是**，`claude login` 后使用 Pro/Max 等计划 | 首选订阅 adapter |
| **Claude Agent SDK** | Python/TS SDK，内部运行 Claude Code binary | **官方条款限制**：未经批准，不允许第三方产品提供 claude.ai 登录/额度 | 订阅场景优先用 Claude Code CLI，不直接用 Agent SDK |
| **LiteLLM `chatgpt/` provider** | LiteLLM 的 ChatGPT 订阅 provider | 可以，但代理自己持有一个账号 | 适合单席位；不是 BYO 多席位方案 |
| **LiteLLM client OAuth 转发** | 客户端把 OAuth token 放 `Authorization`，网关 key 放 `x-litellm-api-key` | 可以，保留每个开发者自己的订阅 | 多席位网关的候选，需验证 |
| **Headroom** | 压缩 / 缓存代理 | 本身不理解订阅 | 可能可用，但必须验证 OAuth 头与流式响应 |

**关键纠正**：用户说的“Agent SDK 消耗订阅额度”，在 OpenAI 侧对应的是 **Codex SDK / Codex CLI**，不是 API 侧的 Agents SDK；在 Anthropic 侧对应的是 **Claude Code CLI 的 headless 模式**，不是任意第三方使用 Claude Agent SDK。

---

## 3. 订阅与 API 的建模差异

| 维度 | API（PAYG） | 订阅（Seat / Allowance） |
|---|---|---|
| 凭证 | API key | OAuth 登录态、席位、工作区 |
| 计费 | 按 token / 请求 | 预付席位费 + 用量窗口 |
| 用量单位 | input/output/cache token | 请求数、token 当量、5 小时/周窗口、额度百分比 |
| 可见性 | 账单和 usage 精确 | 部分可见，窗口和重置时间不总是公开 |
| 并发 | 主要受 RPM/TPM 限制 | 受席位、窗口、并发共同限制 |
| 超额 | 可继续计费或预算熔断 | `429` / 额度耗尽 / 等待重置 |
| 多 worker | 可水平扩展 | 同一席位会互相抢额度 |
| 成本口径 | 美元 | **不能直接记美元**；只能记 API 等价估算 + 额度消耗 |
| Headroom | 压缩能减少账单 | 压缩主要帮助上下文装得下，不直接省钱 |
| 失败模式 | 预算、限流、网络 | 额度窗口、登录过期、端点变更、`429` |

对 Loom 的影响：

1. `CPVT` 仍然有用，但订阅任务的“成本”应拆成 `api_equiv_usd` 与 `allowance_burn`，不能混入真实账单。
2. `WPT` 要包含“等额度重置”的时间；否则订阅路线看起来便宜，实际墙钟更差。
3. 调度器必须知道 **哪个席位、哪个窗口、当前并发**；不能只按模型 alias 轮询。
4. 订阅额度耗尽时，**默认不能静默切到 API**，否则会产生用户没有预期的账单。

额度形态还在变化：部分计划从“请求数/窗口”转向 token/credits 计量，模型 ID 也可能硬下线。因此：

- 账本必须同时记原始 `usage` 字段和 Loom 自己的 `allowance_burn`，不假设额度单位稳定；
- 模型 ID 变更/下线后必须重跑该 lane 的验收基线，不能沿用旧 `CPVT`；
- `loom doctor --auth` 只报告账号和额度状态，不推断“还能跑多少任务”。

---

## 4. 三条可行路线

### 4.1 路线 A：官方客户端 adapter lane（推荐）

```text
loom-adapter-codex  → codex exec / Codex SDK  → ChatGPT 订阅端点
loom-adapter-claude → claude -p               → Anthropic 订阅端点
```

做法：

- 只调用官方 CLI / 官方 SDK；
- 复用本机已有登录态，不读取、不复制、不转发 OAuth token；
- adapter 从 JSON/JSONL 输出中提取 usage、最终消息、退出码；
- 默认不经过 Headroom / LiteLLM；需要统一日志时再显式开启网关实验；
- 每个席位一个并发信号量，`429` 时冷却并等待窗口。

优点：最符合官方使用方式，端点变化由官方客户端吸收，凭证不进入 Loom。
缺点：每个 provider 一个 adapter；usage 字段随版本变化；无法复用 LiteLLM 的统一预算。
结论：**订阅可用后的主路径；当前不启用，先用 API key。**

### 4.2 路线 B：网关转发订阅（实验）

```text
codex / claude
  → Headroom（可选）
  → LiteLLM
  → provider 订阅端点
```

LiteLLM 现在支持两类订阅转发：

1. **客户端 OAuth 转发**：客户端保留自己的 ChatGPT / Claude 登录态，OAuth token 放 `Authorization`，LiteLLM key 放 `x-litellm-api-key`，LiteLLM 透传到上游。
2. **代理持有账号**：LiteLLM 用 `chatgpt/` provider + `auth.json` 自己登录；所有请求共用同一个 ChatGPT 账号，客户端传来的 token 被忽略。

第 2 类适合单席位 / 个人代理，不适合“每个开发者用自己的订阅”；第 1 类才是多席位候选。

风险：

- Headroom 是否原样保留 `Authorization`、`x-litellm-api-key`、`ChatGPT-Account-Id`、`anthropic-beta` 等头，未验证；
- Codex 走 `/v1/responses`，Headroom 对该路径可能只转发、不压缩；
- 订阅端点可能受 Cloudflare / 风控 / 端点变更影响；
- LiteLLM 对订阅请求常记录 **$0**，`max_budget` 不触发，需要改用 RPM/TPM 或自定义额度账本；
- 两层代理增加故障面，排障困难。

结论：**P2 实验，不进入 v1 主路径。** 通过兼容性测试后再考虑。

### 4.3 路线 C：自建 OAuth 代理 / 提取 token（拒绝）

包括：读取 `~/.codex/auth.json`、复制 Claude keychain、把订阅 token 注入自定义 OpenAI-compatible 代理、做多账号轮换等。

拒绝原因：

- 违反 provider 条款的风险高；
- token 刷新、撤销、风控、端点变化都不可控；
- 一旦泄漏，影响的是用户主账号；
- 与 Loom “凭证不进入控制面”的安全边界冲突。

**不实现。**

---

## 5. 对 Loom 协议的最小改动

### 5.1 新增维度，不新增魔法

> 当前阶段只走 API key（GPT + DeepSeek/Kimi 等），先不实现以下订阅维度；等订阅真正进入路线图时再加。API lane 用默认值即可工作。

在 `TaskSpec` / adapter 配置中增加：

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

建议枚举：

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

`CredentialLane` 至少包含：

| 字段 | 说明 |
|---|---|
| `id` | 如 `codex-pro`、`claude-max`、`deepseek-api` |
| `provider` | `openai` / `anthropic` / `deepseek` / ... |
| `auth_mode` | `api` / `subscription` / `local` |
| `billing_mode` | `pay_per_token` / `seat_allowance` / `free` |
| `account_label` | 人类可读，如 `work-chatgpt`；不要存 token |
| `quota_pool` | 共享同一额度的池，如 `chatgpt-pro-personal` |
| `max_concurrency` | 该席位允许的并发 |
| `gateway` | `direct` / `litellm` / `headroom+litellm` |
| `allow_api_fallback` | 额度耗尽时是否允许切 API，默认 `false` |

同一 `credential_lane` 还关联一组 **模型能力位**（`tier` / `thinking_mode` / `reasoning_passthrough` / `wire` / 输入/缓存/输出价格 / `model_id` 等）。配置定义见 [06](06-技术选型.md) §5.6，成本口径见 [04](04-上下文与成本模型.md) §6.5。调度器按能力位过滤，不按模型名写死。

### 5.2 Adapter 能力声明

adapter 应报告自己支持什么，而不是让 orchestrator 猜：

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

缺失字段标 `unknown`，不要让账本用 0 冒充。

### 5.3 账本字段

在现有 `usage.jsonl` / `tools.jsonl` 之外，任务级用量至少增加：

| 字段 | 说明 |
|---|---|
| `auth_mode` | `api` / `subscription` / `local` |
| `billing_mode` | `pay_per_token` / `seat_allowance` |
| `credential_lane` | 席位 / 账号标签 |
| `api_equiv_usd` | 按公开 API 价格估算，**不是账单** |
| `allowance_units` | 订阅额度消耗（provider 原始单位或估算） |
| `quota_window` | `5h` / `weekly` / `monthly` / `unknown` |
| `reset_at` | 已知时的重置时间 |
| `wait_ms` | 因额度/限流等待的时间 |
| `source` | `provider` / `litellm` / `adapter` / `estimated` |

规则：

- `api_equiv_usd` 与真实 API 账单分开聚合；
- 订阅任务不能计入 `spend_usd`，只能计 `api_equiv_usd`；
- `allowance_burn` 与 `api_equiv_usd` 同时保留，用于比较“订阅是否真的更划算”；
- `429`、登录过期、端点错误要单独计数，不能混进“任务失败”。

### 5.4 调度与恢复

- 每个 `credential_lane` 一个信号量 / 令牌桶；
- 同一 `quota_pool` 的任务共享并发上限；
- 收到 `429` 或额度错误时：
  1. 记录 `quota_exhausted`；
  2. 根据 `reset_at` 或指数退避设置冷却；
  3. 默认暂停该 lane，不自动切 API；
  4. 用户显式允许时才走 `allow_api_fallback`。
- `loom resume` 必须能恢复“等待额度”的任务，而不是把它当成失败重跑。
- 交互式使用与 headless 自动化共享额度（ChatGPT / Claude 都可能有共享窗口），调度器应保守估计，不要假设 headless 是独立池。

---

## 6. Headroom / LiteLLM 兼容性判断

### 6.1 Claude 路径

LiteLLM 文档给出的形态：

```text
claude → LiteLLM → api.anthropic.com
```

- Claude Code 保留自己的 OAuth 登录并刷新；
- LiteLLM 通过 `x-litellm-api-key` 做网关认证；
- LiteLLM 转发客户端 OAuth token，日志可标记 `used_client_oauth_token=true`；
- 成本按 Anthropic API 列表价估算，但实际由订阅计划出账。

如果把 Headroom 插在中间：

```text
claude → Headroom → LiteLLM → api.anthropic.com
```

必须验证：

- `Authorization` 是否原样透传；
- `x-litellm-api-key` 是否保留；
- `anthropic-beta` / `anthropic-version` 是否保留；
- 流式 SSE 是否不被改写；
- `mode=cache` 是否仍能命中前缀缓存；
- 订阅态是否触发更保守的压缩策略。

### 6.2 Codex 路径

```text
codex → LiteLLM → chatgpt.com/backend-api/codex
```

LiteLLM 支持两种模式：

- **BYO 订阅**：Codex 的 ChatGPT 登录放 `Authorization`，LiteLLM key 放 `x-litellm-api-key`，LiteLLM 只转发、不存 token；
- **代理账号**：LiteLLM 自己用 `chatgpt/` + `auth.json` 登录，所有请求共用同一账号；客户端 token 被忽略，记录 $0 spend。

注意：

- Codex 使用 Responses API；
- Headroom 另有**原生 `/v1/responses` 代理**，能否压缩与是否只转发要分别验证；社区已有 `msgs=0`、`transforms=none` 的报告；
- LiteLLM 的 Headroom guardrail 在 `pre_call` 阶段改写 `messages`，官方列出的压缩端点只有 `/v1/chat/completions` 与 `/v1/messages`，**不覆盖 Responses**；
- 订阅态可能需要 `requires_openai_auth`、`ChatGPT-Account-ID` 等头/字段，Headroom 是否原样保留未验证；
- Codex 对未知模型 alias 可能发送完整工具定义，导致 prompt 从约 13.5k 涨到约 155k，**快速烧订阅额度**；必须配置模型 catalog 或使用 Codex 已知模型名；
- 订阅端点的模型列表与公开 API 不同，不能假设 `gpt-*` 名称通用。

### 6.3 结论

| 路径 | v1 建议 | 原因 |
|---|---|---|
| API lane → Headroom → LiteLLM | **保留** | 当前设计，账单和缓存收益明确 |
| Claude Code 订阅直连 | **支持** | 官方客户端，最少假设 |
| Codex 订阅直连 | **支持** | 官方客户端，最少假设 |
| Claude 订阅 → LiteLLM | **可实验** | LiteLLM 官方支持，需验证 Headroom |
| Codex 订阅 → LiteLLM | **可实验** | 支持 BYO 转发，但端点/模型 catalog 不稳 |
| 订阅 → Headroom → LiteLLM | **默认关闭** | 头和流式未验证；失败时回退直连 |
| 自建 OAuth 代理 | **拒绝** | 条款、安全、稳定性都不可接受 |

**硬规则**：订阅 lane 不能静默降级到 API lane。降级必须由任务策略显式允许，并写入事件日志。

### 6.4 混合 lane：订阅做决策，API 做执行

对多数长任务，最省的形态不是“全链路套网关”，而是把两个平面分开：

```text
[控制面 / 决策面]
  Codex / Claude 订阅（官方客户端直连，短稳定前缀）
        │  TaskSpec + 验收标准
        ▼
[执行面]
  廉价 API 模型 → Headroom（cache）→ LiteLLM → provider
        │  四行回执 + 文件引用
        ▼
[验收面]
  本地 verify / 廉价模型；只回结构化结果
```

规则：

- 订阅 lane **不要插 LiteLLM**；Responses 的 reasoning item 和自定义工具类型翻译会丢结构；
- 执行面才用 `Headroom → LiteLLM`，这里正是压缩和路由的主场；
- 两平面只交换 **TaskSpec + 四行回执 + 文件引用**；执行面的 transcript 不回流控制面；
- 控制面只保留目标、约束、决策、回执索引，不读 diff/日志全文；
- 账本分开记 `api_equiv_usd`、`allowance_burn`、`provider_cache_discount_usd`，不混加；
- 混合 lane 先作为 M3 的实验，不改变 v1 的“订阅直连 + API 直连”两条独立路径。

---

## 7. 本机验证计划

**2026-11-05 本机状态**（未登录，P0–P4 全部后置；当前先用 API key：GPT + DeepSeek/Kimi/MiniMax/MiMo 等）：

| 客户端 | 推荐安装方式 | 备注 |
|---|---|---|
| `codex` | 官方仓库：`sudo pacman -S openai-codex` | 当前仓库版本 0.166.0-1；不要再用发布包或安装脚本覆盖 `~/.local/bin`，否则会挡住系统包 |
| `claude` | AUR：`yay -S claude-code`（或 `ya-claude-code`） | `claude-code` 固定官方二进制 sha256；`ya-claude-code` 额外按 Anthropic 签名清单校验 |

说明：上一轮临时安装的官方二进制已移除；后续统一走系统包管理器，避免 `~/.local/bin` 与 `/usr/bin` 的版本漂移。

区域与网络：

- `claude.ai` 与 `downloads.claude.ai` 在本机直连会返回区域不可用页面；AUR 构建也要从 `downloads.claude.ai` 拉二进制，需经代理（如 `https_proxy=http://127.0.0.1:10809`）。
- Codex 可从官方仓库直接安装；登录端点是否可用需按实际网络验证。
- 登录必须由账号持有人完成，Loom 不代持账号、不读取 token。

登录命令（供账号持有人执行）：

```bash
# OpenAI：设备码流程，输出 URL + code
codex login --device-auth

# Anthropic：默认 Claude 订阅
claude auth login
```

### P0：确认订阅登录与 headless 输出

```bash
# OpenAI
codex login status
codex exec --json "print hello" > /tmp/codex-hello.json

# Anthropic
claude auth status
claude -p --output-format json "print hello" > /tmp/claude-hello.json
```

记录：

- 登录方式（ChatGPT / API key / Claude 订阅 / API key）；
- JSON 中是否包含 usage、模型名、session id；
- 是否出现额度/限流字段；
- 容器内是否能看到登录态（优先 `CODEX_HOME` 挂载，不复制 token）。

### P1：直连订阅 lane

- 用 `codex exec` 完成一个只读小任务；
- 用 `claude -p` 完成同类任务；
- 记录墙钟、usage、是否触发 `429`；
- 确认没有 API 账单产生（provider 控制台 / 订阅用量页）。

### P2：LiteLLM 转发

Claude：

```bash
export ANTHROPIC_BASE_URL="http://127.0.0.1:4000"
export ANTHROPIC_CUSTOM_HEADERS="x-litellm-api-key: Bearer sk-<virtual-key>"
claude
```

Codex：

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

BYO 订阅还需要 LiteLLM 的 client OAuth 转发配置；按 LiteLLM 当前版本文档执行，确认请求日志出现 `used_client_oauth_token=true` 或等价字段。

### P3：插入 Headroom

```text
claude → Headroom → LiteLLM → provider
codex  → Headroom → LiteLLM → provider
```

逐项检查：

- `Authorization` / `x-litellm-api-key` / `ChatGPT-Account-Id` / `anthropic-beta` 是否到达 LiteLLM；
- 流式响应是否完整；
- `mode=cache` 下前缀缓存是否仍命中；
- Codex `/v1/responses` 是否有压缩（`msgs`、`transforms`、`tokens_saved`）；
- 失败时能否一键回退直连。

### P4：额度与调度

- 连续跑 N 个小任务，记录 `429` 出现时间、重置时间、可用并发；
- 对比订阅 lane 与 API lane 的 `CPVT`、`TPVT`、`WPT`；
- 计算 `api_equiv_usd`，确认“订阅是否真的比 API 便宜”；
- 验证 `allow_api_fallback=false` 时不会产生 API 账单。

### 验收标准

1. `loom doctor --auth` 能报告每条 lane 的登录状态，且不读取/打印 token。
2. 一个真实任务通过订阅 lane 完成，账本标 `auth_mode=subscription`，不产生 API 费用。
3. `429` / 登录过期能进入冷却并恢复，不当作普通任务失败。
4. 订阅 lane 默认不静默切 API；显式允许时才切。
5. Headroom 实验失败时，直连路径仍可用；不需要改核心协议。

---

## 8. 对路线图的影响

| 里程碑 | 改动 |
|---|---|
| **M0** | 不改；继续 API key + goose，不安装/登录订阅客户端。 |
| **M1** | 只做 API lane 的 usage/ledger/`capability`；`auth_mode`、`billing_mode`、`credential_lane` 先不落，等订阅进入路线图再加。 |
| **M2** | 暂不做订阅 adapter；继续 API lane 的 adapter / native executor。 |
| **M3** | 如届时确有订阅需求，再评估 `loom-adapter-codex` / `loom-adapter-claude` 和每 lane 并发信号量。 |
| **M4** | 先完成 2 并行；订阅网关（LiteLLM OAuth 转发）、混合 lane、多席位调度都后置。 |

明确不做：

- 不把订阅 token 写进 `.loom/`、日志、artifact；
- 不做 OAuth 反向代理；
- 不做多账号轮换；
- 不把订阅 `api_equiv_usd` 当真实账单；
- 不为了订阅而重写 Loom 的任务协议。

---

## 9. 开放问题

> 以下问题都依赖订阅 lane；在订阅启用前不阻塞主线。当前先用 API key 多 provider。

1. **OpenAI 当前是否允许第三方工具使用 Codex OAuth？** 官方 Codex CLI/SDK 明确支持 ChatGPT 登录；第三方客户端（如 OpenClaw）有实践，但官方公开条款仍需逐条确认。Loom 先只走官方客户端，降低风险。
2. **Claude Agent SDK 的订阅政策是否还会变？** 官方文档目前明确“未经批准不允许第三方提供 claude.ai 登录/额度”，所以订阅场景用 Claude Code CLI，不用 Agent SDK。
3. **Headroom 是否保留订阅认证头？** 未验证；这是 P3 的阻断项。
4. **Codex `/v1/responses` 是否会被 Headroom 压缩？** 社区报告倾向“不压缩”；需要本机 A/B。
5. **LiteLLM 订阅请求的账本口径**：Claude 可按 API 列表价估算；ChatGPT `chatgpt/` provider 可能记 $0。Loom 自己必须独立记 `allowance_burn`。
6. **额度窗口与重置时间**：provider 不一定给稳定字段；未知时只能保守退避。
7. **容器内认证**：`CODEX_HOME` / keyring / Claude 凭据目录如何只读挂载，需要安全评审；不要把 token 复制进镜像或仓库。
8. **交互式与 headless 共享额度**：Codex 与 ChatGPT Work 可能共享；Claude 的 5 小时/周窗口也可能与交互使用共享。调度必须保守。

---

## 10. 结论

1. **当前主线是 API key 多 provider**：前期可用 GPT、DeepSeek、Kimi、MiniMax、MiMo 等 key，先 GPT frontier + DeepSeek worker 验证上下文、验收、账本；订阅不是前置条件。
2. **订阅是独立 lane**，不是 Headroom 的一个 mode，也不是 LiteLLM 的一个 alias；等有账号且主线稳定后再启用。
3. **订阅只走官方客户端**：Codex CLI/SDK、Claude Code CLI；不提取 OAuth token，不用 Claude Agent SDK 接订阅。
4. **协议维度后置**：`auth_mode` / `billing_mode` / `credential_lane` / `allow_api_fallback` 等订阅进入路线图时再加；现在只保留 `capability` 与 API lane 账本。
5. **Headroom 继续服务 API lane**；订阅 lane 默认直连，网关转发和混合 lane 都后置。
6. **LiteLLM 可作为后续实验网关**：支持 Claude 和 Codex 的客户端 OAuth 转发，但必须验证头、流式、模型 catalog 和额度账本；不要依赖它的 `$0` spend 做预算。
7. **绝不静默 API 回退**；额度耗尽是可恢复状态，不是失败重跑。

一句话：**先把“认证/计费平面”建模对，再谈用订阅省钱；否则只是把不可见的额度消耗藏进日志里。**

---

## 11. 参考

> 以下为 2026-11-05 调研时可见的公开资料；版本和条款会变，落地前以官方文档与本机 `--help` 为准。

- Claude Agent SDK overview（第三方 claude.ai 登录限制）：<https://code.claude.com/docs/en/agent-sdk/overview>
- Claude Code headless / `-p`：<https://code.claude.com/docs/en/headless>
- LiteLLM：Use your Claude subscription with Claude Code：<https://docs.litellm.ai/docs/tutorials/claude_code_max_subscription>
- LiteLLM：ChatGPT Subscription（`chatgpt/` provider）：<https://docs.litellm.ai/docs/providers/chatgpt>
- LiteLLM：Connect Codex CLI to LiteLLM：<https://docs.litellm.ai/docs/proxy/client_setup/codex_cli>
- LiteLLM：Auto Router with Codex and your ChatGPT subscription（客户端 OAuth 转发、双凭证）：<https://docs.litellm.ai/docs/tutorials/codex_chatgpt_auto_router>
- OpenAI Codex SDK：<https://developers.openai.com/codex/codex-sdk>
- OpenAI Codex SDK（ChatGPT Learn 镜像）：<https://learn.chatgpt.com/docs/codex-sdk>
- OpenAI Codex authentication（登录缓存、`CODEX_HOME`、credential store）：<https://learn.chatgpt.com/docs/auth>
- OpenAI Help：Using Codex with your ChatGPT plan：<https://help.openai.com/en/articles/11369540-using-codex-with-your-chatgpt-plan>
- Headroom 仓库与 proxy 文档：<https://github.com/headroomlabs-ai/headroom>
- Headroom Codex `/v1/responses` 路由/压缩问题（社区报告）：<https://github.com/headroomlabs-ai/headroom/issues/425>
