#!/usr/bin/env bash
# Isolate which message shape the DeepSeek relay rejects (the live 403 on the
# second request of a tool loop). Credentials come from .env.local; nothing
# secret is printed. Hard cap: 6 requests x (small prompt + <=512 output).
set -euo pipefail
cd /work/src

: "${LOOM_DEEPSEEK_API_BASE:?copy .env.example to .env.local and fill it}"
: "${LOOM_DEEPSEEK_API_KEY:?copy .env.example to .env.local and fill it}"

export LOOM_API_BASE="$LOOM_DEEPSEEK_API_BASE"
export LOOM_API_KEY="$LOOM_DEEPSEEK_API_KEY"
export LOOM_PROBE_MODELS="${LOOM_PROBE_MODELS:-deepseek-flash,mimo-v2.6-pro}"

cargo test -q -p loom-native --test relay_probe -- --ignored --nocapture
