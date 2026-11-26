#!/bin/sh
# Static checks for .opencode/skills/ — the `skills-check` job of
# .loom.ci.toml (doc/16 §9.2). The rules live in `loom skill verify`
# so there is exactly one implementation; this script only picks the
# toolchain: the container when CI (or any container) provides it,
# otherwise the repo's own container script, never a host toolchain.
set -eu
cd "$(dirname "$0")/.."

if [ -n "${LOOM_CI:-}" ] || [ -e /.dockerenv ] || [ -e /run/.containerenv ]; then
    exec cargo run -q --manifest-path src/Cargo.toml -p loom-cli -- skill verify
fi
exec ./scripts/dev.sh cargo run -q --manifest-path src/Cargo.toml -p loom-cli -- skill verify
