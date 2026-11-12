# 13 Multi-Repo Workspace and Multi-Client Collaboration

> Language: **English** | [简体中文](13-multi-repo-workspace-and-multi-client.zh-CN.md)

> Status: **design draft, not implemented** (2026-11-06).  
> Goal: turn "one top-level directory + repos for multiple platforms + several agents" into a controllable Loom workspace instead of three or four sessions that do not talk to each other.  
> Prerequisite reading: [08-design-roadmap-v0.2.md](08-design-roadmap-v0.2.md) §4 (state ownership), [03-core-protocols.md](03-core-protocols.md) (TaskSpec/events/receipts).

## 0. One-line conclusion

**Do not migrate to a monorepo.** Use a multi-repo workspace of "meta control plane + manifest member repos + same-name-branch aggregated worktrees" to extend Loom's existing externalized state, event log, task dependencies, and four-line receipt to multi-platform scenarios; cross-platform sync runs on **structured state and events**, not on agents passing messages to each other.

## 1. Problem

Current working style and pain points:

| Current state | Consequence |
|---|---|
| Top-level directory clones frontend/backend/mobile and other repos | when a task is dispatched from the top-level directory, the agent's cwd is not inside any member repo |
| One agent per platform, running in different directories | they cannot see each other's progress and problems; only manual relaying by humans |
| Progress told through conversation narration | the same fact gets repeated N times, wasting tokens; and it drifts |
| Top-level directory cannot use git worktree | a worktree boundary is a repo boundary; it cannot cross repos |
| An agent's own git tools cannot see the current branch | branch state is scattered across directories and cannot be aligned at a glance |

**What is essentially missing is not a monorepo but a "workspace" abstraction**:

```text
workspace = 1 control plane (Loom state/tasks/events) + N member checkouts (each an independent git repo)
```

## 2. Survey

### 2.1 monorepo vs multi-repo vs meta-repo

| Option | In one line | Pros | Cost |
|---|---|---|---|
| monorepo | every platform in one repo | atomic cross-platform commits, unified versioning, shared toolchain | coarse permission granularity, release cycles constrain each other, repo size, migration cost |
| multi-repo (polyrepo) | one repo per platform | clear ownership, independent permissions/releases, freedom of tech stack | cross-platform changes need multi-repo coordination, version combination explosion |
| meta-repo / manifest | multiple repos + one manifest repo orchestrating them | keeps multi-repo independence while gaining a unified workspace and state | needs tool support (that is exactly what this design builds) |

Judgment: **the user's scenario is "intentionally separated platforms"** (frontend/backend/mobile, with different release cycles and permissions all along), which is exactly where multi-repo applies. Most of the monorepo's core benefits (atomic commits, unified versioning) can be substituted in AI collaboration by "task graph + frozen contracts + integration verify", while its costs are real.

> Exception: if one day "cross-platform atomic commits" becomes a hard requirement (e.g. an interface change must merge in the same PR), then re-evaluate merging repos. Loom's design does not depend on that choice — the manifest pattern equally holds for a single member (i.e. a monorepo).

### 2.2 Existing multi-repo tools (2025–2026 field practice)

