# 10 · 工具输出协议 v0.1

> 语言：[English](10-tool-output-protocol-v0.1.md) | **简体中文**

> 状态：**草案（2026-11-04）**，基于已确认的四项决策：  
> 1. `items` 用短键，人类可读格式由工具代码转换，不占模型 token；  
> 2. 三级预算先用 **8KB / 24KB / 128KB**，带 telemetry，边用边调；  
> 3. M0 **不做 MCP**；与 Headroom 用 proxy/wrap/SDK 对接；  
> 4. `summary` 默认中文，可切英文/日文。  
>
> 配套 Schema：[`schemas/tool-result.v1.json`](../schemas/tool-result.v1.json)。  
> 相关文档：[08-design-roadmap-v0.2.zh-CN.md](08-design-roadmap-v0.2.zh-CN.md) §6、[09-code-search-and-knowledge-base.zh-CN.md](09-code-search-and-knowledge-base.zh-CN.md) §8。

---

## 1. 目标与非目标

### 1.1 目标

1. **工具端先减肥**：进入模型上下文的是“摘要 + 地址 + 预览”，不是完整结果。
2. **可强制执行**：限额由 wrapper 代码执行，不依赖 prompt 自觉。
3. **传输无关**：同一 envelope 用于 CLI、内部 API、未来的 native executor。
4. **确定性**：排序、字段顺序、截断规则稳定，便于缓存与复现。
5. **可观测**：每次调用都能回答“返回了多少、是否截断、是否落盘、是否重复”。
6. **人类可读不花 token**：模型看短键；人看 `--format pretty` 或 `loom show`，转换在工具层完成。

### 1.2 非目标

- 不做 MCP server（M0）。
- 不做富文本/表格/终端 UI。
- 不返回图片、PDF、二进制内容。
- 不替代 Headroom 的缓存/压缩；两者分层。
- 不在 v1 做跨 run 的内容寻址缓存（预留）。

---

## 2. Envelope v1

```json
{
  "v": 1,
  "ok": true,
  "tool": "search",
  "summary": "4 个文件中 12 处匹配，已返回前 8 条",
  "items": [
    {
      "t": "match",
      "p": "apps/web/src/auth/session.ts",
      "b": "9f3c1a2b",
      "r": [120, 128],
      "s": "Session.refresh",
      "k": "def",
      "x": "export async function refresh(...)",
      "sc": 0.87
    }
  ],
  "ref": "artifacts/search-003.txt",
  "truncated": true,
  "next": "cur_7f3a",
  "stats": { "n": 12, "bytes": 1832, "tok": 420, "spill": true }
}
```

约定：

- **外层长键只出现一次**；`items` 用短键，重复出现时省 token。
- **没有的字段直接省略**，不写 `null`。
- `summary` 是给人看的一行，模型主要读 `items` 与 `ref`。
- `text` 只用于 `read` / `artifact` / `log` 等需要正文的工具，受 `read_bytes` 限制；完整内容仍落 `ref`。
- `stats.bytes` 是本信封实际 UTF-8 字节数；`stats.tok` 是估算值。
- 生产端必须写 `v: 1` 并符合 schema；消费端遇到缺失 `v` 时按 v1 处理，并忽略未知字段（向前兼容）。
- `ref` 相对 run 目录，形如 `artifacts/search-003.txt`。

### 2.1 Item 短键

| 短键 | 含义 | 适用 `t` |
|---|---|---|
| `t` | 类型：`match` / `file` / `dir` / `path` / `hunk` / `log` / `symbol` | 全部 |
| `p` | 仓库相对路径 | 除 `log` 外 |
| `b` | 内容哈希（git blob / sha256 前缀） | `match` / `file` / `symbol` |
| `r` | 1-based 闭区间行号 `[start, end]` | `match` / `file` / `hunk` / `log` / `symbol` |
| `s` | 符号名 | `match` / `symbol` |
| `k` | 种类：`def` / `ref` / `call` / `type` / `test` / `doc` / `config` / `other` | `match` / `symbol` |
| `x` | 单行预览（≤400 字符） | `match` / `log` / `hunk` |
| `sc` | 相关性 0–1 | `match` / `symbol` |
| `n` | 总数/子项数/总行数 | 按 `t` 解释 |
| `a` / `d` | 新增/删除行数 | `hunk` / `file` |
| `l` | 语言标识（rust/ts/py…） | `file` |

**人类视图**由工具代码把短键展开成长键、表格或彩色输出；这一步在本地完成，**不进入模型上下文**。

---

