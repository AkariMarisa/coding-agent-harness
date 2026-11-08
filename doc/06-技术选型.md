# 06 技术选型

> **v0.2 修订（2026-11-04）**：pi 已从“主路径”降为“可选 adapter”；核心改为独立 CLI/daemon + 进程级执行器契约。  
> 选型结论以 [08-设计路线-v0.2.md](08-设计路线-v0.2.md) 为准；本文的工具对照与 Aider/Headroom/LiteLLM 边界仍然有效。

> 本文的目标不是“选一个最强工具”，而是回答：**哪些必须自己写，哪些必须复用，哪些坚决不碰。**  
> 工具版本与能力以 2026-11 前序讨论和本机实测为准；落地前必须逐项核实。

## 0. 选型原则

1. **Runtime 复用**：不在 agent loop、工具执行、沙箱、流式上重造轮子。
2. **网关复用**：压缩用 Headroom，路由用 LiteLLM；不自研。
3. **编排自研**：状态协议、任务图、verify、预算、账本——这些是差异化，且很薄。
4. **可替换**：任何组件都能在一天内回退/替换；不把 provider 地址写死。
5. **可观测**：选型必须能拿到 usage / 错误 / 延迟；黑盒不算合格。
6. **CPVT 优先**：功能多不等于值得；用同任务集 A/B 证明。

## 1. 候选方案对照

| 方案 | Runtime | 多模型 | 长任务 | 子 agent 隔离 | 压缩/缓存 | 适配度 | 结论 |
|---|---|---|---|---|---|---|---|
| **pi + 插件**（当前） | 成熟 | 插件/配置支持，需核实 | goal 插件可续跑 | subagent 独立上下文 | pi-headroom 插件 + proxy | **高** | **主路径** |
| **Codex CLI**（当前） | 成熟 | subagent model override，需核实 | goal mode（体验一般） | 有 subagent | 可经 Headroom | 高 | 可选 worker/回退 |
| **Claude Code + LiteLLM/CCR** | 成熟 | 强（按场景路由） | 支持 | 有 | LiteLLM guardrail/Headroom | 中高 | 可参考，不强制迁移 |
| **Aider** | 成熟 | 底层 LiteLLM，architect/editor 双模型 | 弱（无 goal 语义） | 无（共享 transcript） | 可经 Headroom | 中 | **仅旁路 A/B** |
| **DeepSeek Harness / DSH** | 插件化运行时 | 强 | 支持 | 有 | 插件 | 中 | 预览版，暂不上生产 |
| **Kilo / Roo Code** | IDE 内 | 强（任务级路由） | 中 | 有 | 有限 | 中 | 编辑器内可用，非 CLI 长跑 |
| **自研完整 harness** | 自研 | 自研 | 自研 | 自研 | 自研 | — | **否决** |

### 1.1 说明

- **pi**：已有 `subagents`、`goal`、`headroom`、`context-mode` 等插件，最接近“现成底座”。缺点是插件生态碎片化、版本漂移快；用 Adapter 隔离。
- **Codex**：模型能力强，适合 frontier/回退；多模型与子 agent 配置需按本机版本核实；Responses API 兼容性是风险点。
- **Claude Code + LiteLLM/CCR**：路由与压缩组合成熟，但迁移成本高；如果将来 pi 生态不稳，可作为备选 runtime。
- **Aider**：`--architect` / `--editor-model` / `--weak-model` 是经典双模型分诊；但没有子 agent 上下文隔离、没有 goal 语义、没有 retrieve。只适合做 diff 生成/A-B。
- **DSH**：插件化、模型无关，但官方标注开发者预览，接口会变；不作为生产关键路径。
- **Kilo/Roo**：IDE 内体验好，但不是“常驻 CLI + 长任务 + 文件状态”的形态。

## 2. 推荐组合

