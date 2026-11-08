#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

ENGINE="${CONTAINER_ENGINE:-}"
if [[ -z "$ENGINE" ]]; then
    if command -v podman >/dev/null 2>&1; then
        ENGINE=podman
    elif command -v docker >/dev/null 2>&1; then
        ENGINE=docker
    else
        echo "error: podman or docker is required" >&2
        exit 1
    fi
fi

IMAGE="${LOOM_DEV_IMAGE:-loom-dev}"

if ! "$ENGINE" image exists "$IMAGE" >/dev/null 2>&1; then
    "$ENGINE" build -t "$IMAGE" -f "$ROOT/Containerfile" "$ROOT"
fi

mkdir -p "$ROOT/.cache/cargo"

MOUNT="$ROOT:/work"
if [[ "$ENGINE" == "podman" ]]; then
    MOUNT="$ROOT:/work:Z"
fi

# Provider credentials live in .env.local (already gitignored, see
# .env.example). Only whitelisted names are forwarded into the container, and
# only when they are actually set — nothing is echoed.
if [[ -f "$ROOT/.env.local" ]]; then
    # shellcheck source=/dev/null
    source "$ROOT/.env.local"
fi
ENVS=(-e CARGO_HOME=/work/.cache/cargo)
for name in \
    LOOM_API_BASE LOOM_API_KEY OPENAI_API_KEY \
    LOOM_DEEPSEEK_API_BASE LOOM_DEEPSEEK_API_KEY \
    LOOM_GPT_API_BASE LOOM_GPT_API_KEY \
    LOOM_LIVE_ONLY LOOM_LIVE_DEEPSEEK_MODEL LOOM_LIVE_DEEPSEEK_WIRE \
    LOOM_LIVE_GPT_MODEL LOOM_LIVE_GPT_WIRE LOOM_PROBE_MODELS
do
    if [[ -n "${!name:-}" ]]; then
        ENVS+=(-e "$name")
    fi
done
# Also forward any exported *_API_KEY so [[models]] key_env can name it.
while IFS= read -r name; do
    case "$name" in
        *_API_KEY) ENVS+=(-e "$name") ;;
    esac
done < <(compgen -e)

exec "$ENGINE" run --rm -i \
    -v "$MOUNT" \
    -w /work/src \
    "${ENVS[@]}" \
    "$IMAGE" "$@"
