# 17 工作区开发环境隔离（方向记录）

> **状态：方向记录，未设计、未实现（2026-11-12）。** 只记动机与取舍，防止后续讨论从零开始；动手前先出设计草案。

## 1. 动机

- 需要在 Windows / Linux / macOS 三个主流系统上，按工作区隔离开发环境。
- 痛点：nvm 切工作区会覆写全局 Node 版本，sdkman 同类问题；JetBrains 内置“项目环境配置”但整套 IDE 太重——缺一个“IDE 级的项目环境声明 + 轻量执行”的工具。
- 约束：**尽量无虚拟化**（Windows/macOS 上尤其如此），装不上 docker/podman、不支持虚拟化的机器也要能用。

## 2. 市场判断（简）

- 分层：A 工具版本（mise/asdf/proto）、B 系统库与工具链、C 容器/VM、D 云工作区。
- **A+B 可以无虚拟化**：Nix/devbox 是用户态的（不需要容器/VM，Windows 才要 WSL2）；podman 在 Linux 也无虚拟化。无虚拟化真正排除的是 mac/win 的 VM 桌面方案与云 VM。
- **语言包管理器本身就是 B 层**：uv/npm/cargo/go modules 天然按工作区隔离；跨语言系统库用 conda/Nix；Android 链（JDK/Gradle/SDK）全用户态可装。
- 无容器时的真空白是**中间件多版本**；通用环境管理器是红海（mise/devbox/conda 打了十年），全语言兼容是长尾陷阱——**都不做**。

## 3. Loom 的取舍（待设计）

**声明即产品，隔离是可选能力。**

1. 复用既有标准文件，不发明新格式：`.mise.toml` / `.tool-versions`、`pyproject.toml` 的 `requires-python`、`package.json` 的 `engines`、Gradle wrapper 版本。
2. `loom env` 只做三件事：**探测 → 校验（不符 fail fast，同 `key_env` 哲学）→ 执行声明的 setup**（幂等、可缓存）；安装永远交给现成工具，自己一行不写。
3. **降级矩阵**：有容器引擎 → 现有 Containerfile profile；无 → 用户态 profile（mise + 语言包管理器）。两个 profile 消费同一份声明、跑同一批 acceptance，exit code 契约不变。
4. 密钥白名单透传（现 `scripts/dev.sh` 的 `*_API_KEY` 逻辑）升级为 `loom env` 的一等能力。
5. 人 / agent / pipeline 三受众靠“声明是仓库里的文件”自然成立（mise 在 CI 已证明该模式），不去适配三种用户。

## 4. 非目标

- 不做通用环境管理器，不背语言/中间件兼容长尾；
- 不做云工作区（网络往返、成本、密钥信任面均不匹配）；
- 不要求虚拟化；容器只是可选 profile，不是前提。

## 5. 下一步

设计草案：声明复用清单、`loom env` 命令面、降级矩阵、与 `dispatch` fail-fast 的接点（对齐 `key_env` 缺失即拒派发的既有行为）。**此前不写代码。**
