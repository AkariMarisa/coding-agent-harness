# .loom/ — Loom's Working Directory

> Language: **English** | [简体中文](README.zh-CN.md)

Two-layer structure (root `.gitignore` uses a whitelist: `/.loom/*` plus negations, so new state is ignored by default):

| Layer | Paths | git |
|---|---|---|
| Contract layer | `README.md` (with `README.zh-CN.md`), `goal.md`, `constraints.md`, `specs/` | tracked |
| State layer | everything else — `tasks/`, `runs/`, `ledger/`, `snapshots/`, `locks/`, `config.toml`, `config.local.toml`, `events.jsonl`, etc. | ignored |

- Contract layer: maintained jointly by humans and the control plane, reviewed via git; drafted with `loom goal` / `loom constraints`, diff reviewed by a human; module contracts live in `specs/`.
- State layer: the orchestrator is the only writer; the event log is append-only — never hand-edit the source of truth.
- Local experiment config goes in `config.local.toml` (deep-merge override of `config.toml`; `[[models]]` entries merge by alias; never committed, tests never touch the main file).
- Change config with `loom config set <path> <value>` (keeps comments and order, validates as a whole before writing; `--local` edits the override layer); inspect effective values and their source with `loom config get <path>`.
- ⚠️ `rm -rf .loom/` deletes the contract layer too (tracked files can be recovered from git; the state layer cannot).
- Layering rules: see the repository's `AGENTS.md` §6.
