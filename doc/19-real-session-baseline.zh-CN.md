# 19 真实会话基线：工作流契合度与 token 账本

> 语言：[English](19-real-session-baseline.md) | **简体中文**

> **状态：实测记录（2026-11-19）。** 基线来自 `tumblr-downloader` 上一个**未经** Loom 的 10 天 OpenCode 会话；节省数字是由 Loom 已实测杠杆推导的算术投影，不是 Loom 的 A/B 实测。做该 A/B 的仪器仍缺失（见 [14](14-current-issues-and-blockers.zh-CN.md)）。

## 1. 测了什么

一个真实仓库（`tumblr-downloader`）以单一会话不断膨胀的方式跑了 10 天：分支审查、修 bug、两台手机上的实机测试、GitHub issues、F-Droid 发版，最后是 UI 设计探索。除第一天外，全程没有 `.loom` 派发参与。

数据来源：本地 OpenCode 存储（`session_v2` 与 `session_message` 表），会话 `ses_e45599173ffeI9lc6ycANjbIKC`，2026-11-19 查询。复现：

```bash
sqlite3 ~/.local/share/opencode/opencode.db \
  "SELECT tokens_input, tokens_output, tokens_cache_read FROM session_v2 \
   WHERE id='ses_e45599173ffeI9lc6ycANjbIKC'"
```

| 维度 | 实测值 |
|---|---|
| 跨度 | 2026-11-08 18:13 → 2026-11-19 00:02（10 天） |
| 模型调用 | 2,739 条 assistant 消息，95 条用户消息 |
| Token | input 51.6M、output 1.05M、reasoning 1.68M、**cache read 949M**、cost $0（配额制模型） |
| 单次调用平均上下文 | **约 346k tokens**（949M ÷ 2,739），峰值约 900k |
| 上下文重置 | 7 次 compaction（5 次自动、2 次手动） |
| 工具调用 | 共 3,117 次：shell 2,248（72%）、edit 492、read 276、write 47、skill 14、question 10、grep 11，其余 20 |
| read 中的截图 | 276 次里 201 次，**26.8MB base64** 累积在上下文中 |
| 等待中的 shell 调用 | 2,248 次里 975 次（43%）含 `sleep`/`wait`/`guard`（adb 循环、gradle、CI 轮询） |
| 每日 cache read 峰值 | 11-14：228M、11-15：203M、11-17：175M——恰是三个实机测试日，占 10 天总量的 **64%** |
| 最长无人过问段 | 274 条 assistant 消息、400 分钟（中位数：每个用户轮次 10 条消息） |
| idle → 推一把 | 81 次 idle 事件；每一次 `继续`/`proceed` 都是人唤醒空闲会话（间隔中位数 4.3 分钟，p90 650 分钟） |

11-11 的两个子代理会话（bug 调查，各约 63k input）未计入上表，它们便宜是因为短命。

## 2. 工作流契合度：规划覆盖什么、漏了什么

| 会话中的实际表现 | 证据 | 规划条目 | 状态 |
|---|---|---|---|
| 状态只能靠问模型 | "测试用例还剩下多少"/"待做清单"/"todo 还有什么"——3 次显式发问；`test-plan.md` 重读 95 次；`DownloadStateManager.kt` 读了 13 次 | 事件日志回放 + `loom board` | board 已实现；`loom top` 待做 |
| 协调靠手工汇总 | 2 次显式请求："汇总一下我需要怎么配合"与"先做不需要我配合的，完成后告诉我需要提供哪些" | Attention inbox（[15](15-agent-runtime-governance-and-engineering.zh-CN.md) §2.2） | 已设计、未实现 |
| 问答本就结构化 | 10 次 `question` 工具调用，答案直接落上下文，零痛点 | `questions.json` 协议（2026-11-18 落地） | 服务于退出码为 2 的进程级 adapter；**对直连会话无增量** |
| 上下文膨胀到疼、再失忆 | 平均 346k、峰值约 900k、7 次 compaction 重建；压缩后的重读喂大 raw input | 有界 packet（按角色 8k/24k/30k）+ 每个工作项一个上下文 | 已实现（`loom-context`、`loom-native`） |
| 旧工具结果每轮重现 | 26.8MB 截图 + 2.23MB 冗余重读，一直背到 compaction | 步进投影 | 已实现；16–17KB 历史时单步 input 实测降 46%~55%（[16](16-runtime-governance-adoption-assessment.zh-CN.md) §8.8，n=1） |
| 长跑无人过问 | 274 条消息的长段；idle 后 5 次推一把 | doc/18 四个介入点 + inbox 唤醒 | 方法已提案；idle 唤醒本身属 harness 侧行为 |
| 设备驱动样板烧轮次 | 400+ 次 adb guard/tap/dump 序列，占 shell 调用的 43% | 脚本化验收（该仓库自己的测试计划已写明"device cases are scripted with adb"） | 部分覆盖：确定性一半走脚本，判断留给模型 |

