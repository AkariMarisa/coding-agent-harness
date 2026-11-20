# 20 Codex 会话基线：规模、token 账本与返工代理指标

> 语言：[English](20-codex-session-baseline.md) | **简体中文**

> **状态：实测记录（2026-11-20）。** 基线来自 615 个 Codex CLI rollout 会话（2026-04-01 → 2026-11-20）、两个生产仓库，全程**未经** Loom；节省数字是由 Loom 杠杆推导的算术投影，不是 Loom 的 A/B 实测。做该 A/B 的仪器仍缺失（见 [14](14-current-issues-and-blockers.zh-CN.md)）。与 [19](19-real-session-baseline.zh-CN.md) 互为姊妹基线——同一方法，不同 harness。

## 1. 测了什么

数据来源：Codex CLI 的 rollout 存储（`sessions/YYYY/MM/DD/rollout-*.jsonl` 加 `archived_sessions/`），拷贝到分析机器后于 2026-11-20 查询：**615 个文件、1.2GB、370,378 行 JSONL**。工作负载：Windows 工作站上的两个生产仓库——一个 IoT SaaS 单体仓（Java 后端 + Web 前端）与一个水文/计量 Java 服务——外加重复的模板型请求（如每月汇总 git 提交记录）。全程没有任何 `.loom` 参与。个人路径、仓库名与账号标识在本记录中有意省略。

复现方式：对 rollout 文件做九遍 Python 扫描（逐会话合计 → 聚合 → 工具输出尺寸 → 交互模式 → 返工代理）。这些脚本位于会话工作区（`/tmp/opencode/codex-analysis/pass1.py` … `pass9.py`），属临时产物；§2 记录了它们依赖的格式语义，重写脚本所需的信息全在其中。

| 维度 | 实测值 |
|---|---|
| 时间跨度 | 2026-04-01 → 2026-11-20，56 个活跃日 |
| 会话 | 615 个 rollout：230 主 + 385 子代理（按 `session_meta.thread_source`）；委派为单层（depth 1），子代理路径按任务命名 |
| 模型响应 | 37,403 次带逐响应用量记录 |
| token 覆盖 | 444 个会话有逐响应记录（2026-11，CLI 0.167–0.169）；其余 171 个中的 163 个（2026-04→06 + archived）有会话累计总量；8 个完全无 token 数据——**账本覆盖 607/615** |
| 计费输入 | **6,976.3M（6.98B）**——cached 6,722.4M（**96.4%**），真正新增 254.0M，混合重传比 **27.5×** |
| 分代拆分 | 新格式（444，2026-11）：输入 6,001.5M，cached 98.4%，重传 **60.9×**；旧格式（163，2026-04→06）：输入 974.8M，cached 84.1%，重传 6.3× |
| 输出 / 推理 | 28.75M / 10.15M |
| 单响应平均上下文 | 160,456 tokens（仅新格式——旧格式无逐响应记录） |
| 逐会话重传 | median 26.4×、mean 50.4×、p90 95.2×、max 784×（新格式） |
| 峰值上下文 | median 107,670 / p90 240,725 / max 462,129（607 会话）；上下文窗口 258,400 与 475,000 |
| 主 vs 子代理 | 主（59 个有仪表会话）：17,296 次响应（293/会话），峰值 median 225,251，重传 80×；子代理（385）：20,107 次（52/会话），峰值 median 109,824，重传 24.8× |
| 配额水位 | median 48%；29% 的会话触及 ≥70%，5% 触及 ≥90% |
| compaction | 134 次 / 53 个会话；替换记录 median 110KB、max 1.6MB；最重会话 19 次 |
| 会话时长 | median 0.2h、p90 1.7h、max 140.8h（新格式） |
| 工具调用 | 共 48,383：`exec` 25,254、`shell_command` 11,937、`send_message` 4,484、`wait_agent` 2,422、`apply_patch` 1,948、`sleep` 665、`spawn_agent` 360、`request_user_input`(+async) 299，其余零星 |
| 工具输出 | 48,392 条（两代格式族），合计 292.2M 字符 ≈ **73M tokens**（4 字符/token） |
| 人类消息 | 2,588 条实质消息（剔除上下文/图片载荷）：median 1 条/会话、p90 4、max 147 |

## 2. 格式语义与计数陷阱

值得记录，因为每一条都会静默污染账本：

