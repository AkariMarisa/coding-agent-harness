---
name: 代码质量与架构审查
description: "Use only when the user explicitly asks to review/audit code, check architecture/module boundaries/dependency direction/cycles, find code smells, or evaluate a refactor. Do not use for ordinary feature work, every code edit, formatting/lint, or single-line bug fixes."
---

# Code Quality Review

## Trigger gate

Use this skill only when at least one is true:

- The user asks to review, audit, or critique a diff, PR, branch, module, or codebase.
- The user asks about architecture: boundaries, dependency direction, cycles, layering, coupling, cohesion, data ownership, or public API leakage.
- The user asks to find code smells or evaluate a refactor.
- The task explicitly includes an architecture or maintainability review as an acceptance item.

Do not load it for:

- Ordinary feature implementation or bug fixing.
- Every code edit; do not self-invoke after each change.
- Formatting, import order, or lint issues. Use the formatter/linter instead.
- Single-line correctness bugs. Use the normal debugging path.

## Scope

- Architecture: module boundaries, dependency direction, cycles, layer breaches, shared mutable state, duplicate sources of truth, god modules, shallow modules, leaked internals.
- Code smells: the Fowler baseline in `references/smells.md`.
- Refactor handling: evidence, behavior preservation, incrementality, tests, separation from feature work.

Out of scope: formatting/lint, deep security review, performance profiling, and line-level correctness debugging. Mention them when relevant, but do not turn this into a full audit.

## Process

1. **Define the scope.** If a diff/PR/branch is named, pin the base and use `git diff <base>...HEAD` plus `git log <base>..HEAD --oneline`. If a module is named, identify its entry points and direct callers. If nothing is named, ask or default to the current diff; do not scan the whole repo by default.
2. **Read the project rules.** `AGENTS.md`, `CONTRIBUTING.md`, ADRs, design docs, and formatter/lint configs. A documented repo rule overrides generic advice.
3. **Build evidence.** Read imports, call sites, tests, and data ownership. Do not infer architecture from folder names alone.
4. **Choose a mode.** Use `architecture`, `smells`, or `refactor` from the sections below. A review may use more than one, but keep the report separated.
5. **Report findings.** Use file:line, evidence, why it matters, and a concrete remedy. Mark confidence.
6. **Only edit code when the user asks.** If asked, propose the smallest safe plan first, then make incremental changes and run the project checks.

## Mode: architecture

Read `references/architecture.md` for the full checklist. At minimum:

- Map responsibilities, dependencies, data owners, and public contracts.
- Check dependency direction and cycles against the project's stated architecture.
- Look for layer breaches, leaked internals, shared mutable state, and duplicate sources of truth.
- Check whether a module is shallow: its interface is nearly as complex as its implementation.
- Apply the deletion test: would deleting the module concentrate complexity, or just move it?
- Name the change pressure: what concrete change is unsafe or expensive under the current structure?
- Do not propose a service split or a named pattern unless the evidence justifies it.

## Mode: smells

Use the baseline in `references/smells.md`. Two rules bind it:

- **Repo standards override.** If the project documents a different choice, suppress the smell.
- **Smells are judgement calls.** Say "possible Feature Envy", not "violation". Skip anything tooling already enforces.

Focus on the changed code or the named module. Do not produce a repo-wide smell inventory unless asked.

## Mode: refactor

- Understand why the code exists before changing it (Chesterton's Fence).
- Preserve behavior exactly unless the task explicitly changes behavior.
- Prefer the smallest change that removes a current obligation.
- Separate refactoring from feature work. Do not mix a rename, a move, and a new behavior in one diff.
- Add or run tests before and after. If tests are missing, say what cannot be verified.
- Do not do drive-by refactors outside the agreed scope.
- A refactor that relocates the same complexity is not an improvement; prefer one that removes branches, modes, or layers.

## Output format

```text
## Findings

- [P1] path/to/file.rs:42 — possible Feature Envy. Evidence: the method reads 4 fields from `Other` and none from `self`. Remedy: move the method onto `Other`. Confidence: medium.

## Scope / skipped

- Reviewed: <diff or files>.
- Skipped: formatting/lint (tooling), security deep dive, performance.

## Verdict

- <one-line summary or "no structural findings">
```

Severity:

- `P0`: data loss, security, or a structural change that will break callers.
- `P1`: likely correctness or maintainability problem that should be fixed before merge.
- `P2`: real smell or design debt; fix in this change or open a follow-up.
- `P3`: naming, small cleanup, optional suggestion.

Do not merge architecture and smell findings into one undifferentiated list. Keep them separate when both are in scope.

## Rules

- Do not re-litigate an ADR without concrete evidence that its assumptions no longer hold.
- Do not propose a new abstraction unless it removes a current coordination obligation.
- Do not review generated, vendored, or minified code unless it is the explicit target.
- If the diff is too large to review well in one pass, sample by logical change and say so.
- Prefer quoting the hunk or call site over paraphrasing it.
- Never invent line numbers, callers, or test results.

## References

- `references/architecture.md`: boundary, coupling, cohesion, and evidence checklist.
- `references/smells.md`: Fowler smell baseline and fix directions.

Attribution: see `.opencode/skill-pilot/THIRD_PARTY_NOTICES.md`.
