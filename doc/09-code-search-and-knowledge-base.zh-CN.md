# 09 · 代码检索与知识库（评估笔记）

> 语言：[English](09-code-search-and-knowledge-base.md) | **简体中文**

> 状态：**讨论稿 / 评估笔记**，不是实施计划。  
> 目的：回答一个问题——本地代码 RAG / 知识库值不值得进 Loom，以及怎么进才不会变成负收益。  
> 结论会回写到 [08-design-roadmap-v0.2.zh-CN.md](08-design-roadmap-v0.2.zh-CN.md) 的待定项；M0 不因本文增加实现范围。

> **2026-11-04 决策更新**
>
> 1. **v1 不引入 embedding 模型，也不引入向量库。** 客户机性能与分发体积优先；检索先用确定性方案。
> 2. **语义检索延后**，只有同时满足“模型 ≤ 约 100MB、纯 CPU、查询 < 100ms、可选下载、A/B 有明确收益”才重新评估；vLLM 属服务端方案，不随 CLI 分发。
> 3. **知识库默认仅本项目**，跨项目共享由用户显式开启。
> 4. **工具端 I/O 预算优先于网关压缩**：能在工具边界裁掉的 token，不留给 Headroom。

---

## 1. 结论先行

1. **不建议把“代码向量库”作为主检索路径。** 代码查询的主体是精确符号、调用链、类型和路径，不是“语义相似段落”；向量检索在这类查询上又贵又不准。
2. **把问题拆成两件事**，它们的更新策略完全不同：
   - **Code Index（代码索引）**：当前工作树的派生缓存，必须新鲜，只返回**地址**（`path` + 行号 + 符号），确定性、可删除重建。
   - **Knowledge Base（知识库）**：项目记忆（收据、决策、失败尝试、ADR、issue），append-only，绑定 commit，**不需要实时**。
3. **代码检索的目标不是“找到相似代码”，而是“用最少 token 让 agent 定位到正确的定义/调用点”。** 所以第一优先级是结构化搜索（trigram、符号图、repo map），不是 embedding。
4. **新鲜度不靠“实时重建”，靠“内容寻址 + 增量失效 + 读时校验”。** 索引允许短暂落后，但任何返回给模型的结果必须能证明对应当前文件内容。
5. **语义检索真正的高 ROI 区域是“仓库里没有的东西”**：为什么这么改、上次怎么失败、验收标准怎么来的。这些正好是 Loom 已经在生成的 receipts / decisions / events。
6. **最先做的不是检索，而是工具端预算。** 工具输出从“值”改成“引用 + 摘要”，可以直接打击 `json_bloat` 与 `reread`，且不依赖任何索引或模型。

一句话：**代码用索引，历史用知识库；索引给地址，编译器决定读多少；工具端先减肥，网关再兜底。**

---

## 2. 为什么“代码 RAG”在开发中容易看起来有用、实际不省

| 失败模式 | 机制 | 后果 |
|---|---|---|
| **切块破坏结构** | 函数/类型被切断，丢失 import、泛型、调用关系 | 模型看到片段也不知道能不能改、谁会受影响 |
| **精确性错配** | 代码查询多为 identifier / 类型 / 调用链；向量擅长模糊相似 | “找 X 的所有调用者”几乎必错，还得回到 grep |
| **新鲜度陷阱** | 索引落后于 working tree，命中已删除/改名的代码 | 比没有索引更糟：模型对错误上下文产生信心 |
| **评测自欺** | 用“召回率/相似度”评估，而不是“任务是否更快通过 verify” | 指标好看，TPVT/WPT 不降 |
| **成本转移** | 建索引、算 embedding、chunk stuffing 都要钱 | 每轮 context 变大，抵消“少读文件”的收益 |
| **分发与隐私** | 本地 embedding 模型、向量库、版本漂移 | 单二进制变重，离线/内网场景复杂化 |

近期一些评测的共同倾向是：**在真实仓库里，agentic search（grep/read + 迭代缩小范围）通常优于纯向量检索**；向量检索的优势更多在文档、issue、自然语言知识，而不是精确代码定位。但这个结论依赖任务类型和仓库规模，**不能直接照搬，必须用 Loom 自己的任务集复现**。

