# Loom

> Language: **English** | [简体中文](README.zh-CN.md)

> A token-efficient agent harness for software design, architecture, and development  
> Core thesis: **long tasks, short contexts.**

Loom is a standalone orchestration kernel: state lives in `.loom/`, tasks run in separate worktrees executed by swappable agents, and acceptance relies only on executable commands. Milestones M0–M3 (protocol productization, context and ledger, native executor, deterministic routing with same-tier fallback) and M4 parallelism (path locks, `[parallel]` caps/budgets, the `loom merge` queue, the `loom board` kanban; verified with 2 concurrent workers over 10 tasks) are done; real external adapters and real-model A/B remain blocked by the environment.

## Current Status

| Item | Status |
|---|---|
| Design docs | v0.2, see [doc/README.md](doc/README.md) |
| Tool output protocol | v0.1, see [doc/10-tool-output-protocol-v0.1.md](doc/10-tool-output-protocol-v0.1.md) |
| Rust workspace | `src/`, crates: `loom-core` / `loom-store` / `loom-tools` / `loom-context` / `loom-adapter` / `loom-native` / `loom-cli` |
| Implemented | `loom init/doctor/status`, `loom board`, `loom tool *`, `loom artifact`, `loom capability *`, `loom route`, `loom usage record/ingest`, `loom ledger`, `loom task *`, `loom event list`, `loom context compile`, `loom prompt *`, `loom adapter list/canary`, `loom dispatch` (mock/native), `loom verify`, `loom merge`, `loom resume` |
| Not implemented | Real goose/OpenCode adapter canary (requires installed binaries), native vs adapter A/B baseline, real billing reconciliation. See [doc/14-current-issues-and-blockers.md](doc/14-current-issues-and-blockers.md) |
| Designed but not implemented | Remaining churn governance (L1–L3 graded intervention, cost-aware restart; step-level projection, patch-first/write guards, token attribution and `loom churn stats` are implemented), the question/answer protocol and attention inbox, skills governance, agent/harness-driven local CI (hooks/review/attestation). See [doc/15-agent-runtime-governance-and-engineering.md](doc/15-agent-runtime-governance-and-engineering.md) |

## Quick Start

All Rust commands run inside a container; **do not use a host-installed Rust toolchain**:

```bash
./scripts/dev.sh cargo --version
./scripts/dev.sh cargo test --workspace
./scripts/dev.sh cargo run -p loom-cli -- --help
```

Or:

```bash
make test
make fmt
make clippy
```

The first run builds the `loom-dev` image (Rust + git + rustfmt/clippy). It prefers `podman` and falls back to `docker`; use `CONTAINER_ENGINE=docker` to force one.

Real provider credentials go in `.env.local` at the repository root (ignored by the `.env.*` rule in `.gitignore`; template in [.env.example](.env.example)). `scripts/dev.sh` only passes a whitelist of variables into the container, and child processes (`bash` tool, acceptance commands) strip every `*_API_KEY` / `*_TOKEN` / `*_SECRET`. Never commit keys or paste them into chat.

## Layout

```text
.
├── AGENTS.md                 # repository conventions (AI/human)
├── Containerfile             # dev container
├── Makefile                  # in-container build/test entry points
├── scripts/dev.sh            # podman/docker wrapper
├── doc/                      # design docs
├── schemas/                  # external JSON Schemas
└── src/
    ├── Cargo.toml            # workspace
    └── crates/
        ├── loom-core/        # protocol types, limits, config, TaskSpec/events/receipts
        ├── loom-store/       # .loom layout, event log, task index, artifacts, ledger
        ├── loom-tools/       # search/read/artifact implementations
        ├── loom-context/     # Context Compiler + versioned prompt library
        ├── loom-adapter/     # process-level adapter contract, mock/native/goose/opencode, canary
        ├── loom-native/      # native executor: tool loop, projection, budgets, no-progress
        └── loom-cli/         # loom binary
```

## Minimal Loop (what works today)

```bash
# at any project root
loom init
loom task add --file task.json        # validate and register a TaskSpec
loom route T-001                      # explain role/tier, model, budget, escalation
loom context compile T-001            # compile a bounded context packet
loom dispatch T-001 --adapter mock    # dispatch; native needs loom-native (built in the workspace)
loom verify T-001                     # re-run acceptance independently, write the receipt
loom resume                           # recover state from the event log
loom event list --task T-001
loom ledger --passed-tasks 10 --wall-seconds 1800
loom status
loom doctor
```

`loom tool *` emits a compact JSON envelope by default; add `--format pretty` for a human-readable terminal view. The human view is converted by tool code and never enters the model context.

### native executor

`loom-native` is a separate process inside the workspace, started by `loom dispatch --adapter native`:

```bash
# real provider (OpenAI-compatible)
export LOOM_API_KEY=...                 # or OPENAI_API_KEY
export LOOM_API_BASE=https://api.openai.com/v1
export LOOM_MODEL_ID=gpt-...            # or resolve via a --model alias
loom dispatch T-001 --adapter native

# offline scripted run (tests/CI, no network needed)
LOOM_NATIVE_SCRIPT=/path/to/script.json loom dispatch T-001 --adapter native --in-place
```

