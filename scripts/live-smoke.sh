#!/usr/bin/env bash
# One real round trip per relay: endpoint reachability, model id, tool calls,
# and whether the relay reports cached tokens. Credentials come from
# .env.local via scripts/dev.sh; nothing secret is printed.
#
#   ./scripts/dev.sh bash /work/scripts/live-smoke.sh
#
# Override model ids with LOOM_LIVE_DEEPSEEK_MODEL / LOOM_LIVE_GPT_MODEL.
# LOOM_LIVE_ONLY=deepseek|gpt runs a single relay (the other is broken or
# unavailable); default runs both.
set -euo pipefail
cd /work

# Hard token cap: per-task budget is40k input +12k output (thinking models
# can burn thousands of output tokens deliberating in a single step — the
# relay's deepseek-flash once spent3794 on one), and the script fails if the
# whole run exceeds100k input+output.

ONLY="${LOOM_LIVE_ONLY:-both}"

if [ "$ONLY" = "both" ] || [ "$ONLY" = "deepseek" ]; then
    for name in LOOM_DEEPSEEK_API_BASE LOOM_DEEPSEEK_API_KEY; do
        if [[ -z "${!name:-}" ]]; then
            echo "error: $name is not set; copy .env.example to .env.local and fill it" >&2
            exit 2
        fi
    done
fi
if [ "$ONLY" = "both" ] || [ "$ONLY" = "gpt" ]; then
    for name in LOOM_GPT_API_BASE LOOM_GPT_API_KEY; do
        if [[ -z "${!name:-}" ]]; then
            echo "error: $name is not set; copy .env.example to .env.local and fill it" >&2
            exit 2
        fi
    done
fi

DEEPSEEK_MODEL="${LOOM_LIVE_DEEPSEEK_MODEL:-deepseek-flash}"
DEEPSEEK_WIRE="${LOOM_LIVE_DEEPSEEK_WIRE:-chat}"
DEEPSEEK_BASE="${LOOM_DEEPSEEK_API_BASE:-}"
GPT_MODEL="${LOOM_LIVE_GPT_MODEL:-gpt-6.1-sol}"
GPT_WIRE="${LOOM_LIVE_GPT_WIRE:-chat}"
GPT_BASE="${LOOM_GPT_API_BASE:-}"
export LOOM_API_TIMEOUT_S="${LOOM_API_TIMEOUT_S:-60}"

cargo build -q --manifest-path /work/src/Cargo.toml -p loom-cli -p loom-native
LOOM_BIN=/work/src/target/debug/loom

rm -rf /work/.smoke-live
mkdir -p /work/.smoke-live
(
    cd /work/.smoke-live
    git init -q
    git symbolic-ref HEAD refs/heads/main
    git config user.email "smoke@example.com"
    git config user.name "loom-smoke"
    echo live > README.md
    git add README.md
    git commit -qm "init"
)

loom() {
    "$LOOM_BIN" --root /work/.smoke-live "$@"
}
loom init

# The default goal.md is a placeholder that mentions "orchestrator"; real
# models read it and role-confuse. Give the smoke repo an actual goal.
cat > /work/.smoke-live/.loom/goal.md <<'EOF'
# Goal

本仓库只做一件事：按任务标题在 out/ 下创建指定文件并提交。你是执行者，不是编排器；没有、也不需要派发工具，直接用 read/write/bash 完成任务。
EOF

# Dump every request/response next to the run logs to diagnose relay
# compatibility (bodies carry no credentials).
export LOOM_NATIVE_DEBUG=1

# base_url + key_env exercise the per-model endpoint switch: each alias
# carries its own relay, so one dispatch can cross providers. Only models
# whose relay base is configured get an entry (LOOM_LIVE_ONLY may skip one).
: > /work/.smoke-live/.loom/config.toml
if [ -n "$DEEPSEEK_BASE" ]; then
    cat >> /work/.smoke-live/.loom/config.toml <<EOF
[[models]]
alias = "live-deepseek"
tier = "worker"
model_id = "$DEEPSEEK_MODEL"
tool_calls = true
wire = "$DEEPSEEK_WIRE"
base_url = "$DEEPSEEK_BASE"
key_env = "LOOM_DEEPSEEK_API_KEY"
EOF
fi
if [ -n "$GPT_BASE" ]; then
    cat >> /work/.smoke-live/.loom/config.toml <<EOF
[[models]]
alias = "live-gpt"
tier = "frontier"
model_id = "$GPT_MODEL"
tool_calls = true
wire = "$GPT_WIRE"
base_url = "$GPT_BASE"
key_env = "LOOM_GPT_API_KEY"
EOF
fi

