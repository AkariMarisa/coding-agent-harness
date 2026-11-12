# Skill Pilot Notes

> Language: **English** | [简体中文](README.zh-CN.md)

This is a project-level skill pilot inside the Loom repository; skills are not installed into `~/.config/opencode/skills`.

Location: `.opencode/skills/`. OpenCode discovers them automatically within the current project; teammates get them right after cloning the repository, with no extra installation.

## 1. Currently Integrated Skills

| ID | Purpose | Auto-advertised | Body cost (approx.) | Reference files loaded on demand |
|---|---|---|---:|---|
| `docs-writing` | Document writing, structure, Markdown formatting, de-AI-ifying prose | yes | ~0.6K tokens | `ai-tells.md` ~1.1K; `markdown.md` ~0.5K |
| `code-quality-review` | Architecture boundaries, dependency direction, dependency cycles, code smells, refactor assessment | yes | ~1.2K tokens | `architecture.md` ~0.7K; `smells.md` ~0.7K |
| `architecture-design` | Module boundaries, interfaces, seams, option comparison, ADRs | yes | ~0.8K tokens | `deep-modules.md` ~0.8K; `adr.md` ~0.7K |
| `refactor-code` | Behavior-preserving cleanup, dedup, simplification, fixing confirmed smells | yes | ~0.8K tokens | `techniques.md` ~1.0K |
| `format-code` | Rust, YAML, SQL, JSON, Markdown, Shell, TOML formatting | yes | ~0.6K tokens | `formatters.md` ~0.6K |

The body is loaded only when the model invokes the skill, and reference files are read only when the corresponding mode needs them. The full text is not sent on every conversation turn.

## 2. Trigger Scope

An overly broad trigger scope is a skill's largest hidden cost: once the description gets the model to pick it, the body enters the context; if every answer triggers it, the communication savings get eaten by tokens.

All five current skills declare an explicit `Trigger gate` and state in their `description` what they should **not** be used for.

| ID | Should trigger | Should not trigger |
|---|---|---|
| `docs-writing` | Creating or substantially rewriting READMEs, design docs, ADRs, reports, release notes; the user explicitly asks to polish, de-AI-ify, or format a document | Ordinary conversational answers, code comments, commit messages, short status updates, pure factual Q&A |
| `code-quality-review` | The user explicitly asks for a review/audit; checking module boundaries, dependency direction, dependency cycles; finding code smells; evaluating a refactor | Ordinary feature implementation, self-checking after every code change, formatting/lint, one-line bug fixes |
| `architecture-design` | Designing a new module/interface/seam/data ownership; changing existing boundaries; comparing architecture options; writing ADRs | Reviewing existing code, ordinary feature implementation, naming/style issues, "let's refactor the architecture" with no concrete change pressure |
| `refactor-code` | The user explicitly asks to refactor, clean up, simplify, or deduplicate; landing a review conclusion; naming a specific smell to fix | Review/find-problems-only requests, ordinary feature or bug fixes, formatting/lint, generated code/third-party code, unconfirmed whole-repo rewrites |
| `format-code` | The user explicitly asks to format/check formatting; the target file types have a configured formatter and the task is formatting those files | Ordinary code edits, commits, lint errors, failing tests, projects without formatter configuration |

The boundaries between `code-quality-review`, `architecture-design`, and `refactor-code` are deliberately separated:

- **review** is read-only; it produces findings with `file:line` and confidence levels.
- **design** produces options, interface contracts, migration order, and ADRs; it does not directly change code.
- **refactor** only makes behavior-preserving changes; the moment an interface or behavior must change, it falls back to design or splits into a separate task.

One decision worth calling out: **"answer style" was not made into a separate skill.** If writing rules were auto-loaded on every answer, the body would repeatedly enter the context, costing more than it saves. The three baseline rules (conclusion first, specifics, preserved voice) currently live in `docs-writing`'s baseline and only take effect on document tasks; ordinary answers follow the default language and style conventions in `AGENTS.md`. If a global answer style is genuinely needed later, it belongs in a standing instruction like `AGENTS.md`, not an auto-triggered skill.

## 3. Fixed Cost

OpenCode advertises the ID, name, and description of enabled skills at every model step; the body is not sent. The five descriptions total about 300 tokens/step.

- A 50-step session: about 15K tokens of fixed advertising cost.
- One false trigger: about 0.6K–1.2K tokens for the body; with reference files loaded it reaches 1.5K–2.2K.
- `metadata.opencode/autoinvoke: false` keeps a skill out of the advertisement list, but the user must then invoke it explicitly with `@skill-id`; for now automatic advertising is kept because all five descriptions are narrow enough.

If the number of skills grows later, compress the descriptions first rather than deleting the trigger boundaries. A description should answer "what it does + when to use it + when not to", in no more than two or three sentences.

## 4. Provenance and Licensing

These skills are adapted within the project, not verbatim copies. All sources are MIT-licensed; copyright and full license text are in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

| Source | Version | Used for |
|---|---|---|
| [`petergyang/no-ai-slop`](https://github.com/petergyang/no-ai-slop) | `000650b` | de-AI-ifying rules, detection patterns |
| [`mattpocock/skills`](https://github.com/mattpocock/skills) | `4ae2317` | Fowler code smell baseline, deep module/seam design vocabulary |
| [`addyosmani/agent-skills`](https://github.com/addyosmani/agent-skills) | `301b02b` | doc/ADR conventions, code simplification principles |
| [`cskwork/clean-code-skill`](https://github.com/cskwork/clean-code-skill) | `acd750b` | behavior-preserving refactor loop, Fowler technique catalog |

The Markdown formatting rules and formatter command table were compiled from general tool documentation and do not depend on any single skill.

## 5. Validation

During the pilot, fixed prompts check these boundaries:

1. An ordinary answer does not trigger `docs-writing`.
2. "Review the architecture issues in this diff" triggers only `code-quality-review`, not `architecture-design` or `refactor-code`.
3. "Design this module's interfaces and seams" triggers only `architecture-design` and does not directly change code.
4. "Clean up the duplicated code from the review" triggers `refactor-code` and does not redo a full review.
5. "Format these YAML/SQL files" triggers only `format-code` and does not incidentally change semantics.
6. "Commit this" does not trigger `format-code`.

When Loom later implements skill scanning, skill validation should include "description length, trigger/non-trigger examples, actual load counts, false-trigger cost" rather than only whether the skill is well written.
