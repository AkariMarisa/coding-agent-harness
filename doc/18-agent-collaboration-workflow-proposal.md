# 18 Agent Collaboration Workflow Proposal

> Language: **English** | [简体中文](18-agent-collaboration-workflow-proposal.zh-CN.md)

> **Status: methodology proposal, not implemented (2026-11-12).** Working title is provisionally "The Loom Method"; it is a placeholder and can change at any time.
> Readers: people who plan to develop products with agents but are not familiar with project management.
> Conclusion first: sixty years of process history left only a few surviving principles (small steps, short feedback, acceptance before merge, limiting work in progress, externalized state, budgets instead of estimation); what died is the ceremonies and roles; this proposal re-arranges the cadence around the surviving principles, narrows human participation to four intervention points, and can be picked up with six commands.

## 1. Problem

Two symptoms appear at the same time:

- **complete design, drifting implementation**: once the big spec is written, the agent implements on autopilot, memory drifts, and the direction walks further and further off;
- **burning tokens without shipping**: what the agent spends on "re-understanding the design" far exceeds what lands in the diff.

The root cause is the same thing: treating "the executor can look back at any time and hold a big design for the long term" as a premise, when agents have no such ability. What people need is not a bigger prompt but a different cadence that states clearly what to decide, what to accept, and who to ask when stuck.

## 2. Sixty years of process history: what died, what still holds

| Process | Era | What it solved back then | Status for agent development |
|---|---|---|---|
| Waterfall | 1970s | large contracts, predictability | Dead. It is exactly the origin of "big specs drift in implementation" |
| RUP | 1990s | iteration, front-loaded risk | The iteration idea survives; document volume is dead |
| **XP** | 1990s | short feedback, TDD, continuous integration, small releases | **Highest survival**: all core practices are machine-executable |
| Scrum | 1990s–2000s | priority alignment, cadence, visibility | Cadence and roles partly dead, see §3 |
| Kanban / Lean | 2000s | limit work in progress, visualize flow | Survives. The WIP limit is the concurrency cap |
| DevOps / continuous delivery | 2010s | trunk development, automated verification | Survives, already implemented as merge queue and integration acceptance |
| Stage-gate | product development | human decision gates at the end of each stage | Survives, corresponds to the "four intervention points" |
| V model | hardware/software safety-critical | every requirement paired with a verification | Survives, i.e. acceptance corresponds one-to-one with the spec |
| Shape Up | 2019 | fixed budget, variable scope, no estimation | Survives, i.e. task budgets replace hour estimates |
| Design doc / ADR culture | 2010s | decisions written down and accountable | Survives, corresponds to the contract layer and direction records |
| Spec-driven AI | 2023– | driving agents with big documents | Dead; that is exactly the symptom this document addresses |

What process history keeps inventing and keeps killing is ceremony (daily stand-ups, story points, sprint planning meetings); only a few principles have been validated as effective, see §3.

## 3. The six surviving principles

1. **Small steps**: changes small enough to review and revert. Corresponds to work item slicing and `max_files_touched`.
2. **Short feedback**: the delay between change and verification is minimal. Corresponds to acceptance commands and per-slice verify.
3. **Verification before merge**: changes that do not pass verification do not enter the trunk. Corresponds to the merge queue's pre-merge verify.
4. **Limit work in progress**: no more things in flight than can be reviewed. Corresponds to `parallel.max_running`.
5. **Externalized state**: progress and decisions are written to disk, independent of any participant's memory. Corresponds to the contract layer and the event log, and is also the right fix for agent memory drift.
6. **Budgets instead of estimation**: no hour estimates; give the cap directly, and when it is hit, stop or split. Corresponds to TaskBudget's token/cost/wall clock.

## 4. Already dead, no longer copied

- **Fixed time boxes** (two-week sprints): agents advance by acceptance, not by the calendar;
- **Story points and velocity**: the real currency is tokens, cost, and shipped diff size; `route stats` / `churn stats` are the dashboard;
- **Daily stand-ups**: progress is replayed from the event log; `loom board` is the stand-up;
- **Role titles** (PO/SM/developers): converge into the four intervention points of §5 and three executors;
- **Big up-front specs**: the contract layer holds only direction, invariants, and module contracts; full design text is an input read on demand, not an execution instruction;
- **Meeting-driven coordination**: coordination goes through the protocol, escalation goes through structured reports and the inbox.

## 5. The workflow itself

### 5.1 Vocabulary

