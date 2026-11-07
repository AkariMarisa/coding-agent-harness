---
name: 文档写作与去 AI 味
description: "Use only when creating or substantially rewriting Markdown docs (README, design docs, ADRs, reports, release notes), or when the user explicitly asks to deslop/humanize/polish/format prose. Do not use for normal chat replies, code comments, or short factual answers."
---

# Docs Writing

## Trigger gate

Use this skill only when at least one is true:

- The user asks to write, rewrite, restructure, or review a Markdown document.
- The user asks to remove AI-sounding prose, humanize, deslop, polish, or format a draft.
- The task explicitly produces a document as its deliverable.

Do not load it for:

- Normal conversational answers.
- Code comments, commit messages, or one-line status updates.
- Pure factual Q&A with no document deliverable.

If the user only wants a general answer-style rule, apply the three-line baseline below and stop. Do not read the reference files.

## Baseline

- Lead with the answer; cut throat-clearing.
- Use concrete nouns, numbers, paths, and mechanisms. No empty importance claims.
- Keep the writer's meaning, uncertainty, and voice.

## Modes

- `write`: create a new document.
- `edit`: make the minimum effective edit to an existing draft.
- `detect`: audit only; quote the pattern and give a short fix. Do not rewrite.
- `format`: fix Markdown structure/formatting only; do not rewrite content.

If the mode is unclear, infer it from the request. Ask only when the audience or publication target changes the answer.

## Workflow

1. Read `AGENTS.md`, neighboring docs, and the target's existing style before writing.
2. State the document's job in one sentence and identify the audience.
3. For anything longer than about 300 words, make a short outline first.
4. Draft or edit.
5. For `write`/`edit`/`detect`, read `references/ai-tells.md` and run the pass.
6. For `format`, or when structure is part of the problem, read `references/markdown.md`.
7. Check facts. Do not invent sources, numbers, quotes, or examples.
8. Output the edited document plus a short **What changed** list (3-6 bullets). For `detect`, output findings only.

## Rules

- Preserve frontmatter, code blocks, quoted text, link targets, and tables unless the user asks to change them.
- Chinese docs: use Chinese punctuation; keep identifiers, paths, and protocol fields in English; insert a space between Chinese and Latin/number when it helps readability.
- Do not add emoji headings, decorative bold, or headers over two-sentence sections.
- Do not end with a summary that restates the piece.
- Do not use em dashes as a default rhythm crutch.
- Do not smooth a specific fact into generic importance.
- Do not rewrite a strong human sentence just for consistency.
- Prefer prose over bullets when two sentences are clearer than a list.
- If a claim needs a source and none exists, say so instead of inventing one.

## References

- `references/ai-tells.md`: AI-tell catalog, edit/detect workflow, Chinese-specific patterns.
- `references/markdown.md`: heading/list/code/link/table conventions.

Load a reference only when its mode is in scope. Both files are intentionally not loaded by default.

Attribution: see `.opencode/skill-pilot/THIRD_PARTY_NOTICES.md`.