---

## 3. 但 agentic grep 也不是免费的

否定“代码 RAG 做主路径”，不等于现在的 grep 方式就够好。它的成本在于：

- **每一轮搜索都是一次 LLM 调用**，历史还要重发；定位阶段可能占总 token 的很大一块（需实测）。
- **大仓命中噪声大**：`rg "config"` 可能几千条，agent 要花多轮缩小范围。
- **缺少全局视图**：没有符号图/repo map 时，agent 容易只改到症状，漏掉真正的定义点或所有调用方。
- **工具结果按值回贴**：原始 `rg --json` 或整文件输出会直接放大 `json_bloat`（前序实测最大浪费源，60.6M）。

所以机会不是“用 RAG 替代 grep”，而是：**给搜索一个更好的索引和排序，并把结果压缩成地址。**

---

## 4. 关键拆分：Code Index 与 Knowledge Base

| 维度 | Code Index | Knowledge Base |
|---|---|---|
| 数据 | 当前 working tree / commit 的源码 | 收据、决策、失败、ADR、issue、commit message |
| 变化频率 | 每次编辑 | append-only |
| 新鲜度要求 | **强**（查询时校验） | **弱**（按 commit/日期版本化） |
| 索引方式 | trigram、tree-sitter 符号表、import/call graph | BM25/FTS5 + 元数据过滤（v1 不用 embedding） |
| 失效策略 | git blob hash + 增量重建 | 不删除，追加新版本 |
| 典型失败 | stale hit | 召回噪声、过时结论 |
| 主要价值 | 降低 locate token / 减少无效读 | 避免重复踩坑、重复实现、重复讨论 |
| 结论 | **确定性优先，不靠 embedding** | **语义检索的主战场，但 v1 先用 BM25** |

> 源码本来就在仓库里，grep 得到；真正“不在仓库里”的是**为什么**和**上次发生了什么**。  
> 因此如果只做一件事，先做知识库；如果做代码检索，先做符号索引。

---

## 5. 可借鉴的现成方案

| 方案 | 机制 | 对 Loom 的用法 | 注意 |
|---|---|---|---|
| **ripgrep / git grep** | 正则 + 路径过滤 | R0 默认，永远保留 | 输出必须结构化、限量 |
| **zoekt** | trigram 倒排，代码专用 | 大仓的快速候选召回 | 需额外进程/索引目录 |
| **tree-sitter tags** | AST 提取定义/引用 | R1 符号表，增量解析 | 多语言 grammar 维护 |
| **Aider repo map** | 符号图 + PageRank + 预算内选取 | R1 全局视图/排序 | 只借鉴算法，不依赖 Aider |
| **LSP / SCIP / LSIF** | 精确跳转、调用层级 | M4 可选，精确但重 | 启动/内存成本高 |
| **BM25（FTS5/Tantivy）** | 关键词相关性 | R2 文档/历史检索 | 对代码标识符也有效 |
| **本地 embedding** | 语义相似 | **v1 不用**；未来仅作可选插件 | 体积、性能、版本、隐私 |

**排序建议**：`精确符号 > trigram > BM25 > 向量`。向量只在混合召回里做补充，不单独决定结果。

---

## 6. 分层检索：R0–R3

| 层 | 内容 | 新鲜度 | 何时启用 | 维护成本 |
|---|---|---|---|---|
| **R0** | `git ls-files`、`rg`、`git grep`、路径/扩展名过滤 | 实时 | 永远 | 近零 |
| **R1** | tree-sitter 符号表、import graph、repo map、PageRank 排序 | 查询时校验 + 增量 | M1，若 locate 成本高 | 中 |
| **R2** | BM25/FTS5 + 元数据过滤（路径/语言/commit）；代码仅作 fallback | 按 commit 版本化 | M2，文档/历史优先 | 中 |
| **R3** | issue/wiki/MCP 外部知识 | 松 | M3+ | 取决于源 |

规则：

1. **R0/R1 只返回地址**，不返回大段内容。
2. **R2 返回候选后必须解析成地址**，并用内容哈希校验后才可进上下文。
3. 任何一层失败都**退化为 R0**，不能阻塞派发。
4. 不让模型直接看到原始 `rg --json`；统一走 `loom search` 的紧凑协议。