| Concept | Where it lands | Lifecycle |
|---|---|---|
| Direction | `goal.md` main route + non-goals | changed only when direction changes |
| Contract | `constraints.md` + `.loom/specs/` | evolves slowly, injected every step |
| Batch | one set of work items cut out by a single `loom plan` (chained via `depends_on`) | one batch at a time |
| Work item | TaskSpec + acceptance | one slice, one acceptance, one merge |
| Gate | two levels: machine gate (exit code) and human gate (evidence review) | before merge |
| Escalation | `needs_input` (implemented) + inbox + structured report (see `doc/15`, to be implemented) | anytime |
| Account | usage / churn / route stats | retrospective per batch |
| ADR | `doc/adr/` decision records, including the Model assumptions field | written once; superseded, never edited |

For hardware and hybrid hardware-software products, physical acceptance goes through the human gate: measurements, photos, or test records produced by a work item become artifacts, and a person clears the batch against that evidence when it closes. Parts with unknown risk are cut into spike slices and verified before entering the main route.

### 5.2 Cadence

```text
Set direction (when direction changes)   human asks → loom goal drafts → human reviews the diff
Open a batch (once per batch)            human picks current direction and urgency → model slices → human reviews the TaskSpec
Run slices (per work item)               dispatch → verify → merge, agent loops, escalate when stuck
Close a batch (end of batch)             board + stats retrospective → confirmed failures become regression tests/acceptance (no test, no close) → write a direction record → back to Open a batch
```

The cadence is driven by acceptance and budgets, not by the calendar.

### 5.3 The human's four intervention points

1. **Set direction**: change direction or not;
2. **Open a batch**: which batch now, and why it is urgent;
3. **Review a slice**: look at the diff before merging (i.e. the `loom merge` action itself). As the engineering grows, human review degrades into rubber-stamping; the evolution path for this item is in §8;
4. **Handle escalations**: answer the structured questions in the inbox.

Other than these, humans do not participate. All four are judgment work and cannot be automated; everything else goes to the control plane.

### 5.4 The three executors

| Executor | Responsibility | Boundary |
|---|---|---|
| Human | set direction, open batches, review what shipped, answer escalations | does not do the splitting, does not re-narrate progress |
| agent | only the work items already sliced, delivering acceptance evidence | does not pick topics, does not change acceptance, does not touch forbidden zones; when stuck it submits a report instead of forcing it |
| control plane (Loom) | orchestration, acceptance, bookkeeping, alerting, replaying progress | does not make business judgments, does not decide priority for the human |

## 6. Newcomer onboarding path

The whole skill is two decisions plus six commands:

```text
loom init → loom goal -i → loom constraints   # set direction
loom plan                                      # open a batch (pick groups, slice)
loom dispatch → loom verify → loom merge       # run-slice loop
loom board / loom route stats                  # watch progress and retrospect
```

Path from zero to platform:

- **Day one**: init, set direction, land two or three slices of the first batch, see the first mergeable diff;
- **First week**: multiple batch cycles, `specs/` grows as modules land, direction records accumulate;
- **After that**: the platform grows batch by batch; the contract layer on disk is the platform's memory, and new agents and new people take the same onboarding path.

It does not depend on understanding agents' memory mechanisms and token economics; that is the control plane's business.

## 7. Non-goals

- **No automatic planning**: batches, priorities, and direction are always decided by humans; `AGENTS.md` lists it among things not to do early; the model only slices, it does not pick topics;
- no ceremony for its own sake: no extra meetings or role titles to "look agile";
- no story points or velocity metrics;
- no general-purpose project management SaaS; this workflow is a methodology layer on top of Loom, not a new product line.

## 8. Landing

1. Build `loom plan` (the core command of the open-a-batch ceremony, including non-interactive preview and human review of the diff);
2. the methodology doc and the README quick-start land together with `loom plan`, with the commands and the ceremony validating each other;
3. finish the remaining judgment gate of the four-step plan first; it corresponds to the last link of the machine gate and is also the precondition for the evolution of slice review.

### 8.1 Evolution of slice review

The premise is that people get lazy as the engineering grows: once the change exceeds some size, human review inevitably degrades into rubber-stamping. So agent-assisted review is not optional; it is the condition for this intervention point to survive long term. Three levels, accumulating trust:

| Level | Who looks at what | Human workload |
|---|---|---|
| **v1 (today, confirmed)** | the human reads all diffs and merges manually, same as the current way of doing software | full |
| **v2** | a review task (review role, frontier model) produces a verdict report with file/line references; the human reads the conclusion + spot-checks key diffs | reduced to reading the report |
| **v3** | review intensity is configurable: strict mode requires `verdict: pass` before allowing merge, plus human final review; after trust accumulates it relaxes to proportional spot-checks | spot-checks only |

Boundary: at all three levels the agent only produces suggestions and evidence; **the merge decision stays in the gating policy, the human keeps final approval and veto power; removing human review is out of scope for this proposal**. v2's precondition is that the judgment gate lands so the verdict is consumed by the gate (today verify only checks that the verdict string exists, see the four-step plan).

Until `loom plan` lands, this document is only a proposal.
