# Architecture review checklist

Use source evidence, not folder names. The goal is to find structural friction that makes a concrete change unsafe or expensive.

## 1. Map the system

- What are the entry points?
- What are the top-level modules or crates?
- What does each own: policy, data, I/O, orchestration, rendering?
- Which modules are depended on by almost everything?
- Where are the public contracts (traits, interfaces, event schemas, CLI boundaries)?
- Where are the sources of truth for mutable state?

## 2. Dependency structure

- Does the dependency direction match the project's stated architecture?
- Are there cycles, including cycles through tests or dev-only modules?
- Does a lower layer import a higher layer, or a domain module import an adapter?
- Does a shared module contain feature-specific logic?
- Are dependencies explicit, or hidden behind global state, singletons, or ambient context?
- Do callers reach through several objects (`a.b().c().d()`)?

## 3. Boundaries and cohesion

- Does each module have one clear reason to change?
- Is a module a god module: imported by nearly everything and changed for unrelated reasons?
- Is a module shallow: its interface is nearly as complex as its implementation?
- Is feature logic leaking into a shared/general-purpose module?
- Are internals exposed through a public API instead of a narrow contract?
- Is the same data owned or duplicated in more than one place?

## 4. Change pressure

Name the concrete operation that is unsafe, expensive, or hard:

- Adding a provider or adapter requires edits in many unrelated places.
- Changing one rule forces coordinated edits across several modules (Shotgun Surgery).
- One file changes for several unrelated reasons (Divergent Change).
- A test can only be written by reaching through the whole system.
- A caller must know internal representation to use a module.

A boundary is justified by a real change pressure, not by a pattern name.

## 5. Depth and seams

- Apply the deletion test: would deleting the module concentrate complexity, or just move it?
- Is the interface the test surface, or do tests need internal access?
- Is there one adapter (hypothetical seam) or two (real seam)?
- Does the abstraction remove a current obligation, or only add indirection?
- Are temporary adapters clearly marked with a removal condition?

## 6. Runtime and data

- Is shared mutable state accessed from multiple owners?
- Is there a duplicate source of truth?
- Are failure modes and partial failure explicit at boundaries?
- Does a proposed service split add deployment, latency, consistency, observability, and on-call cost?
- Could a module boundary solve the same ownership problem without a distributed protocol?

## 7. Report

For each finding:

- `file:line` or module path.
- What the current structure is.
- The concrete change pressure it creates.
- The smallest remedy.
- Confidence and what would invalidate the finding.

Avoid:

- Inferring boundaries from directory names.
- Rewriting a working subsystem to match a named architecture.
- Proposing microservices or DDD ceremony without evidence.
- Treating file size alone as a design defect.
- Listing every theoretical refactor an ADR forbids.
