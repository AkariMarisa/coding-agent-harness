# Markdown conventions

Use these rules when structure or formatting is part of the task. They do not override an existing project style.

## Structure

- `#` only for a standalone document title.
- `##` for main sections; `###` for subsections.
- Do not skip heading levels.
- Do not put a heading over a two-sentence section; use prose.
- Keep one blank line between blocks. No trailing whitespace.
- Do not hard-wrap prose unless the project already does.

## Lists

- `-` for unordered lists, not `*` or `+`.
- `1.` for ordered or sequential steps.
- Nest with two spaces.
- Keep list items parallel in grammar.
- Use a list only when the content is genuinely enumerable. Two sentences of prose are often clearer.

## Code

- Inline backticks for identifiers, paths, commands, flags, and field names.
- Fenced blocks must have a language tag: `rust`, `toml`, `yaml`, `sql`, `bash`, `json`, `markdown`, `text`.
- Do not add a language tag that the renderer does not support; use `text` for plain output.
- Keep code blocks byte-accurate when editing prose around them.

## Links and references

- Use descriptive link text: `[installation guide](url)`, not `[click here](url)`.
- Reference issues as `#123` when the project uses that convention.
- Do not change link targets unless the task is to fix links.

## Emphasis

- Bold key terms and warnings; do not bold whole sentences.
- Italics sparingly, for a term or a light aside.
- Do not combine bold and italics.
- Do not use emphasis as a substitute for structure.

## Tables

- Use a table when comparing several items on fixed dimensions.
- Do not use a table for two short facts; prose is smaller and easier to read.
- Keep columns few and headers short.

## Chinese text

- Use Chinese punctuation in Chinese prose: `，。；：？！` and `“”`.
- Keep identifiers, paths, protocol fields, and CLI flags in English.
- Insert a space between Chinese and Latin letters or numbers when it improves reading: `Rust 1.80`, `3 个 crate`.
- Do not translate enum values, error codes, or symbol names.
- Prefer short paragraphs; avoid walls of text.

## Anti-patterns

- Mixed list markers in one list.
- Code blocks without a language tag.
- Headings used as emphasis.
- Emoji headings or decorative bullets.
- Excessive blank lines.
- A summary section that only restates the body.
- Reflowing code blocks or tables while editing prose.
