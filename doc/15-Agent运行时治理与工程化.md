# 15 Agent 运行时治理与工程化

> 状态：**设计稿，未实现**（2026-11-07）。
> 范围：四个已观察问题——写入抖动、提问阻塞、skills 治理、初始化时本地 CI bootstrap。
> 前置阅读：[08-设计路线-v0.2.md](08-设计路线-v0.2.md)（状态所有权 / adapter 契约）、[03-核心协议.md](03-核心协议.md)（TaskSpec / 回执 / 事件）、[10-工具输出协议-v0.1.md](10-工具输出协议-v0.1.md)（工具与 telemetry）、[14-当前问题与卡点.md](14-当前问题与卡点.md)（当前环境限制）。

---

## 0. 结论

这四个问题都不是“换更强的模型”能根治的，而是控制面缺少**行为协议**：

| 问题 | 本质 | 控制面要补的东西 |
|---|---|---|
| 反复删除重写 | 模型把“重写”当成进展，harness 无法识别无效劳动 | 写入历史、churn 指标、分级干预、跨 run 证据 |
| 提问即暂停 | 提问被当成失败或交互等待，而不是一次可恢复的返回 | `questions.json` + `needs_input` 状态 + 回答/假设协议 |
| skills 泛滥 | skill 是上下文成本与供应链风险，不是“装了就有用” | 仓库画像、离线目录、按需加载、验证后晋升 |
| 初始化缺 CI | agent 没有可信的本地反馈回路，验收靠人手写 | 技术栈探测、`ci.toml`、`loom ci`、baseline 预检 |

共同主线：**把 agent 的行为变成可观测、可中断、可恢复的结构化产物**；不引入长会话、不依赖交互式等待、不让 LLM 做本可确定性判断的事。

建议实施顺序：

1. **本地 CI bootstrap**：先让“验证”可信，后面三个方向才有度量基础。
2. **question protocol**：当前 `needs_input` 会被 verify 记成 `task.failed`，属于正确性缺陷，应优先修。
3. **churn guard**：native 工具层可立即做；外部 adapter 只能做粗粒度 watchdog。
4. **skills 治理**：扫描/加载/静态检查可以先做；**晋升评估依赖真实模型与固定任务集**，当前仍受 [14](14-当前问题与卡点.md) 的环境卡点限制。

明确不做：全局安装 skill、联网自动装 skill、embedding/向量检索、交互式等待、为每个问题调用一次 LLM、v1 嵌套容器。

---

## 1. 问题一：反复删除重写（write churn）

### 1.1 现象

DeepSeek v4.1 flash 在修改代码时出现“删除整段、重新填写”的反复行为。根因尚未定位，可能是：

- 模型对 `edit` 的上下文匹配不自信，倾向整文件重写；
- 指令里“修好它”与“最小 diff”冲突，模型选择推倒重来；
- 上下文里缺少当前文件的权威版本，模型凭记忆重建；
- 失败回灌后没有“不要重复同一改动”的硬约束；
- 模型本身在长文件上更容易抖动。

在没有度量前，不应先把原因归给某个模型。**先让 harness 能识别并计费这类行为，再谈干预和模型差异。**

### 1.2 现状盲区（代码核对）

| 现状 | 后果 |
|---|---|
| native 的 no-progress 只看 `mutated: bool` | `write` 即使写回完全相同的内容也返回 `mutated: true`，重写永远算“有进展” |
| `write` 不要求先 `read`，也不限制整文件覆盖 | 模型可以直接重写已存在的大文件 |
| 没有按文件记录内容哈希/版本 | 无法识别 A→B→A 振荡、无操作写入、重复整文件覆盖 |
| telemetry 只记工具结果大小，不记“改了哪个文件、改了多少” | 离线无法回答“哪个模型更容易抖” |
| 外部 adapter 只能看最终 diff | run 结束后才知道 churn，无法中途止损 |

相关实现：`src/crates/loom-native/src/run.rs`（`mutated` / `idle`）、`src/crates/loom-native/src/tools.rs`（`write` / `edit`）、`src/crates/loom-store/src/telemetry.rs`。

### 1.3 指标定义

以**单次 run** 为窗口，按文件记录版本历史。所有指标必须能从工具调用或 worktree 快照重算。