```
Runtime：pi（主）+ Codex（可选 worker/回退）
编排：  自研 TypeScript runner（C1/C4/C6/C7/C13）
状态：  .agent/ + git worktree
网关：  Headroom（cache）+ LiteLLM（P2 起）
账本：  JSONL → SQLite/DuckDB（按需）
验收：  项目原生 test/lint/typecheck/build
旁路：  Aider（仅 A/B / diff 生成）
```

### 2.1 为什么以 pi 为主

- 已有 subagent、goal、headroom、context 插件；
- 插件形态适合“agent 端压缩 + 常驻 proxy”的现有拓扑；
- TypeScript/Node 生态，便于写 runner 和 schema；
- 失败时可回退 Codex，不锁定。

### 2.2 为什么 Codex 不作为主编排

- goal mode 体验一般（前序反馈）；
- 多模型/子 agent 配置受版本影响；
- Responses API 与 LiteLLM/Headroom 的兼容性需要额外验证；
- 适合作为“强模型 worker/回退”，而不是调度中心。

### 2.3 为什么不用 Aider 作为主路径

见 §6。

## 3. 为什么不自研完整 Runtime

| 自研项 | 工作量 | 已有替代 | 差异化 | 结论 |
|---|---|---|---|---|
| Agent loop / 工具执行 | 很大 | pi / Codex / SDK | 低 | 不自研 |
| 沙箱/权限 | 大 | OS/容器/runtime | 低 | 不自研 |
| 压缩/缓存 | 大 | Headroom | 低 | 不自研 |
| Provider 路由/重试 | 中 | LiteLLM | 低 | 不自研 |
| 流式/工具调用协议 | 大 | runtime/SDK | 低 | 不自研 |
| 状态协议 | 小 | 无 | **高** | 自研 |
| 任务图/调度 | 中 | 无 | **高** | 自研 |
| verify/预算/升级 | 小 | 无 | **高** | 自研 |
| 账本/指标 | 小 | 部分 | **高** | 自研 |

**结论**：自研 1–2k 行编排层，换取对 token 结构的控制；不碰 runtime。

## 4. Headroom 能力边界

> 以下来自前序讨论与源码/文档观察；不同版本可能变化，落地前以本机为准。

| 能力 | 现状 | 对 Loom 的影响 | 对策 |
|---|---|---|---|
| `mode` | 进程启动配置（`cache`/`token`） | 不能 per-request 切 | 固定 `cache`；未来按客户端/端口分实例 |
| 客户端分类 | 看 `User-Agent` / `X-Client` | 不认 `model` | 不做按模型归属；用 LiteLLM 账本 |
| AuthMode 策略 | PAYG / Subscription 不同；订阅态更保守 | 压缩策略全局 | 接受；不指望它按模型优化 |
| `size_floor` | 跳过小结果（实测约 71% units） | 零散 JSON 压不掉 | 工具层投影/引用 |
| `WasteSignals` | 累计检测量 | 不能当“节省” | 只看 Δ/Δrequests |
| `retrieve` | 有 TTL；需客户端注册工具 | Aider/Codex 取不回 | 关键链路保留原文文件 |
| stats | 请求/压缩/延迟诊断 | 成本口径不可靠 | provider/LiteLLM 为准 |
| 压缩 ROI | 0.70% vs 缓存 $270 | 不值得调参 | 保持 cache |

### 4.1 结论

Headroom 是**压缩/缓存层**，不是多模型调度层。不要试图让它按 `model` 切策略；把路由放到 LiteLLM，把上下文治理放到 Loom。

## 5. LiteLLM 的放置与职责

### 5.1 放置

```
agent → Headroom → LiteLLM → provider
```

### 5.2 职责

- alias：`frontier` / `worker` / `worker-backup`；
- provider 适配：OpenAI Chat / Responses / Anthropic Messages；
- 重试/fallback/限流；
- 预算/密钥管理；
- 权威请求日志（供账本）。

### 5.3 不负责

- 上下文隔离；
- 任务拆解；
- 压缩策略（不要叠 guardrail）；
- verify/goal。

