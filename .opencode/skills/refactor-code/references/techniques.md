# Refactoring techniques

Adapted and condensed from `cskwork/clean-code-skill` (MIT), which follows Martin Fowler's *Refactoring* catalog. Pick the smallest move that removes the current friction.

## Composing methods

| Technique | Use when | Move |
|---|---|---|
| Extract Method | A block needs a comment to explain it, or the same fragment appears twice. | Move the block into a named method; name the intent, not the mechanics. |
| Inline Method | A method body is as clear as its name, or it only delegates. | Replace calls with the body; delete the method. |
| Extract Variable | A complex expression is hard to read. | Name the intermediate result; keep it immutable where possible. |
| Inline Temp | A temp only holds a value used once and blocks a refactor. | Substitute the expression; delete the temp. |
| Replace Temp with Query | A temp is computed from other fields and used in several places. | Extract the computation into a query method; call it. |
| Split Temporary Variable | One temp is assigned different meanings. | Give each meaning its own variable. |
| Replace Method with Method Object | A long method's locals make extraction impossible. | Move the method into a small object; turn locals into fields. |
| Substitute Algorithm | A clearer algorithm produces the same result. | Replace the body; keep the interface and tests. |

## Conditionals

| Technique | Use when | Move |
|---|---|---|
| Decompose Conditional | A complex `if` needs a comment to explain its parts. | Extract condition, then-branch, and else-branch into named methods. |
| Consolidate Conditional Expression | Several conditions lead to the same result. | Combine them into one named predicate. |
| Replace Nested Conditional with Guard Clauses | Deep nesting hides the normal path. | Return early for edge cases; leave the happy path flat. |
| Replace Conditional with Polymorphism | The same type switch repeats and grows with each new case. | Introduce a type per case with one method; dispatch through the interface. |
| Introduce Null Object | Callers repeat `if x is None` around the same object. | Provide a do-nothing implementation with the same interface. |
| Remove Control Flag | A boolean drives loop or branch control from inside. | Use `break`, `return`, or a helper predicate. |

## Moving behavior

| Technique | Use when | Move |
|---|---|---|
| Move Method | A method uses another object's data more than its own. | Move it to the data owner; update callers. |
| Move Field | A field is read and written mostly by another object. | Move the field and its accessors. |
| Extract Class | One class has several reasons to change, or fields form a cluster. | Split by responsibility; delegate or move behavior. |
| Inline Class | A class no longer earns its keep. | Move its members to the caller; delete it. |
| Hide Delegate | Callers navigate `a.b().c()`. | Add one method on `a` that performs the walk. |
| Remove Middle Man | A class mostly forwards calls. | Let callers talk to the real target; delete the wrapper. |

## Data and parameters

| Technique | Use when | Move |
|---|---|---|
| Replace Magic Number with Constant | A literal encodes a domain rule. | Name it once; use the constant. |
| Introduce Parameter Object | The same few parameters travel together. | Create a small type; pass it. |
| Preserve Whole Object | A caller pulls several fields off one object to pass them. | Pass the object; let the callee read what it needs. |
| Replace Data Value with Object | A primitive stands in for a domain concept. | Give the concept a small type with validation. |
| Encapsulate Field / Collection | A field or collection is exposed and mutated from outside. | Hide it behind accessors; return a read-only view. |
| Replace Type Code with Class / Subclasses / State-Strategy | A string or int selects behavior. | Model the cases explicitly; dispatch through the type. |

## Generalization

| Technique | Use when | Move |
|---|---|---|
| Pull Up / Push Down | Members sit at the wrong level of a hierarchy. | Move them to the shared parent or the concrete child. |
| Extract Superclass / Subclass / Interface | Shared behavior or a real contract is repeated. | Extract the common piece; keep it minimal. |
| Collapse Hierarchy | A subclass adds nothing meaningful. | Merge it into the parent. |
| Replace Inheritance with Delegation | A subclass uses little of what it inherits. | Hold the other object; forward only the needed calls. |

## Safety checks

Before applying any technique:

- The target behavior is pinned by a test, or the change is provably mechanical.
- The technique does not change public error types, ordering, or timing.
- The diff stays within the agreed scope.
- The project's own check command passes after the step.

After applying:

- Read the result as a reviewer. Is it clearly easier to change?
- If not, revert. Fewer moving parts beats a textbook-shaped diff.