| 指标 | 定义 | 用途 |
|---|---|---|
| `touches` | 该文件被 `edit` / `write` / `bash` 改动影响的总次数 | 粗粒度活跃度 |
| `write_passes` | 对**已存在文件**执行的整文件 `write` 次数（内容长度 ≥ 当前文件 50% 且 ≥ 512B） | 识别“推倒重来” |
| `bytes_written` | 所有 `write` 的 content 字节 + `edit` 插入的字节 | 写入放大分子 |
| `write_amplification` | `bytes_written / max(1, final_bytes)` | 最终 8KB 的文件写了 120KB → 15× |
| `oscillations` | 内容哈希序列中出现 A→B→A（至少隔一个不同版本） | 来回改 |
| `noop_writes` | 写入后哈希不变 | 纯浪费 |
| `history_lines` | 所有工具调用累计插入 + 删除的行数 | 历史总改动 |
| `final_diff_lines` | 结束时相对 base 的 `insertions + deletions` | 净改动 |
| `churn_ratio` | `history_lines / max(1, final_diff_lines)` | 最终只改 10 行却动了 300 行 → 30× |
| `stalled_rewrites` | 连续 N 次重写之间，acceptance/verify 没有从 fail 变 pass | 区分“重构”与“打转” |

> `touches` 里的 `bash` 只能通过快照归属到文件；native 的 `edit`/`write` 可以精确归属。无法归属时记到 `unknown`，不要猜。

### 1.4 落盘格式

每次 run 结束写 `logs/churn.json`（或 `outbox/churn.json`，二选一，建议 `logs/`）：

```json
{
  "v": 1,
  "run_id": "R-20261107-001",
  "task_id": "T-20261107-002",
  "source": "native",
  "files": [
    {
      "path": "src/parser.rs",
      "touches": 7,
      "write_passes": 3,
      "bytes_written": 122880,
      "final_bytes": 8192,
      "write_amplification": 15.0,
      "oscillations": 1,
      "noop_writes": 2,
      "history_lines": 312,
      "final_diff_lines": 11,
      "churn_ratio": 28.4,
      "stalled_rewrites": 3
    }
  ],
  "totals": {
    "files_touched": 1,
    "bytes_written": 122880,
    "final_bytes": 8192,
    "write_amplification": 15.0,
    "churn_ratio": 28.4
  },
  "interventions": [
    {"step": 9, "level": "nudge", "path": "src/parser.rs"},
    {"step": 13, "level": "block", "path": "src/parser.rs"}
  ],
  "verdict": "warn"
}
```

只有发生干预时才写事件，避免每个 run 都污染 `events.jsonl`。建议 `task.churn`：

```json
{
  "type": "task.churn",
  "data": {
    "level": "nudge|block|stop",
    "path": "src/parser.rs",
    "write_passes": 3,
    "churn_ratio": 28.4,
    "action": "continue|blocked"
  }
}
```

新增事件类型时，按 `AGENTS.md` §4 同步：`loom-core` 枚举、`schemas/event.v1.json`、`doc/03` §11.2、至少一个测试。

### 1.5 分级干预

原则：**先提醒、再阻断、最后停 run**；默认不把“抖动”直接算成模型能力失败。

| 级别 | 触发（默认值） | 动作 | 结果 |
|---|---|---|---|
| L0 观察 | 总是 | 记录 `churn.json`，不影响执行 | 继续 |
| L1 提醒 | 同文件第 2 次整文件重写，且 verify 无改善 | 注入一条工具/用户消息：列出当前文件哈希、已重写次数、要求先 `read`、用 `edit` 做最小改动、若卡住就 `finish(blocked)` | 继续，成本几乎为零 |
| L2 阻断 | 第 3 次整文件重写，或 `write_amplification > 10` | `write`/`edit` 返回错误观察：“churn guard 拒绝第 3 次整文件重写；请 read 当前内容后用 edit，或 finish(blocked) 说明卡点” | 工具调用失败，模型可见；不直接杀进程 |
| L3 停止 | 第 5 次整文件重写，或 `churn_ratio > 8` 且连续 3 次无 verify 改善，或 churn 消耗超过预算 20% | 终止执行，写 `churn.json` + `task.churn`，`finish(blocked, code=churn_guard)` | 退出码 `2`；默认 `blocked`，可配置为 `failed` |

配置建议：

