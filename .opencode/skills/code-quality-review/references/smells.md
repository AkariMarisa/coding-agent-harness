# Fowler code smell baseline

Adapted from the smell baseline in `mattpocock/skills` (MIT), which draws on Martin Fowler's *Refactoring*, chapter 3. Treat every item as a labelled heuristic, not a hard violation.

## Rules

- A documented repo standard overrides the baseline.
- Skip anything tooling already enforces (formatting, import order, lint).
- Only flag smells visible in the target diff or module. Do not produce a repo-wide inventory unless asked.
- Say "possible X" and quote the hunk or call site.
- The remedy is a direction, not a mandate. Prefer the smallest change that removes the current friction.

## Baseline

| Smell | What it is | Fix direction |
|---|---|---|
| Mysterious Name | A function, variable, or type whose name does not reveal what it does or holds. | Rename it. If no honest name comes, the design is murky. |
| Duplicated Code | The same logic shape appears in more than one hunk or file. | Extract the shared shape and call it from both sites. |
| Feature Envy | A method reaches into another object's data more than its own. | Move the method onto the data it envies. |
| Data Clumps | The same few fields or parameters keep travelling together. | Bundle them into one type and pass that. |
| Primitive Obsession | A primitive or string stands in for a domain concept that deserves its own type. | Give the concept a small type. |
| Repeated Switches | The same switch/if-cascade on the same type recurs across the change. | Replace with polymorphism or one shared map/dispatcher. |
| Shotgun Surgery | One logical change forces scattered edits across many files. | Gather what changes together into one module. |
| Divergent Change | One file or module is edited for several unrelated reasons. | Split so each module changes for one reason. |
| Speculative Generality | Abstraction, parameters, or hooks added for needs the spec does not have. | Delete it; inline back until a real need shows. |
| Message Chains | Long `a.b().c().d()` navigation the caller should not depend on. | Hide the walk behind one method on the first object. |
| Middle Man | A class or function that mostly just delegates onward. | Cut it; call the real target directly. |
| Refused Bequest | A subclass or implementer ignores or overrides most of what it inherits. | Drop the inheritance and use composition. |

## Severity guidance

- `P1`: the smell creates a likely correctness or coordination problem in the current change.
- `P2`: real maintainability debt; fix here or open a follow-up.
- `P3`: naming or small cleanup; optional.

A smell alone is not a blocker. Escalate only when you can name the concrete change it makes unsafe or expensive.

## Not smells by themselves

- A large file that has one cohesive responsibility.
- A small abstraction that names a real domain concept.
- A wrapper that isolates a real external contract.
- Duplication across tests when sharing would make the tests harder to read.
- A simple `if` that is clearer than a dispatch table.
- Code that follows an existing repo convention.
