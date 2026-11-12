# Loom

> 语言：[English](README.md) | **简体中文**

> 面向软件设计、架构与开发的 token 高效 Agent Harness  
> 核心命题：**长任务，短上下文。**

Loom 是一个独立编排内核：状态放在 `.loom/`，任务在独立 worktree 中由可替换的 agent 执行，验收只认可执行命令。当前已完成 M0–M3（协议产品化、上下文与账本、native executor、确定性路由与同层 fallback）与 M4 并行（路径锁、`[parallel]` 上限/预算、`loom merge` 合并队列、`loom board` 看板，2 并发 10 任务实测通过）；真实外部 adapter / 真实模型 A/B 仍受环境限制。

## 当前状态

| 项 | 状态 |
|---|---|
| 设计文档 | v0.2，见 [doc/README.zh-CN.md](doc/README.zh-CN.md) |
| 工具输出协议 | v0.1，见 [doc/10-tool-output-protocol-v0.1.zh-CN.md](doc/10-tool-output-protocol-v0.1.zh-CN.md) |
| Rust workspace | `src/`，crate 为 `loom-core` / `loom-store` / `loom-tools` / `loom-context` / `loom-adapter` / `loom-native` / `loom-cli` |
| 已实现 | `loom init/doctor/status`、`loom board`、`loom tool *`、`loom artifact`、`loom capability *`、`loom route`、`loom usage record/ingest`、`loom ledger`、`loom task *`、`loom event list`、`loom context compile`、`loom prompt *`、`loom adapter list/canary`、`loom dispatch`（mock/native）、`loom verify`、`loom merge`、`loom resume` |
| 未实现 | 真实 goose/OpenCode adapter canary（需安装二进制）、native vs adapter A/B 基线、真实账单对账。详见 [doc/14-current-issues-and-blockers.zh-CN.md](doc/14-current-issues-and-blockers.zh-CN.md) |
| 已设计未实现 | churn 治理余项（L1–L3 分级干预、成本感知重启；步骤级投影、patch-first/write 守卫、token 归因与 `loom churn stats` 已实现）、提问/回答协议与 attention inbox、skills 治理、agent/harness 驱动的本地 CI（hooks/review/attestation），见 [doc/15-agent-runtime-governance-and-engineering.zh-CN.md](doc/15-agent-runtime-governance-and-engineering.zh-CN.md) |

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

真实 provider 凭据放在仓库根目录的 `.env.local`（已被 `.gitignore` 的 `.env.*` 忽略，模板见 [.env.example](.env.example)）；`scripts/dev.sh` 只把白名单变量透传进容器，子进程（`bash` 工具、acceptance 命令）会剥离一切 `*_API_KEY`/`*_TOKEN`/`*_SECRET`。key 不要提交，也不要贴进对话。

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

可选环境变量：`LOOM_NATIVE_BIN`（指定二进制路径）、`LOOM_NATIVE_MAX_TOKENS`、`LOOM_NATIVE_MAX_IDLE`、`LOOM_NATIVE_MAX_VERIFY_ROUNDS`、`LOOM_NATIVE_TEMPERATURE`、`LOOM_NATIVE_SKIP_ACCEPTANCE`、`LOOM_NATIVE_DEBUG`（请求/响应转储到 run 日志）、`LOOM_NATIVE_HISTORY_BYTES` / `LOOM_NATIVE_KEEP_TURNS` / `LOOM_NATIVE_MIN_CLEAR`（步骤级投影，0 关闭；见 [doc/15](doc/15-agent-runtime-governance-and-engineering.zh-CN.md) §1）。

- acceptance 在 native 内预跑；失败时把命令、退出码和截断输出回灌给模型，默认最多 2 轮（`LOOM_NATIVE_MAX_VERIFY_ROUNDS`，最小 1），轮次耗尽标记 `failed`。
- 成本：provider 返回成本时记 `cost_source=provider`；否则用派发模型的 `ModelCapability` 价格表估算并记 `pricing_table`；价格未知则省略 `cost_usd` 并记 `unknown`。估算成本同样计入 `TaskBudget.max_cost_usd`，超限以 `budget_exceeded` / exit 3 停止。
- `loom dispatch` 会读取同一任务上一 run 的失败回执与 `verify.log`，生成失败包注入 Context Compiler，下一 run 直接看到上次失败的命令与输出。
- native 写出的 `final.md` 只是 claim，最终状态仍以 `loom verify` 为准。

### 路由与升级

路由是确定性的，不调用 LLM 判断任务难度。`.loom/config.toml` 先用 `[[models]]` 注册模型别名（每个 alias 可以指向不同中转），再用 `role → tier`、升级阈值、fallback 和 tier 成本上限配置路由：

```toml
[[models]]
alias = "worker"
model_id = "deepseek-chat"        # provider 侧的真实模型名
tier = "worker"
wire = "chat"                     # chat | messages；responses 未实现
tool_calls = true
base_url = "https://api.deepseek.com/v1"   # 可选：覆盖全局 LOOM_API_BASE
key_env = "DEEPSEEK_API_KEY"      # 可选：该模型用哪个环境变量装 key（只存变量名，值只在 .env.local）
input_usd_per_mtok = 0.27         # 价格表可选；缺省时 cost 记 unknown
output_usd_per_mtok = 1.10

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

- 只看一篇：[08-design-roadmap-v0.2.zh-CN.md](doc/08-design-roadmap-v0.2.zh-CN.md)
- 检索/知识库：[09-code-search-and-knowledge-base.zh-CN.md](doc/09-code-search-and-knowledge-base.zh-CN.md)
- 工具协议：[10-tool-output-protocol-v0.1.zh-CN.md](doc/10-tool-output-protocol-v0.1.zh-CN.md)
- 多仓库工作区（设计稿）：[13-multi-repo-workspace-and-multi-client.zh-CN.md](doc/13-multi-repo-workspace-and-multi-client.zh-CN.md)
- 当前问题与卡点：[14-current-issues-and-blockers.zh-CN.md](doc/14-current-issues-and-blockers.zh-CN.md)
- 运行时治理（设计稿）：[15-agent-runtime-governance-and-engineering.zh-CN.md](doc/15-agent-runtime-governance-and-engineering.zh-CN.md)
- 文档总览：[doc/README.zh-CN.md](doc/README.zh-CN.md)

## 参与贡献

欢迎贡献，先读 [CONTRIBUTING.zh-CN.md](CONTRIBUTING.zh-CN.md)。所有参与受[行为准则](CODE_OF_CONDUCT.zh-CN.md)约束。安全问题见 [SECURITY.zh-CN.md](SECURITY.zh-CN.md)。

## 许可

Copyright (c) 2026 The Loom authors。保留所有权利。

本仓库采用 [PolyForm Noncommercial License 1.0.0](LICENSE)（SPDX：`PolyForm-Noncommercial-1.0.0`，以英文原文为准）：非商业目的可使用、修改与分发；商业使用需另行取得著作权人授权。许可证为暂行版本，最终确定后本节会同步更新。