```toml
[guardrails.churn]
enabled = true
max_write_passes = 3
max_amplification = 10.0
max_churn_ratio = 8.0
max_stalled_rewrites = 3
on_exceeded = "blocked"   # blocked | failed
allow = ["**/generated/**", "**/*.lock", "**/dist/**"]
```

- `on_exceeded = "blocked"`：不计数到升级阈值，交给人判断；适合“不确定是模型问题还是任务本身需要大改”。
- `on_exceeded = "failed"`：计入 `task.failed`，可触发 M3 升级；适合明确要求“最小 diff”的任务。
- 这是策略选择，不在代码里写死。

### 1.6 前置约束（比事后检测更便宜）

在 native 工具层增加两条低成本规则：

1. **大文件先读后写**：对已存在且 > 40 行的文件，第一次 `write` 前必须在本 run 内 `read` 过它；否则返回错误观察，提示先读。
2. **整文件覆盖要显式**：`write` 已存在文件时要求 `mode = "replace"`（或 `overwrite = true`），默认只允许创建新文件/小文件覆盖；`edit` 仍是首选。

这两条不依赖模型自省，直接改变工具的可行动空间。对“确实需要整文件重写”的场景，模型显式声明即可，同时会被计入 `write_passes`。

### 1.7 外部 adapter 的 watchdog

外部 adapter 不能拦截工具调用，只能采样 worktree：

- 每 2s 对 `git ls-files` + 未跟踪文件做哈希快照（排除 `.git`、`.loom`、配置的 allowlist）；
- 统计同一文件哈希变化次数、净大小变化；
- 只在**明显 gross loop**时触发 L3：同一文件变化 ≥ 8 次且净 diff 很小，或 run 超过墙钟 50% 但 diff 仍为 0；
- 触发后先 `SIGTERM`，宽限 5s 再 `SIGKILL`；写 `churn.json`，`source = "watchdog"`，`confidence = "low|medium"`；
- 不尝试 L1/L2，因为采样看不到工具调用边界，误判成本高。

watchdog 的价值是“止损”，不是“精确计费”。精确 churn 数据仍以 native 为准。

### 1.8 反馈到下一次 attempt

`dispatch` 生成 `FailurePack` 时，若上一 run 有 `churn.json`，只注入**一行摘要 + 前 3 个文件**：

```text
上一 run 的写入抖动：src/parser.rs 整文件重写 3 次，churn_ratio 28.4。
本次要求：先 read，使用 edit 做最小 diff；不要重新生成整个文件。
```

不要把完整 `churn.json` 塞进 packet。

### 1.9 度量与模型差异

新增 `loom churn stats`（先做 JSON，后做 pretty）：

- 按 model / role / task 聚合 `write_amplification`、`churn_ratio`、`stalled_rewrites`、干预次数；
- 关联 `usage` 账本，估算抖动浪费的 token（按发生重写的 step 的 usage 求和，标记为估算）；
- 输出“DeepSeek v4.1 flash vs 其他 worker”的对比，而不是预设结论。

这是回答“是否模型问题”的唯一可靠方式；单看几个案例容易过拟合。

### 1.10 可行性

| 部分 | 可行性 | 说明 |
|---|---|---|
| native 精确 churn | **高** | 工具层拥有 content 与 path，1–2 天可出指标与 L1/L2 |
| 外部 adapter watchdog | **中** | 只能粗粒度、低频采样；误判与漏判都存在 |
| 阈值调参 | **中** | 需要真实任务集；当前只能用脚本化 provider 做回归 |
| 模型差异结论 | **低（现在）** | 缺真实 key/账单/固定任务集，先积累 telemetry |
| 误伤重构/格式化 | **中** | 用 allowlist、verify 改善判断、连续无改善才升级来降低 |

结论：**native 先做，外部 adapter 后做**；指标先于拦截，避免把正常重构误杀。

---

## 2. 问题二：提问导致会话暂停

### 2.1 现状（这是一个正确性缺陷）

当前链路：

1. adapter 退出码 `2`，回执状态 `needs_input`；
2. `loom verify` 只把 `ReceiptStatus::Done` 当作 `claim_done`；
3. 于是 `needs_input` / `blocked` 都会走到 `task.failed`（`code = claim_not_done`）；
4. M3 升级只数 `task.failed` 事件 → **agent 提一个问题，会被记成一次失败，甚至触发 worker→frontier 升级**；
5. `dispatch` 只接受 `pending/ready/failed/escalated`，没有“已回答、继续”的状态；
6. 外部 agent 若真在等 stdin，会挂到超时，烧墙钟。

