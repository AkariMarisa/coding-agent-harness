---
name: Architecture & Interface Design
description: "Use when asked to design or change module boundaries, interfaces, seams, or data ownership, compare architecture options, or record an ADR. Do not use for code review/audit, routine implementation, or style questions."
---

# Architecture Design

## Trigger gate

Use this skill only when at least one is true:

- The user asks to design a new module, interface, seam, or ownership boundary.
- The user asks to change an existing boundary or split/merge modules.
- The user asks to compare architecture options or record an architecture decision (ADR).
- A task introduces a new dependency, cross-module contract, or hard-to-reverse choice, and the user asks for a design.

Do not load it for:

- Reviewing or auditing existing code. Use `code-quality-review`.
- Routine feature implementation inside an established pattern.
- Formatting, naming, or local style questions.
- Speculative rewrites, microservice splits, or DDD ceremony without a concrete change pressure.

## Process

1. **Read the project first.** `AGENTS.md`, design docs, existing ADRs, `GLOSSARY.md`, and the call sites around the area. An existing convention or accepted ADR overrides generic advice.
2. **State the forces.** Write down the concrete change that is currently unsafe or expensive, the constraints, and the non-goals. A boundary needs a real change pressure, not a pattern name.
3. **Map the current structure.** Identify modules, their interfaces, data owners, and callers. Do not infer structure from folder names.
4. **Apply the deletion test.** If deleting a module would concentrate complexity in callers, it earns its keep. If it only moves complexity around, it is a pass-through.
5. **Generate at least two options.** Compare on leverage for callers, locality for maintainers, testability through the interface, failure modes, migration cost, and operational cost. One option must be "change nothing".
6. **Choose and define the interface contract.** Include inputs/outputs, invariants, ordering constraints, error modes, required configuration, and performance characteristics. The interface is the test surface.
7. **Plan the rollout.** List files to touch, the order of changes, how to keep callers compiling, what tests prove it works, and how to roll back.
8. **Record the decision.** Write an ADR when the choice is hard to reverse. Otherwise leave a short design note or PR description. Read `references/adr.md`.

Do not implement code unless the user asks. This skill produces a design and a decision, not a surprise diff.

## Seam discipline

- **One adapter is a hypothetical seam. Two adapters make it real.** Do not introduce a trait or port for something that never varies.
- Prefer a deep module: a small interface over a large implementation.
- Put the seam where behavior actually varies, not where a named pattern says it should go.
- A local module boundary is usually cheaper than a service boundary. A service split adds deployment, latency, consistency, observability, and on-call cost.
- Keep domain language stable; use the project's glossary terms.

## Rules

- Do not propose an abstraction unless it removes a current obligation.
- Do not re-litigate an accepted ADR without evidence that its assumptions no longer hold.
- Do not recommend a rewrite when incremental deepening solves the problem.
- Do not invent requirements or constraints the user did not state.
- Name the tradeoff you are accepting, not only the benefits.
- If the evidence is insufficient, say what you would need to inspect or measure.

## Output format

```text
## Decision

<one paragraph: what changes and why now.>

## Options

| Option | Leverage | Locality | Testability | Cost / risk |
|---|---|---|---|---|
| ... | ... | ... | ... | ... |

## Chosen interface

- Inputs / outputs:
- Invariants:
- Error modes:
- Ordering / concurrency:
- Configuration:
- Performance characteristics:

## Rollout

1. ...
2. ...

## Non-goals

- ...

## Verification

- Test / command that proves the design works.
```

## References

- `references/deep-modules.md`: module, interface, depth, seam, adapter, deletion test.
- `references/adr.md`: when to write an ADR, template, lifecycle, and agent-readiness checks.

Attribution: see `.opencode/skill-pilot/THIRD_PARTY_NOTICES.md`.