1. **判别式在记录类型之间翻转。** `token_usage_record`、`session_meta`、`turn_context` 没有 `payload.type`——顶层 `type` 才是判别式；`token_count` 正相反（顶层 `event_msg`，判别式在 payload 里）。一套规则套两种记录会把所有 token 聚合清零。
2. **`token_usage_record.usage` 是逐响应增量**（已验证：Σ = `token_count.info.total_token_usage`）。旧格式没有这种记录，其总量来自每个会话最后一条 `total_token_usage` 快照。
3. **工具输出有两族记录。** `function_call_output`（旧代）与 `custom_tool_call_output`（新代——`exec`/`shell_command` 是 "custom" 工具）；`output` 可能是字符串**或**内容块列表。只数一族会把最新会话少算约 99%（最大会话：13 vs 1,462 条）。
4. **`custom_tool_call.status` 恒为 `completed`**——不是失败信号；失败只能从输出标记里 detect。
5. **171 个会话没有逐响应记录**（2026-04→06 + archived），其中 8 个连累计总量也没有。任何不带这条注记的"总计"都是错的。
6. **`compacted` 记录内嵌完整替换历史**（median 110KB）——文件大小 ≠ 会话大小。

## 3. 工作流契合度：规划覆盖什么、漏什么

| 会话中的观察 | 证据 | 规划条目 | 状态 |
|---|---|---|---|
| 手工组装的多代理扇出 | 360 次 `spawn_agent`、4,484 次 `send_message`（median 806 字符）、386 个子代理会话、单层、按任务命名路径 | `loom task` 派发 + ledger + `loom top` | 派发/ledger/top 已实现；inbox 待做 |
| 两侧都在轮询状态 | agent 侧：2,542 次 `wait_agent`+`list_agents`（占全部工具调用 5%）；人侧：约 8% 的消息是状态查询（关键词法） | `loom top` 只读 watcher | 已实现（2026-11-19） |
| 结构化问答已有原生形态 | 299 次 `request_user_input`，带 `header/id/question/options/label/(Recommended)` 与 `answers` 映射——与 `schemas/questions.v1.json` 同构；捕获 191 份回答 | `questions.json` 协议 | 已实现；**平价而非增益**——与 [19](19-real-session-baseline.zh-CN.md) 结论一致 |
| 上下文被原始工具输出主导 | 逐会话"超 8KB 线的输出 token"= 峰值上下文的 median **−25.7pp**（新代）/ −23.8pp（旧代）；全部输出的 19.3% 超 8KB；`exec`/`shell_command` median 约 2.5KB、p90 约 19KB；assistant 散文 median 仅 103 字符 | 工具 envelope（8KB/24KB/128KB + summary/address/preview，[10](10-tool-output-protocol-v0.1.zh-CN.md)） | 已实现 |
| 每请求固定开销 | 工具定义 median 每会话 41.7KB 随历史重发；系统/developer 消息 p90 21.5KB（指令文件内联） | skills 有界 packet（main/worker 4k、judgment 2k） | 已实现；adapter mention 未验证 |
| 长时间无人值守 | human 消息 median 1 条/会话（p90 4、max 147）；时长 median 0.2h 但 p90 1.7h、max 140.8h；65 次 `turn_aborted` | [18](18-agent-collaboration-workflow-proposal.zh-CN.md) 四个人工干预点 + attention inbox | 方法已提出；inbox 已设计、未实现 |
| 手工跨会话传状态 | 59 次续接式开场；至少一次明确的"把提示词粘贴到新会话"交接，携带手写接续摘要 | `.loom` 契约层 + 热片路径 | 契约层已实现；热片路径是已知缺口（[18 §5.5](18-agent-collaboration-workflow-proposal.zh-CN.md)） |
| compaction 翻腾 | 134 次 / 53 个会话；替换记录 median 110KB | 切片——每个工作项一个上下文 | 已实现；总量效应未测 |
| 图片依旧昂贵 | `view_image`：42 次、median 192KB、合计 26.8M 字符 | envelope 砍不了"第一眼" | 同 [19](19-real-session-baseline.zh-CN.md)：砍的是重读，不是首看 |

## 4. token 账本：6.98B 花在哪、杠杆能收回多少

账本即 `Σ(每次响应的上下文尺寸)`：37,403 × 160k 平均 ≈ 6.0B（新代）加 0.97B 累计（旧代）。cached 96.4% 意味着**成本与配额跟着上下文尺寸走，而不是那 254M 新内容**——每多一个上下文 token 就被重发一次，直到 compaction。