**v1 的 R2 不含 embedding**：BM25 + 标识符精确匹配 + 标签/时间过滤，已经能覆盖“上次哪个任务改过这里”“这个决定为什么做的”这类查询，而且零模型、零 GPU。

---

## 7. “地址而非内容”：检索结果协议

```json
{
  "path": "src/auth/session.rs",
  "blob": "9f3c1a...",
  "range": [120, 168],
  "symbol": "Session::refresh",
  "kind": "definition",
  "score": 0.87,
  "why": "caller of rotate_token; touched by T-003",
  "est_tokens": 740
}
```

约定：

- **`blob` 是内容哈希**（`git hash-object` 或等价），不是 mtime。
- **`est_tokens` 由索引估算**，Context Compiler 据此决定展开哪些、展开多少。
- **`why` 可审计**：既方便人排查，也方便评估 precision。
- **seen-set 去重**：同一 run 内已发送过的 `(blob, range)` 不再重发，只发 diff 或一行摘要。
- **读时校验**：展开前重新读文件并比对哈希；不匹配则丢弃、重查，并记 `stale_hit` 事件。
- **默认不展开**：先给地址列表（几十 token），模型需要时再按 range 读。

这套协议同时解决三个老问题：`json_bloat`、`reread`、跨任务重复读。

---

## 8. 工具端预算：比压缩更早的一层

> 这一节是本次讨论新增的优先级最高的部分。  
> 前序实测里，Headroom 压缩只省了 **0.70%**，而 `json_bloat` 是 **60.6M**、`reread` 是 **5.67M**。  
> 结论：**先控制工具 I/O，再谈压缩和检索。**  
> 具体字段、限额与 telemetry 见 [10-tool-output-protocol-v0.1.zh-CN.md](10-tool-output-protocol-v0.1.zh-CN.md)。

### 8.1 原则

1. **能在工具边界裁掉的，不要留给网关或模型。**
2. **工具输出是引用，不是值。** 默认返回“摘要 + 地址 + artifact 路径”，全文落盘。
3. **预算硬上限。** 每个工具、每次调用、每个 run 都有字节/token 上限，模型不能通过参数绕过。
4. **确定性裁剪。** 同样的输入产生同样的截断结果，便于缓存和复现。
5. **网关是兜底。** Headroom 继续 `mode=cache`，只处理模型层的重复前缀；工具端先把 bytes 降下来。

### 8.2 输出协议（示意）

```json
{
  "ok": true,
  "summary": "12 matches in 4 files",
  "ref": ".loom/runs/R-001/artifacts/search-001.txt",
  "items": [
    {"path": "src/auth/session.rs", "range": [120, 128], "preview": "fn refresh(...)"}
  ],
  "truncated": true,
  "next": "loom search --cursor eyJvZmZzZXQiOjUwfQ",
  "bytes": 1832
}
```

规则：

- **默认上限（示例，落地时校准）**：
  - `search`：50 条命中 / 8KB；
  - `read`：200 行 / 8KB，超出必须按 range 续读；
  - `tree`：深度 3 / 200 项；
  - `git diff`：500 行，超出回 stat + artifact；
  - `test/verify`：尾部 200 行 + 完整日志引用。
- **超限即落盘**：artifact 放 `.loom/runs/<run>/artifacts/`，结果里只回 `ref` + 顶部若干条。
- **去重**：同一 `(path, blob, range)` 本 run 已发过 → 回 `{"unchanged": true, "ref": ...}`，不重发内容。
- **不返回原始 JSON/HTML/ANSI/进度条**；adapter 负责归一化。
- **错误只回**：exit code + 一行原因 + 可选 `ref`；不回 stack trace。
- **字段顺序稳定**，便于前缀缓存。

### 8.3 输入协议

- 工具 schema 自带默认值和硬上限：`max_results`、`max_bytes`、`context_lines`、`path_glob`。
- **拒绝昂贵调用**：无路径的全仓 grep、整文件 read、无深度限制的递归列目录 → 自动降级为受限调用，并在结果里说明。
- 模型的 tool call 参数先规范化再执行；预算不能靠 prompt 约束，必须由 wrapper 强制执行。
- **单 run 总工具输出预算**（示例 64KB / 16k tokens）；超预算后只允许 `read --range`、`verify` 日志 tail 和必要的写操作。

