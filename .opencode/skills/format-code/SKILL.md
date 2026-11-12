---
name: Code Formatting
description: "Use when asked to format or check formatting, or when the task is specifically formatting Rust/YAML/SQL/JSON/Markdown/Shell/TOML files that already have formatter config. Do not use for ordinary edits, commits, lint, or tests."
---

# Code Formatting

## Trigger gate

Use this skill only when at least one is true:

- The user explicitly asks to format, reformat, or check formatting.
- The target files are Rust, YAML, SQL, JSON, Markdown, Shell, or TOML, and the project already has formatter configuration, and the task is specifically about formatting those files.

Do not load it for:

- Ordinary code edits, commits, or feature work.
- Lint findings, type errors, or failing tests.
- A repo with no formatter config, unless the user asks to add one.

## Process

1. Find the project root and read `.editorconfig`, formatter configs, `Makefile`, `justfile`, `pre-commit` config, and CI config.
2. Prefer the repo's own command (`make fmt`, `just fmt`, `pnpm format`, `./scripts/dev.sh ...`) over a raw tool invocation.
3. Run the check mode first. Report what would change.
4. Apply the formatter only to the target files. Respect excludes, generated files, and vendored code.
5. Re-run the check mode. Show `git diff --stat` or an equivalent summary.
6. Report the command, files changed, and any files skipped. Do not commit.

## Tool selection

Read `references/formatters.md` for config files and commands. Short version:

| Type | Check | Write |
|---|---|---|
| Rust | `cargo fmt --all -- --check` | `cargo fmt --all` |
| YAML | `yamllint .` or `prettier --check "**/*.{yml,yaml}"` | `prettier --write ...` |
| SQL | `sqlfluff lint` | `sqlfluff fix` |
| JSON | `prettier --check ...` or `jq . file` | `prettier --write ...` |
| Markdown | `prettier --check ...` or `markdownlint` | `prettier --write ...` |
| Shell | `shfmt -d` | `shfmt -w` |
| TOML | `taplo fmt --check` | `taplo fmt` |

`cargo clippy`, `eslint`, and `sqlfluff lint` without `fix` are lint, not formatting. Do not present lint fixes as formatting.

## Loom repository note

Do not run host `cargo`, `rustfmt`, or `clippy`. Use the container entry point:

```bash
./scripts/dev.sh cargo fmt --all -- --check
./scripts/dev.sh cargo fmt --all
./scripts/dev.sh cargo clippy --workspace --all-targets -- -D warnings
```

For other tools, use them only if they are available in the container. If a tool is missing, report it; do not fall back to the host.

## Rules

- Formatting only. Do not change identifiers, control flow, semantics, or comments beyond what the formatter does.
- Do not hand-author style fixes and call them formatting.
- Do not format generated, vendored, minified, or lock files unless the user names them.
- If no config exists, do not impose defaults. Ask whether to add a config.
- If the formatter would change semantics or fails, stop and show the error.
- For YAML and SQL, run a syntax/parse check after formatting.
- For Markdown, do not reflow prose unless the project convention says so.
- Never commit or push as part of this skill.

## References

- `references/formatters.md`: config discovery, commands, and per-language notes.

Attribution: see `.opencode/skill-pilot/THIRD_PARTY_NOTICES.md`.
