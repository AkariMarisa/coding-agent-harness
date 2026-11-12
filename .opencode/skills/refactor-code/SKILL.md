---
name: Code Refactoring & Cleanup
description: "Use when asked to apply a refactor, clean up, simplify, or fix named code smells. Do not use for review-only requests (use code-quality-review), feature work, formatting/lint, or speculative rewrites."
---

# Refactor Code

## Trigger gate

Use this skill only when at least one is true:

- The user explicitly asks to refactor, clean up, simplify, or reduce duplication.
- The user asks to apply findings from a review, or names a concrete smell to fix.
- A task explicitly includes behavior-preserving cleanup as an acceptance item.

Do not load it for:

- Review, audit, or "what smells are here?" requests. Use `code-quality-review`.
- Ordinary feature work or bug fixing. Behavior changes are not refactors.
- Formatting, import order, or lint. Use `format-code` or the project tooling.
- Whole-repo rewrites or architecture changes. Get explicit scope first; use `architecture-design` when boundaries change.
- Generated, vendored, or minified code.

## Non-negotiables

- **Behavior-preserving only.** If observable behavior must change, stop and split it into a feature or bugfix task.
- **Tests are the safety net.** Find the existing checks first. If the target has no behavioral coverage, add characterization tests that pin current behavior before touching it.
- **Small steps, always green.** One coherent transformation per step, then run the focused checks.
- **One concern per diff.** Do not mix a rename, a move, and a new behavior in one change.
- **Stay surgical.** Leave unrelated code, names, and formatting exactly as found.
- **Match the repo.** Existing conventions override generic style advice.
- **A refactor that relocates the same complexity is not an improvement.** Prefer one that removes branches, modes, or layers.

## Process

1. **Pin the scope.** Name the target as `file:line` or a module and the concrete cost it creates. If the requested scope is the whole repo, propose a short prioritized list before editing.
2. **Record the baseline.** Find the project's own check command (`make check`, `just test`, `./scripts/dev.sh cargo test`, `pnpm test`, ...). Run it, or the focused subset, and note pre-existing failures separately.
3. **Build the safety net.** If the target lacks coverage, write the smallest characterization test that pins the current input/output behavior. Do not build a new test framework.
4. **Name the problem and pick one technique.** Use `references/techniques.md`. Prefer the smallest transformation that removes the current friction.
5. **Apply one step.** Keep the diff reviewable. Do not opportunistically rename or reformat nearby code.
6. **Verify.** Run focused checks after each coherent batch; broaden when the changed surface warrants it. Reuse results for unchanged state.
7. **Judge the result.** Read the diff. If it is not clearly easier to read or change, revert it. Mechanical rule compliance that hurts readability is a net loss.
8. **Report.** List what changed, why, the exact commands run, pre-existing failures, and anything left unverified.

## When tests fail

- A behavior test fails: assume you introduced a regression. Fix the code first.
- A test breaks only because it asserts internals: the test is too coupled. Change it to assert behavior through the module's interface.
- The failure was already present at baseline: report it; do not fold an unrelated fix into this change.

## Stop conditions

Stop and report instead of pushing through when:

- The smallest safe change requires an interface or ownership change.
- The target has no tests and characterization is more expensive than the requested cleanup.
- The diff starts touching unrelated modules.
- The "cleanup" would change public behavior, error semantics, ordering, or performance characteristics.

## Output format

```text
## Changed

- path/to/file.rs:42 — extracted `parse_retry_after`; removed duplicated header logic.

## Verified

- `./scripts/dev.sh cargo test -p loom-tools` — pass.
- Baseline had 1 pre-existing failure in `test_x`; unchanged.

## Not covered

- No integration test for timeout ordering; behavior assumed unchanged from reading.
```

## References

- `references/techniques.md`: Fowler-style transformation catalog, grouped by problem.

Attribution: see `.opencode/skill-pilot/THIRD_PARTY_NOTICES.md`.