### 8.4 与 Headroom 的关系

| 层 | 负责 | 不负责 |
|---|---|---|
| 工具 wrapper | 值 → 引用、截断、去重、结构化 | 模型层重复、跨轮缓存 |
| Context Compiler | 选哪些地址进 prompt、预算分配 | 工具内部的大结果 |
| Headroom | 缓存、可压缩的重复内容 | 工具输出治理 |

不重叠，也不互相替代。工具端省下来的是**真实 bytes**；网关省下来的是**重复发送**。

### 8.5 为什么先做这个

- 不需要索引、不需要模型、不需要新依赖；
- 直接打击 `json_bloat` 与 `reread`；
- 对任何 adapter 都有效（goose / OpenCode / native 走同一 wrapper）；
- 是 Context Compiler 的前置：先把工具结果变小，再谈编译。

---

## 9. 新鲜度：不追实时，追“可证明一致”

用户担心的核心是“源码一直在变，索引是不是要实时更新”。答案是：**不需要实时全量重建，需要可证明一致。**

1. **索引不是事实源。** 事实源是 git object + working tree；索引永远可丢弃、可重建。
2. **内容寻址。** cache key 用 `path + blob_sha`，不用 mtime；rename 用 `git diff -M` 识别。
3. **读时校验。** 命中后读实际文件比对哈希；不一致就失效。这样即使索引落后几秒，也不会把过期内容喂给模型。
4. **增量更新。**
   - 派发前：`git status/diff` 得到 dirty set；
   - worker 结束后：`git diff --name-status` 只重建变更文件；
   - 大仓/daemon：后台 debounce，按需触发，不阻塞派发。
5. **版本化。** index schema、parser 都带版本；不匹配就重建，不混用。
6. **边界。** 尊重 `.gitignore` + `.loomignore`；跳过 vendor/generated/build/二进制；单文件和总大小设上限。
7. **多 worktree。** 优先按 blob 内容寻址，天然跨 worktree 共享；按 worktree 区分的只有路径映射。
8. **一致性窗口可接受。** 允许索引短暂落后，但**返回给模型的每个地址都必须经过读时校验**。

> 结论：把“实时索引”换成“内容寻址 + 增量 + 读时校验”，复杂度从“持续同步”降为“按需失效”。

---

## 10. 和 Loom 现有设计的接口

| 位置 | 接入方式 |
|---|---|
| 工具 wrapper | 所有 adapter 的工具调用走同一层：限额、结构化、落盘、去重 |
| `context.compile()` | 用 TaskSpec 调 retrieval，拿地址列表；由 Compiler 决定展开与预算 |
| worker 工具面 | 提供 `loom search`（结构化、限量、返回地址），替代裸 grep；可选 `loom read --range` |
| adapter 归一化 | 把 agent 的搜索结果统一成上面的 hit 结构，过滤原始 JSON |
| receipt | 记录 `retrieval` 与 `tool_io` 统计（queries、hits、stale、bytes、truncated），进入账本 |
| `.loom/index/` | gitignored、可删除、可重建；不进快照，不进 worktree |
| `loom doctor` | 校验 index 版本/哈希/孤儿条目；失败只告警，不阻塞 |

关键点：**检索是 Context Compiler 的一个子模块，不是独立产品。** 它只负责“候选 + 排序 + 地址”，不负责“把内容拼进 prompt”。

---

## 11. 路线：什么时候做什么

| 阶段 | 做什么 | 不做什么 | 退出标准 |
|---|---|---|---|
| **M0** | R0（`rg` + 按 range 读）+ **工具输出预算/落盘/去重**；记录 retrieval trace | 不建索引、不引入 embedding/向量库 | 拿到 locate 成本与工具 bloat 基线 |
| **M1** | 若 trace 显示定位占比高：加 R1 符号表 + repo map；`loom search` 紧凑输出 | 不做语义检索 | locate token / tool calls 明显下降，verify 不退化 |
| **M2** | 若失败集中在“找不到相关实现/重复实现”：加 R2（**仅 BM25/FTS5** + 元数据，知识库优先） | 不让语义相似度决定主结果 | A/B 显示 TPVT 或 first-pass 提升 |
| **M3** | 跨项目知识库（用户显式开启）、issue/ADR、历史收据检索 | 不索引全部 git 历史 | 复用率/重复踩坑下降 |
| **M4** | 可选 LSP/SCIP/zoekt 级精确索引 | 不首期自研 | 大仓场景有明确收益 |