## 3. 限额

### 3.1 单次调用

| 工具 | 默认返回 | 硬上限 | 超限行为 |
|---|---:|---:|---|
| `search` | 20 条 / 4KB | 50 条 / 8KB | 落盘，回前 N 条 + `next` |
| `read` | 120 行 / 8KB | 200 行 / 12KB | 必须按 `range` 续读 |
| `tree` | 深度 2 / 100 项 | 深度 3 / 200 项 | 回目录摘要 + `ref` |
| `glob` | 100 路径 / 4KB | 200 / 8KB | 截断 + `next` |
| `diff` | 300 行 / 12KB | 500 行 / 20KB | 回每文件 stat + `ref` |
| `verify` / `log` | 尾部 120 行 / 8KB | 200 行 / 12KB | 完整日志进 `ref` |
| `git` | 20 条单行 | 50 条 | 截断 |
| `bash` | 尾部 120 行 / 8KB | 200 行 / 12KB | 完整输出进 `ref` |
| `edit` / `write` | 不返回全文 | — | 只回路径、hunk 数、`a`/`d` |

统一规则：

- `search` 的 `x` 默认 1 行、最多 160 字符；`context_lines` 默认 0，最大 3。
- 单行最多 400 字符；在 UTF-8 字符边界截断，不能切坏中文/日文。
- 二进制（含 NUL）→ `err.code=binary`，不回内容。
- 大于 1MB 的文件必须带 `range`。
- 空结果也要回 `summary`，不能回空字符串。
- 排序固定：`sc` 降序 → `p` 字典序 → `r[0]` 升序。
- 字段顺序固定，便于前缀缓存。

### 3.2 三级预算

| 层级 | 默认 | 硬上限 | 超限后 |
|---|---:|---:|---|
| 单次调用 | 8KB | 16KB | 落盘 + 截断 |
| 单次 agent invocation | 24KB | 48KB | 只回 `summary + ref`；`read` 降到 40 行 |
| 单 run（含重试） | 128KB | 256KB | 强制收窄 `scope`，否则 `err.code=budget` |

- 计数口径：**实际写入 envelope 的 UTF-8 字节**，不含落盘全文。
- 达到 75% 时附带 `budget` 字段：

```json
"budget": { "u": 98304, "l": 131072, "p": 75, "at": "run" }
```

- 8KB 中文约 2700 字，token 密度高于英文；因此同时记录 `bytes` 与 `tok`，不要只按字节判断。
- 默认值在 `.loom/config.toml` 可调低；硬上限编译进代码，只有显式管理员配置能提高。

### 3.3 输入侧

- `search.q` ≤ 200 字符；`path_glob` ≤ 5 个；`max_results` 超限自动截到硬上限。
- `read.path` 必须是仓库相对路径；拒绝 `..`、绝对路径、符号链接逃逸。
- 无 `path` 的全仓搜索：允许，但自动加 ignore 与结果上限；任务声明 `scope` 时默认只搜 scope。
- `edit` patch ≤ 200 行；超出要求拆分。
- 所有参数先规范化再执行；**预算不能靠 prompt 约束**。

---

## 4. 分页、artifact 与去重

### 4.1 分页

- `next` 是 opaque cursor，编码 `query_hash + offset + snapshot`。
- 仓库变化后 cursor 失效 → `err.code=stale_cursor`，要求重查，不返回错位结果。
- cursor 只在同一 run 内有效；不跨 run 复用。

### 4.2 artifact

路径：`.loom/runs/<run>/artifacts/<tool>-<seq>.txt`

首行是 JSON header，后面是全文：

```json
{"v":1,"tool":"search","args_hash":"ab12","git":"9f3c1a2","cwd":".","created":"2026-11-04T12:00:00Z","count":12,"sha256":"..."}
```

- `ref` 只暴露相对路径；worker 通过 `loom artifact --ref ... --range ...` 读取。
- artifact 是只读的；每次调用新文件，不覆盖。
- 保留到 run 结束；`loom gc` 统一清理。
- 不在 artifact 里写 secret；命令参数只记 `args_hash`，调试模式才记脱敏后的 args。

### 4.3 去重

- 同一 run 内 `(p, b, r, tool)` 已返回过 → 回：

```json
{"v":1,"ok":true,"tool":"read","unchanged":true,"ref":"artifacts/read-002.txt","summary":"内容未变，本次运行已返回过"}
```

- 模型显式 `force` 才重发。
- 搜索去重按 `query_hash + result_set_hash`；结果集不变则回 `unchanged`。
- 文件变了（`b` 不同）不算重复。

