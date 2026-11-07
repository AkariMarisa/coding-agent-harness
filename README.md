# Loom

> 面向软件设计、架构与开发的 token 高效 Agent Harness  
> 核心命题：**长任务，短上下文。**

Loom 是一个独立编排内核：状态放在 `.loom/`，任务在独立 worktree 中由可替换的 agent 执行，验收只认可执行命令。当前已完成 M1（上下文与账本）、M2 的 native executor 闭环，并开始 M3（确定性路由与成本护栏）；真实外部 adapter / 真实模型 A/B 仍受环境限制。

## 当前状态

| 项 | 状态 |
|---|---|
| 设计文档 | v0.2，见 [doc/README.md](doc/README.md) |
| 工具输出协议 | v0.1，见 [doc/10-工具输出协议-v0.1.md](doc/10-工具输出协议-v0.1.md) |
| Rust workspace | `src/`，crate 为 `loom-core` / `loom-store` / `loom-tools` / `loom-context` / `loom-adapter` / `loom-native` / `loom-cli` |
| 已实现 | `loom init/doctor/status`、`loom tool *`、`loom artifact`、`loom capability *`、`loom route`、`loom usage record/ingest`、`loom ledger`、`loom task *`、`loom event list`、`loom context compile`、`loom prompt *`、`loom adapter list/canary`、`loom dispatch`（mock/native）、`loom verify`、`loom resume` |
| 未实现 | 真实 goose/OpenCode adapter canary（需安装二进制）、native vs adapter A/B 基线、真实 CPVT/成本校准、锁与合并队列、并行调度。详见 [doc/14-当前问题与卡点.md](doc/14-当前问题与卡点.md) |
| 已设计未实现 | 写入抖动（churn）治理、提问/回答协议与 attention inbox、skills 治理、agent/harness 驱动的本地 CI（hooks/review/attestation），见 [doc/15-Agent运行时治理与工程化.md](doc/15-Agent运行时治理与工程化.md) |

## 快速开始

所有 Rust 命令都在容器里跑，**不要使用宿主机安装的 Rust**：

```bash
./scripts/dev.sh cargo --version
./scripts/dev.sh cargo test --workspace
./scripts/dev.sh cargo run -p loom-cli -- --help
```

或者：

```bash
make test
make fmt
make clippy
```

首次运行会构建 `loom-dev` 镜像（Rust + git + rustfmt/clippy）。默认优先使用 `podman`，没有时回退到 `docker`；可用 `CONTAINER_ENGINE=docker` 强制指定。

## 目录

```text
.
├── AGENTS.md                 # 仓库协作约定（AI/人类）
├── Containerfile             # 开发容器
├── Makefile                  # 容器内构建/测试入口
├── scripts/dev.sh            # podman/docker 包装
├── doc/                      # 设计文档
├── schemas/                  # 对外 JSON Schema
└── src/
    ├── Cargo.toml            # workspace
    └── crates/
        ├── loom-core/        # 协议类型、限额、配置、TaskSpec/事件/回执
        ├── loom-store/       # .loom 布局、事件日志、任务索引、artifact、账本
        ├── loom-tools/       # search/read/artifact 实现
        ├── loom-context/     # Context Compiler + 版本化 prompt 库
        ├── loom-adapter/     # 进程级 adapter 契约、mock/native/goose/opencode、canary
        ├── loom-native/      # native executor：工具循环、投影、预算、no-progress
        └── loom-cli/         # loom 二进制
```

## 最小闭环（当前可用的部分）

```bash
# 在任意项目根目录
loom init
loom task add --file task.json        # 校验并登记 TaskSpec
loom route T-001                      # 解释 role/tier、模型、预算与升级决策
loom context compile T-001            # 编译有界上下文 packet
loom dispatch T-001 --adapter mock    # 派发；native 需要 loom-native（workspace 内构建）
loom verify T-001                     # 独立重跑 acceptance，写回执
loom resume                           # 从事件日志恢复状态
loom event list --task T-001
loom ledger --passed-tasks 10 --wall-seconds 1800
loom status
loom doctor
```

`loom tool *` 默认输出紧凑 JSON envelope；加 `--format pretty` 在终端看人类可读视图。人类视图由工具代码转换，不进入模型上下文。

### native executor

`loom-native` 是 workspace 内的独立进程，由 `loom dispatch --adapter native` 启动：