Optional environment variables: `LOOM_NATIVE_BIN` (binary path), `LOOM_NATIVE_MAX_TOKENS`, `LOOM_NATIVE_MAX_IDLE`, `LOOM_NATIVE_MAX_VERIFY_ROUNDS`, `LOOM_NATIVE_TEMPERATURE`, `LOOM_NATIVE_SKIP_ACCEPTANCE`, `LOOM_NATIVE_DEBUG` (dump requests/responses into the run log), `LOOM_NATIVE_HISTORY_BYTES` / `LOOM_NATIVE_KEEP_TURNS` / `LOOM_NATIVE_MIN_CLEAR` (step-level projection, 0 disables; see [doc/15](doc/15-agent-runtime-governance-and-engineering.md) §1).

- Acceptance runs pre-emptively inside native; on failure the command, exit code, and truncated output are fed back to the model, by default at most 2 rounds (`LOOM_NATIVE_MAX_VERIFY_ROUNDS`, minimum 1); once rounds are exhausted the task is marked `failed`.
- Cost: when the provider returns a cost it is recorded with `cost_source=provider`; otherwise the dispatched model's `ModelCapability` price table is used for an estimate, recorded as `pricing_table`; if the price is unknown `cost_usd` is omitted and recorded as `unknown`. Estimated cost still counts against `TaskBudget.max_cost_usd`; exceeding the limit stops with `budget_exceeded` / exit 3.
- `loom dispatch` reads the previous run's failed receipt and `verify.log` for the same task, builds a failure packet, and injects it into the Context Compiler so the next run sees the previous failure's command and output directly.
- `final.md` written by native is only a claim; the final state is still whatever `loom verify` says.

### Routing and escalation

Routing is deterministic; no LLM decides task difficulty. In `.loom/config.toml`, register model aliases under `[[models]]` (each alias may point at a different relay), then configure routing with `role → tier` mappings, escalation thresholds, fallbacks, and per-tier cost limits:

```toml
[[models]]
alias = "worker"
model_id = "deepseek-chat"        # real provider-side model name
tier = "worker"
wire = "chat"                     # chat | messages; responses not implemented
tool_calls = true
base_url = "https://api.deepseek.com/v1"   # optional: overrides global LOOM_API_BASE
key_env = "DEEPSEEK_API_KEY"      # optional: which env var holds the key for this model (only the name is stored; the value lives in .env.local)
input_usd_per_mtok = 0.27         # price table is optional; without it cost is recorded as unknown
output_usd_per_mtok = 1.10

[routing]
escalate_after_failures = 2      # escalate after the worker fails 2 times in a row
max_escalations_per_task = 1

[routing.roles]
review = "frontier"
judgment = "frontier"
implement = "worker"
docs = "worker"

[routing.fallbacks]
worker = ["worker-b"]        # must be a configured same-tier alias; cross-tier is rejected

[routing.cost_limits]
worker = 0.25
frontier = 1.0
```

Priority: explicit `--model` > `[routing.roles]` mapping > the TaskSpec's own `tier`. Escalation only counts `task.failed` events.

`fallbacks` are **same-tier redundancy**: only availability failures (adapter exit code 4 or timeout) switch; `budget_exceeded` / `needs_input` / `blocked` do not — switching models cannot un-spend the budget already burned. Every switch writes `task.fallback` (`run_id` points to the new attempt's run, `parent_run_id` to the failed run) and opens a separate run for the new attempt, so receipts and usage never overwrite each other; `attempt` stays unchanged, so escalation thresholds are not polluted. An explicit `--model` pins the alias, which sets `fallback_enabled=false` and disables switching. If a task's budget exceeds the tier limit before dispatch, or the worst-case cost estimated from the price table exceeds it, `loom dispatch` refuses.

```bash
loom route T-001                 # JSON: tier, model, reasons, fallback, budget, estimates
loom route T-001 --format pretty
loom route stats                 # worker share, escalation/switch counts, routing reasons from the event log
```

`route stats` computes the worker share from each task's last dispatch; retries count toward `dispatches` but do not double-count tasks; `fallbacks` counts `task.fallback` events. This is the direct data source for the "worker ≥50%" M3 exit criterion.

## Design Entry Points

- Read only one: [08-design-roadmap-v0.2.md](doc/08-design-roadmap-v0.2.md)
- Search / knowledge base: [09-code-search-and-knowledge-base.md](doc/09-code-search-and-knowledge-base.md)
- Tool protocol: [10-tool-output-protocol-v0.1.md](doc/10-tool-output-protocol-v0.1.md)
- Multi-repo workspace (design draft): [13-multi-repo-workspace-and-multi-client.md](doc/13-multi-repo-workspace-and-multi-client.md)
- Current issues and blockers: [14-current-issues-and-blockers.md](doc/14-current-issues-and-blockers.md)
- Runtime governance (design draft): [15-agent-runtime-governance-and-engineering.md](doc/15-agent-runtime-governance-and-engineering.md)
- Doc index: [doc/README.md](doc/README.md)

## Contributing

Contributions are welcome; read [CONTRIBUTING.md](CONTRIBUTING.md) first. All participation is governed by the [Code of Conduct](CODE_OF_CONDUCT.md). To report a security issue, see [SECURITY.md](SECURITY.md).

## License

Copyright (c) 2026 The Loom authors. All rights reserved.

Licensed under the [PolyForm Noncommercial License 1.0.0](LICENSE) (SPDX: `PolyForm-Noncommercial-1.0.0`). You may use, modify, and share this software for noncommercial purposes; commercial use requires separate permission from the copyright holder. This is a provisional license: it may be replaced once final licensing is decided, at which point this section will be updated.