相关实现：`src/crates/loom-cli/src/verify_cmd.rs`（`claim_done` / `TaskFailed`）、`src/crates/loom-adapter/src/mock.rs`（`NeedsInput`）、`src/crates/loom-core/src/task.rs`（没有 `NeedsInput` 状态）。

### 2.2 原则

1. **提问是返回值，不是暂停。** 进程退出，状态落盘；回答后开新 run 继续，不保留会话。
2. **提问必须结构化。** 不接受“最后一段自然语言里有个问号”作为协议。
3. **分级授权。** 能在 spec/AC/约束内确定的，直接答；可逆的，用默认值 + 假设记录；不可逆/安全/范围/预算问题，必须人答。
4. **不把提问计入失败。** `needs_input` 是正常状态，不是 `failed`。
5. **批处理、去重、TTL。** 一次回答尽量覆盖多个问题；过期用默认值或明确阻塞。
6. **父子 agent 不互相等待。** 子 agent 把问题作为返回值交给父级；父级能答就答，不能答就上浮，同时继续独立任务。

### 2.3 契约

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

### 2.4 状态与事件

建议新增：

| 事件 | data | 状态变化 |
|---|---|---|
| `task.needs_input` | `question_ids`, `blocking`, `scopes` | `receipt → needs_input` |
| `task.answered` | `question_ids`, `by`, `source`, `assumption_ids` | `needs_input → ready` |

新增 `TaskStatus::NeedsInput`。不要复用 `blocked`：`blocked` 表示依赖/锁/环境阻塞，`needs_input` 表示等人或等上游回答；两者混在一起会让统计和升级都失真。

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

### 2.5 回答协议

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

### 2.6 假设账本

对“可逆 + 默认值继续”的问题，写 `.loom/assumptions.jsonl`：

```json
{"v":1,"id":"A-001","task_id":"T-...","run_id":"R-...","assumption":"保留旧版 schema 兼容","source":"policy","reversible":true,"verify_hint":"迁移后跑 A3 回滚测试"}
```

review / verify 可以检查假设是否被后续证据推翻。这样“先继续”不会变成“悄悄猜”。

### 2.7 父子 agent

- 子 agent 没有“提问通道”，只有返回值：`questions.json` + exit 2。
- 父 agent 的 `[questions]` 策略决定它能否代答：`allowed_scopes`、`max_auto_answers`、`max_question_tokens`。
- 父 agent **不 sleep、不等待**：子任务进入 `needs_input`，父 agent 继续处理不依赖它的任务；依赖它的任务保持 `pending`。
- 去重：按 `task_id + normalized_question` 哈希；同一问题在多个子任务出现时合并为一个 inbox 条目。
- 预算：回答问题的 judgment 调用单独记账，设 `max_cost_usd`，不允许用回答把任务预算吃穿。

### 2.8 外部 adapter 兜底

- 在 adapter prompt 里明确：“不要交互式提问；写 `outbox/questions.json` 并退出 2。”
- 启动参数统一 `stdin = null`。
- 若进程仍挂起：watchdog 在“无 token、无文件变化、无日志增长”持续 N 秒后终止，读取最后的 agent 消息，合成一个 `Q?`（`confidence=low`），标记 `needs_input`，而不是 `failed`。
- 若 adapter 退出 0 但内容明显是提问且没有 diff：不要自动判成 `needs_input`（容易误判）；让 verify 失败，并在 `next_hint` 里提示“若需要输入，请用 questions.json + exit 2”。只有 adapter 明确声明 `supports_questions` 时才启用启发式。

### 2.9 可行性

| 部分 | 可行性 | 说明 |
|---|---|---|
| 控制面状态/事件/回答 | **高** | 纯文件与事件，可先于真实模型完成 |
| native `ask` 工具 | **高** | 写 questions.json + 返回 exit 2；脚本化 provider 可测 |
| 外部 adapter 遵守协议 | **中** | 取决于 agent 是否听话；需要 canary 与兜底 |
| 自动回答质量 | **中** | 需要明确授权边界；宁可上浮，不要乱答 |
| 父子多 agent | **中** | 当前无并发；顺序执行也能先定义协议，M4 再并行 |