**M0 明确不做**：embedding、向量库、LLM 生成索引摘要、文件监听 daemon。

---

## 12. 怎么证明它有效（A/B）

| 指标 | 定义 | 说明 |
|---|---|---|
| `locate_cost` | 任务开始到首次读到“正确文件”的 tool calls + tokens | 最直接 |
| `tool_output_ratio` | 工具输出 tokens / 总输入 tokens | 先看这个 |
| `spill_rate` | 写入 artifact 的调用比例 | 太高说明默认限额不合适 |
| `retrieval_precision@k` | 返回地址中被实际读取且最终用于修改的比例 | 不是召回率 |
| `wasted_reads` | 读了但没用于修改的文件数/字节 | 反例指标 |
| `stale_hits` | 哈希不匹配次数 | 新鲜度健康度 |
| `TPVT` / `WPT` | 计费 token、墙钟 / 通过任务 | 最终指标 |

实验设计：

- 20–30 个真实任务，同一模型、同一 prompt、同一 verify；
- 先做“工具预算开/关”的 A/B，再做 R0 vs R0+R1；
- 以 **verify 通过** 为硬指标，token/墙钟为成本指标；
- 记录每次定位的 trace，避免只看平均值。

**反模式**：用“检索命中率”单独证明有效；用 LLM 生成摘要/嵌入再入库（贵、漂移、不可复现）。

---

## 13. 反模式清单

1. 把整文件切成固定 chunk 存向量，查询后拼 10 个 chunk 塞 context。
2. 用 LLM 给每个文件生成摘要再索引（成本高、会漂移、不可复现）。
3. 让索引成为必经路径；索引挂了任务就跑不了。
4. 不校验哈希就把索引结果当事实。
5. 在 M0 就上向量库/embedding 服务。
6. 用语义相似度替代精确符号查询。
7. 索引全部 git 历史/blame（体积爆炸）；只索引收据/决策摘要。
8. 让 worker 直接看原始 `rg --json`。
9. 把工具限额交给 prompt 约束，而不是 wrapper 强制执行。

---

## 14. 已定与待定

### 14.1 已定

| 项 | 决定 | 理由 |
|---|---|---|
| embedding / 向量库 | **v1 不引入** | 客户机性能、分发体积、离线与隐私 |
| 语义检索 | **延后，可选插件** | 需同时满足体积/CPU/延迟/A-B 门槛 |
| 知识库共享 | **默认仅本项目**，用户可显式开启跨项目 | 避免默认泄露与索引膨胀 |
| 优先级 | **工具端 I/O 预算 > 检索索引 > 网关压缩** | 先砍真实 bytes，再谈压缩率 |
| R2 实现 | BM25/FTS5 + 元数据过滤 | 零模型、可解释、可复现 |

### 14.2 待定/需 spike

1. **目标代码仓库的规模与语言**（这里指 Loom 运行其上的项目，不是知识库）：单仓/多仓、monorepo、多语言，决定 R1 parser 覆盖与 zoekt 是否值得。
2. **`.loom/index/` 体积上限**：几百 MB 是否可接受，还是只放临时目录。
3. **`loom search` 输出协议**：建议自定；复用 `rg --json` 会把噪声带进上下文。
4. **未来 embedding 的门槛**：模型 ≤ 约 100MB、纯 CPU、查询 < 100ms、可选下载（校验和）、A/B 有收益；运行时优先 llama.cpp / ONNX Runtime / candle，vLLM 不随 CLI 分发。
5. **Windows 是否在支持范围**：影响文件监听、长路径、进程组。

---

## 15. 一句话总结

**代码检索要做的是“更便宜地定位”，不是“更聪明地相似”；索引必须是可丢弃、内容寻址、读时校验的派生缓存；而第一刀应该砍在工具 I/O 上，而不是网关压缩上。**