唯一一行 Loom 没有增量的是**问答**：OpenCode 原生工具在会话内已经做得很好。Loom 的 `questions.json` 是为进程级 adapter（退出码 2 + 文件）存在的，不是为直连对话。

## 3. Token 账本：949M 花在哪、杠杆能收回多少

949M cache read 就是 `Σ(每次调用的上下文尺寸)`：2,739 次 × 平均约 346k。三把刀砍这个乘积，证据等级各不相同：

| 杠杆 | 机制 | 证据等级 |
|---|---|---|
| 有界 packet | 每次运行从 ≤30k（main）/24k（worker）/8k（judgment）起步，而不是 10 天历史 | 已实现；预算是代码（`compiler.rs` 的 `budget()`），尚无规模级实测 |
| 切片 | 每个工作项一个全新上下文；7 次 compaction 重建变 0，因为历史从不跨项累积 | 已实现（M4 已验证 2 并发 worker）；聚合效果未实测 |
| 投影 | 历史增长时旧工具结果压成首行 + 引用 | 已实测：16–17KB 夹具上单步 input 降 46%~55%（[16](16-runtime-governance-adoption-assessment.zh-CN.md) §8.8，n=1，官方 `deepseek-flash`） |

**投影（杠杆上的算术，不是测量）：** 若同样的 2,739 次调用在平均 100–150k 而非 346k 的上下文里跑，cache read 落在 **约 300–400M，即 −55%~65%**。raw input 降得更多（compaction 重建变少），调用次数也随之一降（3 次状态查询由 `loom board` 回答、冗余重读消失、设备样板脚本化）。账本另一侧的交叉验证：本仓库唯一一个 Loom 驱动的真实任务（#24，[14](14-current-issues-and-blockers.zh-CN.md) §9）8 次尝试、每步 input 12–31k，比本会话 346k 的平均值低一个数量级。

投影依赖的假设，写明以便被证伪：

1. 工作负载与模型不变，只改上下文生命周期；
2. 模型必须看一眼的截图不砍——杠杆砍的是**重读**，不是首见。176 个带图轮次依然昂贵；
3. 95 个人类轮次原样保留，交互性不是目标；
4. 聚合数字在 native vs adapter A/B 跑通之前未经验证（[14](14-current-issues-and-blockers.zh-CN.md)）；本记录给那次 A/B 留了一个要打败的基线。

这些数字为什么不止关乎 token：本仓库自己的配额史显示，单个任务就烧掉 1.52M tokens ≈ 2M 配额的 76%（[14](14-current-issues-and-blockers.zh-CN.md) §9）。上下文重读砍掉 55%~65%，差别是"一次配额补充做一个任务"还是"能干一个工作日"。

## 4. 暴露的缺口

**反应式人在环工作没有入口。** 会话的主导形状：人在设备上测试、报告坏了什么、agent 修、下一份报告几小时后才来——一次一项，由实时旁观的人验证。技术路径是存在的（`loom task add` → `dispatch` → `verify` → `merge`，#24 用的正是它），但 [18](18-agent-collaboration-workflow-proposal.zh-CN.md) 的方法描述的是批次，单个到来的报告没有一等入口；而"观察-修-再观察"的紧密循环根本不适配 dispatch→verify→merge。观察到的后果：`.loom/` 于 2026-11-08 15:35 初始化，最后一条事件是 17:08 的 #24 合并，会话 18:13 开始，此后 10 天（2,739 次调用、949M cache read）再未碰过它。已记为 [18](18-agent-collaboration-workflow-proposal.zh-CN.md) §5.5 的已知缺口。

**等待外部系统是刻意无原语。** CI pipeline、F-Droid 构建、应用安装靠 shell 轮询（占那 43% 等待调用的一部分）或靠人回来问。Loom 没有外部事件原语，v1 也不该长一个；inbox 只覆盖 Loom 侧的升级。

**直连会话一把杠杆都继承不到。** 上表所有内容都活在 `loom dispatch` 后面。OpenCode 对话除非把工作路由经 Loom，否则一点都拿不到——或者把投影思路移植到宿主 harness，那不在本项目路线图内。

## 5. 这改变了什么

实测之后的优先级：**attention inbox 第一**（一个会话里人两次伸手要它，且是唯一"已设计、未动过"的一行），其次 **`loom top`**（消掉状态查询轮次），再次是反应式工作的**热片路径**（[18](18-agent-collaboration-workflow-proposal.zh-CN.md) §5.5）。token 杠杆本身都已实现；缺的是（a）让真实工作大规模流经它们，（b）把 §3 的投影变成实测的那次 A/B。