---

## 5. 错误与退出码

```json
{
  "v": 1,
  "ok": false,
  "tool": "read",
  "summary": "文件过大，请指定行范围",
  "err": { "code": "too_large", "hint": "read --range 1:200" }
}
```

`err.code` 固定枚举：

`invalid_args`、`not_found`、`too_large`、`binary`、`timeout`、`permission`、`stale`、`stale_cursor`、`budget`、`internal`。

- 只回一行原因；`detail` ≤500 字符，禁止 stack trace。
- `summary` 本地化；`code` 不翻译。
- 工具 CLI 退出码：`0` 成功，`2` 参数错误，`3` 超时/预算，`4` 内部错误，`5` 过期。
- **adapter 的退出码契约（0/2/3/4）保持不变**，两者不要混用。

---

## 6. 本地化与 CJK

- `LOOM_LANG=zh-CN`（默认）| `en` | `ja`。
- 只翻译 `summary`、`err.hint`、`err.detail` 和人类视图；路径、符号、枚举、字段名不翻译。
- 中文优先；英文模板随功能补；日文最后。
- `rg` 对 CJK 精确子串可用；未来 BM25 用**字符 bigram**，不引入分词依赖；日文额外做 NFKC 与假名归一化。
- token 估算（粗略，落地后校准）：

```text
tok_est ≈ ascii_bytes / 4 + cjk_chars
```

- 混合文本按字符统计，不按字节猜。

---

## 7. monorepo 与 scope

- `search`/`glob` 支持 `--scope apps/web`；任务声明 `paths` 时默认只搜这些路径。
- 默认 ignore：`.gitignore` + `.loomignore` + `node_modules`、`target`、`dist`、`build`、`.venv`、`vendor`。
- 编程语言按仓库实际出现逐步接 tree-sitter；M1 不预装全量 grammar。
- 多包配置后续再加；v1 只认仓库根 `.loom/config.toml`。

---

## 8. Telemetry 与调参

每次工具调用追加一行到 `.loom/runs/<run>/logs/tools.jsonl`：

| 字段 | 说明 |
|---|---|
| `ts` | 时间戳 |
| `run` / `task` | 归属 |
| `tool` | 工具名 |
| `args_hash` | 参数哈希；调试模式可记脱敏参数 |
| `scope` | 搜索范围 |
| `items` | 返回条数 |
| `bytes` | envelope 字节数 |
| `tok_est` | token 估算 |
| `truncated` / `spill` / `dedupe` | 截断/落盘/去重 |
| `stale` | 哈希不匹配次数 |
| `ms` | 工具耗时（只进日志，不进 envelope） |
| `exit` | 退出码 |
| `budget_at` | 触发的预算层级 |

聚合到账本：

- `tool_output_ratio` = 工具输出 tokens / 总输入 tokens；
- `spill_rate`、`truncation_rate`、`dedupe_rate`；
- `budget_hit` 次数；
- `tok_est_error`：用 provider 实际 usage 校准估算系数。

调参顺序：先看 `tool_output_ratio`，再看 `spill_rate`。如果大量调用都落盘，说明默认限额太小或 agent 搜索太散；如果几乎不落盘但 token 仍高，说明预览/条数上限还可以再降。

**隐私**：默认只记路径、哈希、大小，不记文件内容；`LOOM_TELEMETRY=debug` 才记脱敏参数，且不进共享知识库。

### 8.1 Headroom / 成本归因的观测口径

| 现象 | 正确口径 |
|---|---|
| `WasteSignals` 数值很大 | **累计检测量**，不是“已节省”；A/B 只看 `Δ值 / Δrequests` 的增速 |
| `reread_compressed: 0` | 归因逻辑尚未落地，不能当作“压缩失效”的判据 |
| `share_of_saved_percent` >100% | 分母可能用错，**禁用**，不作为优化目标 |
| `agent_usage` 出现 `codex` / `pi-extension` | 可能来自 aggregate fallback；精确归属看 `agents[].source` |
| `requests.by_client` 为空 | 不能作为唯一归属来源 |
| `mode` | **进程级配置**，不能按请求或 model 切换；要两种模式只能按客户端/端口分实例 |
| 客户端分类 | 看 `User-Agent` / `X-Client`，**不看 model** |
| 订阅态压缩 | `AuthMode=Subscription` 时更保守（lossless-only / live-zone-only / 更低 `max_lossy_ratio`），需单独统计 |

规则：

