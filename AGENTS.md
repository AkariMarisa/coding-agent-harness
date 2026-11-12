# AGENTS.md

This file is the shared convention for AI agents and human collaborators in this repository.  
The goals: keep Loom's control plane thin, the protocol stable, and the token budget verifiable.

## 1. What to Read First

In order:

1. `doc/08-design-roadmap-v0.2.md` (highest-priority design currently)
2. `doc/14-current-issues-and-blockers.md` (what cannot be proven / done yet)
3. `doc/09-code-search-and-knowledge-base.md` (retrieval strategy and boundaries)
4. `doc/10-tool-output-protocol-v0.1.md` (tool envelope and limits)
5. `schemas/tool-result.v1.json` (the external contract)

Older docs `doc/02`–`doc/07` remain valid, but where they conflict with `08/09/10`, the newer docs win.

## 2. Environment: Containers Only

**Do not use a host-installed Rust / cargo / rustc.**

Unified entry points:

```bash
./scripts/dev.sh cargo build --workspace
./scripts/dev.sh cargo test --workspace
./scripts/dev.sh cargo fmt --all
./scripts/dev.sh cargo clippy --workspace --all-targets -- -D warnings
```

Or `make build/test/fmt/clippy/check`.

- `podman` is preferred, `docker` is the fallback.
- The image is built from `Containerfile`; dependency cache lives in `.cache/cargo`.
- Never commit `target/`, `.cache/`, `.loom/` state.

## 3. Code Layout

```text
src/
├── Cargo.toml          # workspace; register new crates here
└── crates/
    ├── loom-core       # pure types/constants/config/estimation; avoid IO
    ├── loom-store      # .loom file layout, artifacts, telemetry, dedup
    ├── loom-tools      # search/read/artifact tool logic
    ├── loom-context    # Context Compiler + versioned prompt library
    ├── loom-adapter    # process-level adapter contract, mock/native/goose/opencode, canary
    ├── loom-native     # native executor: tool loop, result projection, budgets, no-progress
    └── loom-cli        # clap commands and output formatting
```

Boundaries:

- `loom-core` does not depend on `loom-store/tools/cli`.
- `loom-tools` depends on `core` + `store` and does not parse CLI arguments directly.
- `loom-cli` only parses arguments, calls into the others, formats output, and sets exit codes.
- Later `loom-context` / `loom-adapter-goose` are added under the same boundaries.

## 4. The Protocol Is a Contract

- `schemas/tool-result.v1.json` is the contract between models and tools.
- When changing fields, short-key meanings, limits, or exit codes, **you must update all of**:
  1. the `loom-core` types;
  2. `schemas/tool-result.v1.json`;
  3. `doc/10-tool-output-protocol-v0.1.md`;
  4. at least one test or example.
- Incremental fields only; a breaking change bumps to `v2` — never silently change semantics.
- Producers conform strictly to the schema; consumers ignore unknown fields.

## 5. Tool-Side Token Budget

- Cut at the tool layer whatever can be cut there; do not leave it to Headroom.
- Tool output defaults to "summary + address + preview"; full text goes to an artifact.
- Three-tier budget: 8KB per call / 24KB per invocation / 128KB per run.
- Hard caps: 16KB / 48KB / 256KB; configuration may only lower them, only admins may raise them.
- Never return raw `rg --json`, HTML, ANSI, or stack traces.
- Every call writes to `logs/tools.jsonl`; fields are defined in `doc/10` §8.

## 6. State and File Ownership

- `.loom/` has two layers; the root `.gitignore` uses a **whitelist** (`/.loom/*` plus negations, so new state is ignored by default):
  - **Contract layer**, tracked: `README.md` (and its `README.zh-CN.md` sibling), `goal.md`, `constraints.md`, `specs/` (ownership table in `.loom/README.md`);
  - **State layer**, ignored: `tasks/`, `runs/`, `ledger/`, `snapshots/`, `locks/`, `config.toml`, `events.jsonl`, etc. — **never commit these**, and never hand-edit the source of truth (the event log is append-only).
- The orchestrator is the only writer of the state layer; workers only write code + `outbox/`; the contract layer is maintained jointly by humans and the control plane and goes through git review (`loom goal` / `loom constraints` to draft, human reviews the diff, see §10).
- Never copy state-layer content into `doc/` or test fixtures.

## 7. Tests and Quality

Before committing, at minimum:

```bash
make check
```

which includes `fmt --check`, `clippy -D warnings`, and `cargo test --workspace`.

New behavior gets unit tests first; tests touching the filesystem/search use temp directories and do not depend on the local repository's contents.

## 8. Docs and Language

- **Docs are English-first**: human-readable documents use English as the primary file, with a `*.zh-CN.md` Chinese sibling linked from a language-switcher line right after the H1. Keep the two versions in sync; the English file is what outsiders see first.
- **Agent-facing docs are English-only**: `AGENTS.md`, skills, and similar files that only AI reads are not duplicated in Chinese.
- Code identifiers, protocol fields, and error codes are English.
- CLI human-readable output defaults to Chinese and can be switched with `LOOM_LANG` (`en` / `ja`); this is runtime behavior, not docs.
- Paths, symbols, and enums are never translated.
- Model names, ports, and CLI arguments in docs are **examples/placeholders**; verify against `--help` and official docs before relying on them.

### 8.1 Answers and Writing

- Lead with the conclusion, then the necessary evidence; no empty openers, flattery, or restating summaries.
- Prefer specifics: give paths, commands, `file:line`, and verifiable results; say "I don't know" when unsure.
- Preserve limitations, failures, and real uncertainty; do not smooth them away for fluency.
- Long documents follow the rules in `.opencode/skills/docs-writing/`; short ordinary answers do not need that skill loaded.

