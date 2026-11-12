# 17 Workspace Dev Environment Isolation (direction record)

> Language: **English** | [简体中文](17-workspace-dev-environment-isolation.zh-CN.md)

> **Status: direction record, not designed, not implemented (2026-11-12).** Only records motivation and trade-offs so later discussion does not start from zero; produce a design draft before writing code.

## 1. Motivation

- Need to isolate dev environments per workspace on the three mainstream OSes: Windows / Linux / macOS.
- Pain points: switching workspaces with nvm overwrites the global Node version, sdkman has the same class of problem; JetBrains has built-in "project environment configuration" but the whole IDE is too heavy — what is missing is a tool for "IDE-level project environment declaration + lightweight execution".
- Constraint: **avoid virtualization wherever possible** (especially on Windows/macOS); machines that cannot install docker/podman or do not support virtualization must also work.

## 2. Market assessment (brief)

- Layering: A tool versions (mise/asdf/proto), B system libraries and toolchains, C container/VM, D cloud workspaces.
- **A+B can be virtualization-free**: Nix/devbox are user-space (no container/VM needed; only Windows requires WSL2); podman is also virtualization-free on Linux. What virtualization-free truly rules out is mac/win VM desktop options and cloud VMs.
- **Language package managers are inherently the B layer**: uv/npm/cargo/go modules isolate per workspace by nature; cross-language system libraries use conda/Nix; the Android chain (JDK/Gradle/SDK) is fully installable in user space.
- The real gap without containers is **middleware multi-versions**; a general-purpose environment manager is a red ocean (mise/devbox/conda have fought for a decade), and full-language compatibility is a long-tail trap — **do neither**.

## 3. Loom's trade-offs (to be designed)

**The declaration is the product; isolation is an optional capability.**

1. Reuse existing standard files, do not invent new formats: `.mise.toml` / `.tool-versions`, `requires-python` in `pyproject.toml`, `engines` in `package.json`, Gradle wrapper versions.
2. `loom env` does exactly three things: **probe → validate (fail fast on mismatch, same philosophy as `key_env`) → run the declared setup** (idempotent, cacheable); installation is always left to existing tools; we do not write a single line of it.
3. **Degradation matrix**: with a container engine → the existing Containerfile profile; without → a user-space profile (mise + language package manager). Both profiles consume the same declaration and run the same acceptance batch; the exit code contract does not change.
4. The secret whitelist pass-through (today's `*_API_KEY` logic in `scripts/dev.sh`) is promoted to a first-class capability of `loom env`.
5. The three audiences human / agent / pipeline hold naturally because "the declaration is a file in the repo" (mise has already proven this pattern in CI); no need to adapt to three kinds of users.

## 4. Non-goals

- No general-purpose environment manager; do not take on the language/middleware compatibility long tail;
- No cloud workspaces (network round trips, cost, and the key-trust surface all fit poorly);
- Virtualization is not required; the container is only an optional profile, not a prerequisite.

## 5. Next step

Design draft: declaration reuse list, `loom env` command surface, degradation matrix, and the seam with `dispatch` fail-fast (aligned with the existing behavior where a missing `key_env` rejects dispatch). **No code before this.**
