# 15 Agent 运行时治理与工程化

> 状态：**设计稿，未实现**（2026-11-07）。
> 范围：四个已观察问题——写入抖动、提问阻塞与人工事项可见性、skills 治理、agent/harness 驱动的本地 CI。
> 前置阅读：[08-设计路线-v0.2.md](08-设计路线-v0.2.md)（状态所有权 / adapter 契约）、[03-核心协议.md](03-核心协议.md)（TaskSpec / 回执 / 事件）、[10-工具输出协议-v0.1.md](10-工具输出协议-v0.1.md)（工具与 telemetry）、[14-当前问题与卡点.md](14-当前问题与卡点.md)（当前环境限制）。

---

## 0. 结论

这四个问题都不是“换更强的模型”能根治的，而是控制面缺少**行为协议**：

| 问题 | 本质 | 控制面要补的东西 |
|---|---|---|
| 反复删除重写 | 重写本身不贵；贵在全文进入历史后每一步重发，且模型用整文件输出代替 patch | 步骤级上下文重编译、patch-first、token 归因、成本感知止损 |
| 提问即暂停 | 提问被当成失败或交互等待，而不是一次可恢复的返回；真正需要人做的事项又淹没在后续对话里 | `questions.json` + `needs_input` 状态 + 回答/假设协议 + 常驻 attention inbox |
| skills 泛滥 | skill 是上下文成本与供应链风险，不是“装了就有用” | 仓库画像、离线目录、按需加载、验证后晋升 |
| 本地 CI 缺位 | 本地 CI 不是验收的附属品；它是 agent 和人都能调用的仓库级检查与提交前闸门 | 技术栈探测、`.loom.ci.toml`、`loom ci`、hooks、review/attestation、skills 化检查 |

共同主线：**把 agent 的行为变成可观测、可中断、可恢复的结构化产物**；不引入长会话、不依赖交互式等待、不让 LLM 做本可确定性判断的事。

另一条主线：**需要人处理的事项必须有独立于聊天历史的常驻出口**——既包括提问，也包括审批、操作、复核和提醒；不能只靠“往上翻消息”。

建议实施顺序：

1. **本地 CI 核心**：先让“验证”可信，后面三个方向才有度量基础；先做探测、`.loom.ci.toml`、顺序执行与 doctor，不急着上 hooks/attestation。
2. **question protocol + attention inbox**：当前 `needs_input` 会被 verify 记成 `task.failed`，属于正确性缺陷；同时把“人需要处理的事项”从聊天历史里拿出来，做成常驻可见的 inbox。
3. **churn 治理**：先做 native 的步骤级上下文重编译 + patch-first + token 归因；拦截写入只是最后一道闸。外部 adapter 只能做粗粒度 watchdog 和选型。
4. **本地 CI 前移**：pre-commit/pre-push、确定性自动修复、review job、attestation，把能本地解决的检查挡在提交/推送前，减少服务器 CI 的重复劳动。
5. **skills 治理**：扫描/加载/静态检查可以先做；CI job 模板与 review 规则可以作为 skill 的早期落点；**晋升评估依赖真实模型与固定任务集**，当前仍受 [14](14-当前问题与卡点.md) 的环境卡点限制。

明确不做：全局安装 skill、联网自动装 skill、embedding/向量检索、交互式等待、为每个问题调用一次 LLM、v1 嵌套容器。

---

## 1. 问题一：反复删除重写（write churn）

### 1.1 先算 token 账

churn 的浪费不在磁盘，而在**请求历史**。一次“删除后重写”通常同时发生三笔费用：

| 费用 | 来源 | 说明 |
|---|---|---|
| 输出 token | 模型重新生成整个文件内容，放进 `tool_calls.arguments` | 8KB 文件约 2k tokens/次 |
| 输入 token 复利 | assistant 消息里带着完整内容，native 每步把 `messages` 全量重发 | 旧版本会在后续**每一步**继续计费 |
| 额外步数 / 重试 | churn 拉长 run；acceptance 失败回灌；严重时整 run 重开 | 每一步又重发一遍历史 |

一个直观例子：同一个 8KB 文件重写 5 次，假设每次之后还剩 20、16、12、8、4 步：

| 项目 | 无治理 | 有步骤级投影 + patch |
|---|---:|---:|
| 生成 5 次全文 | 约 10k output tokens | 约 0.5k（每次只发 patch） |
| 旧版本在后续请求中被重发 | 约 2k × (20+16+12+8+4) ≈ **120k input tokens** | 旧版本被投影掉，只保留当前文件快照，约 2k/步 |
| 第 3 次重写被硬拦 | 省约 2k output，但前两次的全文仍在历史里继续计费 | 先投影历史，再决定是否拦 |

结论：**阻止第 N 次文件写入，只省一次输出；真正的大头是让旧内容不再进入后续请求。** 所以第一优先级是“步骤级上下文重编译 + patch-first”，不是拦截文件操作。硬拦只适合作为最后一道闸，而且可能诱发模型换一种写法重试，反而增加 token。

### 1.2 现状（为什么现在会复利）

| 现状 | 后果 |
|---|---|
| native 的 `messages` 只增不减，每步 `provider.complete(messages.clone())` | 历史越长，每一步的 input 越贵；重写越多，复利越重 |
| assistant 的 `tool_calls.arguments` 里保存完整文件内容 | 每次 `write` 的全文都会留在历史里，后续每一步重发 |
| `write` 不要求先 `read`，也不限制整文件覆盖 | 模型可以随时用全文重写替代 patch |
| `edit` 匹配失败时只返回错误 | 模型容易放弃 edit，转成整文件重写 |
| 只有 run 级 usage，没有 step 级归因 | 无法回答“这次重写多花了多少 token” |
| no-progress 只看 `mutated` | 重写也算“有进展”，不会触发任何收敛 |

相关实现：`src/crates/loom-native/src/run.rs`（`messages.push` / `provider.complete`）、`src/crates/loom-native/src/tools.rs`（`write` / `edit`）、`src/crates/loom-store/src/telemetry.rs`。

### 1.3 节省 token 的四个杠杆（按 ROI 排序）

#### A. 步骤级上下文重编译（最大杠杆）

把 native 从“长会话”改成“每步重新编译有界上下文”，这也符合 [08](08-设计路线-v0.2.md) 原本的 Context Compiler 原则：

```text
[稳定前缀] system + task + AC + 当前 skill 索引
[工作区状态] 本 run 改过的文件：path / sha / 行数 / 有界当前内容或预览
[进度账本] step / tool / path / result / verify 状态（一到两行一条）
[最近交互] 最近 1–2 条原始 tool call / result
```

规则：

- 旧的 `write` / `edit` 参数替换成一行引用：`[elided write path=src/x.rs sha=ab12 lines=240]`；
- 旧的 `read` / `search` 结果替换成 `[elided read path=... range=... sha=... ref=...]`；
- **当前版本**保留一份有界快照（单文件 ≤ 8KB；更大的文件只留 sha + 关键区间 + “需要时 read”）；
- 稳定前缀保持不动，优先保住 provider 前缀缓存；只重编译动态尾部；
- 每步输入预算建议与 worker 一致（24k），超了先裁旧交互与快照，不裁 goal / AC / constraints。

这样每步成本从 `O(所有历史)` 变成 `O(稳定前缀 + 当前状态 + 最近交互)`。旧重写不再复利，这是省 token 的主力。

> 代价：重编译会让“动态尾部”的前缀缓存失效；需要在真实 provider 上 A/B，比较“少发的 input”与“失去的缓存折扣”。稳定前缀不变，缓存仍可命中一部分。

#### B. Patch-first 工具（直接减少 output 与历史体积）

- 新增 `patch` 工具（unified diff / 小 hunk），或对已存在文件强制走 `edit`；
- `write` 只用于新文件、小文件，或显式 `mode = "replace"`；
- `write` 单次内容设上限（例如 16KB），超过要求 patch 或分块；
- 提升 `edit` 的匹配鲁棒性（CRLF、首尾空白、上下文锚点），并在失败时返回“请 read 精确区间”，**不要**提示“重写整个文件”；
- acceptance 失败包加一句：“用 edit/patch 做最小改动；不要重新生成整个文件。”

patch 的收益是双重的：output 变少，进入历史的 payload 也变小。

#### C. Token 感知的止损 / 重启

- 按 step 记录 usage，并把“包含整文件重写的 step”单独归因；
- 估算继续成本 ≈ `剩余步数 × 当前上下文 token`，重启成本 ≈ `新 run 的稳定前缀 + 已知状态 + 重做步数`；
- 只有 `继续成本 > 重启成本 + 余量` 时才停；默认最多重启 1 次，避免重启抖动；
- 重启时用**新编译的短上下文** + 一段 churn 摘要，而不是把旧 run 的完整历史带过去。

这条是“止损”，不是主要节省手段；它解决的是已经进入坏轨迹、继续跑明显更贵的情况。

#### D. Prompt / 路由（降低发生概率）

- prompt 明确：最小 diff、先 read 后 edit、不要凭记忆重建文件；
- 失败包明确：不要重复同一失败改动，不要整文件重写；
- `loom churn stats --by model` 产出按模型的 token 浪费，edit 密集型任务避开高 churn 模型，或改用更强 tier；
- 这些是辅助手段，不能替代 A/B。

### 1.4 指标：token 优先，文件指标只做诊断

`churn.json` 里同时记录两类指标：