结论：**先修 verify 的 `needs_input` 误判，再做 questions.json 与 `loom inbox/answer`；外部 adapter 的合规性等二进制到位后加 canary。**

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

## 4. 问题四：初始化与本地 CI bootstrap

### 4.1 现状

`loom init` 只做：

- 建 `.loom/{tasks,runs,ledger,snapshots,locks}`；
- 写默认 `config.toml`、`goal.md`、`constraints.md`；
- 更新 `.gitignore`。

`loom doctor` 只检查 `.loom`、git、config、可写。它不探测技术栈、不检查工具链、不生成 acceptance、不提供本地 CI。结果是：

- 每个项目手写 acceptance；
- agent 不知道“这个仓库的正常检查是什么”；
- 环境缺 `pnpm`/`cargo`/`just` 时，直到 verify 才失败；
- 没有 baseline 概念，agent 可能把“本来就红的测试”当成自己改坏。

### 4.2 初始化设计

`loom init` 增加**探测 + 生成 + 报告**，但遵守“不覆盖用户文件”：

1. 扫描仓库，输出 `stack-profile.json`（与 §3.2 共用）；
2. 生成 `.loom/ci.toml`（已存在则跳过，`--force` 只覆盖 Loom 生成的文件）；
3. 生成 acceptance 模板（写入 `goal.md` 的注释或 `.loom/acceptance.toml`，不直接改 TaskSpec）；
4. 生成 CI workflow（可选 `--ci github`，默认不生成）；
5. 打印 `doctor` 报告：检测到的栈、命令、缺失工具、建议。

`loom init --dry-run` 只打印计划，不写文件。

### 4.3 `ci.toml`

```toml
version = 1

[profiles.fast]
description = "提交前快速检查"
jobs = ["fmt", "clippy"]

[profiles.full]
description = "完整验证"
jobs = ["fmt", "clippy", "test", "build"]

[jobs.fmt]
cmd = ["cargo", "fmt", "--all", "--", "--check"]
timeout_s = 60
required_tools = ["cargo"]

[jobs.clippy]
cmd = ["cargo", "clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]
timeout_s = 600
required_tools = ["cargo"]

[jobs.test]
cmd = ["cargo", "test", "--workspace"]
timeout_s = 900
required_tools = ["cargo"]
needs = ["clippy"]

[jobs.build]
cmd = ["cargo", "build", "--workspace"]
timeout_s = 600
required_tools = ["cargo"]
needs = ["test"]
```

约束：

- `cmd` 用 argv 数组，避免 shell 引号与注入；确实需要 shell 时用 `shell = "..."` 显式声明；
- `timeout_s` 必填；
- `required_tools` 在运行前检查，缺失直接给出可行动的报错；
- `needs` 先做顺序依赖，不做并行（M4 前）；
- 允许 `env`，但默认清空 `LOOM_API_KEY` / `OPENAI_API_KEY` 等 secret；
- 日志写 `.loom/ci/<run_id>/<job>.log`，按 8KB/24KB 预算截断后回给 agent，全文留 artifact。

### 4.4 命令

```bash
loom init --dry-run                 # 只看探测结果
loom ci plan                        # 探测并打印建议的 jobs + 证据
loom ci plan --write                # 写 .loom/ci.toml（不覆盖已有）
loom ci run --profile fast          # 顺序执行，JSON/pretty
loom ci run --profile fast --changed # 只跑受影响 job（后续）
loom ci doctor                      # 工具链、版本、缺失命令、baseline 状态
```

`loom ci run` 输出：

```json
{
  "v": 1,
  "profile": "fast",
  "ok": false,
  "jobs": [
    {"id": "fmt", "exit": 0, "duration_ms": 120, "log": ".loom/ci/R-1/fmt.log"},
    {"id": "clippy", "exit": 101, "duration_ms": 8300, "log": ".loom/ci/R-1/clippy.log"}
  ],
  "failed": ["clippy"]
}
```

### 4.5 与 dispatch / verify 的关系

