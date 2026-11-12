# 13 多仓库工作区与多端协同

> 语言：[English](13-multi-repo-workspace-and-multi-client.md) | **简体中文**

> 状态：**设计稿，未实现**（2026-11-06）。  
> 目标：让"一个顶级目录 + 多端仓库 + 多个 agent"变成一个可控的 Loom 工作区，而不是三四个互不通讯的会话。  
> 前置阅读：[08-design-roadmap-v0.2.zh-CN.md](08-design-roadmap-v0.2.zh-CN.md) §4（状态所有权）、[03-core-protocols.zh-CN.md](03-core-protocols.zh-CN.md)（TaskSpec/事件/回执）。

## 0. 一句话结论

**不迁 monorepo。** 用 "meta 控制面 + manifest 成员仓库 + 同名分支聚合 worktree" 的多仓库工作区，把 Loom 已有的状态外置、事件日志、任务依赖、四行回执扩展到多端场景；跨端同步靠**结构化状态与事件**，不靠 agent 之间传话。

## 1. 问题

当前工作方式与痛点：

| 现状 | 后果 |
|---|---|
| 顶级目录 clone 前端/后端/移动端等多个仓库 | 从顶级目录下发任务时，agent 的 cwd 不在任何成员仓库里 |
| 每个平台开一个 agent，跑在不同目录 | 彼此看不到进度与问题，只能靠人肉转述 |
| 进度靠对话叙述 | 同一事实被重复讲 N 遍，token 浪费；且会漂移 |
| 顶级目录不好用 git worktree | worktree 边界就是仓库边界，跨不了仓库 |
| agent 自带 git 工具看不到当前分支 | 分支状态散落在各目录，无法一眼对齐 |

**本质缺的不是 monorepo，而是一个"工作区（workspace）"抽象**：

```text
workspace = 1 个控制面（Loom 状态/任务/事件） + N 个成员 checkout（各自是独立 git 仓库）
```

## 2. 调研

### 2.1 monorepo vs multi-repo vs meta-repo

| 方案 | 一句话 | 优点 | 代价 |
|---|---|---|---|
| monorepo | 所有端在一个仓库 | 原子跨端提交、统一版本、共享工具链 | 权限粒度差、发布周期互相牵制、仓库体积、迁移成本 |
| multi-repo（polyrepo） | 每端一个仓库 | 归属清晰、权限/发布独立、技术栈自由 | 跨端改动要多仓库协调、版本组合爆炸 |
| meta-repo / manifest | 多仓库 + 一个清单仓库统一编排 | 保留 multi-repo 的独立性，获得统一的工作区与状态 | 需要工具支持（本设计要做的就是这个） |

判断：**用户的场景是"有意分离的多端"**（前端/后端/移动端，发布周期与权限本就不同），这正是 multi-repo 的适用场景。monorepo 的核心收益（原子提交、统一版本）在 AI 协作里大部分可以由"任务图 + 冻结契约 + 集成 verify"替代，而它的代价是真实的。

> 例外：如果某天"跨端原子提交"成为硬需求（例如接口变更必须同 PR 合入），再评估合并仓库。Loom 的设计不依赖这个选择——manifest 模式对单成员（即 monorepo）同样成立。

### 2.2 现有多仓库工具（2025–2026 实测实践）