gen_task() {
    local n="$1" tier="$2" file="$3" title="$4"
    local id="T-20261108-$n"
    cat > "/work/.smoke-live/.loom/gen-$n.json" <<EOF
{
  "schema_version": "1.0",
  "task_id": "$id",
  "goal_id": "G-20261108-live",
  "title": "$title",
  "role": "implement",
  "tier": "$tier",
  "depends_on": [],
  "workspace": {
    "mode": "worktree",
    "base_ref": "main",
    "branch": "agent/$id",
    "allowed_paths": ["$file"],
    "forbidden_paths": []
  },
  "inputs": [],
  "constraints": {"max_files_touched": 4, "no_new_dependencies": true, "style": []},
  "acceptance": [{"id": "A1", "cmd": "test -f $file", "expect_exit": 0, "timeout_s": 30}],
  "budget": {"max_input_tokens": 40000, "max_output_tokens": 12000, "max_steps": 8, "max_wall_s": 600, "max_cost_usd": 0.25},
  "output": {"receipt": "4-line", "report_path": ".loom/runs/local/outbox/report.md", "required_artifacts": ["verify_log"]}
}
EOF
    loom task add --file "/work/.smoke-live/.loom/gen-$n.json"
}

gen_task 021 worker "out/live-deepseek.txt" \
    "用 write 工具创建 out/live-deepseek.txt，内容为 ok，然后调用 finish(status=done)"
gen_task 022 frontier "out/live-gpt.txt" \
    "用 write 工具创建 out/live-gpt.txt，内容为 ok，然后调用 finish(status=done)"

dump_failure() {
    local label="$1"
    echo "--- $label: adapter stderr ---"
    find /work/.smoke-live/.loom/runs -name adapter.stderr.log -newer /work/.smoke-live/README.md \
        -exec tail -n 40 {} + 2>/dev/null || true
    echo "--- $label: provider debug (last exchange) ---"
    find /work/.smoke-live/.loom/runs -name provider.debug.log -newer /work/.smoke-live/README.md \
        -exec tail -n 120 {} + 2>/dev/null || true
    echo "--- $label: events ---"
    loom --format pretty event list --limit 10 || true
}

report_run() {
    local n="$1" label="$2"
    local run_dir
    run_dir=$(ls -dt /work/.smoke-live/.loom/runs/*/ | head -1)
    echo "--- $label: usage ---"
    cat "$run_dir/outbox/usage.jsonl" 2>/dev/null || echo "(no usage rows)"
    if grep -oE '"cached_input_tokens": *[0-9]+' "$run_dir/outbox/usage.jsonl" 2>/dev/null \
        | grep -qvE ': *0$'; then
        echo "cached_tokens: relay reports a non-zero value"
    else
        echo "cached_tokens: zero or absent in relay usage"
    fi
    echo "--- $label: final ---"
    cat "$run_dir/outbox/final.md" 2>/dev/null || true
    echo "--- $label: tools ---"
    cat "$run_dir/logs/tools.jsonl" 2>/dev/null || echo "(no tool telemetry)"
    echo "--- $label: finish reasons ---"
    grep -o '"finish_reason":"[a-z]*"' "$run_dir/logs/provider.debug.log" 2>/dev/null | sort | uniq -c || true
}

run_live() {
    local n="$1" alias="$2" label="$3"
    echo "== $label ($alias) =="
    local started=$SECONDS
    if ! loom --format pretty dispatch "T-20261108-$n" --adapter native --model "$alias"; then
        dump_failure "$label"
        return 1
    fi
    if ! loom --format pretty verify "T-20261108-$n"; then
        dump_failure "$label"
        return 1
    fi
    echo "$label: dispatch+verify ok in $((SECONDS - started))s"
    report_run "$n" "$label"
}

fail=0
if [ "$ONLY" = "both" ] || [ "$ONLY" = "deepseek" ]; then
    run_live 021 live-deepseek "$DEEPSEEK_MODEL" || fail=1
fi
if [ "$ONLY" = "both" ] || [ "$ONLY" = "gpt" ]; then
    run_live 022 live-gpt "$GPT_MODEL" || fail=1
fi

if [ "$fail" -ne 0 ]; then
    echo "LIVE_SMOKE_FAILED"
    exit 1
fi

# Hard token cap for the whole script: input+output across both runs must
# stay under 100k (cached tokens are a subset of input and not double
# counted). Per-task budgets already cap each run at 40k+5k.
total_tokens=$(
    cat /work/.smoke-live/.loom/runs/*/outbox/usage.jsonl 2>/dev/null \
        | grep -oE '"(input|output)_tokens":[0-9]+' \
        | grep -oE '[0-9]+' \
        | awk '{sum += $1} END {print sum + 0}'
)
echo "total tokens this run: $total_tokens (cap 100000)"
if [ "$total_tokens" -gt 100000 ]; then
    echo "token cap exceeded" >&2
    exit 1
fi
echo "LIVE_SMOKE_OK"