| 指标 | 类型 | 定义 |
|---|---|---|
| `rewrite_output_tokens` | 实际 | 含整文件重写的 step 的 output tokens；一个 step 多个调用时按 payload 占比分摊，标 `shared` |
| `history_carry_tokens` | 估算 | 旧 payload 若不被投影，会在后续 N 步被重发的 token 数；投影后实际 carry 接近 0 |
| `avoided_input_tokens` | 估算 | 步骤级投影实际省下的 input tokens |
| `retry_tokens` | 实际 | 因 churn 触发的 acceptance 回灌 / 重启额外消耗 |
| `wasted_tokens_est` | 估算 | `rewrite_output_tokens + history_carry_tokens + retry_tokens` |
| `wasted_cost_usd_est` | 估算 | 按模型价格表计算；价格未知标 `unknown`，不要猜 |
| `write_passes` / `churn_ratio` / `oscillations` | 诊断 | 用于解释“为什么贵”，不作为 KPI |

- 估算统一用 `loom-core::text::estimate_tokens`，并在 `churn.json` 标 `estimated: true`；
- `loom churn stats` 的第一屏是 token / 成本，第二屏才是文件操作指标；
- 这样才能回答“DeepSeek v4.1 flash 到底多花了多少 token”，而不是只看到“重写了 3 次”。

### 1.5 干预阶梯（重排）

| 级别 | 触发 | 动作 | 省 token 的位置 |
|---|---|---|---|
| L0 基线 | 始终 | 步骤级上下文重编译 + 当前文件快照 + patch-first | 防止旧 payload 复利；减少 output |
| L1 投影 | 同文件第 2 次整文件重写，且 verify 无改善 | 立即投影旧 payload；只保留当前版本；注入一行状态和“用 edit/patch” | 后续每一步都少发旧全文 |
| L2 约束 | 第 3 次整文件重写 | 拒绝无 `mode=replace` 的整文件 `write`；错误信息很短，要求 patch/edit | 阻止新的全文进入历史 |
| L3 止损 | churn token 超预算阈值且连续无 verify 改善 | 比较继续/重启成本；必要时停 run，用新编译上下文重启 | 截断剩余的复利 |

明确：**L2 不是主手段**。它的价值是“防止下一次全文进入历史”，而不是“省下这一次写入”。如果 L2 让模型反复重试，应该回退到 L1/L3。

### 1.6 外部 adapter

外部 agent 拥有自己的会话历史，Loom 无法逐步投影。能做的只有：

- 在 adapter prompt / 配置里要求 patch-first、最小 diff、上下文上限（如果 adapter 支持）；
- watchdog 只在 gross loop 且“继续成本明显 > 重启成本”时终止；外部 agent 重启可能重新读一遍仓库，收益不确定，默认保守；
- 事后用 `churn.json` 和 `loom churn stats` 做模型/agent 选择；
- 真正要省 token，优先用支持短上下文/可重编译的 adapter（native 或被包装过的外部 agent）。

### 1.7 可行性

| 部分 | 可行性 | 说明 |
|---|---|---|
| 步骤级上下文重编译 | **高** | native 自己拥有 messages；2–4 天可出原型；需 A/B 质量与缓存 |
| patch 工具 / 强制 edit | **高** | 工具层改动明确；2–3 天含测试 |
| token 归因与 churn.json | **高** | 已有 per-call usage，补 step 维度即可 |
| 成本感知重启 | **中** | 需要可靠的剩余成本估算；容易重启抖动，默认最多 1 次 |
| 外部 adapter 省 token | **低–中** | 历史不在 Loom 手里；主要靠 prompt 和选型 |
| 模型差异结论 | **低（现在）** | 缺真实 key / 账单 / 固定任务集；先积累 token 指标 |

### 1.8 验收（未来实现时）

- [ ] 脚本化 provider 对同一 8KB 文件连续重写 5 次；无投影时 input 随步数复利增长，有投影时有界。
- [ ] `churn.json` 输出 `rewrite_output_tokens`、`history_carry_tokens`、`avoided_input_tokens`、`wasted_tokens_est`，并标 `estimated`。
- [ ] 第 2 次重写后旧 payload 被投影；第 3 次无 `mode=replace` 的 `write` 被短错误拒绝；L3 触发后重启只带 churn 摘要。
- [ ] 同一改动用 patch 与用整文件 write 的 output token 有可量化差距。
- [ ] 投影/重启后 acceptance 仍能通过，且不会无限重启。

---

## 2. 问题二：提问导致会话暂停，人工事项又被历史淹没

### 2.1 现状（这是一个正确性缺陷 + 一个可见性缺陷）

当前链路：

1. adapter 退出码 `2`，回执状态 `needs_input`；
2. `loom verify` 只把 `ReceiptStatus::Done` 当作 `claim_done`；
3. 于是 `needs_input` / `blocked` 都会走到 `task.failed`（`code = claim_not_done`）；
4. M3 升级只数 `task.failed` 事件 → **agent 提一个问题，会被记成一次失败，甚至触发 worker→frontier 升级**；
5. `dispatch` 只接受 `pending/ready/failed/escalated`，没有“已回答、继续”的状态；
6. 外部 agent 若真在等 stdin，会挂到超时，烧墙钟。

第二个问题是**可见性**：即使提问被正确记成 `needs_input`，它仍然只是一条事件或一份 `questions.json`；用户离开当前会话、或者继续问别的问题后，提醒就沉到历史里了。而且真正需要人介入的并不只有“问题”：

| 需要人做的事项 | 例子 | 当前问题 |
|---|---|---|
| 回答 | “迁移是否兼容旧 schema？” | `questions.json` 只覆盖 agent 主动提问，且没有常驻入口 |
| 审批 | 改 AC、跨 tier 升级、提高预算、不可逆迁移、合并/推送 | 散落在失败包、回执、日志里，没有统一待办 |
| 操作 | 安装 `goose`、提供 API key、启动服务、跑一次迁移 | `doctor` 只在被调用时输出，用户不看就不知道 |
| 复核 | 高风险 diff、红基线、skill 晋升、review 发现 | 没有“待我确认”的队列 |
| 提醒 | 任务等待回答超时、预算接近上限、锁即将过期 | 只能靠轮询或记忆 |

所以问题不只是“提问即暂停”，而是**控制面缺少一个独立于聊天历史的人工事项出口**。这个出口必须同时承载问题、审批、操作、复核和提醒，并且在工作区的某个固定位置一直可见。

相关实现：`src/crates/loom-cli/src/verify_cmd.rs`（`claim_done` / `TaskFailed`）、`src/crates/loom-adapter/src/mock.rs`（`NeedsInput`）、`src/crates/loom-core/src/task.rs`（没有 `NeedsInput` 状态）、`src/crates/loom-cli/src/doctor_cmd.rs`（只在显式调用时输出）。

### 2.2 人工事项收件箱（attention inbox）

目标：任何时候都能用一条命令回答“现在有什么需要我处理”，并且在 harness 的一个固定角落持续显示，而不是靠用户回翻对话。事项**不只包括问题**，还包括审批、操作、复核和提醒。

它不是聊天工具，也不是工单系统：一个事项是“结构化待办 + 证据引用 + 可执行动作”，没有消息线程，不承担讨论。

#### 事项类型

| `kind` | 含义 | 典型来源 | 关闭条件 |
|---|---|---|---|
| `question` | agent 明确提问，等人或上游回答 | `questions.json` | `task.answered` 或显式回答 |
| `approval` | 需要明确批准/拒绝 | 改 AC、跨 tier 升级、提高预算、不可逆迁移、合并/推送 | 显式批准/拒绝 |
| `operation` | 人必须执行一个动作 | 缺工具、缺 API key、服务未启动、需要跑迁移 | 探测器通过或显式完成 |
| `review` | 需要人的判断 | 高风险 diff、红基线、skill 晋升、review 发现 | 显式复核结论 |
| `reminder` | 时间/阈值提醒 | 回答超时、预算接近上限、锁即将过期 | 条件解除或时间过去 |

`severity` 只有三档：

- `blocking`：相关工作已经停住，不处理就无法继续（例如 `needs_input`、缺关键工具）；
- `soon`：还能继续，但应在下一次提交/派发前处理（例如非阻塞问题、红基线、review 建议）；
- `info`：只进完整列表，不进固定角落，不计数为待办。

#### 数据模型

`.loom/inbox.json` 是**视图**，由事件重放生成；事实源仍是 `events.jsonl`。不要手改。

```json
{
  "v": 1,
  "generated_at": "2026-11-07T12:00:00Z",
  "counts": {"blocking": 1, "soon": 2, "info": 3},
  "items": [
    {
      "id": "ATT-20261107-003",
      "kind": "operation",
      "source": "env",
      "severity": "blocking",
      "status": "open",
      "title": "容器内缺少 goose，无法跑 adapter canary",
      "detail": "PATH 中未找到 goose；先补镜像或挂载二进制",
      "refs": ["doc/14-当前问题与卡点.md#2", "artifact:adapter-canary-3"],
      "task_id": null,
      "run_id": null,
      "dedupe_key": "env:missing-tool:goose",
      "occurrences": 2,
      "created_at": "2026-11-07T09:00:00Z",
      "last_seen_at": "2026-11-07T12:00:00Z",
      "defer_until": null,
      "expires_at": null,
      "actions": [
        {"id": "show", "label": "查看安装方式", "cmd": "loom doctor --explain goose"},
        {"id": "ack", "label": "我看到了", "cmd": "loom inbox ack ATT-20261107-003"}
      ]
    }
  ]
}
```