- Headroom 继续 `mode=cache`，不按模型切模式；
- `provider_cache_discount_usd` 单独记，不与压缩节省相加；
- 订阅请求不能用 LiteLLM 的 `$0` spend 做预算，改用 RPM/TPM 或 Loom 自己的 `allowance_burn`；
- 归因头（`X-Loom-Run` / `X-Loom-Task`）只有在 Headroom 确认透传后才用于账本；透传失败时以进程/任务边界归因；
- 订阅相关口径（`allowance_burn`、订阅态压缩）等订阅 lane 启用后再用；当前 API key 多 provider 阶段不涉及。

---

## 9. 传输与 Headroom 对接

M0 只做 **CLI + 内部 API**，不做 MCP：

```bash
loom tool search --q "refresh token" --scope apps/web --json
loom tool read --path apps/web/src/auth/session.ts --range 120:168 --json
loom artifact --ref artifacts/search-003.txt --range 1:120
```

与 Headroom 的三种接法，按优先级：

| 方式 | 做法 | 评价 |
|---|---|---|
| **proxy** | 模型请求的 `base_url` 指向 Headroom；Headroom 再转 LiteLLM/provider | **首选**：零耦合，Loom 不感知 |
| **wrap** | Loom 的 HTTP client 包一层，加 `X-Loom-Run`/`X-Loom-Task` 归因头 | 需要 Headroom 透传；用于成本归因 |
| **SDK** | 直接调用 Headroom 的库 | 最后考虑；避免绑定内部 API |

工具端已经把 bytes 降下来，Headroom 继续 `mode=cache`，只处理模型层重复前缀。两者不重叠。

---

## 10. 版本化

- `v:1`；只做增量字段，未知字段忽略。
- 破坏性变更 → `v:2`，不静默改语义。
- Schema 文件：`schemas/tool-result.v1.json`（JSON Schema 2020-12）。
- 契约测试：每个工具产出必须通过 schema 校验；adapter canary 里加一条 envelope 校验。
- `summary` 模板版本单独记录，便于对比本地化改动。

---

## 11. 示例

### 11.1 搜索结果被截断

```json
{
  "v": 1,
  "ok": true,
  "tool": "search",
  "summary": "4 个文件中 12 处匹配，已返回前 8 条",
  "items": [
    {"t": "match", "p": "apps/web/src/auth/session.ts", "b": "9f3c1a2b", "r": [120, 128], "s": "Session.refresh", "k": "def", "x": "export async function refresh(...)", "sc": 0.87}
  ],
  "ref": "artifacts/search-003.txt",
  "truncated": true,
  "next": "cur_7f3a",
  "stats": {"n": 12, "bytes": 1832, "tok": 420, "spill": true}
}
```

### 11.2 重复读取

```json
{"v":1,"ok":true,"tool":"read","unchanged":true,"ref":"artifacts/read-002.txt","summary":"内容未变，本次运行已返回过"}
```

### 11.3 文件过大

```json
{"v":1,"ok":false,"tool":"read","summary":"文件过大，请指定行范围","err":{"code":"too_large","hint":"read --range 1:200"}}
```

### 11.4 编辑成功

```json
{"v":1,"ok":true,"tool":"edit","summary":"已修改 1 个文件","items":[{"t":"file","p":"src/auth/session.ts","b":"3a7d9e01","a":12,"d":3,"n":2,"l":"ts"}],"stats":{"n":1,"bytes":186,"tok":48}}
```

### 11.5 预算告警

```json
{"v":1,"ok":true,"tool":"search","summary":"3 处匹配","items":[{"t":"match","p":"src/lib.rs","b":"7c1e2a9","r":[42,42],"x":"pub fn refresh()","sc":0.9}],"stats":{"n":3,"bytes":140,"tok":36},"budget":{"u":98304,"l":131072,"p":75,"at":"run"}}
```

---

## 12. 待办与待验证

1. **校准 token 估算**：用实际 provider usage 对比 `tok_est`，分别拟合英文/中文/日文系数。
2. **确认 `.loom/config.toml` 的键名**：`[tools.limits]` 下放默认值，硬上限在代码里。
3. **确认 artifact 命名与 GC 策略**：`<tool>-<seq>.txt` 是否够用，是否需要内容寻址。
4. **确认 `edit`/`write` 的返回粒度**：只回 stat，还是回 hunk header（不含全文）。
5. **契约测试接入点**：schema 校验放在工具 wrapper 还是 adapter 归一化层。
6. **M1 是否提前 native executor**：取决于 telemetry 里 goose 内置工具的输出占比。