| 工具 | 做法 | 与本设计的关系 |
|---|---|---|
| [`meta` / `metarepo`](https://github.com/codyaverett/metarepo)（Rust） | `.meta` manifest；bare repo + worktree；`meta worktree add <branch>` **跨所有成员建同名分支 worktree**；`meta exec` 跨仓库执行 | 同名分支聚合 worktree 的成熟范式 |
| [`git-worktree-manager`](https://github.com/nanasess/git-worktree-manager)（Bash） | 批量管理多仓库 worktree；worktree 放在**项目外部** `project.worktrees/<branch>/<member>/`；hook 负责装依赖 | 目录布局与"避让代码搜索"的直接参考 |
| [Canopy](https://www.ricky-dev.com/coding/2026/01/agentic-tooling-across-multiple-repositories)（MCP server） | 一次调用返回所有成员的 branch/status 结构化 JSON；`.canopy/worktrees/<task>/<member>` 同名分支 | "结构化状态替代逐 repo `git status`"的范式 |
| `worktrunk` / `git-worktree-runner` | 面向 AI 并行开发的 worktree 编排 | 同类问题的 AI 侧解法 |
| [Ricky Smith 的手工做法](https://www.ricky-dev.com/coding/2026/01/agentic-tooling-across-multiple-repositories) | 用"分支命名的父目录"聚合多仓库 worktree，父目录放任务上下文 | 证明了该布局在真实 agent 工作流中可用 |

这些工具的**共同模式**（本设计采纳）：

1. **同名分支跨仓库**：一个任务一个分支名（`agent/<task_id>`），涉及的每个成员仓库都检出同名分支；
2. **worktree 外置**：worktree 不放进仓库内部，避免污染 agent 的代码搜索/索引（Loom 的 `.worktrees/` 已经是这个思路）；
3. **manifest 驱动**：成员清单是声明式文件，不是人肉记忆；
4. **结构化状态查询**：给 agent 一次 JSON，而不是让它逐目录解析 `git status` 文本。

### 2.3 对 AI agent 工作流的启示

- **worktree 的边界 = 仓库边界**。跨仓库任务不能靠一个 worktree 解决，要靠"目录聚合 + 同名分支"模拟。
- **agent 的 cwd 必须在成员 checkout 内**。控制面命令（`loom dispatch` 等）从顶级目录发起，但 worker 进程必须落在成员 worktree 里；分支、diff、LSP 才都正常。
- **结构化状态比文本省 token 也少误解**：一次 `loom workspace status --json`（几十行）替代 N 次 `git status` + 人工拼图。
- **不要新增 agent 间通信通道**。跨端同步的正确载体是：事件日志（唯一事实源）+ 任务依赖 + 四行回执——Loom 已有，本设计只是让它认识"成员"。

## 3. 设计决策

| # | 决策 | 理由 |
|---|---|---|
| D1 | **不做 monorepo 迁移** | 多端独立性是真实需求；monorepo 收益可由契约 + 任务图替代 |
| D2 | 顶级目录 = **meta 控制面仓库** | `loom.workspace.toml` 与 `.loom/` 都在这里（`.loom/` 白名单分层：契约层 `goal.md`/`constraints.md`/`specs/` 提交，状态层忽略，见 `AGENTS.md` §6）；成员代码是独立 clone，不提交进控制面 |
| D3 | 成员仓库 = 独立 clone，路径由 manifest 声明 | 保持各端权限/发布/工具链独立 |
| D4 | 跨仓库任务 = **同名分支聚合 worktree** | `.workspaces/<task_id>/<member>/`，每个成员都是自己仓库的 worktree；默认**单成员任务**，多成员任务是例外 |
| D5 | 跨端同步 = 事件日志 + 依赖 + 回执 | 不引入 agent 互聊、不引入 MCP（沿用 D2 进程级契约） |
| D6 | 分支命名 `agent/<task_id>` 跨成员统一 | 合并对齐、冲突定位、`task.merged` 事件可记录 per-member sha |
| D7 | 结构化状态走 CLI JSON，不走 MCP 插件 | 与 [08](08-design-roadmap-v0.2.zh-CN.md) D2 一致：稳定契约是进程 + 文件 + JSON + exit code |
| D8 | 顶层 goal/constraints 只写**平台级**；端特有约束进 `.loom/specs/<end>/`；成员仓库可携带自己的 `.loom/goal.md` / `constraints.md`，dispatch 按任务所属成员仓库选择 | 顶层避免杂物化（goal/constraints 每步重注入、各有 1000 token cap，`loom doctor` 超 cap 警告）；成员独立工作时上下文自足。**未实现，M5 落地** |

## 4. 目录布局

```text
workspace-root/                      # 顶级目录 = meta 控制面仓库（提交到 git）
├── loom.workspace.toml              # 成员清单（提交）
├── .loom/                           # 白名单分层（AGENTS.md §6）：契约层提交，状态层忽略
│   ├── README.md                    # 契约层：分层说明与 ownership 表（提交）
│   ├── goal.md                      # 契约层：平台级目标，loom goal 起草（提交）
│   ├── constraints.md               # 契约层：全局约束，loom constraints 起草（提交）
│   ├── specs/                       # 契约层：模块 spec；端特有约束落这里（提交）
│   ├── config.toml                  # 状态层：模型/路由/预算（忽略）
│   ├── config.local.toml            # 状态层：本地实验覆盖（忽略）
│   ├── events.jsonl                 # 状态层：唯一事实源（忽略）
│   ├── tasks/
│   ├── ledger/
│   ├── runs/R-.../
│   └── ...
├── frontend/                        # 成员仓库 1（独立 git 仓库，gitignore 或子目录排除）
├── backend/                         # 成员仓库 2
├── mobile/                          # 成员仓库 3
└── .workspaces/                     # 跨仓库任务 worktree（gitignore）
    └── T-20261106-001/              # 一个任务一个目录
        ├── frontend/                # frontend 仓库的 worktree，分支 agent/T-20261106-001
        ├── backend/                 # backend 仓库的 worktree，分支 agent/T-20261106-001
        └── context/                 # 可选：任务级共享上下文（编译后的 packet 等）
```

要点：

- **单成员任务**只创建一个成员的 worktree（`.workspaces/<task>/<member>`），其余成员不动；
- **多成员任务**在同名下聚合，文件所有权必须不相交（沿用 M4 并行的约束）；
- `.workspaces/` 与 `.loom/` 都 gitignore；成员仓库本身是普通 clone，各自有自己的远端；
- 兼容现状：没有 `member` 的任务仍走旧的 `.worktrees/<task>/` 布局（单仓库项目零迁移）。

## 5. Manifest：`loom.workspace.toml`

```toml
schema_version = "1.0"

[workspace]
name = "my-platform"
control_repo = true          # 顶级目录本身是否是 git 仓库
default_base_ref = "main"

[[members]]
name = "frontend"
path = "frontend"
url = "git@github.com:acme/frontend.git"
default_branch = "main"
required = true              # 缺失时 workspace sync / doctor 报错
bare = false                 # true 则用 bare repo + worktree（后期选项）
setup = "pnpm install"       # worktree 建好后执行（带超时、写日志）
verify = "pnpm -r build"     # 成员级集成 verify（可选）

[[members]]
name = "backend"
path = "backend"
url = "git@github.com:acme/backend.git"
default_branch = "main"
setup = "pnpm install"

[defaults]
timeout_s = 1800
allowed_cross_members = ["frontend", "backend"]   # 允许出现在多成员任务里
```

字段说明：

| 字段 | 必填 | 说明 |
|---|---|---|
| `members[].name` | 是 | 成员标识，进入 TaskSpec / 事件 / 回执 |
| `members[].path` | 是 | 相对顶级目录的路径；禁止 `..` 与绝对路径 |
| `members[].url` | sync 时是 | 远端地址；本地已有仓库可省略 |
| `members[].default_branch` | 是 | worktree 的 `base_ref` 默认值 |
| `members[].required` | 否 | 缺失是否阻塞 |
| `members[].setup` | 否 | worktree 初始化命令（非交互、有超时、日志落盘） |
| `members[].verify` | 否 | 成员级集成验收命令 |
| `defaults.allowed_cross_members` | 否 | 多成员任务的白名单 |

> manifest 提交到 git（它是"人与工具的契约"）；`.loom/` 不提交（它是运行时状态）。这与 [03](03-core-protocols.zh-CN.md) §1.1 的提交/忽略表一致。

## 6. 协议增量（向后兼容）

只加**可选字段**，破坏性变更才升 `v2`：

### 6.1 TaskSpec.workspace

```json
{
  "workspace": {
    "mode": "worktree",
    "member": "frontend",
    "members": ["frontend", "backend"],
    "base_ref": "main",
    "branch": "agent/T-20261106-001",
    "allowed_paths": ["src/**"],
    "forbidden_paths": ["migrations/**"]
  }
}
```

| 字段 | 说明 |
|---|---|
| `member` | 单成员任务（默认模式）；缺省 = 单仓库项目，走旧布局 |
| `members` | 多成员任务（与 `member` 二选一）；每个成员建同名分支 worktree |
| `base_ref` | 缺省取 manifest 的 `default_branch` |
| `branch` | 跨成员统一；缺省由 orchestrator 生成 `agent/<task_id>` |

校验增量：`member`/`members` 必须在 manifest 中存在且 ∈ `allowed_cross_members`（若配置）；`members` 长度 ≥ 1 且去重；多成员任务的 `allowed_paths` 需按成员命名空间写（如 `frontend/src/**`），或每个成员一份（实现时定，见 §11 开放问题）。

### 6.2 事件 `data`

`task.dispatched` / `task.started` / `task.receipt` / `task.merged` 的 `data` 增加：

```json
{
  "member": "frontend",
  "members": ["frontend", "backend"],
  "branch": "agent/T-20261106-001",
  "sha": "7f3c2a1",
  "workspaces": {"frontend": ".workspaces/T-20261106-001/frontend"}
}
```

`task.merged` 的 `data` 用 `merge_sha` 的 per-member 映射：`{"frontend": "aaa1111", "backend": "bbb2222"}`（无跨端原子提交，见 §8.3）。

### 6.3 Receipt

结构化回执增加可选字段：

```json
{
  "branches": [
    {"member": "frontend", "branch": "agent/T-20261106-001", "head_sha": "7f3c2a1"},
    {"member": "backend",  "branch": "agent/T-20261106-001", "head_sha": "9aa0b22"}
  ]
}
```

四行回执的 `branch:` 行在多成员任务下写 `branch: agent/T-...@<sha>`（取主成员 sha，主成员 = `members[0]`），详情进 `branches[]` 与 report。**四行格式不变**，避免破坏解析器。

### 6.4 Schema 文件

- `schemas/task-spec.v1.json`：`workspace` 增加可选 `member` / `members`；
- `schemas/event.v1.json`：`data` 本就是自由对象，无需改；
- `schemas/receipt.v1.json`：增加可选 `branches`；
- 新增 `schemas/workspace-manifest.v1.json`。

## 7. 命令面（新增）

```bash
loom workspace init                    # 从当前目录发现成员仓库，生成 loom.workspace.toml
loom workspace sync [--member M]       # clone/fetch，检出 default_branch，跑 setup
loom workspace status [--json]         # 每成员 branch/sha/dirty/ahead/behind + 被哪个任务占用
loom workspace worktree add <task>     # 为任务建跨成员同名分支 worktree
loom workspace worktree remove <task>
loom workspace worktree list
```

`loom workspace status --json` 输出（给 agent 读，替代逐目录 `git status`）：

```json
{
  "members": [
    {
      "name": "frontend",
      "path": "frontend",
      "branch": "main",
      "sha": "7f3c2a1",
      "dirty": 3,
      "ahead": 0,
      "behind": 2,
      "occupied_by": null
    },
    {
      "name": "backend",
      "path": "backend",
      "branch": "agent/T-20261106-001",
      "sha": "9aa0b22",
      "dirty": 0,
      "ahead": 1,
      "behind": 0,
      "occupied_by": "T-20261106-001"
    }
  ],
  "worktrees": [
    {"task_id": "T-20261106-001", "members": ["backend"], "path": ".workspaces/T-20261106-001"}
  ]
}
```

`loom dispatch` 的行为变化：

1. 解析 `TaskSpec.workspace.member(s)`；
2. 为每个成员准备 worktree（同名分支），执行 `setup`（带超时、日志）；
3. **worker 进程 cwd = 成员 worktree**（单成员）或聚合目录（多成员）；
4. 事件记录 per-member 路径与 sha；
5. `loom verify` 在同样的 worktree 里重跑 acceptance，逐成员收集证据。

`loom status` 顶层增加一行成员概览（`frontend@main(3 dirty) backend@agent/T-...`）。

### 7.1 agent 工具如何看到分支/ worktree（cwd 契约）

2026-11-06 本机实测（git 标准行为，与具体 agent 工具无关）：

| agent 的 cwd | `git branch --show-current` | 实际表现 | 判定 |
|---|---|---|---|
| 成员 worktree 内（`.workspaces/T-001/frontend/`） | `agent/T-001` | `--show-toplevel`/`status --porcelain=v2` 全部正确 | ✅ 原生支持 |
| 成员主 clone 内（`frontend/`） | `main` | 正确 | ✅ |
| 聚合目录（`.workspaces/T-001/`，非 git 目录） | meta 仓库的分支 | find-up 到控制面仓库，**显示完全错误** | ❌ |
| bare 仓库根（bare 模式的 `frontend/`） | `main`（HEAD 指向） | `git status` fatal "必须在工作区中运行" | ⚠️ 看似正常实不可用 |

结论与硬约束：

1. **worktree 对原生 git 工具完全透明**——`.git` 是标准 gitdir 指针，任何用 `git` 命令的 agent 都能正确显示分支。不需要为工具做适配。
2. **分支不可见的根因是 cwd 归属，不是 worktree**：cwd 落在控制面仓库时，git find-up 只会看到 meta 仓库的分支。因此 **dispatch 必须把 worker 进程 cwd 落到成员 checkout 内**（P0 验收标准：在 worktree 里 `git branch --show-current` = `agent/<task_id>`，且 agent 界面显示同一分支）。
3. **聚合目录（多成员任务）不承诺原生 git 显示**：单 cwd 无法同时表达两个仓库的分支。此时 agent 只能读 `loom workspace status --json`（等价于 Canopy 的 MCP 能力，走 D7 的 CLI 契约）。聚合目录下放 `.loom-workspace.json` marker（记录 task/member 映射），供 Loom 自己的工具识别，不依赖 git find-up。
4. **bare 模式（P3）必须先挂好 worktree 再交给 agent**：裸仓库根目录的 `status`/`diff` 不可用，`branch --show-current` 还会给出误导性的正常输出。
5. **会话启动 cwd 要一次到位**：部分 agent 工具在启动时缓存 repo root，中途 cd 不刷新。dispatch 在生成 packet 时把 `cwd` 写进事件与 manifest，`loom workspace status` 提供启动前自检。

## 8. 跨端任务怎么拆

### 8.1 模式 A：契约先行（推荐，覆盖 90% 场景）

```text
T-001 [frontier, survey/spec-tests]  冻结接口契约（openapi/schema）
   ├── T-002 [worker, frontend]  按契约实现前端  ┐ 可并行（依赖 T-001）
   ├── T-003 [worker, backend]   按契约实现后端  ┘
   └── T-004 [worker, mobile]    按契约实现移动端
T-005 [frontier, review]  集成 verify（跨端联调验收）
```

- 每个任务**单成员**，文件所有权天然不相交；
- 跨端一致性由**冻结契约**保证，不由"agent 互相看代码"保证；
- 集成 verify 在成员各自的仓库跑 + 一个跨端 smoke（起前后端打真实请求）。

### 8.2 模式 B：单任务多成员（例外）

适合"一个改动同时碰两端且拆不开"的场景（如改一个跨端共享的错误码约定 + 两端调用点）：

- `members = ["frontend", "backend"]`，同名分支聚合 worktree；
- 一个 worker 在聚合目录里工作，回执 `branches[]` 记两个 sha；
- 硬约束：**文件所有权不相交**（`allowed_paths` 按成员命名空间），否则回退到模式 A。

### 8.3 合并与跨端原子性

- 每个成员独立合并（独立 PR/merge queue），`task.merged` 记录 per-member `merge_sha`；
- **没有跨仓库原子提交**——这是 multi-repo 的固有代价，用两层兜底：
  1. 契约冻结（T-001）保证接口不漂移；
  2. 集成 verify（T-005）保证合入后端到端仍通过；
- 若某次变更必须原子合入，说明它属于"应合并仓库"的信号，写进 `DECISION.md` 再议（D1 的例外路径）。

## 9. Token 与通讯预算

| 浪费源 | 对策 |
|---|---|
| 每个 agent 重复叙述进度 | 进度只写事件日志；`PROGRESS.md` 是确定性生成的视图；回执四行 |
| 逐目录 `git status` 拼图 | `loom workspace status --json` 一次读取（几十行） |
| agent 互相转述问题 | 跨端问题 = `task.blocked`/`task.escalated` 事件 + 证据包路径，不贴日志 |
| 上下文里混入多仓库全量状态 | Context Compiler 的"依赖摘要"按成员折叠：`frontend@agent/T-... [passed]` 一行一个 |
| worktree 重复装依赖 | `setup` 命令 + 包管理器缓存（pnpm store / cargo target 共享），必要时每成员 `target/` 共享挂载 |

原则不变（[04](04-context-and-cost-model.zh-CN.md) §3）：**结构化、按引用、可折叠**；多仓库不新增"聊天"这一层。

## 10. 风险与开放问题

| 风险/问题 | 缓解/待定 |
|---|---|
| worktree 每次都要装依赖，慢且费 token | `setup` hook + 缓存；后期评估 bare repo + 共享 `node_modules` 挂载（有符号链接风险，需 spike） |
| 分支不可见（agent 工具不认新布局） | 根因是 cwd 归属而非 worktree，实测与硬约束见 §7.1：dispatch 强制 worker cwd 落成员 checkout；聚合目录靠 `workspace status --json` |
| 跨成员任务的 `allowed_paths` 命名空间 | 待定：`member:path` 前缀 vs 每成员一个 workspace 对象（倾向后者，见 §11 P1） |
| 合并顺序导致中间态坏掉 | 集成 verify 作为门禁；跨端任务合并顺序由 orchestrator 排（契约任务先合） |
| 成员仓库被 agent 误改 `.loom`/manifest | 沿用 [08](08-design-roadmap-v0.2.zh-CN.md) §4.3 防覆写：受保护路径 + 派发前后哈希比对 |
| manifest 与 `.loom` 的版本漂移 | `loom doctor` 校验 manifest schema 与成员可达性 |
| 与 monorepo 的迁移路径 | manifest 单成员即 monorepo；不需要特殊改造 |

## 11. 分期（建议 M5，待排期）

| 期 | 内容 | 退出标准 |
|---|---|---|
| **P0** | `loom.workspace.toml` + `loom workspace init/sync/status`；`TaskSpec.workspace.member`；dispatch 单成员 cwd 落到成员 worktree | 单成员任务端到端（含分支可见）；`workspace status --json` 稳定 |
| **P1** | 跨成员同名分支 worktree（`.workspaces/<task>/<member>`）；回执 `branches[]`；per-member 事件 | 模式 B 任务端到端；合并事件记录多 sha |
| **P2** | `setup` hook + 依赖缓存；成员级 verify + 跨端集成 verify | 第二次 dispatch 的 setup 时间显著下降；集成 verify 可用 |
| **P3** | bare repo 选项、`meta exec` 式批量操作、与 goose/OpenCode 的多成员 canary | 按需，不默认排期 |

**不提前做**：把成员仓库合并成 monorepo、跨仓库原子提交工具、MCP 状态服务（用 CLI JSON 等价实现）。

## 12. 与现有文档的关系

- [03](03-core-protocols.zh-CN.md)：只加可选字段（§6），核心语义不变；
- [08](08-design-roadmap-v0.2.zh-CN.md) §4 目录布局：`.worktrees/` 扩展为 `.workspaces/<task>/<member>/`（单成员兼容旧路径）；路线图表加 M5 行；
- [04](04-context-and-cost-model.zh-CN.md)：Context Compiler 的依赖摘要按成员折叠；
- 本设计不改变 D1–D3（控制面自研、进程级契约、状态是数据库）。