约束：

- `title` ≤ 120 字符，`detail` ≤ 2KB；`refs` 只放路径/事件/artifact 引用，不放全文、不放日志；
- `source`：`task | ci | skills | env | git | human`；
- `dedupe_key` 在未关闭事项中唯一；同一问题重复出现只增加 `occurrences` 和 `last_seen_at`，不刷屏；
- `actions[].cmd` 是**建议命令**，只作为文本展示，永不自动执行，也不做 shell 求值；
- 事项里禁止出现 secret、token、完整 provider 响应；
- 已解决事项在视图中保留 7 天便于回看，事件永久保留。

#### 生命周期与事件

| 事件 | `data` | 效果 |
|---|---|---|
| `attention.opened` | `item_id`, `kind`, `source`, `severity`, `dedupe_key`, `title`, `refs`, `task_id?`, `run_id?` | 新建事项；同 `dedupe_key` 未关闭时只累加 `occurrences` |
| `attention.updated` | `item_id`, `changes`（`severity` / `detail` / `defer_until` / `occurrences`） | 更新事项，不改变身份 |
| `attention.resolved` | `item_id`, `by`, `reason`, `assumption_id?` | 关闭事项 |
| `attention.expired` | `item_id`, `policy` | 过期关闭；只允许可逆/提醒类，不可逆的 `blocking` 不允许静默过期 |

状态：`open → acked → resolved`，旁路 `deferred` / `expired`。`acked` 只表示“已看到”，**不关闭事项、不降低 `blocking` 计数**；`defer` 只允许 `soon`/`info`，避免阻塞事项被“稍后处理”弄丢。

任务事件与事项的联动：

| 触发 | 事项 |
|---|---|
| `task.needs_input` | 每个 blocking 问题开一个 `question`；非阻塞问题开 `soon` |
| `task.answered` | 自动 resolve 对应 `question` |
| `task.blocked` | 开 `operation` 或 `approval`，带 `reason` |
| `verify` 发现红基线 / CI 失败 | 开 `review`（是否接受基线/如何修），附日志 artifact |
| `loom doctor` 缺工具/缺 key | 开 `operation` |
| 路由成本护栏拒绝派发 | 开 `approval`（提高预算、换 tier、缩范围） |
| skill 达到晋升条件 | 开 `review` |
| 同一任务连续 `task.failed` | 开 `review`（升级、改 AC、拆任务） |

控制面在写任务事件时同步写 `attention.*`。两者不是重复状态：任务事件决定任务状态，`attention.*` 决定“人需要看到什么”。

#### 固定角落在哪里

`.loom/inbox.json` 是给所有前端（CLI、TUI、桌面端、编辑器插件）读的稳定视图；“角落”不绑定某个 UI。v1 先把 CLI footer 与 statusline 做出来，后续界面复用同一视图，不另建状态。

| 出口 | 内容 | 约束 |
|---|---|---|
| 每条人类命令的 footer | `[loom] 1 阻塞 · 2 待办 → loom inbox` | 写 **stderr**；仅 TTY；不污染 `--json` 的 stdout；非 TTY/CI 自动关闭 |
| `loom status` | 计数 + 前 3 条标题 | 与 footer 共用同一视图 |
| `loom inbox` | 完整列表，按 `blocking → soon → info`、再按时间排序 | 支持 `--json`、`--kind`、`--task`、`--all` |
| `loom inbox --line` | 单行，供 shell prompt / tmux / starship / statusline 轮询 | 只读、无副作用、开销固定；`[!] 1 blocking · 2 pending` |
| `loom inbox --check` | 有 `blocking` 时退出码 `2` | 给脚本/钩子用；不打印大段内容 |
| TUI / 桌面端（后续） | 同一视图的可点击列表 | 不另建状态 |

补充规则：

- `--no-inbox` 或 `LOOM_NO_INBOX=1` 关闭 footer；`loom inbox` 自己不再打印 footer；
- footer 只显示一行，取最高优先级事项的标题并截断，禁止 ANSI 注入；
- **默认不把 inbox 注入模型上下文**：这是给人看的通道。若确需让 agent 知道“有人在等你”，只注入计数（`[attention] include_in_packet = "counts"`），并受上下文预算约束；
- 固定角落不是模态弹窗，不阻塞命令执行；`blocking` 只影响 `--check` 和任务派发策略，不强制打断用户；
- 优先级：`blocking` > `soon` > 最近更新；同一任务的事项聚合展示，但保留独立 `id`。

配置示例：

```toml
[attention]
footer = "tty"              # tty | always | off
include_in_packet = "off"   # off | counts
dedupe_cooldown_s = 600
retain_resolved_days = 7
```

#### 降噪

- 去重：`dedupe_key`（如 `task:T-...:question:Q1`、`env:missing-tool:goose`、`ci:clippy:<tree>`）；
- 聚合：同一任务的多个 `soon` 事项在 footer 只算一条，在完整列表中展开；
- 冷却：同一 `dedupe_key` 的重复提醒默认 10 分钟内不重复计数；
- 过期：带默认值的可逆问题可过期并采用默认值（写 assumption）；不可逆/安全/预算问题不自动过期；
- 关闭：条件解除时自动 resolve；无法自动判断时必须显式 `loom inbox resolve <id> --reason ...`；
- 不做“已读即消失”：`acked` 仍保留在列表中，避免用户点一下就忘。

#### 父子 agent 与多任务

- 子 agent 的问题仍以 `questions.json` + exit 2 返回；控制面按 `task_id` 开事项；
- 父 agent 能代答就代答并 resolve；不能代答就上浮，用户只看到一个 inbox，不需要逐个会话查看；
- 同一规范化问题在多个子任务出现时合并为一个事项，`refs` 列出所有 `task_id`；
- 父 agent 不等待、不 sleep；子任务保持 `needs_input`，父 agent 继续处理其他任务。

#### 安全

- 事项文本来自 agent/仓库，视为**不可信输入**：转义控制字符、限制长度、`refs` 不跟随 symlink 到仓库外；
- `actions[].cmd` 只展示，不自动执行；用户显式运行后才生效；
- 不把事项正文拼进 shell；`loom inbox resolve --note` 只写文件；
- 未来若做 TUI/桌面端，动作按钮也必须走同一命令层，不允许直接调用内部函数绕过校验。

### 2.3 原则

1. **提问是返回值，不是暂停。** 进程退出，状态落盘；回答后开新 run 继续，不保留会话。
2. **提问必须结构化。** 不接受“最后一段自然语言里有个问号”作为协议。
3. **分级授权。** 能在 spec/AC/约束内确定的，直接答；可逆的，用默认值 + 假设记录；不可逆/安全/范围/预算问题，必须人答。
4. **不把提问计入失败。** `needs_input` 是正常状态，不是 `failed`。
5. **批处理、去重、TTL。** 一次回答尽量覆盖多个问题；过期用默认值或明确阻塞。
6. **父子 agent 不互相等待。** 子 agent 把问题作为返回值交给父级；父级能答就答，不能答就上浮，同时继续独立任务。
7. **人工事项统一出口。** 问题、审批、操作、复核、提醒都进 attention inbox；不能只发一条聊天消息或只写一份日志。
8. **常驻可见但低噪。** footer/statusline 一直可见；去重、聚合、`ack`/`defer` 控制噪声；但 `blocking` 不因“已读”而消失，也不默认注入模型上下文。

### 2.4 契约

adapter 需要输入时：

- 写 `outbox/questions.json`；
- `final.md` 状态行写 `needs_input`；
- 退出码 `2`；
- 进程以 `stdin = /dev/null` 启动（已发生的交互式询问立即 EOF，而不是挂起）。

`questions.json` v1：

```json
{
  "v": 1,
  "task_id": "T-20261107-002",
  "run_id": "R-20261107-001",
  "questions": [
    {
      "id": "Q1",
      "question": "迁移脚本是否需要兼容旧版 schema？",
      "why_now": "迁移会改写 users 表，兼容分支影响回滚策略",
      "blocking": true,
      "scope": "design",
      "options": [
        {"id": "A", "label": "兼容旧版，双写一个版本", "consequence": "多 1 次迁移，回滚安全"},
        {"id": "B", "label": "只支持新版", "consequence": "改动小，旧客户端会失败"}
      ],
      "recommended": "A",
      "default": "A",
      "reversible": false,
      "evidence": ["src/db/migrate.rs:40-90", "artifacts/read-3"],
      "ttl_s": 86400
    }
  ]
}
```

字段约束：

- `scope`：`implementation | design | contract | security | budget | scope`；
- `blocking=false` 的问题可以带默认值继续，不必停；
- `evidence` 只放路径/artifact 引用，不放全文；
- 单任务最多 3 个 blocking 问题；超过则合并或拆任务。

### 2.5 状态与事件

建议新增：

| 事件 | data | 状态变化 |
|---|---|---|
| `task.needs_input` | `question_ids`, `blocking`, `scopes` | `receipt → needs_input` |
| `task.answered` | `question_ids`, `by`, `source`, `assumption_ids` | `needs_input → ready` |

新增 `TaskStatus::NeedsInput`。不要复用 `blocked`：`blocked` 表示依赖/锁/环境阻塞，`needs_input` 表示等人或等上游回答；两者混在一起会让统计和升级都失真。

写 `task.needs_input` / `task.answered` 时，控制面同步写 `attention.opened` / `attention.resolved`（§2.2），保证问题不仅改变任务状态，还进入常驻 inbox。