## 9. Git / GitHub

- The remote is a private repository; never commit secrets, tokens, or personal paths.
- Commit messages are short imperative sentences, e.g.:
  - `scaffold rust workspace`
  - `add tool result envelope`
  - `implement search and read tools`
- Confirm design docs are in sync before large changes.
- Do not `git push --force` to rewrite main-branch history unless explicitly asked.

## 10. Guide Rather Than Restrict; Responsibility Sits with the User

- When information from the user is needed, generate it through commands/workflows (tools draft, humans review the diff); never output "please fill in this template" for humans to hand-write.
- Users may edit, delete, or configure anything; the consequences are theirs. **Do not scatter restrictions through the architecture for extreme abuse.** Recoverable misuse (deleting onboarding docs, editing config, clearing local state) is left alone, covered by git / re-init.
- Only unrecoverable destruction **inside a tool** (wiping event history, overwriting uncommitted work, deleting tracked contracts) warrants a prominent warning: state the consequence and the recovery path once; do not block, do not chain confirmations.
- Boundary of "restriction" vs "fail fast": paths that **cannot possibly succeed** (e.g. a spec that is gitignored yet dispatched) fail immediately with an explanation — that is not a restriction; paths the **user can accomplish** are never blocked.
- This clause constrains restrictions on **humans**; agent-side guards (write guards, scope gates, retry circuit-breakers) are unaffected — they defend against agent drift, not user abuse.

## 11. Current Milestone (M0)

Done / in progress:

1. workspace + containerized dev environment;
2. tool envelope + `search`/`read`/`artifact`;
3. telemetry and the three-tier budget;
4. model capability bits + request ledger + CPVT/TPVT/WPT;
5. TaskSpec / event log / task index and state replay;
6. Context Compiler (stable prefix, hard budgets, failure packets, manifest) + versioned prompt library;
7. process-level adapter contract, mock adapter, goose/opencode presets, adapter canary;
8. the `dispatch` / `verify` / `resume` minimal loop (mock end-to-end passing);
9. native executor (M2 start): OpenAI-compatible tool loop, read/search/edit/write/bash, result projection to artifacts, token/wall-clock/step/cost budgets, acceptance-failure feedback, previous-run failure-packet injection, no-progress, scripted provider tests, `--adapter native` end-to-end passing.
10. control-plane routing (M3 start): `[routing]` role→tier, verify-failure escalation, tier cost guards, worst-case cost check before dispatch, the `loom route` explain command, `loom route stats` worker share and switch counts, same-tier fallback auto-switching (`task.fallback` + a separate run); `task.escalated` and `TaskDispatched.data.route` are wired up.
11. M4 parallelism (completed 2026-11-08): path locks (owner/lease/pid, RAII releasing only paths it wrote, dead-owner reclamation), locking in `dispatch`/`verify`, `task.blocked(code=lock_owner/parallel_cap/inflight_budget)`, `[parallel] max_running` (default 2) and `max_inflight_cost_usd`, the `loom merge` queue (global `**` lock, verify before merge, rebase, integration verify, rollback on failure, worktree/branch cleanup), the `loom board` kanban, JSONL single-writer and atomic run-id reservation; `scripts/smoke-parallel.sh` measured 10 tasks at 2 concurrent with 0 failures and 10 merges.

Next:

1. container image/mounts for goose, opencode, curl, optional LiteLLM;
2. real goose/OpenCode adapter canary (requires installed binaries);
3. native vs adapter A/B (same task set, by CPVT/TPVT/wall-clock);
4. calibrate `tool_output_ratio` / `spill_rate` / cost estimates against a real repository;
5. merge-conflict-rate statistics on a real repository (3-way parallel A/B already measured with `scripts/ab-parallel.sh`: wall 13s→6s, tokens only +5.8%, cache 48%→89%, see `doc/08` §9.5);
6. record CPVT/TPVT/WPT baselines (first version shipped: `scripts/ab-parallel.sh --report` has wall/TPVT/CPVT columns, see `doc/08` §9.5; price table in `scripts/model-prices.sh`; pending more rounds/models).

Current blockers are tracked in `doc/14-current-issues-and-blockers.md`.

**Designed but not implemented**:

- Multi-repo workspace / multi-client collaboration (manifest member repos, `loom workspace *`, cross-member same-name-branch worktrees), see `doc/13-multi-repo-workspace-and-multi-client.md` (M5, P0–P3).
- Runtime governance and engineering (implemented: step-level context projection `loom-native/src/project.rs` + `scripts/smoke-churn.sh`, patch-first/write guards, step-level token attribution + `logs/churn.json` + `loom churn stats`; still pending — L1–L3 graded intervention, `questions.json` + attention inbox, skills scan/on-demand load/verified promotion, agent/harness-driven local CI and hooks/review/attestation — see `doc/15-agent-runtime-governance-and-engineering.md`).

**Directions and proposals (not implemented)**:

- Workspace dev-environment isolation: three platforms (Windows/Linux/macOS), A+B layers without virtualization, reusing existing declarations like `.mise.toml`, containers demoted to an optional profile; not a general environment manager. See `doc/17-workspace-dev-environment-isolation.md`.
- Agent collaboration working method: surviving principles from sixty years of process history (small steps, short feedback, verification before merge, limiting work in progress, externalized state, budget-based estimation) re-sequenced, human participation collapsed into four intervention points (direction-setting, batch-opening, review-slicing, taking escalations), companion `loom plan`; no auto-planning or ceremony. See `doc/18-agent-collaboration-workflow-proposal.md`.

**Do not do early**: embeddings/vector stores, MCP, cross-project knowledge bases, auto-planning, parallel scheduling, merging member repos into a monorepo.
