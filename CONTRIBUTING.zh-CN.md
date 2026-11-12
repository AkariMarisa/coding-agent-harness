# 参与 Loom 贡献

> 语言：[English](CONTRIBUTING.md) | **简体中文**

感谢你有兴趣参与贡献。本文档覆盖工作流与约定；详细的工程规则见 [AGENTS.md](AGENTS.md)。

## 开始之前

- 阅读[行为准则](CODE_OF_CONDUCT.zh-CN.md)；参与即表示接受。
- 阅读 [README.zh-CN.md](README.zh-CN.md) 了解当前状态与快速开始，阅读 [doc/README.zh-CN.md](doc/README.zh-CN.md) 了解设计文档地图。
- 安全问题请按 [SECURITY.zh-CN.md](SECURITY.zh-CN.md) 处理，不要开公开 issue。

## 开发环境

所有 Rust 构建与测试都在容器内运行；不使用宿主机安装的工具链：

```bash
./scripts/dev.sh cargo build --workspace
./scripts/dev.sh cargo test --workspace
./scripts/dev.sh cargo fmt --all
./scripts/dev.sh cargo clippy --workspace --all-targets -- -D warnings
```

或使用 Make 封装：`make build` / `make test` / `make fmt` / `make clippy` / `make check`。

- 优先 `podman`，回退 `docker`（`CONTAINER_ENGINE=docker` 可强制指定）。
- 真实 provider 凭据放在 `.env.local`（已被 git 忽略）；永远不要提交 key。

## 修改流程

- 从默认分支拉出分支；改动保持聚焦、便于审查。
- 新行为优先写单元测试。涉及文件系统或搜索的测试用临时目录，不依赖本仓库自身内容。
- 开 PR 之前运行 `make check`（fmt、clippy `-D warnings`、workspace 测试）。
- 协议变更（字段、短键含义、限额、退出码）必须同步更新四件套：`loom-core` 类型、`schemas/*.json`、`doc/10-tool-output-protocol-v0.1.zh-CN.md`、至少一个测试或示例——见 AGENTS.md §4。
- 大改动应在同一个 PR 里同步相关设计文档（`doc/`）。

## 文档语言

- 人类可读文档**英文为主**：英文文件是主文件，`*.zh-CN.md` 中文版通过 H1 后的切换行互链；改内容时两版都要更新。
- 面向 AI 的文档（`AGENTS.md`、skills）只保留英文。
- CLI 输出语言是运行时行为，不是文档问题（见 AGENTS.md §8 的 `LOOM_LANG`）。

## 提交与 PR

- 提交信息用短祈使句：`add tool result envelope`、`fix lock lease renewal`。
- PR 里说明改了什么、为什么改、如何验证（跑过的命令、测试输出）。
- 优先小而可审查的 PR，避免大而混杂的改动。

## 报告问题

- 缺陷与功能请求：开 issue，附复现步骤或具体用例。
- 安全问题：见 [SECURITY.zh-CN.md](SECURITY.zh-CN.md)。
- 行为准则相关：见 [CODE_OF_CONDUCT.zh-CN.md](CODE_OF_CONDUCT.zh-CN.md)。