`verify` 的结果矩阵要一起改：

| 回执状态 | acceptance | 结果 |
|---|---|---|
| `done` | 全 pass | `task.passed` |
| `done` | 有 fail | `task.failed`（`verify_fail`） |
| `needs_input` | 任意 | `task.needs_input`，**不写 `task.failed`** |
| `blocked` | 任意 | `task.blocked`，**不写 `task.failed`** |
| `budget_exceeded` | 任意 | `budget.exceeded` + `task.failed`（或按策略 `blocked`） |
| `failed` | 任意 | `task.failed` |

这是本次设计里**最应该先修的现有行为**：提问/阻塞不能污染升级计数。

### 2.6 回答协议

```text
子 agent / worker 提问
        │  exit 2 + questions.json
        ▼
控制面：task.needs_input（不入 failed）
        │
        ├─ 能在 spec/AC/constraints 内确定 ──► 自动回答（引用证据）
        ├─ 可逆且策略允许 ────────────────► 采用默认值 + 记 assumption
        ├─ 需要设计判断 ──────────────────► judgment/frontier 有界回答
        └─ 不可逆/安全/范围/预算 ─────────► 人工 inbox
```

授权矩阵：

| `scope` | worker 自答 | 父/同级 judgment | 必须人工 |
|---|---|---|---|
| `implementation`，可逆 | 默认值/策略 | 可覆盖 | 否 |
| `design`，可逆 | 否 | 可以，须引用 spec | 否 |
| `contract` / AC 语义 | 否 | 只能提出候选 | 变更 AC/目标时必须 |
| `security` / 数据删除 / 不可逆迁移 | 否 | 否 | 是 |
| `budget` / 跨 tier / 扩大范围 | 否 | 否 | 是 |

回答落盘 `outbox/answers.json`（或控制面 `.loom/answers/<task>.json`）：

```json
{
  "v": 1,
  "task_id": "T-20261107-002",
  "answers": [
    {
      "question_id": "Q1",
      "choice": "A",
      "by": "human|judgment|policy|spec",
      "rationale": "保留回滚能力",
      "assumption_id": "A-001"
    }
  ]
}
```

下一次 `dispatch` / `resume`：

- 把 `questions.json` + `answers.json` 的**摘要**注入 packet；
- 新开 run，`parent_run_id` 指向提问时的 run；
- `attempt` 不变（提问不是失败），另记 `question_round`；
- 失败包只带未回答问题，不带已答问题。

`loom answer` 写 `answers.json` 的同时写 `task.answered` 与 `attention.resolved`，所以回答后对应 inbox 事项立即消失；`--choice` 也允许直接按 `recommended`/`default` 批量回答非阻塞问题。

### 2.7 假设账本

对“可逆 + 默认值继续”的问题，写 `.loom/assumptions.jsonl`：

```json
{"v":1,"id":"A-001","task_id":"T-...","run_id":"R-...","assumption":"保留旧版 schema 兼容","source":"policy","reversible":true,"verify_hint":"迁移后跑 A3 回滚测试"}
```

review / verify 可以检查假设是否被后续证据推翻。这样“先继续”不会变成“悄悄猜”。

### 2.8 父子 agent

- 子 agent 没有“提问通道”，只有返回值：`questions.json` + exit 2。
- 父 agent 的 `[questions]` 策略决定它能否代答：`allowed_scopes`、`max_auto_answers`、`max_question_tokens`。
- 父 agent **不 sleep、不等待**：子任务进入 `needs_input`，父 agent 继续处理不依赖它的任务；依赖它的任务保持 `pending`。
- 去重：按 `task_id + normalized_question` 哈希；同一问题在多个子任务出现时合并为一个 inbox 条目。
- 预算：回答问题的 judgment 调用单独记账，设 `max_cost_usd`，不允许用回答把任务预算吃穿。

### 2.9 外部 adapter 兜底

- 在 adapter prompt 里明确：“不要交互式提问；写 `outbox/questions.json` 并退出 2。”
- 启动参数统一 `stdin = null`。
- 若进程仍挂起：watchdog 在“无 token、无文件变化、无日志增长”持续 N 秒后终止，读取最后的 agent 消息，合成一个 `Q?`（`confidence=low`），标记 `needs_input`，同时开一个 `blocking` 的 attention 事项，而不是 `failed`。
- 若 adapter 退出 0 但内容明显是提问且没有 diff：不要自动判成 `needs_input`（容易误判）；让 verify 失败，并在 `next_hint` 里提示“若需要输入，请用 questions.json + exit 2”。只有 adapter 明确声明 `supports_questions` 时才启用启发式。

### 2.10 可行性

| 部分 | 可行性 | 说明 |
|---|---|---|
| 控制面状态/事件/回答 | **高** | 纯文件与事件，可先于真实模型完成 |
| attention inbox 视图 + footer/`--line` | **高** | 事件重放 + 只读视图；无模型、无网络依赖 |
| native `ask` 工具 | **高** | 写 questions.json + 返回 exit 2；脚本化 provider 可测 |
| 外部 adapter 遵守协议 | **中** | 取决于 agent 是否听话；需要 canary 与兜底 |
| 自动回答质量 | **中** | 需要明确授权边界；宁可上浮，不要乱答 |
| 父子多 agent | **中** | 当前无并发；顺序执行也能先定义协议，M4 再并行 |
| 把“操作/审批”自动闭环 | **低–中** | 探测型（缺工具）可自动关闭；需要判断的仍要人 |

结论：**先修 verify 的 `needs_input` 误判，同时做 `attention.*` + `loom inbox`，再做 `questions.json` 与 `loom answer`；外部 adapter 的合规性等二进制到位后加 canary。**

可见性可以独立于真实模型先做完：即使没有 agent 提问，`doctor`/`ci`/路由护栏也能立刻产生 `operation`/`approval`/`review` 事项。

---

## 3. 问题三：skills 治理

### 3.1 问题

大量 skill 没有验证；全局安装会：

- 用 name+description 常驻上下文，几十个就吃掉 25k–40k token；
- 引入未审查的脚本、网络访问、许可证与提示注入风险；
- 让模型在冲突的 skill 之间摇摆；
- 无法回答“这个 skill 到底让 CPVT 变好还是变坏”。

