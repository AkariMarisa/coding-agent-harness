# Architecture Decision Records

An ADR records a decision that is expensive to reverse, so future humans and agents know why the code looks the way it does. It is not a design doc, a tutorial, or a changelog.

## When to write one

Write an ADR when at least one is true:

- The choice adds a dependency, protocol, storage engine, or external service.
- It sets a cross-module contract or a pattern other code must follow.
- It changes data ownership, error semantics, or a public interface.
- It is hard to reverse once code is written against it.
- There were real alternatives and the rejection reasons matter.

Do not write one for:

- Routine implementation inside an established pattern.
- Bug fixes, typo fixes, or formatting.
- A decision already covered by an existing ADR. Update or supersede that one.
- Style rules already enforced by a formatter or linter.

## Location and format

Match the repository first. Look for `docs/adr/`, `docs/decisions/`, `adr/`, `Documentation/Decisions/`, or an `.adr-dir` file. Reuse the existing directory, numbering, filename pattern, and heading set.

If no convention exists, use `docs/adr/NNNN-short-title.md` with sequential numbering.

## Template

```markdown
# ADR-0001: Choose a storage engine for task events

## Status

Proposed | Accepted | Superseded by ADR-XXXX | Deprecated

## Date

YYYY-MM-DD

## Context

The concrete problem, constraints, and forces. Include what breaks if nothing changes.
State non-goals explicitly.

## Decision

The choice, in one or two specific sentences. No vague "use a scalable solution".

## Alternatives considered

### Option A

- Pros:
- Cons:
- Why rejected:

### Option B

- Pros:
- Cons:
- Why rejected:

## Consequences

- What becomes easier.
- What becomes harder.
- New operational or migration obligations.
- Follow-up work.

## Implementation plan

- Files or modules to change.
- Order of changes and how callers stay working.
- Tests or commands that prove the decision was implemented.
- Rollback path.
```

## Lifecycle

```text
PROPOSED -> ACCEPTED -> SUPERSEDED or DEPRECATED
```

- Never delete an old ADR. It records historical context.
- When a decision changes, write a new ADR that references and supersedes the old one.
- Reference the ADR from code comments only where the "why" is not obvious from the code.

## Agent-readiness checklist

An ADR should let an agent implement the decision without asking follow-up questions:

- The decision is specific and measurable.
- Constraints and non-goals are explicit.
- Affected files or modules are named.
- Existing patterns to follow are named.
- The verification command or test is named.
- The rollback or migration path is stated.
- No tribal knowledge is assumed.

## Review checks

- Does the context state a real force, not a generic benefit?
- Is there at least one rejected alternative with a concrete reason?
- Does the decision contradict an accepted ADR? If so, say which one and why.
- Are consequences honest about cost and new obligations?
- Is the implementation plan actionable, or only aspirational?

## Anti-patterns

- An ADR that restates the code without explaining the choice.
- A decision with no alternatives, or alternatives rejected only by preference.
- "Best practice" as the sole justification.
- Hiding a hard tradeoff in the consequences section.
- Writing an ADR after the fact to bless an unreviewed change.
