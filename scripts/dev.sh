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

exec "$ENGINE" run --rm -i \
    -v "$MOUNT" \
    -w /work/src \
    -e CARGO_HOME=/work/.cache/cargo \
    "$IMAGE" "$@"
