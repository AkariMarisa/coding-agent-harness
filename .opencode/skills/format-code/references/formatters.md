# Formatter reference

Use the project's own command first. The table below is a fallback for discovering the right tool, not a license to impose a new style.

## Config discovery

Check, in order:

1. `.editorconfig`
2. `Makefile`, `justfile`, `Taskfile.yml`, `package.json` scripts
3. `.pre-commit-config.yaml`
4. CI workflow files
5. Language-specific config:
   - Rust: `rustfmt.toml`, `.rustfmt.toml`
   - YAML: `.yamllint`, `.yamllint.yaml`, `.yamllint.yml`
   - SQL: `.sqlfluff`, `pyproject.toml` (`[tool.sqlfluff]`), `.sqlfluffignore`
   - JSON/Markdown: `.prettierrc`, `prettier.config.*`, `.markdownlint.*`
   - Shell: `.shellcheckrc`, `.editorconfig`
   - TOML: `.taplo.toml`, `taplo.toml`

If a config exists, use the tool it configures. If several exist, prefer the repo command that already aggregates them.

## Rust

- Check: `cargo fmt --all -- --check`
- Write: `cargo fmt --all`
- Single file: `rustfmt --check path/to/file.rs` / `rustfmt path/to/file.rs`
- Lint: `cargo clippy --workspace --all-targets -- -D warnings`
- Loom: prepend `./scripts/dev.sh`; do not use host cargo.

## YAML

- Lint/format check: `yamllint .`
- Prettier check: `prettier --check "**/*.{yml,yaml}"`
- Prettier write: `prettier --write "**/*.{yml,yaml}"`
- Parse only: `yq eval '.' file.yaml >/dev/null` or a project-specific validator.
- Do not rewrite anchors, aliases, or comments unless the formatter owns them.

## SQL

- Lint: `sqlfluff lint path/`
- Fix: `sqlfluff fix path/`
- Check only: `sqlfluff lint --format github-annotation-native path/`
- Alternative formatter: `sqlformat --check file.sql`, `pg_format --check file.sql`.
- Dialect matters. Read `.sqlfluff` or ask before assuming PostgreSQL/MySQL/SQLite.
- Do not reformat migration files if the project treats them as immutable.

## JSON

- Check: `prettier --check file.json`
- Write: `prettier --write file.json`
- Minimal normalization/validation: `jq . file.json >/dev/null`
- Do not rewrite JSON with comments unless the tool supports JSONC.

## Markdown

- Check: `prettier --check "**/*.md"` or `markdownlint "**/*.md"`
- Write: `prettier --write "**/*.md"` or `markdownlint --fix "**/*.md"`
- Do not reflow prose line wrapping unless the project already does.
- Preserve frontmatter and fenced code blocks.

## Shell

- Check: `shfmt -d path/`
- Write: `shfmt -w path/`
- Lint: `shellcheck path/`
- Respect `.editorconfig` for indentation and shell dialect.

## TOML

- Check: `taplo fmt --check`
- Write: `taplo fmt`
- Do not reorder tables or comments beyond what `taplo` does.

## General safety

- Run check mode before write mode.
- Limit the file set to the task. Do not format the whole repo unless asked.
- Exclude `target/`, `node_modules/`, `vendor/`, generated code, and lock files.
- Show the changed files and a diff summary.
- If a formatter is not installed, report the missing binary and the expected config; do not substitute an unrelated tool.
- Formatting must be idempotent: run check again after write.
