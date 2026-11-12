# Contributing to Loom

> Language: **English** | [简体中文](CONTRIBUTING.zh-CN.md)

Thanks for your interest in contributing. This document covers the workflow and conventions; the detailed engineering rules live in [AGENTS.md](AGENTS.md).

## Before You Start

- Read the [Code of Conduct](CODE_OF_CONDUCT.md); participation implies accepting it.
- Read [README.md](README.md) for the current status and quick start, and [doc/README.md](doc/README.md) for the design document map.
- For security issues, follow [SECURITY.md](SECURITY.md) instead of opening a public issue.

## Development Environment

All Rust builds and tests run inside a container; a host-installed toolchain is not used:

```bash
./scripts/dev.sh cargo build --workspace
./scripts/dev.sh cargo test --workspace
./scripts/dev.sh cargo fmt --all
./scripts/dev.sh cargo clippy --workspace --all-targets -- -D warnings
```

Or the Make wrappers: `make build` / `make test` / `make fmt` / `make clippy` / `make check`.

- `podman` is preferred, `docker` is the fallback (`CONTAINER_ENGINE=docker` forces it).
- Real provider credentials go in `.env.local` (git-ignored); never commit keys.

## Making Changes

- Branch from the default branch; keep changes focused and reviewable.
- New behavior gets unit tests first. Tests that touch the filesystem or search use temp directories and must not depend on this repository's own contents.
- Run `make check` (fmt, clippy with `-D warnings`, workspace tests) before opening a pull request.
- Protocol changes (fields, short-key meanings, limits, exit codes) must update all four artifacts together: `loom-core` types, `schemas/*.json`, `doc/10-tool-output-protocol-v0.1.md`, and at least one test or example — see AGENTS.md §4.
- Large changes should sync the relevant design docs (`doc/`) in the same pull request.

## Documentation Language

- Human-readable docs are **English-first**: the English file is the primary one, with a `*.zh-CN.md` Chinese sibling linked from the switcher line right after the H1. Update both when you change content.
- Agent-facing docs (`AGENTS.md`, skills) are English-only.
- CLI output language is runtime behavior, not a docs concern (see `LOOM_LANG` in AGENTS.md §8).

## Commits and Pull Requests

- Commit messages are short imperative sentences: `add tool result envelope`, `fix lock lease renewal`.
- Describe what changed and why, and how you verified it (commands run, test output).
- Small, reviewable pull requests are preferred over large mixed ones.

## Reporting Issues

- Bugs and feature requests: open an issue with reproduction steps or a concrete use case.
- Security: see [SECURITY.md](SECURITY.md).
- Conduct concerns: see [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).