- **baseline 预检**：`[ci] preflight = "fast" | "off"`。默认 `off`，避免老仓库第一次接入就被红基线卡住；开启后，`loom dispatch` 在派发前跑 fast profile，红了就拒绝并提示“先修 baseline 或加 `--allow-red-baseline`”。
- **native 提示**：system prompt 明确“finish 前运行 `loom ci run --profile fast`”，失败输出作为工具观察回灌；不替代 verify。
- **verify 仍是权威**：`loom verify` 只跑 TaskSpec 的 acceptance，不从 `ci.toml` 推断 AC；`ci.toml` 是仓库级检查，TaskSpec acceptance 是任务级契约。
- **acceptance 模板**：`loom init` 可从 `ci.toml` 生成候选 AC（如 `cargo test --workspace`），由人确认后写入 TaskSpec，而不是自动生效。

### 4.6 技术栈探测

| 证据 | 建议 jobs |
|---|---|
| `Cargo.toml` | `cargo fmt --check`、`cargo clippy -D warnings`、`cargo test --workspace`、`cargo build --workspace` |
| `package.json` + lock | 按 lock 选 `pnpm/yarn/npm`；`lint`、`typecheck`、`test`、`build`（只取存在的 script） |
| `pyproject.toml` | `ruff check`、`mypy`/`pyright`、`pytest`（按配置） |
| `go.mod` | `gofmt -l`、`go vet ./...`、`go test ./...` |
| `Makefile` / `justfile` | 优先采纳已有的 `check`/`test`/`lint` target；不重复造 |
| `.github/workflows/*.yml` | 只提取可识别的命令作为**证据**，不强行解析 YAML；无法确定时给建议不写配置 |
| `rust-toolchain.toml` / `.nvmrc` / `.python-version` | 记录版本要求；doctor 检查当前版本是否匹配 |
| `Containerfile` / `compose.yaml` | 记录为环境证据；v1 不自动嵌套容器 |

探测不到时：只写注释模板，不猜命令。

### 4.7 环境与安全

- v1 在**当前环境**执行，不启动嵌套容器；容器内缺工具时明确报错，不静默跳过；
- 命令来自仓库配置，视为可信；仍要清空 provider secret、限制输出、设置超时；
- 默认不联网；需要联网的 job 显式 `network = true`；
- 写操作只允许在仓库内；不跟随 symlink 到仓库外；
- 不完全替换现有 CI：只补本地入口，GitHub Actions 仍可独立存在。

### 4.8 可行性

| 部分 | 可行性 | 说明 |
|---|---|---|
| 探测 + 生成 ci.toml | **高** | 规则明确，`--dry-run` 可先验证 |
| 顺序执行 + 日志预算 | **高** | 复用现有工具输出协议 |
| baseline 预检 | **中** | 老仓库红基线常见，默认应关闭 |
| 环境一致性 | **中** | 容器/版本漂移；v1 只检测不修复 |
| monorepo / 多栈 | **中** | 等 M5 工作区设计；v1 按根目录单栈 |
| 嵌套容器 | **低** | 当前 dev 已用 podman/docker，嵌套复杂；后置 |

结论：**先做探测 + `ci.toml` + 顺序 `loom ci run` + doctor；并行、缓存、容器化、affected-only 后置。**

---

## 5. 横向契约变更

四项都会新增文件/命令，但共享一组边界。按 `AGENTS.md` §4，任何事件/字段变更都要同步类型、schema、文档、测试。

| 方向 | 新增 artifact | 新增事件 | 配置 | CLI |
|---|---|---|---|---|
| churn | `logs/churn.json`（新 schema） | `task.churn`（仅干预时） | `[guardrails.churn]` | `loom churn stats` |
| questions | `outbox/questions.json`、`answers.json`、`assumptions.jsonl` | `task.needs_input`、`task.answered` | `[questions]` | `loom inbox`、`loom answer` |
| skills | `stack-profile.json`、`skills.catalog.json`、`skills.lock` | 无（skill 加载不是任务状态） | `[skills]` | `loom skill scan/recommend/add/show/verify/stats` |
| CI | `.loom/ci.toml`、`.loom/ci/<run>/` | 无（job 结果进 artifact/回执） | `[ci]` | `loom ci plan/run/doctor` |

共同约束：