| Tool | Approach | Relation to this design |
|---|---|---|
| [`meta` / `metarepo`](https://github.com/codyaverett/metarepo) (Rust) | `.meta` manifest; bare repo + worktree; `meta worktree add <branch>` **creates a same-name-branch worktree across all members**; `meta exec` runs across repos | a mature pattern for same-name-branch aggregated worktrees |
| [`git-worktree-manager`](https://github.com/nanasess/git-worktree-manager) (Bash) | batch-manages multi-repo worktrees; worktrees live **outside the project** at `project.worktrees/<branch>/<member>/`; a hook installs dependencies | a direct reference for directory layout and "staying clear of code search" |
| [Canopy](https://www.ricky-dev.com/coding/2026/01/agentic-tooling-across-multiple-repositories) (MCP server) | one call returns structured JSON of branch/status for all members; `.canopy/worktrees/<task>/<member>` same-name branches | the pattern for "structured state replacing per-repo `git status`" |
| `worktrunk` / `git-worktree-runner` | worktree orchestration for AI parallel development | an AI-side solution to the same class of problem |
| [Ricky Smith's manual approach](https://www.ricky-dev.com/coding/2026/01/agentic-tooling-across-multiple-repositories) | aggregates multi-repo worktrees with a "parent directory named after the branch", holding task context in the parent directory | proves this layout works in real agent workflows |

The **common pattern** of these tools (adopted by this design):

1. **Same-name branches across repos**: one task, one branch name (`agent/<task_id>`); every member repo involved checks out a branch with that name;
2. **Worktrees live outside**: worktrees are not placed inside repos, so as not to pollute the agent's code search/index (Loom's `.worktrees/` already follows this idea);
3. **Manifest-driven**: the member list is a declarative file, not human memory;
4. **Structured state queries**: hand the agent one JSON instead of making it parse `git status` text per directory.

### 2.3 Implications for AI agent workflows

- **The worktree boundary = the repo boundary**. Cross-repo tasks cannot be solved by a single worktree; they must be simulated with "directory aggregation + same-name branches".
- **The agent's cwd must be inside a member checkout**. Control-plane commands (`loom dispatch` etc.) are issued from the top-level directory, but the worker process must land in the member worktree; only then do branches, diffs, and LSP all behave.
- **Structured state saves tokens and misreads versus text**: one `loom workspace status --json` (a few dozen lines) replaces N `git status` calls plus manual jigsaw work.
- **Do not add a new agent-to-agent communication channel**. The right carrier for cross-platform sync is: event log (single source of truth) + task dependencies + four-line receipt — Loom already has all of it; this design only makes it recognize "members".

## 3. Design decisions

| # | Decision | Rationale |
|---|---|---|
| D1 | **No monorepo migration** | multi-platform independence is a real need; the monorepo's benefits can be substituted by contracts + task graph |
| D2 | Top-level directory = **meta control plane repo** | `loom.workspace.toml` and `.loom/` both live here (`.loom/` whitelist layering: contract layer `goal.md`/`constraints.md`/`specs/` is committed, state layer ignored, see `AGENTS.md` §6); member code is independent clones and is not committed into the control plane |
| D3 | Member repos = independent clones, paths declared by the manifest | keeps permissions/releases/toolchains independent per platform |
| D4 | Cross-repo task = **same-name-branch aggregated worktree** | `.workspaces/<task_id>/<member>/`, each member is a worktree of its own repo; the default is a **single-member task**, multi-member tasks are the exception |
| D5 | Cross-platform sync = event log + dependencies + receipts | no agent-to-agent chat, no MCP (keeping D2's process-level contract) |
| D6 | Branch naming `agent/<task_id>` unified across members | aligns merges, locates conflicts, and lets the `task.merged` event record per-member shas |
| D7 | Structured state goes over CLI JSON, not MCP plugins | consistent with [08](08-design-roadmap-v0.2.md) D2: the stable contract is process + files + JSON + exit code |
| D8 | Top-level goal/constraints hold only **platform-level** content; platform-specific constraints go in `.loom/specs/<platform>/`; member repos may carry their own `.loom/goal.md` / `constraints.md`, and dispatch picks by the member repo the task belongs to | keeps the top level from cluttering (goal/constraints are re-injected every step, each with a 1000 token cap; `loom doctor` warns over cap); context stays self-sufficient when a member works independently. **Not implemented, lands in M5** |

## 4. Directory layout

```text
workspace-root/                      # top-level directory = meta control-plane repo (committed to git)
├── loom.workspace.toml              # member manifest (committed)
├── .loom/                           # whitelist layering (AGENTS.md §6): contract layer committed, state layer ignored
│   ├── README.md                    # contract layer: layering notes and ownership table (committed)
│   ├── goal.md                      # contract layer: platform-level goals, drafted by loom goal (committed)
│   ├── constraints.md               # contract layer: global constraints, drafted by loom constraints (committed)
│   ├── specs/                       # contract layer: module specs; platform-specific constraints land here (committed)
│   ├── config.toml                  # state layer: model/routing/budget (ignored)
│   ├── config.local.toml            # state layer: local experiment overrides (ignored)
│   ├── events.jsonl                 # state layer: single source of truth (ignored)
│   ├── tasks/
│   ├── ledger/
│   ├── runs/R-.../
│   └── ...
├── frontend/                        # member repo 1 (independent git repo, gitignored or excluded as a subdirectory)
├── backend/                         # member repo 2
├── mobile/                          # member repo 3
└── .workspaces/                     # cross-repo task worktrees (gitignored)
    └── T-20261106-001/              # one directory per task
        ├── frontend/                # worktree of the frontend repo, branch agent/T-20261106-001
        ├── backend/                 # worktree of the backend repo, branch agent/T-20261106-001
        └── context/                 # optional: task-level shared context (compiled bounded context packets etc.)
```

Key points:

- a **single-member task** creates a worktree for only one member (`.workspaces/<task>/<member>`); the other members are untouched;
- **multi-member tasks** aggregate under the same name; file ownership must be disjoint (keeping M4's parallel constraints);
- `.workspaces/` and `.loom/` are both gitignored; the member repos themselves are ordinary clones, each with its own remote;
- backward compatible with today: tasks without a `member` still use the old `.worktrees/<task>/` layout (zero migration for single-repo projects).

## 5. Manifest: `loom.workspace.toml`

```toml
schema_version = "1.0"

[workspace]
name = "my-platform"
control_repo = true          # whether the top-level directory is itself a git repo
default_base_ref = "main"

[[members]]
name = "frontend"
path = "frontend"
url = "git@github.com:acme/frontend.git"
default_branch = "main"
required = true              # when missing, workspace sync / doctor errors
bare = false                 # if true, use bare repo + worktree (a later option)
setup = "pnpm install"       # run after the worktree is created (with timeout, writes logs)
verify = "pnpm -r build"     # member-level integration verify (optional)

[[members]]
name = "backend"
path = "backend"
url = "git@github.com:acme/backend.git"
default_branch = "main"
setup = "pnpm install"

[defaults]
timeout_s = 1800
allowed_cross_members = ["frontend", "backend"]   # allowed to appear in multi-member tasks
```

Field notes:

| Field | Required | Description |
|---|---|---|
| `members[].name` | yes | member identifier, enters TaskSpec / events / receipts |
| `members[].path` | yes | path relative to the top-level directory; `..` and absolute paths are forbidden |
| `members[].url` | yes when syncing | remote address; may be omitted when the repo exists locally |
| `members[].default_branch` | yes | default `base_ref` for worktrees |
| `members[].required` | no | whether absence blocks |
| `members[].setup` | no | worktree initialization command (non-interactive, with timeout, logs written to disk) |
| `members[].verify` | no | member-level integration acceptance command |
| `defaults.allowed_cross_members` | no | whitelist for multi-member tasks |

> The manifest is committed to git (it is the "contract between people and tools"); `.loom/` is not committed (it is runtime state). This matches the commit/ignore table in [03](03-core-protocols.md) §1.1.

## 6. Protocol increments (backward compatible)

Only **optional fields** are added; breaking changes bump to `v2`:

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

| Field | Description |
|---|---|
| `member` | single-member task (the default mode); absent = single-repo project, uses the old layout |
| `members` | multi-member task (mutually exclusive with `member`); a same-name-branch worktree per member |
| `base_ref` | defaults to the manifest's `default_branch` |
| `branch` | unified across members; defaults to orchestrator-generated `agent/<task_id>` |

Validation increments: `member`/`members` must exist in the manifest and be ∈ `allowed_cross_members` (if configured); `members` length ≥ 1 and deduplicated; `allowed_paths` for a multi-member task must be written in member namespaces (e.g. `frontend/src/**`), or one set per member (decide at implementation time, see §11 open questions).

### 6.2 Event `data`

The `data` of `task.dispatched` / `task.started` / `task.receipt` / `task.merged` gains:

```json
{
  "member": "frontend",
  "members": ["frontend", "backend"],
  "branch": "agent/T-20261106-001",
  "sha": "7f3c2a1",
  "workspaces": {"frontend": ".workspaces/T-20261106-001/frontend"}
}
```

The `data` of `task.merged` uses a per-member mapping of `merge_sha`: `{"frontend": "aaa1111", "backend": "bbb2222"}` (no cross-platform atomic commit; see §8.3).

### 6.3 Receipt

The structured receipt gains optional fields:

```json
{
  "branches": [
    {"member": "frontend", "branch": "agent/T-20261106-001", "head_sha": "7f3c2a1"},
    {"member": "backend",  "branch": "agent/T-20261106-001", "head_sha": "9aa0b22"}
  ]
}
```

For multi-member tasks the `branch:` line of the four-line receipt writes `branch: agent/T-...@<sha>` (taking the primary member's sha; primary member = `members[0]`), with details going into `branches[]` and the report. **The four-line format does not change**, to avoid breaking parsers.

### 6.4 Schema files

- `schemas/task-spec.v1.json`: `workspace` gains optional `member` / `members`;
- `schemas/event.v1.json`: `data` is already a free-form object, no change needed;
- `schemas/receipt.v1.json`: gains optional `branches`;
- new `schemas/workspace-manifest.v1.json`.

## 7. Command surface (new)

```bash
loom workspace init                    # discover member repos from the current directory, generate loom.workspace.toml
loom workspace sync [--member M]       # clone/fetch, check out default_branch, run setup
loom workspace status [--json]         # per-member branch/sha/dirty/ahead/behind + which task occupies it
loom workspace worktree add <task>     # create cross-member same-name-branch worktrees for the task
loom workspace worktree remove <task>
loom workspace worktree list
```

`loom workspace status --json` output (for agents to read, replacing per-directory `git status`):

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

Behavioral changes in `loom dispatch`:

1. parse `TaskSpec.workspace.member(s)`;
2. prepare a worktree (same-name branch) for each member and run `setup` (with timeout, logs);
3. **worker process cwd = member worktree** (single member) or the aggregation directory (multi-member);
4. events record per-member paths and shas;
5. `loom verify` re-runs acceptance in the same worktrees, collecting evidence per member.

`loom status` gains a member overview line at the top (`frontend@main(3 dirty) backend@agent/T-...`).

### 7.1 How agent tools see branches/worktrees (the cwd contract)

Measured locally on 2026-11-06 (standard git behavior, independent of any particular agent tool):

| agent's cwd | `git branch --show-current` | Actual behavior | Verdict |
|---|---|---|---|
| inside a member worktree (`.workspaces/T-001/frontend/`) | `agent/T-001` | `--show-toplevel` / `status --porcelain=v2` all correct | ✅ natively supported |
| inside a member's main clone (`frontend/`) | `main` | correct | ✅ |
| aggregation directory (`.workspaces/T-001/`, not a git directory) | the meta repo's branch | find-up lands on the control repo, **displays completely wrong** | ❌ |
| bare repo root (`frontend/` in bare mode) | `main` (what HEAD points to) | `git status` fatal "must be run in a working tree" | ⚠️ looks fine but unusable |

Conclusions and hard constraints:

1. **Worktrees are fully transparent to native git tools** — `.git` is a standard gitdir pointer, and any agent using `git` commands displays branches correctly. No per-tool adaptation is needed.
2. **The root cause of invisible branches is cwd ownership, not worktrees**: when the cwd lands in the control repo, git's find-up only sees the meta repo's branch. Therefore **dispatch must put the worker process cwd inside a member checkout** (P0 acceptance criteria: inside the worktree, `git branch --show-current` = `agent/<task_id>` and the agent's UI shows the same branch).
3. **Aggregation directories (multi-member tasks) make no promise of native git display**: a single cwd cannot express the branches of two repos at once. In that case the agent can only read `loom workspace status --json` (equivalent to Canopy's MCP capability, over D7's CLI contract). A `.loom-workspace.json` marker (recording the task/member mapping) is placed in the aggregation directory so Loom's own tools can recognize it, without relying on git find-up.
4. **Bare mode (P3) must have worktrees mounted before being handed to the agent**: at a bare repo root, `status` / `diff` are unusable and `branch --show-current` gives misleadingly normal output.
5. **The session's startup cwd must be right the first time**: some agent tools cache the repo root at startup and do not refresh on mid-session cd. When generating a packet, dispatch writes `cwd` into the events and manifest, and `loom workspace status` provides a pre-start self-check.

## 8. How to split cross-platform tasks

### 8.1 Pattern A: contracts first (recommended, covers 90% of cases)

```text
T-001 [frontier, survey/spec-tests]  freeze the interface contract (openapi/schema)
   ├── T-002 [worker, frontend]  implement frontend per contract  ┐ can run in parallel (depends on T-001)
   ├── T-003 [worker, backend]   implement backend per contract   ┘
   └── T-004 [worker, mobile]    implement mobile per contract
T-005 [frontier, review]  integration verify (cross-platform joint debugging acceptance)
```

- every task is **single-member**, so file ownership is naturally disjoint;
- cross-platform consistency is guaranteed by the **frozen contract**, not by "agents reading each other's code";
- integration verify runs in each member's own repo plus one cross-platform smoke (starting frontend and backend and sending real requests).

### 8.2 Pattern B: one task, multiple members (exception)

Fits cases where "one change touches both ends at once and cannot be split" (e.g. changing a cross-platform shared error-code convention + call sites on both ends):

- `members = ["frontend", "backend"]`, same-name-branch aggregated worktree;
- one worker works in the aggregation directory; the receipt's `branches[]` records both shas;
- hard constraint: **file ownership is disjoint** (`allowed_paths` written per member namespace); otherwise fall back to Pattern A.

### 8.3 Merging and cross-platform atomicity

- each member merges independently (independent PR / merge queue); `task.merged` records per-member `merge_sha`;
- **there is no cross-repo atomic commit** — this is an inherent cost of multi-repo, covered by two layers:
  1. contract freezing (T-001) keeps interfaces from drifting;
  2. integration verify (T-005) keeps the end-to-end path passing after merge;
- if some change must merge atomically, that is a signal it belongs in "repos that should be merged"; write it into `DECISION.md` and revisit (D1's exception path).

## 9. Token and communication budget

| Waste source | Countermeasure |
|---|---|
| every agent re-narrating progress | progress is written only to the event log; `PROGRESS.md` is a deterministically generated view; four-line receipts |
| per-directory `git status` jigsaw | one `loom workspace status --json` read (a few dozen lines) |
| agents relaying problems to each other | cross-platform problems = `task.blocked` / `task.escalated` events + evidence pack paths, no pasted logs |
| full multi-repo state mixed into context | the Context Compiler's "dependency summary" folds per member: `frontend@agent/T-... [passed]`, one line each |
| worktrees reinstalling dependencies | `setup` command + package manager cache (shared pnpm store / cargo target), shared `target/` mounts per member when necessary |

The principle is unchanged ([04](04-context-and-cost-model.md) §3): **structured, by reference, foldable**; multi-repo does not add a "chatting" layer.

## 10. Risks and open questions

| Risk / question | Mitigation / pending |
|---|---|
| every worktree must install dependencies — slow and token-hungry | `setup` hook + cache; later evaluate bare repo + shared `node_modules` mount (symlink risk, needs a spike) |
| invisible branches (agent tools do not recognize the new layout) | the root cause is cwd ownership, not worktrees; measurements and hard constraints in §7.1: dispatch forces the worker cwd into the member checkout; aggregation directories rely on `workspace status --json` |
| `allowed_paths` namespace for multi-member tasks | pending: `member:path` prefixes vs one workspace object per member (leaning toward the latter, see §11 P1) |
| merge order breaking intermediate states | integration verify as a gate; cross-platform merge order scheduled by the orchestrator (contract tasks merge first) |
| agent accidentally modifying `.loom`/manifest in a member repo | keep [08](08-design-roadmap-v0.2.md) §4.3 overwrite protection: protected paths + hash comparison before and after dispatch |
| version drift between manifest and `.loom` | `loom doctor` validates the manifest schema and member reachability |
| migration path to a monorepo | a manifest with a single member is a monorepo; no special rework needed |

## 11. Phasing (proposed M5, pending scheduling)

| Phase | Content | Exit criteria |
|---|---|---|
| **P0** | `loom.workspace.toml` + `loom workspace init/sync/status`; `TaskSpec.workspace.member`; dispatch puts single-member cwd into the member worktree | single-member task end to end (including visible branch); `workspace status --json` stable |
| **P1** | cross-member same-name-branch worktrees (`.workspaces/<task>/<member>`); receipt `branches[]`; per-member events | Pattern B task end to end; merge event records multiple shas |
| **P2** | `setup` hook + dependency cache; member-level verify + cross-platform integration verify | setup time drops noticeably on the second dispatch; integration verify usable |
| **P3** | bare repo option, `meta exec`-style batch operations, multi-member canary with goose/OpenCode | on demand, not scheduled by default |

**Not doing early**: merging member repos into a monorepo, cross-repo atomic commit tooling, MCP state service (an equivalent CLI JSON implementation is used instead).

## 12. Relation to existing documents

- [03](03-core-protocols.md): only optional fields are added (§6), core semantics unchanged;
- [08](08-design-roadmap-v0.2.md) §4 directory layout: `.worktrees/` extends to `.workspaces/<task>/<member>/` (single member keeps the old path); add an M5 row to the roadmap table;
- [04](04-context-and-cost-model.md): the Context Compiler's dependency summary folds per member;
- this design does not change D1–D3 (self-built control plane, process-level contract, state is a database).