```bash
# 真实 provider（OpenAI-compatible）
export LOOM_API_KEY=...                 # 或 OPENAI_API_KEY
export LOOM_API_BASE=https://api.openai.com/v1
export LOOM_MODEL_ID=gpt-...            # 也可用 --model 别名解析
loom dispatch T-001 --adapter native

# 离线脚本化运行（测试/CI，不需要网络）
LOOM_NATIVE_SCRIPT=/path/to/script.json loom dispatch T-001 --adapter native --in-place
```

可选环境变量：`LOOM_NATIVE_BIN`（指定二进制路径）、`LOOM_NATIVE_MAX_TOKENS`、`LOOM_NATIVE_MAX_IDLE`、`LOOM_NATIVE_MAX_VERIFY_ROUNDS`、`LOOM_NATIVE_TEMPERATURE`、`LOOM_NATIVE_SKIP_ACCEPTANCE`。

- acceptance 在 native 内预跑；失败时把命令、退出码和截断输出回灌给模型，默认最多 2 轮（`LOOM_NATIVE_MAX_VERIFY_ROUNDS`，最小 1），轮次耗尽标记 `failed`。
- 成本：provider 返回成本时记 `cost_source=provider`；否则用派发模型的 `ModelCapability` 价格表估算并记 `pricing_table`；价格未知则省略 `cost_usd` 并记 `unknown`。估算成本同样计入 `TaskBudget.max_cost_usd`，超限以 `budget_exceeded` / exit 3 停止。
- `loom dispatch` 会读取同一任务上一 run 的失败回执与 `verify.log`，生成失败包注入 Context Compiler，下一 run 直接看到上次失败的命令与输出。
- native 写出的 `final.md` 只是 claim，最终状态仍以 `loom verify` 为准。

### 路由与升级

路由是确定性的，不调用 LLM 判断任务难度。`.loom/config.toml` 可以显式配置 `role → tier`、升级阈值、fallback 和 tier 成本上限：

```toml
[routing]
escalate_after_failures = 2      # worker 连续失败 2 次后升级
max_escalations_per_task = 1

[routing.roles]
review = "frontier"
judgment = "frontier"
implement = "worker"
docs = "worker"

[routing.fallbacks]
worker = ["worker-b"]        # 必须是已配置的同层 alias；跨层会被拒绝

[routing.cost_limits]
worker = 0.25
frontier = 1.0
```

优先级：显式 `--model` > `[routing.roles]` 映射 > TaskSpec 自带 `tier`。升级只读取 `task.failed` 事件次数。

`fallbacks` 是**同层冗余**：只有可用性失败（adapter 退出码 4 或超时）才切换，`budget_exceeded` / `needs_input` / `blocked` 不切换——换模型撤销不了已经烧掉的预算。每次切换写 `task.fallback`（`run_id` 指向新 attempt 的 run，`parent_run_id` 指向失败的 run），并为新 attempt 开独立 run，receipt 与 usage 不互相覆盖；`attempt` 不变，所以不会污染升级阈值。显式 `--model` 会钉住 alias，此时 `fallback_enabled=false`，不切换。派发前若任务预算超过 tier 上限，或价格表估算的最坏成本超过上限，`loom dispatch` 会拒绝。

```bash
loom route T-001                 # JSON：tier、模型、原因、fallback、预算、估算
loom route T-001 --format pretty
loom route stats                 # 按事件日志统计 worker 占比、升级/切换次数与路由原因
```

`route stats` 以每个任务最后一次派发计算 worker 占比；重试计入 `dispatches`，但不重复计任务；`fallbacks` 是 `task.fallback` 事件数。这是 M3 退出标准里 “worker ≥50%” 的直接数据来源。

## 设计入口

- 只看一篇：[08-设计路线-v0.2.md](doc/08-设计路线-v0.2.md)
- 检索/知识库：[09-代码检索与知识库.md](doc/09-代码检索与知识库.md)
- 工具协议：[10-工具输出协议-v0.1.md](doc/10-工具输出协议-v0.1.md)
- 多仓库工作区（设计稿）：[13-多仓库工作区与多端协同.md](doc/13-多仓库工作区与多端协同.md)
- 当前问题与卡点：[14-当前问题与卡点.md](doc/14-当前问题与卡点.md)
- 运行时治理（设计稿）：[15-Agent运行时治理与工程化.md](doc/15-Agent运行时治理与工程化.md)
- 文档总览：[doc/README.md](doc/README.md)

## 许可

待定（TBD）。在确定前不要假设可以对外分发。