### 5.4 风险

- 不同客户端协议（OpenAI/Anthropic/Responses）兼容性不一致；
- alias 可能导致 Headroom 看不到真实模型；
- 模型元数据（context window / cost）缺失会报错；
- 两个槽位混用协议前缀可能被拒。

**对策**：兼容性矩阵 + canary + 直连回退；账本记录 alias + 真实模型。

### 5.5 现有插件盘点与冲突检查

当前 `pi list` 里的插件需要逐项确认职责，避免与 Loom 的状态/通信/压缩层重叠。下表是**待核实清单**，不是结论：

| 插件 | 可能职责 | 与 Loom 的潜在冲突 | 建议 |
|---|---|---|---|
| `pi-cache-optimizer` | 缓存优化/前缀稳定 | 若自动改写前缀，会破坏缓存 | 保留但监控 cache hit；禁止中途重写 system/tools |
| `context-mode` | 上下文管理/自动压缩 | 可能与“状态外置/不静默摘要”冲突 | orchestrator 关闭自动压缩；worker 超限即停 |
| `pi-hermes-memory` | 长期记忆 | 可能与 `.agent/` 状态重复/污染 | 只作辅助；事实源仍是 `.agent/` |
| `pi-web-access` | 网页抓取 | HTML/大结果直接进上下文 | 强制“正文提取 + 文件引用 + 截断” |
| `@ff-labs/pi-fff` | 搜索/文件定位 | 结果过大、重复读 | 用于批量定位；按 range 读；结果按引用 |
| `pi-intercom` | Agent 间通信 | **与 hub-and-spoke 冲突**，易产生网状通信 | 只允许 orchestrator↔worker；禁止 worker 互聊 |
| `ponytail` | 未知（需核实） | 不确定 | 盘点后再决定是否保留 |
| `@tintinweb/pi-subagents` | 子 agent 派发 | 可能默认注入父上下文 | 配置为“只传 TaskSpec”；关闭 transcript 继承（若可） |
| `@narumitw/pi-goal` | 续跑 | 可能无限续跑 | 设 `automaticTurns` / `noProgressTurns` |
| `@casualjim/pi-headroom` | agent 端压缩 | 与 proxy/guardrail 叠加 | 一条链路只保留一个压缩归属 |

**原则**：插件可以保留，但必须明确“谁是事实源、谁是压缩者、谁负责通信”。职责重叠时，优先关掉自动行为，改用 Loom 的显式协议。

### 5.6 模型能力位

模型名和价格变化很快，配置里写死模型名会迅速腐烂。选型时记录能力位，别名只映射到“真实模型 + 能力位”：

| 能力位 | 取值 | 选型影响 |
|---|---|---|
| `tier` | `frontier` / `worker` | 任务类别 → 层级路由；先从两档开始 |
| `thinking_mode` | `off` / `optional` / `forced` | 强制 thinking 的模型不进多轮工具链，除非能完整透传 |
| `reasoning_passthrough` | `required` / `optional` | 需要原样回传 `reasoning_content`，压缩器不能改写 |
| `wire` | `chat` / `responses` / `messages` | 决定 Headroom / LiteLLM / 计费是否覆盖 |
| `context_window` / `max_output` | 数值 | 切片、预算、是否适合长上下文扫描 |
| `input_usd_per_mtok` / `cached_input_usd_per_mtok` / `cache_write_usd_per_mtok` / `output_usd_per_mtok` | 数值 | 真实成本主项；缓存价单独记，比列表单价更重要 |
| `tool_calls` / `streaming` | 布尔 | adapter 能力声明；缺失按未知处理 |
| `model_id` | 固定版本或日期 | 变更/下线后必须重跑验收基线 |
| `pricing_version` | 字符串 | 价格来源版本；对账时可追溯到哪张价目表 |

配置示例（价格仅作占位，落地前以 provider 账单为准）：

