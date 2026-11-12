# .loom/ — Loom 的工作目录

> 语言：[English](README.md) | **简体中文**

两层结构（根 `.gitignore` 白名单：`/.loom/*` + 反选，新增状态默认忽略）：

| 层 | 路径 | git |
|---|---|---|
| 契约层 | `README.md`、`README.zh-CN.md`、`goal.md`、`constraints.md`、`specs/` | tracked |
| 状态层 | `tasks/`、`runs/`、`ledger/`、`snapshots/`、`locks/`、`config.toml`、`config.local.toml`、`events.jsonl` 等其余全部 | ignored |

- 契约层：人与控制面共同维护，走 git review；`loom goal` / `loom constraints` 起草，人审 diff；模块契约放 `specs/`。
- 状态层：orchestrator 唯一写者；事件日志 append-only，不要手改事实源。
- 本地实验配置写 `config.local.toml`（深合并覆盖 `config.toml`，`[[models]]` 按 alias 合并；永不提交，测试不碰主文件）。
- 改配置用 `loom config set <path> <value>`（保留注释与顺序、整体校验后落盘，`--local` 改覆盖层）；`loom config get <path>` 看生效值与来源。
- ⚠️ `rm -rf .loom/` 会连契约层一起删（tracked 可从 git 恢复），状态层不可恢复。
- 分层规则见仓库 `AGENTS.md` §6。
