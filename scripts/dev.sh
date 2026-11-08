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

# Optional same-path mounts for target-project work: mounting at the original
# absolute path keeps project config (local.properties, build_local.sh) valid.
# Set them in .env.local; missing paths are skipped.
for name in LOOM_TARGET_REPO LOOM_ANDROID_SDK LOOM_JAVA_HOME LOOM_GRADLE_HOME; do
    path="${!name:-}"
    if [[ -n "$path" && -d "$path" ]]; then
        ENVS+=(-v "$path:$path:Z" -e "$name")
        case "$name" in
            LOOM_JAVA_HOME) ENVS+=(-e "JAVA_HOME=$path") ;;
            LOOM_ANDROID_SDK)
                ENVS+=(-e "ANDROID_HOME=$path" -e "ANDROID_SDK_ROOT=$path")
                ;;
            LOOM_GRADLE_HOME) ENVS+=(-e "GRADLE_USER_HOME=$path") ;;
        esac
    fi
done

# Maven local cache (Robolectric downloads android-all into ~/.m2 at test
# time). HOME must point at its parent so `user.home/.m2` resolves there.
if [[ -n "${LOOM_MAVEN_HOME:-}" && -d "$LOOM_MAVEN_HOME" ]]; then
    ENVS+=(-v "$LOOM_MAVEN_HOME:$LOOM_MAVEN_HOME:Z" -e "HOME=$(dirname "$LOOM_MAVEN_HOME")")
fi

exec "$ENGINE" run --rm -i \
    -v "$MOUNT" \
    -w /work/src \
    "${ENVS[@]}" \
    "$IMAGE" "$@"