- **状态仍由事件重放**：churn/questions 的 artifact 是证据，不是状态事实源；状态变化必须写事件。
- **不新增交互式命令**：`loom inbox/answer` 是批处理，不进入等待循环。
- **不改变退出码语义**：`0` 成功、`2` 需要人工/阻塞、`3` 预算/超时、`4` adapter 失败；`churn_guard` / `needs_input` 都是 `2`，用事件 `data.code` 区分。
- **上下文硬预算不变**：churn 摘要、问题、skill 正文都受 8KB/24KB/128KB 与 Context Compiler 约束。
- **不依赖真实模型即可测试**：前三项都能用 mock/native 脚本化 provider 覆盖。

---

## 6. 优先级与里程碑建议

| 顺序 | 方向 | 为什么先做 | 依赖 | 粗略工作量 |
|---|---|---|---|---|
| P0 | CI bootstrap | 让 verify/acceptance 可信；后续度量都靠它 | 无 | 3–5 人日 |
| P0 | question protocol | 修 `needs_input → task.failed` 的正确性缺陷 | 无 | 3–4 人日 |
| P1 | churn guard（native） | 直接减少可见浪费；指标可先于拦截 | 无 | 3–5 人日 |
| P1 | churn watchdog（外部 adapter） | 止损外部 agent 的 gross loop | 容器内需有 adapter 二进制 | 2–3 人日 |
| P2 | skills：scan + 本地 catalog + lock + 按需加载 | 结构先立起来，避免全局安装 | CI/任务集 | 5–8 人日 |
| P2 | skills：L2 评估与晋升 | 证明 skill 真的有用 | 真实模型 + 固定任务集 + 账单 | 后置 |
| P3 | 图案/embedding/联网目录 | 收益未证明 | — | 不做 |

注意：

- 当前没有并发/锁，P2 的“父子 agent 不等待”只定义协议，不实现并行调度；M4 再做。
- 真实 goose/OpenCode canary 仍受二进制与 API key 阻塞，见 [14](14-当前问题与卡点.md)。
- 不要为了 churn 指标去改 provider 请求格式；它只观察工具与 worktree。

---

## 7. 不做什么

- 不做全局 skill 安装，不做“装得越多越好”。
- 不做联网自动安装/自动执行 skill 脚本。
- 不做 embedding/向量库、MCP 跨项目知识库。
- 不做交互式提问等待；提问必须是文件 + 退出码。
- 不为每个问题调用一次 LLM 分类；先走确定性策略。
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
5. `ci.toml` 与现有 `Makefile`/CI 的优先级？
   - 倾向“采纳已有命令”，`ci.toml` 只做聚合与超时/日志包装。
6. baseline 预检默认开还是关？
   - 倾向默认关，项目稳定后再开，避免接入摩擦。
7. 父子 agent 的问题上浮是否需要独立“问题预算”？
   - 需要；否则回答可能吃掉任务预算，建议单独记账并设上限。

---

## 9. 最小验收清单（未来实现时）

- [ ] `loom ci plan --dry-run` 能在本仓库识别 Rust workspace，并给出 fmt/clippy/test/build 与证据。
- [ ] `loom init` 不覆盖已有 `ci.toml` / workflow；`--dry-run` 无写操作。
- [ ] 缺 `cargo`/`pnpm` 时 `loom ci doctor` 给出可行动报错，而不是静默跳过。
- [ ] `needs_input` 回执经 `loom verify` 后状态为 `needs_input`，**不产生 `task.failed`**，不增加升级计数。
- [ ] 脚本化 provider 调用 `ask` 工具 → `questions.json` → exit 2 → `loom inbox` 可见 → `loom answer` → 新 run 收到答案并通过 verify。
- [ ] native 对同一文件连续 3 次整文件重写触发 L2，第 5 次触发 L3；`churn.json` 指标可重算。
- [ ] `loom churn stats --by model` 能输出按模型的 `write_amplification` / `churn_ratio`（先用脚本化数据验证）。
- [ ] `loom skill scan` 输出的每个标签都有证据文件；无证据不出现。
- [ ] `loom skill add` 只写项目级目录与 `skills.lock`；未通过静态检查的 skill 拒绝安装。
- [ ] 加载 skill 时只注入索引行；正文经 `loom skill show` 按需读取，受 4KB/8KB 预算。
- [ ] 所有新增事件同步了 `loom-core` 类型、`schemas/event.v1.json`、`doc/03` §11.2 与至少一个测试。
