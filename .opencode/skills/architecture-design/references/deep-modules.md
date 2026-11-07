# Deep modules and seams

Adapted from the design vocabulary in `mattpocock/skills` (MIT), following John Ousterhout's *A Philosophy of Software Design* and Michael Feathers' seam concept.

## Terms

**Module**: anything with an interface and an implementation. A function, class, crate, package, or tier-spanning slice. Prefer "module" over "component" or "service" when discussing structure.

**Interface**: everything a caller must know to use the module correctly. The type signature, plus invariants, ordering constraints, error modes, required configuration, and performance characteristics. Do not reduce "interface" to the language keyword.

**Implementation**: the body of code behind the interface. An adapter is a module whose job is to satisfy a seam; it can have a large or small implementation.

**Depth**: leverage at the interface. A module is deep when a large amount of behavior sits behind a small interface. It is shallow when the interface is nearly as complex as the implementation.

**Seam** (Feathers): a place where behavior can be altered without editing in that place. The location where an interface lives. Choosing the seam is separate from choosing what sits behind it.

**Adapter**: a concrete thing that satisfies an interface at a seam. Describes a role, not a substance.

**Leverage**: what callers get from depth. More capability per unit of interface they must learn.

**Locality**: what maintainers get from depth. Change, bugs, knowledge, and verification concentrate in one place instead of spreading across callers.

## Design checks

- **Deletion test.** Imagine deleting the module. If complexity vanishes, it was a pass-through. If complexity reappears across N callers, it earns its keep.
- **Interface as test surface.** Callers and tests should cross the same seam. If a test must reach past the interface, the module is probably the wrong shape.
- **One adapter is hypothetical; two make a seam real.** Do not introduce a port for something that never varies.
- **Depth is about the interface, not the implementation.** A deep module can contain small, swappable internal parts; they just are not part of the contract.
- **Accept dependencies, do not create them.** Pass collaborators in; do not instantiate external systems inside a module.
- **Return results, do not produce hidden side effects.** A function that returns a value is easier to test than one that mutates shared state.
- **Small surface area.** Fewer methods and parameters mean fewer cases to learn and test.

## When a module is too shallow

Signals:

- The interface has nearly as many methods as the implementation has lines.
- A wrapper only forwards calls.
- Every caller must know internal representation to use it.
- Tests need mocks for several internal collaborators.

Response: merge it into the caller, hide the walk behind one method, or move behavior to the data owner. Do not add another layer to "fix" shallowness.

## When to split

Split only when there is a concrete change pressure:

- One module changes for unrelated reasons (divergent change).
- One logical change forces scattered edits (shotgun surgery).
- Two responsibilities have different data ownership or failure modes.
- A real seam exists: two adapters, or a boundary that tests must cross.

Do not split because a file is large, a diagram looks cleaner, or a pattern name suggests it. A large cohesive module is often cheaper than several shallow ones.

## Reporting

For each candidate, state:

- Current structure and the concrete operation it makes unsafe or expensive.
- The proposed seam and what sits behind it.
- Expected leverage and locality gains.
- Migration cost and the test that proves behavior is preserved.
- What would invalidate the recommendation.