| 杠杆 | 在这份数据上的机制 | 证据等级 |
|---|---|---|
| 工具 envelope | 48,392 条输出中 19.3% 超 8KB 默认线（9.2% 超 16KB）；截到 8KB 砍掉输出体量的 **53.3%**；超 8KB 线的输出占峰值上下文 median **−24~26pp**（两代一致） | **对日志尺寸的算术推导**（48,392 条输出）；假设 summary+address+preview 不损任务成功率、无额外重读——均未验证 |
| 步级投影 | 历史增长时只留首行 + 引用，不带原始历史 | 夹具实测：单步 input −46~55%（[16 §8.8](16-runtime-governance-adoption-assessment.zh-CN.md)，n=1）；整体设计目标 −55~65%（[19 §3](19-real-session-baseline.zh-CN.md)）——**envelope 是它的子集，不叠加** |
| 切片 | 每个工作项一个新上下文；134 次 compaction 变 0，因为历史从不跨工作项累积 | 已实现；总量效应未测 |

**envelope 算术（不是实测）：** 中位会话上下文缩小约 24~26%；按此比例作用于实测的 6.98B 计费输入，整段历史约 **−1.7B**。假设列出来以便证伪（与 [19 §3](19-real-session-baseline.zh-CN.md) 对齐）：

1. 工作负载与模型不变，只改上下文生命周期。
2. 图片不截——杠杆砍的是重读不是首看；42 次 `view_image` 照旧昂贵。
3. 2,588 条人类消息照旧；交互性不是目标。
4. 不计重读抵消：spill 到 artifact 后再读花的是有界调用、可能增加调用数——净额未测。
5. 聚合数字在 native-vs-adapter A/B 跑通前未经验证（[14](14-current-issues-and-blockers.zh-CN.md)）；本记录给该 A/B 提供第二个待打败的基线。

**不会缩小的：** 重传倍数本身（无状态 API 的属性）、图片轮次、人类轮次。**直接兑现的：** 29% 的有仪表会话触及过 ≥70% 配额水位——上下文削减一比一换成配额余量。

## 5. 返工与准确性：代理基线，不是地面真值

| 代理指标 | 实测值 | 说明 |
|---|---|---|
| 带失败标记的输出 | **3.0%**（1,431/48,392） | 错误文本 / 非零退出标记 |
| 连续重复命令 | **2.45%**（1,187/48,383） | 紧邻的立即重试 |
| git 回滚命令 | 48,383 中 **1** 次（`restore`/`reset --hard`/`revert`） | 返工在会话内原地改对，从不回滚 |
| `turn_aborted` | 65 | 人按了 Esc |
| 纠错类人类消息 | **≤18.2%**（472/2,588） | 关键词上界；采样目测：真正的"agent 做错→人指出"只占少数——这个负载里 bug 报告**本身就是任务**，另有用户自我纠错、假设句与误报 |

**地面真值注记：** 真返工（同一文件反复改）存在于两个仓库的 git 历史里，不在分析机器上——单靠会话文本只能拿到代理指标，且 18.2% 是高估。这正是 Loom 的步级归因 + `logs/churn.json` + `loom churn stats` 在真实工作跑过后能精确测量的东西。

各机制的作用方向（假设，非实测）：

| 机制 | 命中 | 预期 |
|---|---|---|
| no-progress 检测（已实现） | 重复/循环类（2.45%） | 直接命中，量级小 |
| patch-first / 写保护（已实现） | 覆盖自己的未提交工作 | 低频高危；观测到的会话内回滚为 0——要么罕见，要么没有保护就救不回 |
| envelope + 投影 | compaction 后漂移（134 次）、长跑漂移（max 140.8h 会话） | 方向合理；反向风险：截断 → 重读/漏细节，未测 |
| `questions.json`（已实现） | 范围理解错 | **平价**：codex 原生协议（299 次）已与 schema 同构；Loom 的增量是持久化 + 编排器可见性 |
| skills（已实现） | 跳步骤 | 未验证（adapter canary 待 binary） |

本记录不给 Loom 做任何准确性背书：Loom 侧还没有真实模型运行。

## 6. 这改变了什么

1. **envelope 成为真实负载上证据最强的 token 杠杆。** [19](19-real-session-baseline.zh-CN.md) 里投影领先；这里原始工具输出主导上下文（截断前占峰值 median 55.7%/58.1%），且 19.3% 的调用越过 8KB 线。
2. **优先级再确认：** attention inbox + 热片路径（§3 第 1/2/6/7 行都指向它）。手工粘贴提示词跨会话是 [18 §5.5](18-agent-collaboration-workflow-proposal.zh-CN.md) 缺口的第三个实例。
3. **questions 平价在第二个 harness 上再确认**——结论不变：直连会话里原生问答够用；Loom 在这里的價值是持久化与编排器可见性，不是模式本身。
4. **本账本给 A/B 提供第二个待打败的基线**（[14](14-current-issues-and-blockers.zh-CN.md)）——§5 的返工代理给它提供了质量轴，且这一轴第一次能在 Loom 侧经 churn stats 精确测量。