```toml
[[models]]
alias = "worker"
provider = "deepseek"
model_id = "deepseek-v4-flash"
tier = "worker"
wire = "chat"
base_url = "https://api.deepseek.com/v1"   # 可选：按模型切 endpoint
key_env = "DEEPSEEK_API_KEY"      # 可选：按模型选 key 的环境变量名（值不进配置）
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

检查命令：

```bash
loom capability list
loom capability show worker
loom capability check
```

价格未知时保持留空，不要为了账本“好看”而猜价格；账本会记 `cost_source=unknown`，`loom ledger` 的 `cpvt` 返回 `n/a`。`loom capability check` 会把“强制 thinking 但没有 reasoning 透传”判为错误，把缺价格判为警告。

选型规则：

- 执行层优先选**能关 thinking、协议干净、可无状态一次调用**的模型；
- 先 **1 主力 + 1 fallback**，按 task 类型 A/B 后再扩，不一次接四家；
- 长上下文模型只做“全仓扫描 → 结构化候选”，不直接定稿 spec；
- 账本同时记 alias、真实模型和能力位，避免用别名对账。

## 6. Aider 评估

### 6.1 能否直接 compose

**能，但只能顶半边天。** Aider 底层走 LiteLLM，可通过兼容端点指向 Headroom/LiteLLM：

```bash
export OPENAI_API_BASE=http://127.0.0.1:8787/v1
export OPENAI_API_KEY=dummy
aider --model openai/frontier \
      --architect \
      --editor-model openai/worker \
      --weak-model openai/worker