参考 [基于仓库特征推荐安装 Agent Skill](https://yb.tencent.com/s/DsY6aNRnbq6v) 的分层：**Scanner（零 LLM 扫描）→ Catalog（离线索引）→ Resolver（打分）→ Installer（项目级安装）→ Runtime（渐进披露）**。这套分层与 Loom 的“状态外置 + 按引用交付 + 上下文硬预算”一致，采纳；但做两处收紧：

1. **不做全局安装**，只装项目级并写 lock；
2. **v1 不做 embedding 检索、不做联网自动安装**，与 [09](09-代码检索与知识库.md) 的 R0/R1 边界一致。

skill 的第一个真实使用场景不是“装一堆能力”，而是本地 CI 的 review/检查模板（§4.9）：仓库画像决定推荐什么，lock 保证可复现，静态检查限制权限，L2 评估决定能否从 advisory 升为 gate。

### 3.2 仓库画像

`loom skill scan` 输出 `stack-profile.json`（确定性，无 LLM）：

```json
{
  "v": 1,
  "root": "/work",
  "git_head": "365bc01",
  "lang": ["rust"],
  "frameworks": [],
  "infra": ["podman", "make"],
  "tests": ["cargo-test"],
  "toolchain": {"rust": "1.99.1", "container": "podman"},
  "evidence": {
    "rust": ["Cargo.toml", "src/Cargo.toml"],
    "container": ["Containerfile", "scripts/dev.sh"],
    "make": ["Makefile"]
  },
  "hash": "sha256:..."
}
```

规则：

- 每个标签必须能指向证据文件；没有证据的标签不出现；
- 缓存 key = `git_head + 关键文件 mtime`；不因每次会话重扫；
- 扫描结果只读，不自动改仓库；
- 这是“画像”，不是“推荐安装”。

### 3.3 目录与元数据

本地 catalog（`skills.catalog.json`，离线缓存，可定期从受信源增量刷新）：

```json
{
  "v": 1,
  "generated_at": "2026-11-07T00:00:00Z",
  "skills": [
    {
      "id": "rust-minimal-diff",
      "version": "0.2.0",
      "source": "internal/skills",
      "commit": "abc123",
      "sha256": "...",
      "license": "MIT",
      "tags": ["rust", "edit"],
      "triggers": {"lang": ["rust"], "globs": ["**/*.rs"], "roles": ["implement"]},
      "tokens": 900,
      "requires": {"tools": ["read", "edit"], "network": false},
      "permissions": {"shell": false, "write": ["src/**"]},
      "eval": {"tasks": 40, "first_pass_delta": 0.06, "cpvt_delta": -0.11},
      "status": "core|project|experimental|blocked",
      "min_loom": "0.2"
    }
  ]
}
```

`status` 的含义：

| 状态 | 含义 | 默认加载 |
|---|---|---|
| `core` | 通过 L2 评估、广泛适用、体积小 | 可按 role 默认 |
| `project` | 项目特有，已评审 | 仅该项目 |
| `experimental` | 未验证或评估样本不足 | 显式 `--skill` 才加载 |
| `blocked` | 安全/许可证/冲突 | 永不加载 |

### 3.4 解析与打分

`loom skill recommend --task T-...`：

```text
score = tag_match * 3
      + evidence_match * 2
      + role_match * 2
      + trust * 2
      - token_cost * 0.02
      - conflict_penalty
      - already_loaded_penalty
```

约束：

- 最多输出 5 个建议，最多 2 个自动加载；
- 低于阈值只显示 `suggest`，不加载；
- 用户 `always_load` / `never_load` 覆盖；
- 输出必须带 `why`（命中哪个标签、哪个证据文件）；
- 不做 embedding；关键词/标签/glob 足够 v1。

### 3.5 安装与锁

- 只写项目级目录：`.loom/skills/<id>/`（或仓库内 `skills/`，二选一，建议 `.loom/skills` 避免污染代码搜索）；
- `skills.lock` 记录 `id/version/source/commit/sha256/license/installed_at`；
- `loom skill add/remove/update/verify`；
- 禁止全局安装；禁止自动执行安装脚本；
- 安装前做静态检查，失败则拒绝；
- 回滚 = 按 lock 恢复上一版本。

### 3.6 运行时加载

**渐进披露**：

1. packet 里只放 skill 索引：`id | 一行触发条件 | token 估算`；
2. 模型或控制面用 `loom skill show <id>` 读取正文；
3. 正文进上下文前过 Context Compiler 的硬预算：单 skill ≤ 4KB，总 skills ≤ 8KB；
4. 超过预算先裁 skill 正文，不裁 goal/AC/constraints；
5. skill 不能修改 TaskSpec/AC/预算；违反直接拒绝加载。

### 3.7 验证与晋升

| 级别 | 检查 | 能证明什么 |
|---|---|---|
| L0 静态 | frontmatter、schema、大小、链接、许可证白名单、脚本/网络权限、提示注入模式 | 结构合法、无明显风险 |
| L1 沙箱 | 在临时目录跑脚本，断网、清空 `HOME`、只读挂载 | 不会立刻破坏环境 |
| L2 评估 | 固定任务集 A/B：加载 vs 不加载，比较首过 verify、CPVT、TPVT、墙钟 | 是否真的有用 |
| L3 生产 | 真实任务 telemetry：加载率、任务结果、人工返工 | 长期是否值得保留 |

晋升规则：

- `experimental → project`：L1 通过 + 至少 10 个任务的人工评审；
- `project → core`：L2 通过（≥ 20 任务，首过率不降，CPVT 不升）+ 许可证/安全复核；
- L3 持续负收益 → 降级或 `blocked`；
- 任何脚本/网络权限的 skill 必须人工复核，不能自动晋升。

**预置策略**：v1 不预置领域 skill；核心 prompt 已在 `loom-context` 的版本化 prompt 库中。最多预置 2–3 个“元 skill”（如最小 diff、verify-first），且必须体积小、可度量、可关闭。

### 3.8 安全与供应链

- 许可证白名单（MIT/Apache-2.0/BSD；其他人工评审）；
- 校验 `sha256` + 固定 commit，禁止浮动分支；
- 默认无网络、无 shell；需要时显式声明权限；
- 提示注入扫描：外部链接、隐藏指令、`ignore previous`、读取 secret 的模式；
- skill 正文视为**不可信输入**，不能覆盖系统约束；
- 卸载/禁用要即时生效，不影响已产生的 run 记录。

### 3.9 观测

`loom skill stats`：

- 每个 skill：被推荐/加载/实际引用次数；
- 加载后的首过率、CPVT、TPVT、墙钟；
- 与不加载基线对比；
- 冲突：同一任务加载了互相矛盾的 skill 时报警。

### 3.10 可行性

| 部分 | 可行性 | 阻塞 |
|---|---|---|
| 仓库扫描 / profile | **高** | 无 |
| 本地 catalog / lock / resolver | **高** | 无 |
| 静态检查 / 权限模型 | **高** | 无 |
| L2 评估 / 晋升 | **低（现在）** | 缺真实模型、固定任务集、账单 |
| 联网目录同步 | **中** | 需要策展与信任源；v1 不做 |
| embedding 检索 | **不做** | 与 09 的边界冲突，收益未证明 |

结论：**先做“扫描 + 本地目录 + 按需加载 + 静态检查 + lock”，把评估与联网目录后置。**

---

## 4. 问题四：初始化与 agent/harness 驱动的本地 CI

### 4.1 重新定位

本地 CI **不只是验收 agent 的工作**。它首先是项目自己的仓库级检查面：把格式化、lint、类型检查、单元测试、代码审查、提交信息检查等能在本地完成的动作前移，让 agent 和人都能在 `git commit` / `git push` 之前解决，减少服务器 CI 的重复劳动。

**作用对象**：本节描述的是 Loom 管理的**目标项目**，不是 Loom 自身的源码仓库。`loom ci` 始终在 `--root` 指向的项目根目录运行，配置属于那个项目。Loom 仓库使用同一套机制属于 dogfood；`./scripts/dev.sh`、Rust workspace 只是 Loom 自身的一个 runner 示例，不是产品默认，也不应写进生成器的通用规则。

三个层次要分清：

| 层次 | 负责什么 | 权威性 |
|---|---|---|
| TaskSpec acceptance | 这个任务是否完成了约定目标 | `loom verify`，任务级权威 |
| 仓库本地 CI | 这个仓库当前是否健康、能否提交/推送 | `loom ci run`，本地闸门与反馈 |
| 服务器 CI | 跨平台、集成、发布、安全与合规 | 服务器，最终权威 |

目标不是“用本地替代服务器”，而是：

- 确定性的快速检查在本地跑，失败立刻反馈给 agent 或人；
- 提交/推送前解决格式、lint、review 能发现的问题；
- 本地产生可核验的报告/attestation，服务器只重跑必须由可信环境执行的 job；
- agent 通过同一套命令获得反馈，不需要每个项目重新猜“这里怎么检查”；
- skills 提供可复用、可验证的检查模板与 review 规则，而不是散落的脚本。

### 4.2 现状

`loom init` 只做：

- 建 `.loom/{tasks,runs,ledger,snapshots,locks}`；
- 写默认 `config.toml`、`goal.md`、`constraints.md`；
- 更新 `.gitignore`。

`loom doctor` 只检查 `.loom`、git、config、可写。它不探测技术栈、不检查工具链、不生成 acceptance、不提供本地 CI。结果是：

- 每个项目手写 acceptance；
- agent 不知道“这个仓库的正常检查是什么”；
- 环境缺 `pnpm`/`cargo`/`just` 时，直到 verify 才失败；
- 没有 baseline 概念，agent 可能把“本来就红的测试”当成自己改坏；
- 格式/lint/提交信息等检查只能在服务器跑，反馈慢、额度浪费；
- 代码审查依赖人记得做，agent 也没有结构化的 review 入口。

### 4.3 本地与服务器的分工

| 阶段 | 本地默认做 | 服务器仍应做 |
|---|---|---|
| pre-commit | format、lint（可自动修的 safe fix）、类型检查、提交信息、secret/调试残留扫描、diff 规则 | 可信环境下的 secret/许可证/依赖审计复核 |
| pre-push | 单元测试、build、受影响范围检查、advisory review、baseline 对比 | 平台矩阵、集成/e2e、并发/性能、发布构建 |
| 任务完成前 | agent 跑 `fast` profile，把失败当观察回灌 | 不适用（verify 仍是 AC 权威） |
| 发布/合并 | 生成本地报告与 attestation | 签名、制品、部署、合规与最终门禁 |

原则：

- **确定性检查优先**：能用一个命令判断的，不要调用 LLM；
- **review 先 advisory**：LLM/agent review 默认只给发现，不直接阻断；积累误报率后再考虑升为 gate；
- **本地证据不是信任边界**：v1 不根据本地 attestation 跳过服务器 CI；attestation 只作证据与度量。后续有签名/受信 runner 后，也只对 `local_sufficient = true` 的确定性 job 开放，安全、发布、签名类 job 永远在服务器执行；
- **不自动提交/推送**：本地 CI 可以修文件和生成报告，但 commit/push 必须由人或显式策略触发。

### 4.4 初始化设计

`loom init` 增加**探测 + 生成 + 报告**，但遵守“不覆盖用户文件”：

1. 扫描仓库，输出 `.loom/stack-profile.json`（与 §3.2 共用；运行时视图，不提交）；
2. 在目标项目根目录生成 `.loom.ci.toml`（已存在则跳过，`--force` 只覆盖 Loom 生成的文件）。这是**项目策略**，应纳入 git；不要放 `.loom/`——`.loom/` 是运行时状态且被整体忽略，worktree、队友和服务器都看不到。个人覆盖可后置 `.loom/ci.local.toml`（ignored），v1 不做；
3. 生成 acceptance 模板（写入 `goal.md` 的注释或 `.loom/acceptance.toml`，不直接改 TaskSpec）；
4. 可选安装 git hooks（`--hooks pre-commit,pre-push`，默认不装）；
5. 可选生成服务器 CI 的薄封装（`--ci github`，默认不生成）：只做 attestation 校验 + server-only jobs，不复制全部本地命令；
6. 打印 `doctor` 报告：检测到的栈、建议 jobs、缺失工具、可安装的 skills、baseline 状态。

`loom init --dry-run` 只打印计划，不写文件。

### 4.5 `.loom.ci.toml`

示例使用 Rust，是因为 Loom 自身也是 Rust；看起来像“本仓库的 CI”，但字段对所有目标项目通用。生成器不能把 `cargo` 或 `./scripts/dev.sh` 写死成默认值。

```toml
version = 1

[profiles.pre-commit]
description = "提交前快速闸门"
jobs = ["fmt-fix", "commit-msg"]

[profiles.pre-push]
description = "推送前完整闸门"
jobs = ["fmt-check", "clippy", "test", "review"]

[profiles.fast]
description = "agent finish 前的快速检查"
jobs = ["fmt-check", "clippy"]

[profiles.full]
description = "完整验证"
jobs = ["fmt-check", "clippy", "test", "build", "review"]

[jobs.fmt-fix]
kind = "command"
cmd = ["cargo", "fmt", "--all", "--", "--check"]
fix_cmd = ["cargo", "fmt", "--all"]
safe_fix = true
timeout_s = 60
required_tools = ["cargo"]

[jobs.fmt-check]
kind = "command"
cmd = ["cargo", "fmt", "--all", "--", "--check"]
timeout_s = 60
required_tools = ["cargo"]

[jobs.clippy]
kind = "command"
cmd = ["cargo", "clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]
timeout_s = 600
required_tools = ["cargo"]

[jobs.test]
kind = "command"
cmd = ["cargo", "test", "--workspace"]
timeout_s = 900
required_tools = ["cargo"]
needs = ["clippy"]

[jobs.build]
kind = "command"
cmd = ["cargo", "build", "--workspace"]
timeout_s = 600
required_tools = ["cargo"]
needs = ["test"]

[jobs.commit-msg]
kind = "command"
cmd = ["loom", "ci", "check-commit-msg", "--file", "{commit_msg_file}"]
timeout_s = 10

[jobs.review]
kind = "review"
diff = "staged"                  # staged | head | range
rules = ["no-debug-prints", "no-secrets", "minimal-diff"]
advisory = true
max_tokens = 6000
timeout_s = 120
local_sufficient = false

[jobs.skill-review]
kind = "skill"
skill = "rust-review"
diff = "staged"
advisory = true
max_tokens = 8000
timeout_s = 120
```

约束：

- `cmd` 用 argv 数组，避免 shell 引号与注入；确实需要 shell 时用 `shell = "..."` 显式声明；
- `timeout_s` 必填；`required_tools` 在运行前检查，缺失直接给出可行动的报错；
- `needs` 先做顺序依赖，不做并行（M4 前）；
- `kind = "command"` 是默认；`review` 调用有界 agent review；`skill` 加载 §3 的已验证 skill；
- `fix_cmd` 只允许确定性、可逆的修复；必须显式 `safe_fix = true`，执行后展示 diff，不自动提交；
- `advisory = true` 表示发现只作为建议/attention 事项，不阻断提交；升为 gate 需要评估；
- `local_sufficient = true` 只表示该 job 在受信条件下可被服务器参考；**v1 不据此跳过服务器 job**。默认 false，安全/发布类永远 false；后续有签名/受信 runner 才讨论跳过；
- 允许 `env`，但默认清空 `LOOM_API_KEY` / `OPENAI_API_KEY` 等 secret；
- 日志写 `.loom/ci/<run_id>/<job>.log`，按 8KB/24KB 预算截断后回给 agent，全文留 artifact。

> 字段形态说明：本节示例用 `[jobs.<id>]` + `cmd`；[16](16-运行时治理落地评估.md) §3.4 建议统一为 `[[jobs]]` + `argv` + `[runner]`，便于目标项目配置容器前缀、避免 `sh -c`。实现前二选一，倾向 16 的写法，不要同时支持两套。

全局配置（`.loom/config.toml`）：

```toml
[ci]
default_profile = "fast"
preflight = "off"           # off | fast
hooks_fix_safe = true       # pre-commit 先跑 safe_fix，再重跑检查
hooks_fail_open = true      # loom 不在 PATH 时是否放行
attestation = true
sign = "none"               # none | ed25519
```

### 4.6 命令

```bash
loom init --dry-run                       # 只看探测结果
loom ci plan                              # 探测并打印建议的 jobs + 证据 + 推荐 skills
loom ci plan --write                      # 写 .loom.ci.toml（不覆盖已有）
loom ci run --profile pre-commit          # 提交前闸门
loom ci run --profile pre-push --changed  # 推送前检查（affected-only 后续）
loom ci run --profile fast --fix          # 只执行 safe_fix job，展示 diff
loom ci install-hooks --profile pre-commit,pre-push
loom ci remove-hooks
loom ci doctor                            # 工具链、版本、缺失命令、baseline、hook 状态
loom ci report --last                     # 最近一次报告摘要（token 有界）
loom ci verify-attest <file>              # 校验本地报告/attestation，不重跑
```

`loom ci run` 输出：

```json
{
  "v": 1,
  "profile": "pre-commit",
  "ok": false,
  "tree": "git-tree-sha",
  "tool_versions": {"cargo": "1.99.1"},
  "jobs": [
    {"id": "fmt-fix", "kind": "command", "exit": 0, "duration_ms": 120, "fixed": true, "log": ".loom/ci/R-1/fmt-fix.log"},
    {"id": "commit-msg", "kind": "command", "exit": 1, "duration_ms": 5, "log": ".loom/ci/R-1/commit-msg.log"}
  ],
  "failed": ["commit-msg"],
  "report": ".loom/ci/R-1/report.json",
  "attestation": ".loom/ci/R-1/attestation.json"
}
```

报告/attestation 至少记录：`tree`、命令 argv、工具版本、开始/结束时间、退出码、日志 hash、`local_sufficient`、hook 是否实际运行/跳过原因。`--no-verify` 无法被 hook 观测，不记录。签名可选（`sign = "none|ed25519"`），但**未签名不等于可信**。

### 4.7 git hooks

- 只由 `loom ci install-hooks` 显式安装，不静默改 `.git/hooks` 或全局 `core.hooksPath`；
- 写入带 `# loom-ci-managed` 标记的 `pre-commit` / `pre-push`；
- 已存在非 Loom 管理的 hook：默认拒绝；`--force` 先备份为 `<hook>.bak.<timestamp>`，再安装；
- pre-commit 默认跑 `pre-commit` profile，目标 ≤ 30 秒；pre-push 默认跑 `pre-push` profile，目标 ≤ 5 分钟；超时/长任务留给服务器；
- `hooks_fix_safe = true` 时，pre-commit 先对 `safe_fix` job 执行修复并重跑检查；若工作区被改动，hook 退出 `1` 并提示重新 `git add`，**不自动 stage、不自动 commit**；
- hook 失败即阻断提交/推送；`git commit --no-verify` 仍可用。**hook 无法观测 `--no-verify`**，所以不承诺记录 bypass；`loom ci report` 只记录 hook 实际运行的结果，服务器以最终检查为准；
- `loom` 不在 PATH 时，默认 fail-open 并警告（避免破坏新克隆），但记录；可配置 `hooks_fail_open = false`；
- hook 只调用公开 CLI，不直接 import 内部 crate。

### 4.8 agent / harness 循环

```text
agent 改代码
   │
   ├─ loom ci run --profile fast
   │     ├─ pass ──► 继续 / finish
   │     └─ fail ──► 失败摘要作为工具观察回灌
   │                  ├─ 确定性错误：agent 修，重跑（有界）
   │                  ├─ safe fix：loom ci run --fix，展示 diff
   │                  └─ review/判断类：开 attention review 事项
   ▼
loom verify（TaskSpec acceptance，仍是任务权威）
```

- **native 提示**：system prompt 明确“finish 前运行 `loom ci run --profile fast`”；失败输出按工具预算截断后回灌；不替代 verify；
- **baseline 预检**：`[ci] preflight = "fast" | "off"`，默认 `off`；开启后 `loom dispatch` 在派发前跑 fast profile，红了就拒绝并提示“先修 baseline 或加 `--allow-red-baseline`”；
- **review job**：先跑确定性规则（debug 残留、secret 模式、超大 diff、生成物未更新），再可选调用 agent；发现写成结构化 `findings`，默认进 attention `review`，不直接改代码；
- **验收模板**：`loom init` 可从 `.loom.ci.toml` 生成候选 AC（如 `cargo test --workspace`），由人确认后写入 TaskSpec，而不是自动生效；
- **服务器消费**：v1 只把 `loom ci verify-attest` 当证据/度量，不根据本地 attestation 跳过服务器 job；安全/发布/签名、跨平台矩阵、e2e 永远重跑。后续有签名或受信 runner 后，才考虑对 `local_sufficient = true` 的确定性 job 开放跳过。

### 4.9 与 skills 的衔接

本地 CI 是把 skills 用起来的早期落点，但要防止 skill 变成绕过检查的后门：

| 用法 | 说明 |
|---|---|
| `loom ci plan` 推荐 skill | 根据 §3.2 仓库画像，推荐 `rust-review`、`conventional-commit`、`secret-scan` 等；输出 `why` 与证据 |
| `kind = "skill"` job | 只加载已安装、已静态检查、在 `skills.lock` 中的 skill；正文受 4KB/8KB 预算 |
| review 规则模板 | skill 提供检查清单/规则，但确定性规则仍优先；LLM 只做判断与解释 |
| 本地修复建议 | skill 可以建议 patch，但必须通过 `edit`/`patch` 工具落到 diff，不能直接执行任意脚本 |
| 晋升 | advisory review 的误报率、采纳率、CPVT 影响通过 §3.7 的 L2 评估后再考虑 gate |

硬约束：

- skill **不能**关闭/降低已有 job 的 `local_sufficient` 或 `advisory`；
- skill **不能**声明未在 `.loom.ci.toml` 中出现的任意 shell 命令；需要命令就走 `kind = "command"` 并接受同样的超时/权限/静态检查；
- skill **不能**修改 TaskSpec/AC/预算；
- 无网络、无 shell 是默认；需要时在 `.loom.ci.toml` 显式声明，不由 skill 自行决定；
- 安装/升级 skill 要更新 lock；CI 运行不使用浮动分支。

### 4.10 技术栈探测

| 证据 | 建议 jobs | 服务器保留 |
|---|---|---|
| `Cargo.toml` | `cargo fmt --check`、`cargo clippy -D warnings`、`cargo test --workspace`、`cargo build --workspace` | 跨平台/MSRV 矩阵、集成测试 |
| `package.json` + lock | 按 lock 选 `pnpm/yarn/npm`；`lint`、`typecheck`、`test`、`build`（只取存在的 script） | e2e、浏览器矩阵、发布构建 |
| `pyproject.toml` | `ruff check`、`mypy`/`pyright`、`pytest`（按配置） | 多 Python 版本、集成 |
| `go.mod` | `gofmt -l`、`go vet ./...`、`go test ./...` | race、跨平台、集成 |
| `Makefile` / `justfile` | 优先采纳已有的 `check`/`test`/`lint` target；不重复造 | 视内容 |
| `.github/workflows/*.yml` | 只提取可识别的命令作为**证据**，不强行解析 YAML；无法确定时给建议不写配置 | 原有 workflow 保留 |
| `rust-toolchain.toml` / `.nvmrc` / `.python-version` | 记录版本要求；doctor 检查当前版本是否匹配 | 矩阵可覆盖 |
| `Containerfile` / `compose.yaml` | 记录为环境证据；v1 不自动嵌套容器 | 容器化集成 |

探测不到时：只写注释模板，不猜命令。review 规则同理：只给通用规则（debug 残留、secret 模式、超大 diff），不凭猜测生成领域规则。

### 4.11 环境与安全

- v1 在**当前环境**执行，不启动嵌套容器；容器内缺工具时明确报错，不静默跳过；
- 命令来自仓库配置，视为可信；仍要清空 provider secret、限制输出、设置超时；
- 默认不联网；需要联网的 job 显式 `network = true`；
- 写操作只允许在仓库内；不跟随 symlink 到仓库外；
- `--fix` 只允许 `safe_fix = true` 的 job；执行后保留 diff，不自动 commit/push；
- 本地报告可能包含代码片段和日志：按 artifact 管理，写前做 secret 模式检查/截断；
- git hooks 显式安装、可卸载、可备份；不改全局 git 配置；
- attestation 只是“某个环境跑过这些命令”的证据，不是安全边界；不可信来源不享受跳过；
- 不完全替换现有 CI：v1 只补本地入口与证据，不做服务器跳过；GitHub Actions 等仍独立存在。

### 4.12 观测

`loom ci stats`：

- 本地各 profile 的运行次数、通过率、P50/P95 墙钟；
- `safe_fix` 触发次数、agent 修复后重跑次数、首过率；
- hook 实际运行/失败/跳过次数（`--no-verify` 无法观测，不统计）；
- review findings 数量、采纳/拒绝/误报率；
- 本地拦截的失败与预计节省的服务器重跑时间（v1 只做估算；启用服务器跳过后再统计真实节省）；
- CI 失败产生的 attention 事项与处理时延。

目标函数是**减少服务器 CI 的重复工作并缩短反馈回路**，不是单纯把本地命令跑得更多。

### 4.13 可行性

| 部分 | 可行性 | 说明 |
|---|---|---|
| 探测 + 生成 `.loom.ci.toml` | **高** | 规则明确，`--dry-run` 可先验证 |
| 顺序执行 + 日志预算 | **高** | 复用现有工具输出协议 |
| pre-commit/pre-push hooks | **高** | 纯 shell/CLI 包装；注意备份与 fail-open |
| safe fix + diff 展示 | **高** | 只限确定性命令；不自动提交 |
| 本地 report/attestation | **中** | 格式与 tree 校验容易；信任模型需明确 |
| review job（agent） | **中** | 质量/误报率需真实任务集；默认 advisory |
| skill 化 review/检查 | **中** | 依赖 §3 的 lock/静态检查；不能绕过 gate |
| baseline 预检 | **中** | 老仓库红基线常见，默认应关闭 |
| 环境一致性 | **中** | 容器/版本漂移；v1 只检测不修复 |
| monorepo / 多栈 | **中** | 等 M5 工作区设计；v1 按根目录单栈 |
| 服务器按 attestation 跳过 | **低（v1 不做）** | 本地文件可伪造；需签名/受信 runner，后置 |
| 嵌套容器 | **低** | 当前 dev 已用 podman/docker，嵌套复杂；后置 |

结论：**先做探测 + `.loom.ci.toml` + 顺序 `loom ci run` + doctor；再做 hooks、safe fix、report/attestation；review/skill job 先 advisory。并行、缓存、容器化、affected-only 后置。**

### 4.14 验收（未来实现时）

- [ ] `loom init` 在目标项目（例如临时 Rust/Node/Python 仓库；Loom 自身 dogfood 单独验证）识别技术栈，生成 `.loom.ci.toml`，且不覆盖已有文件。
- [ ] `loom ci run --profile pre-commit` 在格式错误时失败，`--fix` 只修 `safe_fix` job 并展示 diff。
- [ ] `hooks_fix_safe = true` 时 pre-commit 自动修安全项并重跑；有改动时要求重新 `git add`，不自动 stage/commit。
- [ ] `loom ci install-hooks` 幂等；已有非 Loom hook 时拒绝，`--force` 先备份；不承诺记录 `--no-verify`。
- [ ] 报告/attestation 含 tree、argv、工具版本、退出码；同 tree 校验通过，不同 tree 失败。
- [ ] v1 不使用本地 attestation 跳过服务器 job；后续有签名/受信 runner 后，才只跳过 `local_sufficient = true` 的确定性 job，安全/发布/矩阵永远不跳过。
- [ ] review job 有 token/时间上限，默认 advisory，findings 结构化；无网络。
- [ ] `kind = "skill"` 只能加载 lock 内、通过静态检查的 skill，不能关闭 gate。
- [ ] CI 失败开 attention `review`/`operation` 事项；同 tree 成功后自动 resolve。
- [ ] `loom verify` 行为不变，仍只以 TaskSpec acceptance 为任务权威。

---

## 5. 横向契约变更

这些方向都会新增文件/命令，但共享一组边界。按 `AGENTS.md` §4，任何事件/字段变更都要同步类型、schema、文档、测试。

| 方向 | 新增 artifact | 新增事件 | 配置 | CLI |
|---|---|---|---|---|
| churn | `logs/churn.json`（新 schema） | `task.churn`（仅干预时） | `[guardrails.churn]` | `loom churn stats` |
| attention | `.loom/inbox.json`（只读视图） | `attention.opened`、`attention.updated`、`attention.resolved`、`attention.expired` | `[attention]` | `loom inbox [--line/--check]`、`loom inbox ack/defer/resolve` |
| questions | `outbox/questions.json`、`answers.json`、`assumptions.jsonl` | `task.needs_input`、`task.answered` | `[questions]` | `loom answer` |
| skills | `stack-profile.json`、`skills.catalog.json`、`skills.lock` | 无（skill 加载不是任务状态） | `[skills]` | `loom skill scan/recommend/add/show/verify/stats` |
| CI | `.loom.ci.toml`（tracked）、`.loom/ci/<run>/{report,attestation}.json`（runtime） | `attention.*`（失败/待复核时） | `[ci]` | `loom ci plan/run/doctor/install-hooks/report/verify-attest/stats` |

共同约束：

- **状态仍由事件重放**：churn/questions/attention 的 artifact 是证据或视图，不是状态事实源；状态变化必须写事件。
- **不新增交互式命令**：`loom inbox/answer` 是批处理，不进入等待循环。
- **固定角落不污染机器输出**：footer 只写 stderr 且仅 TTY；`--json` 的 stdout 保持纯 JSON；非 TTY/CI 默认关闭。
- **不改变退出码语义**：`0` 成功、`2` 需要人工/阻塞、`3` 预算/超时、`4` adapter 失败；`churn_guard` / `needs_input` 都是 `2`，用事件 `data.code` 区分。
- **上下文硬预算不变**：churn 摘要、问题、skill 正文都受 8KB/24KB/128KB 与 Context Compiler 约束；attention 默认不进模型上下文。
- **本地 CI 不是信任边界**：v1 不用本地 attestation 跳过服务器 job；后续有签名/受信 runner 后，才讨论对 `local_sufficient` 的确定性 job 跳过；安全/发布/签名永远在服务器执行。
- **不依赖真实模型即可测试**：attention/questions/CI 核心、churn 的投影都能用 mock/native 脚本化 provider 覆盖；review job 的模型部分单独标 advisory。
- **churn 的 KPI 是 `wasted_tokens_est` / `wasted_cost_usd_est`，不是写入次数**：`write_passes` / `churn_ratio` 只用于解释原因；L2 拦截不是主要节省手段。
- **churn 优化的是 `messages` 投影与工具形态**（patch/edit），不改变 provider 的 wire 协议。

---

## 6. 优先级与里程碑建议

| 顺序 | 方向 | 为什么先做 | 依赖 | 粗略工作量 |
|---|---|---|---|---|
| P0 | CI 核心：init 探测 + `.loom.ci.toml` + 顺序 `loom ci run` + doctor | 让“本地检查”先有统一入口；后续度量与 hooks 都靠它 | 无 | 5–7 人日 |
| P0 | question protocol + attention inbox | 修 `needs_input → task.failed` 的正确性缺陷；同时解决人工事项被历史淹没 | 无 | 4–5 人日 |
| P1 | CI 前移：pre-commit/pre-push hooks + safe fix + report | 把格式/lint/提交信息挡在提交前，直接减少服务器重复工作 | CI 核心 | 3–4 人日 |
| P1 | churn：native 步骤级上下文重编译 + patch-first + token 归因 | 先切断旧全文的输入复利，再谈拦截；指标必须先于策略 | 无 | 4–6 人日 |
| P1 | churn watchdog（外部 adapter） | 只做 gross loop 止损与选型依据 | 容器内需有 adapter 二进制 | 2–3 人日 |
| P2 | CI review/skill job + attestation 证据 | 把 review 与可复用检查模板纳入；先 advisory；服务器跳过后置 | CI 前移 + skills | 3–5 人日 |
| P2 | skills：scan + 本地 catalog + lock + 按需加载 | 结构先立起来，避免全局安装；CI 是首个使用场景 | CI/任务集 | 5–8 人日 |
| P2 | skills：L2 评估与晋升 | 证明 skill 真的有用；review advisory→gate 也靠它 | 真实模型 + 固定任务集 + 账单 | 后置 |
| P3 | embedding/联网目录 | 收益未证明 | — | 不做 |

注意：

- 当前没有并发/锁，P2 的“父子 agent 不等待”只定义协议，不实现并行调度；M4 再做。
- 真实 goose/OpenCode canary 仍受二进制与 API key 阻塞，见 [14](14-当前问题与卡点.md)。
- churn 优化的是 `messages` 投影与工具形态，不改 provider 请求格式；外部 adapter 只能观察 worktree，不能逐步投影。
- attention inbox 不依赖真实模型，应和 question protocol 一起先做；否则即使 `needs_input` 修对了，用户仍然找不到待办。
- CI 前移与服务器跳过是两步：先本地跑起来，再谈 attestation；不要在没有可信 tree/工具版本记录前就跳过服务器检查。

---

## 7. 不做什么

- 不做全局 skill 安装，不做“装得越多越好”。
- 不做联网自动安装/自动执行 skill 脚本。
- 不做 embedding/向量库、MCP 跨项目知识库。
- 不做交互式提问等待；提问必须是文件 + 退出码。
- 不为每个问题调用一次 LLM 分类；先走确定性策略。
- 不把 attention inbox 做成聊天/工单系统，也不做常驻 daemon；v1 只做事件 + 视图 + 只读轮询。
- 不自动执行 inbox 里的 `actions[].cmd`；不自动关闭 `blocking` 事项，不用 `acked` 冒充 `resolved`。
- 不静默安装 git hooks，不修改全局 git 配置；不承诺记录 `--no-verify`，服务器最终门禁兜底。
- 不用本地 CI 替代服务器安全/发布/签名门禁；不信任未签名或来源不明的 attestation。
- 不让 skill 直接定义任意 CI shell 命令或关闭已有 gate；命令必须出现在 `.loom.ci.toml` 并接受同样的检查。
- 不做嵌套容器 CI（v1）。
- 不把 churn 直接归因于某个模型；先积累按模型的度量。
- 不自动修改 AC/目标来“让任务通过”；AC 变更必须走 judgment/人工。

---

## 8. 开放问题

1. `needs_input` 用新事件还是复用 `task.blocked` 的 `code`？
   - 倾向新事件 + 新状态，避免阻塞统计与升级计数继续失真。
2. churn 超限默认 `blocked` 还是 `failed`？
   - 倾向 `blocked`；由 `[guardrails.churn]` 显式选择是否计入升级。
3. 回答问题时 `attempt` 是否递增？
   - 倾向不递增；新 run + `question_round`，避免“提问=重试”的语义。
4. skills 的 catalog 信任源与许可证策略由谁维护？
   - 需要团队策展；v1 只支持本地目录与手工导入。
5. `.loom.ci.toml` 与现有 `Makefile`/CI 的优先级？
   - 倾向“采纳已有命令”，`.loom.ci.toml` 只做聚合与超时/日志包装。
6. baseline 预检默认开还是关？
   - 倾向默认关，项目稳定后再开，避免接入摩擦。
7. 父子 agent 的问题上浮是否需要独立“问题预算”？
   - 需要；否则回答可能吃掉任务预算，建议单独记账并设上限。
8. attention 的固定角落在 CLI 里首选哪种形态？
   - 倾向“TTY 时每条命令的 stderr footer + `loom inbox --line` 给 statusline”；TUI/桌面端后置，避免先做一个没人看的界面。
9. `attention.*` 是全量显式事件，还是只给非任务来源、任务来源由投影器派生？
   - 倾向任务事件照常写，同时显式写 `attention.*`，保持“任务状态”和“人看到什么”两个维度；但要防重复，需在投影器做幂等。
10. pre-commit 在 `loom` 缺失时 fail-open 还是 fail-closed？
    - 倾向 fail-open + 警告 + 记录，避免新克隆无法提交；服务器仍做最终门禁。
11. 哪些 CI job 可以标 `local_sufficient = true`？
    - 倾向只限 format/lint/单元测试等确定性、低环境依赖项；安全、发布、签名、跨平台矩阵永远 false。
12. review job 从 advisory 升为 gate 的阈值？
    - 需要误报率、采纳率、人工复核与固定任务集数据；当前没有，所以先 advisory。
13. skill 提供的 CI job 模板是否需要独立签名/策展源？
    - 倾向需要；至少 lock + 静态检查 + 项目级安装，不能直接引用浮动分支。

---

## 9. 最小验收清单（未来实现时）

- [ ] `loom ci plan` 能在目标项目识别技术栈（Loom 自身 dogfood 时是 Rust workspace），并给出 jobs 与证据。
- [ ] `loom init` 不覆盖已有 `.loom.ci.toml` / workflow / hook；`--dry-run` 无写操作。
- [ ] 缺 `cargo`/`pnpm` 时 `loom ci doctor` 给出可行动报错，而不是静默跳过。
- [ ] `loom ci run --profile pre-commit` 在格式错误时失败；`--fix` 只修 `safe_fix` job 并展示 diff，不自动提交。
- [ ] `hooks_fix_safe` 生效时 pre-commit 自动修安全项并重跑；有改动时要求重新 `git add`，不自动 stage/commit。
- [ ] `loom ci install-hooks` 幂等；已有非 Loom hook 时拒绝，`--force` 先备份；不承诺记录 `--no-verify`。
- [ ] 报告/attestation 含 tree、argv、工具版本、退出码；同 tree 校验通过，不同 tree 失败；v1 不据此跳过服务器 job。
- [ ] review job 有 token/时间上限、默认 advisory、findings 结构化、无网络；`kind=skill` 只能加载 lock 内且通过静态检查的 skill。
- [ ] `needs_input` 回执经 `loom verify` 后状态为 `needs_input`，**不产生 `task.failed`**，不增加升级计数。
- [ ] `task.needs_input` 自动开 `question` attention 事项；`loom answer` 后自动 resolve。
- [ ] `loom doctor` 缺工具时开 `operation` 事项；`loom ci` 失败时开 `review`/`operation` 事项；条件解除后自动 resolve。
- [ ] 人类命令在 TTY 下 stderr 出现一行 footer；`--json` stdout 不含 footer；非 TTY/CI 不打印；`LOOM_NO_INBOX=1` 可关闭。
- [ ] `loom inbox` 同时显示问题与操作类事项；`loom inbox --check` 在存在 `blocking` 时退出 `2`；`ack` 不关闭事项，`defer` 只作用于 `soon/info`。
- [ ] 脚本化 provider 调用 `ask` 工具 → `questions.json` → exit 2 → `loom inbox` 可见 → `loom answer` → 新 run 收到答案并通过 verify。
- [ ] 脚本化 provider 对同一 8KB 文件连续重写 5 次；无投影时 input 随步数复利增长，有投影时有界。
- [ ] `churn.json` 输出 `rewrite_output_tokens`、`history_carry_tokens`、`avoided_input_tokens`、`wasted_tokens_est`，并标 `estimated`。
- [ ] 第 2 次重写后旧 payload 被投影；第 3 次无 `mode=replace` 的 `write` 被短错误拒绝；L3 触发后重启只带 churn 摘要。
- [ ] 同一改动用 patch 与用整文件 `write` 的 output token 有可量化差距。
- [ ] `loom churn stats --by model` 第一屏输出 token / 成本，第二屏才是 `write_passes` / `churn_ratio`（先用脚本化数据验证）。
- [ ] `loom skill scan` 输出的每个标签都有证据文件；无证据不出现。
- [ ] `loom skill add` 只写项目级目录与 `skills.lock`；未通过静态检查的 skill 拒绝安装。
- [ ] 加载 skill 时只注入索引行；正文经 `loom skill show` 按需读取，受 4KB/8KB 预算。
- [ ] 所有新增事件同步了 `loom-core` 类型、`schemas/event.v1.json`、`doc/03` §11.2 与至少一个测试。

---

## 10. 落地评估

基于当前代码的接入点、契约变更、分期人日与风险，以及三处需要先修正的设计（`.loom.ci.toml` 位置与归属、attestation 授权、`verify` 的 claim/`needs_input` 语义），见 [16-运行时治理落地评估.md](16-运行时治理落地评估.md)。
