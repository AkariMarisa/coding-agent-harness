# AGENTS.md

本文件是仓库内 AI agent 与人类协作者的共同约定。  
目标：保持 Loom 的控制面薄、协议稳定、token 预算可验证。

## 1. 先读什么

按顺序读：

1. `doc/08-设计路线-v0.2.md`（当前最高优先级设计）
2. `doc/14-当前问题与卡点.md`（现在还不能证明/不能做的事）
3. `doc/09-代码检索与知识库.md`（检索策略与边界）
4. `doc/10-工具输出协议-v0.1.md`（工具 envelope 与限额）
5. `schemas/tool-result.v1.json`（对外契约）

旧文档 `doc/02`–`doc/07` 仍然有效，但与 `08/09/10` 冲突时以新文档为准。

## 2. 环境：只用容器

**不要使用宿主机安装的 Rust / cargo / rustc。**

统一入口：

```bash
./scripts/dev.sh cargo build --workspace
./scripts/dev.sh cargo test --workspace
./scripts/dev.sh cargo fmt --all
./scripts/dev.sh cargo clippy --workspace --all-targets -- -D warnings
```

或 `make build/test/fmt/clippy/check`。

- 默认优先 `podman`，其次 `docker`。
- 镜像由 `Containerfile` 构建；依赖缓存在 `.cache/cargo`。
- 不要在仓库里提交 `target/`、`.cache/`、`.loom/`。

## 3. 代码布局

```text
src/
├── Cargo.toml          # workspace，新增 crate 在这里注册
└── crates/
    ├── loom-core       # 纯类型/常量/配置/估算；尽量不做 IO
    ├── loom-store      # .loom 文件布局、artifact、telemetry、去重
    ├── loom-tools      # search/read/artifact 等工具逻辑
    ├── loom-context    # Context Compiler + 版本化 prompt 库
    ├── loom-adapter    # 进程级 adapter 契约、mock/native/goose/opencode、canary
    ├── loom-native     # native executor：工具循环、结果投影、预算、no-progress
    └── loom-cli        # clap 命令与输出格式化
```

边界：

- `loom-core` 不依赖 `loom-store/tools/cli`。
- `loom-tools` 依赖 `core` + `store`，不直接解析 CLI 参数。
- `loom-cli` 只做参数解析、调用、输出、退出码。
- 后续 `loom-context` / `loom-adapter-goose` 按同样边界添加。

## 4. 协议是契约

- `schemas/tool-result.v1.json` 是模型与工具之间的契约。
- 改字段、短键含义、限额、退出码时，**必须同时更新**：
  1. `loom-core` 类型；
  2. `schemas/tool-result.v1.json`；
  3. `doc/10-工具输出协议-v0.1.md`；
  4. 至少一个测试或示例。
- 只做增量字段；破坏性变更升 `v2`，不要静默改语义。
- 生产端严格符合 schema；消费端忽略未知字段。

## 5. 工具端 token 预算

- 能在工具层裁掉的，不要留给 Headroom。
- 工具输出默认“摘要 + 地址 + 预览”，全文落 artifact。
- 三级预算：单次 8KB / 单次 invocation 24KB / 单 run 128KB。
- 硬上限：16KB / 48KB / 256KB；配置只能调低，管理员才能提高。
- 不返回原始 `rg --json`、HTML、ANSI、stack trace。
- 每次调用写 `logs/tools.jsonl`，字段见 `doc/10` §8。

## 6. 状态与文件所有权

- `.loom/` 是运行时状态，**不要提交**，也不要手改事实源。
- orchestrator 是唯一写者；worker 只写代码 + `outbox/`。
- 当前 M0 只有 `loom init` 创建骨架；任务图/事件日志尚未实现。
- 不要把 `.loom/` 的内容复制进 `doc/` 或测试夹具。

## 7. 测试与质量

提交前至少：

```bash
make check
```

包括 `fmt --check`、`clippy -D warnings`、`cargo test --workspace`。

新增行为优先写单元测试；涉及文件系统/搜索的，用临时目录，不依赖本机仓库内容。

## 8. 文档与语言

- 设计文档用中文；代码标识符、协议字段、错误码用英文。
- 人类可读文本默认中文，`LOOM_LANG` 可切 `en` / `ja`。
- 路径、符号、枚举不翻译。
- 文档中的模型名、端口、CLI 参数均为示例，落地前以 `--help` 和官方文档为准。

## 9. Git / GitHub

- 远程是私有仓库；不要提交 secret、token、个人路径。
- 提交信息用短祈使句，例如：
  - `scaffold rust workspace`
  - `add tool result envelope`
  - `implement search and read tools`
- 大改动前先确认设计文档已同步。
- 不要用 `git push --force` 改写主分支历史，除非用户明确要求。

## 10. 当前里程碑（M0）

已完成/进行中：

1. workspace + 容器化开发环境；
2. 工具 envelope + `search`/`read`/`artifact`；
3. telemetry 与三级预算；
4. 模型能力位 + 请求账本 + CPVT/TPVT/WPT；
5. TaskSpec / 事件日志 / 任务索引与状态重放；
6. Context Compiler（稳定前缀、硬预算、失败包、manifest）+ 版本化 prompt 库；
7. 进程级 adapter 契约、mock adapter、goose/opencode preset、adapter canary；
8. `dispatch` / `verify` / `resume` 最小闭环（mock 端到端通过）；
9. native executor（M2 起步）：OpenAI-compatible 工具循环、read/search/edit/write/bash、结果投影到 artifact、token/墙钟/步数/成本预算、acceptance 失败回灌、上一 run 失败包注入、no-progress、脚本化 provider 测试、`--adapter native` 端到端通过。
10. 控制面路由（M3 起步）：`[routing]` role→tier、verify 失败升级、tier 成本护栏、派发前最坏成本检查、`loom route` 解释命令、`loom route stats` worker 占比与切换统计、同层 fallback 自动切换（`task.fallback` + 独立 run）；`task.escalated` 与 `TaskDispatched.data.route` 已接线。

下一步：

1. 补容器镜像/挂载：goose、opencode、curl、可选 LiteLLM；
2. 真实 goose/OpenCode adapter canary（需安装二进制）；
3. native vs adapter A/B（同任务集，按 CPVT/TPVT/墙钟）；
4. 用真实仓库校准 `tool_output_ratio` / `spill_rate` / 成本估算；
5. 锁、合并队列与 2 并行；
6. 记录 CPVT/TPVT/WPT 基线。

当前卡点详见 `doc/14-当前问题与卡点.md`。

**已设计待实现**：

- 多仓库工作区/多端协同（manifest 成员仓库、`loom workspace *`、跨成员同名分支 worktree），见 `doc/13-多仓库工作区与多端协同.md`（M5，P0–P3）。
- 运行时治理与工程化（churn 指标与分级干预、`questions.json` + `needs_input` + attention inbox、skills 扫描/按需加载/验证晋升、agent/harness 驱动的本地 CI 与 hooks/review/attestation），见 `doc/15-Agent运行时治理与工程化.md`。

**不要提前做**：embedding/向量库、MCP、跨项目知识库、自动规划、并行调度、把成员仓库合并成 monorepo。
