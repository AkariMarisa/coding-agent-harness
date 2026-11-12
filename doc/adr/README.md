# ADR Index

> Language: **English** | [简体中文](README.zh-CN.md)

Architecture Decision Records for Loom. An ADR captures *why* a decision was made and which alternatives were rejected; that is the part that decays into "why did we do this?" six months later. Write one for decisions that are expensive to reverse or that cross module boundaries. Routine choices stay in the design docs.

## Format

Files are named `ADR-NNNN-kebab-title.md`; take the next number from the index below. Template:

````markdown
# ADR-0007: <title>

Status: Proposed | Accepted | Deprecated | Superseded by ADR-00xx

## Context
What forced a decision now: constraints, problem shape, why the status quo hurts.

## Decision
What was chosen. Three lines are usually enough.

## Consequences
What gets easier, what gets harder, and what follow-up work this creates.

## Alternatives rejected
Why the other options lost. This is the section that pays for the ADR.

## Model assumptions
Which model capabilities this decision depends on (e.g. "the relay's chat wire
accepts reasoning_effort=high", "official deepseek usage numbers are
trustworthy"), and which change triggers a re-review (model upgrade, provider
swap, wire change).
````

## Rules

- An accepted ADR is never edited. A reversal writes a new ADR and marks the old one `Superseded by ADR-NNNN`; the old text stays as history.
- The **Model assumptions** section is mandatory. Decisions built on model behavior rot when the model changes; this section is the re-review trigger list.
- Every ADR is registered in the index below and linked from `doc/README.md`.
- Language follows `AGENTS.md` §8: English primary, `*.zh-CN.md` sibling.

## Index

| ADR | Title | Status |
|---|---|---|
| — | none yet | — |