```

也可以用 `headroom wrap aider`，但生命周期绑定，不适合常驻 proxy 场景；更推荐常驻 Headroom + 环境变量。

### 6.2 能解决什么

- `--architect` / `--editor-model`：贵模型出方案、便宜模型出 diff；
- 快速 diff 生成、单文件/小范围重构；
- 作为 A/B 对照：测“便宜模型能不能写对 diff”。

### 6.3 硬边界

| 限制 | 后果 | 缓解 |
|---|---|---|
| 没有 `headroom_retrieve` | 压掉的原文取不回 | 走 `mode=cache`；失败日志写文件 |
| 没有子 agent 上下文隔离 | architect/editor 共享 transcript，历史重发仍在 | 只用于短任务；不用于长跑 |
| 没有 goal 语义 | 没有“续跑到可验证完成” | 用外部 runner 包一层 |
| `--auto-test` 只是失败再修一轮 | 没有 no-progress 治理 | 外部预算/熔断 |
| 会话内费用读数不是账单 | 成本误判 | 用 LiteLLM/provider 账本 |
| 两个槽位协议前缀必须一致 | 混 `anthropic/` + `openai/` 可能被拒 | 统一走 OpenAI 兼容 alias |
| 需要模型元数据 | 报 `Unknown context window size and costs` | 配 `.aider.model.metadata.json` |

### 6.4 结论

- **不作为主路径**；
- 作为 **A/B 旁路** 和 **diff 生成器**；
- 如果要用，必须外挂 runner（读切片 → `aider --message` → verify → 升级 → 写状态），约 100 行脚本；
- 不指望它解决 token 面积问题。

## 7. 备选与退出策略

### 7.1 如果 pi 生态不稳定

- 切 Codex 作为主 runtime；runner 通过 Adapter 不变。
- 或切 Claude Agent SDK / Claude Code + LiteLLM；协议不变。
- 代价：重写 Adapter，不重写编排/状态/账本。

### 7.2 如果 Headroom 故障

- 客户端临时直连 provider；
- 保留 `DIRECT_*` 环境变量/配置；
- 告警 cache hit 失效；
- 恢复后切回。

### 7.3 如果 LiteLLM 不兼容

- 按客户端降级：pi 走 LiteLLM，Codex 直连；
- 或只用 LiteLLM 做 worker 路由，frontier 直连；
- 最坏回退单模型，保留上下文治理收益。

### 7.4 如果 worker 模型质量不达标

- 回到全 frontier；
- 保留切片/回执/verify/账本（这些收益与模型无关）；
- 用 CPVT 证明是否值得再试。

### 7.5 如果 Aider 旁路无收益

- 直接删除；不影响主路径。

## 8. 版本锁定与升级

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

> 前期 API key 可覆盖 GPT 与多家国产模型：`worker` 先用 DeepSeek，Kimi/MiniMax/MiMo 等按 A/B 替换或作为 fallback，并重跑基线。

### 8.2 升级流程

1. 记录当前版本 + 基线。
2. 读 changelog；标出 breaking changes。
3. 跑兼容性清单（07）。
4. 小任务 canary（1–2 个）。
5. 观察 1 天：CPVT、cache hit、错误率、tool call 成功率。
6. 全量；保留回滚包。

### 8.3 禁止

- 生产 run 中途升级 runtime/gateway。
- 同时升级多个关键组件。
- 无基线、无 canary 直接升级。
- 升级后不更新 `env.lock`。

## 9. 常见问题（直接回答）

### Q1 LiteLLM 应该放在 agent 和 Headroom 中间吗？

不。顺序是 `agent → Headroom → LiteLLM → provider`。Headroom 是透明压缩/缓存层；LiteLLM 在其后做 alias 路由。反过来的话，每个 provider 都要配一套 Headroom，且 LiteLLM 的重试/fallback 会绕过压缩层。

### Q2 多个 Anthropic 格式模型，怎么按 model 切 token/cache？

**不要切，也切不了。** `mode` 是进程启动配置，不是 per-request；Headroom 在 LiteLLM 前面也看不到最终模型。统一 `cache`，把槽位名映射交给 LiteLLM。若未来确实要两种模式，只能按客户端/端口在进程边界分实例。

### Q3 现在只用单 GPT，能先降成本吗？

能，而且应该先做。S 阶段不接多模型也能拿到大部分收益：状态外置、切片、四行回执、按引用交付、verify。多模型是 P2 的放大器，不是止血药。

### Q4 Aider 能不能配合 LiteLLM + Headroom 直接用？

能 compose，但只适合旁路/A-B/diff 生成。它没有 `retrieve`、没有子 agent 上下文隔离、没有 goal 语义，共享 transcript；不要拿它做长跑主路径。

### Q5 要不要自研完整 harness？

不要。自研薄编排层（状态协议、任务图、verify、预算、账本）即可；Runtime、压缩、路由、沙箱都用现成的。完整 harness 是重活且无差异化。

### Q6 便宜模型为什么可能更贵？

因为计费形状是 `turns × context`。便宜模型如果多试几轮、多读几次文件、返工，`cost per successful task` 可能高于 frontier 一次通过。必须用 CPVT + 质量 guardrail 验证，不能只看单价。

## 10. 选型结论

| 问题 | 结论 |
|---|---|
| 要不要自研完整 harness？ | **不要**；自研薄编排层（1–2k 行） |
| 主 runtime 用什么？ | **pi**；Codex 作为可选 worker/回退 |
| 压缩用什么？ | **Headroom**，保持 `cache`；不调参 |
| 路由用什么？ | **LiteLLM**，放在 Headroom 之后；P2 再上 |
| Aider 能不能用？ | **能 compose，但只做旁路/A-B**；不做主路径 |
| 多模型怎么分？ | **两档 + 显式 tier**；不做 LLM 自评路由 |
| 状态放哪？ | **`.agent/` 文件**；聊天不是状态 |
| 成本看哪？ | **provider/LiteLLM**；Headroom stats 只诊断 |
| 核心 KPI？ | **CPVT**（成本）+ **TPVT**（token） |

**一句话**：把“换模型”当成二阶优化，把“上下文生命周期 + 工具结果 + 验收协议”当成一阶优化。前者是省钱，后者是止血。
