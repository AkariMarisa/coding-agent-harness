#!/usr/bin/env bash
# M4 parallel A/B: the same fixed task set under one max_running arm per
# invocation. Each run appends a row to .loom/ab/results.tsv; --report prints
# the comparison table.
#
#   ./scripts/dev.sh bash /work/scripts/ab-parallel.sh --arm 1
#   ./scripts/dev.sh bash /work/scripts/ab-parallel.sh --arm 2
#   ./scripts/dev.sh bash /work/scripts/ab-parallel.sh --arm 3
#   ./scripts/dev.sh bash /work/scripts/ab-parallel.sh --report
#
# Default model is official deepseek-flash (trustworthy usage numbers). To
# probe mimo through the GPT relay instead:
#   LOOM_AB_MODEL=mimo-v2.6-pro LOOM_AB_BASE="$LOOM_GPT_API_BASE" \
#   LOOM_AB_KEY_ENV=LOOM_GPT_API_KEY ./scripts/dev.sh bash /work/scripts/ab-parallel.sh --arm 1
#
# Token cap: LOOM_AB_MAX_TOKENS (default20000) input+output per invocation,
# checked after every batch so one batch cannot silently burn the budget.
set -euo pipefail
cd /work

TOKEN_CAP="${LOOM_AB_MAX_TOKENS:-20000}"
AB_MODEL="${LOOM_AB_MODEL:-deepseek-flash}"
AB_WIRE="${LOOM_AB_WIRE:-chat}"
AB_BASE="${LOOM_AB_BASE:-${LOOM_DEEPSEEK_API_BASE:-}}"
AB_KEY_ENV="${LOOM_AB_KEY_ENV:-LOOM_DEEPSEEK_API_KEY}"
TASKS="${LOOM_AB_TASKS:-3}"
RESULTS=/work/.loom/ab/results.tsv

arm=0
report=0
while [ $# -gt 0 ]; do
    case "$1" in
        --arm) arm="$2"; shift 2 ;;
        --tasks) TASKS="$2"; shift 2 ;;
        --report) report=1; shift ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

if [ "$report" = 1 ]; then
    if [ ! -s "$RESULTS" ]; then
        echo "no A/B results yet; run with --arm first" >&2
        exit 2
    fi
    printf '%-6s %-6s %-18s %-10s %-10s %-8s %-8s %-8s %-8s %-6s %-6s\n' \
        arm tasks model wall_s max_task input cached output tokens cache% ok
    sort -t$'\t' -k2,2n -k1,1 "$RESULTS" | while IFS=$'\t' \
        read -r ts a t model wall wallmax input cached output tokens ok; do
        cache_pct=0
        if [ "$input" -gt 0 ]; then
            cache_pct=$((cached * 100 / input))
        fi
        printf '%-6s %-6s %-18s %-10s %-10s %-8s %-8s %-8s %-8s %-6s %-6s\n' \
            "$a" "$t" "$model" "$wall" "$wallmax" "$input" "$cached" \
            "$output" "$tokens" "${cache_pct}%" "$ok"
    done
    exit 0
fi

if [ "$arm" -lt 1 ] || [ "$arm" -gt 3 ]; then
    echo "usage: ab-parallel.sh --arm 1|2|3 | --report [--tasks N]" >&2
    exit 2
fi
if [ -z "$AB_BASE" ]; then
    echo "error: no endpoint; set LOOM_DEEPSEEK_API_BASE (or LOOM_AB_BASE)" >&2
    exit 2
fi

cargo build -q --manifest-path /work/src/Cargo.toml -p loom-cli -p loom-native
LOOM_BIN=/work/src/target/debug/loom

# Fixed task set in a throw-away repo, rebuilt every run so all arms face the
# same starting state.
rm -rf /work/.smoke-ab
mkdir -p /work/.smoke-ab
(
    cd /work/.smoke-ab
    git init -q
    git symbolic-ref HEAD refs/heads/main
    git config user.email "ab@example.com"
    git config user.name "loom-ab"
    echo ab > README.md
    git add README.md
    git commit -qm "init"
)
mkdir -p /work/.smoke-ab/.loom/gen

loom() {
    "$LOOM_BIN" --root /work/.smoke-ab "$@"
}
loom init > /dev/null

cat > /work/.smoke-ab/.loom/config.toml <<EOF
[[models]]
alias = "ab"
tier = "worker"
model_id = "$AB_MODEL"
tool_calls = true
wire = "$AB_WIRE"
base_url = "$AB_BASE"
key_env = "$AB_KEY_ENV"

[parallel]
max_running = $arm
EOF

# Lean tasks: write once and finish. No recon allowed — multi-step history is
# what makes one task cost5k+ tokens, and the whole arm must fit the cap.
for ((i = 1; i <= TASKS; i++)); do
    n=$(printf '%03d' "$i")
    id="T-20261108-$n"
    cat > "/work/.smoke-ab/.loom/gen-$n.json" <<EOF
{
  "schema_version": "1.0",
  "task_id": "$id",
  "goal_id": "G-20261108-ab",
  "title": "用 write 工具创建 out/$n.txt（内容为 $n），禁止运行 bash、禁止读文件，写完立即 finish(status=done)",
  "role": "implement",
  "tier": "worker",
  "depends_on": [],
  "workspace": {
    "mode": "worktree",
    "base_ref": "main",
    "branch": "agent/$id",
    "allowed_paths": ["out/$n.txt"],
    "forbidden_paths": []
  },
  "inputs": [],
  "constraints": {"max_files_touched": 1, "no_new_dependencies": true, "style": []},
  "acceptance": [{"id": "A1", "cmd": "test -f out/$n.txt", "expect_exit": 0, "timeout_s": 30}],
  "budget": {"max_input_tokens": 6000, "max_output_tokens": 1500, "max_steps": 6, "max_wall_s": 600, "max_cost_usd": 0.25},
  "output": {"receipt": "4-line", "report_path": ".loom/runs/local/outbox/report.md", "required_artifacts": []}
}
EOF
    loom task add --file "/work/.smoke-ab/.loom/gen-$n.json" > /dev/null
done

sum_field() {
    cat /work/.smoke-ab/.loom/runs/*/outbox/usage.jsonl 2>/dev/null \
        | grep -oE "\"$1\":[0-9]+" | grep -oE '[0-9]+' \
        | awk '{s += $1} END {print s + 0}' \
        || true
}

token_total() {
    echo $(( $(sum_field input_tokens) + $(sum_field output_tokens) ))
}

cap_check() {
    local total
    total=$(token_total)
    echo "tokens so far: $total (cap $TOKEN_CAP)"
    if [ "$total" -gt "$TOKEN_CAP" ]; then
        echo "token cap exceeded: $total > $TOKEN_CAP" >&2
        exit 1
    fi
}

export LOOM_NATIVE_DEBUG=1
export LOOM_API_TIMEOUT_S="${LOOM_API_TIMEOUT_S:-60}"

walls_file=$(mktemp)
trap 'rm -f "$walls_file"' EXIT
fail=0

run_one() {
    local n="$1" t0 t1 status=0
    t0=$(date +%s)
    loom --format pretty dispatch "T-20261108-$n" --adapter native --model ab \
        || status=$?
    t1=$(date +%s)
    echo "$n $((t1 - t0)) $status" >> "$walls_file"
    return "$status"
}

# Dispatch in batches of $arm (the [parallel] max_running under test).
n=1
dispatch_start=$(date +%s)
while [ "$n" -le "$TASKS" ]; do
    batch=()
    while [ "${#batch[@]}" -lt "$arm" ] && [ "$n" -le "$TASKS" ]; do
        run_one "$(printf '%03d' "$n")" &
        batch+=($!)
        n=$((n + 1))
    done
    for pid in "${batch[@]}"; do
        wait "$pid" || fail=1
    done
    cap_check
done
dispatch_wall=$(( $(date +%s) - dispatch_start ))

for ((i = 1; i <= TASKS; i++)); do
    n=$(printf '%03d' "$i")
    loom --format pretty verify "T-20261108-$n" || fail=1
done
cap_check

max_task_wall=$(awk '{if ($2 > m) m = $2} END {print m + 0}' "$walls_file")
input=$(sum_field input_tokens)
cached=$(sum_field cached_input_tokens)
output=$(sum_field output_tokens)
total=$((input + output))
ok=ok
if [ "$fail" -ne 0 ]; then
    ok=FAIL
fi

mkdir -p "$(dirname "$RESULTS")"
printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$arm" "$TASKS" "$AB_MODEL" \
    "$dispatch_wall" "$max_task_wall" "$input" "$cached" "$output" \
    "$total" "$ok" >> "$RESULTS"

echo "arm=$arm tasks=$TASKS model=$AB_MODEL dispatch_wall=${dispatch_wall}s max_task=${max_task_wall}s tokens=$total/$TOKEN_CAP ok=$ok"
echo "results appended to $RESULTS (use --report to compare arms)"
if [ "$fail" -ne 0 ]; then
    exit 1
fi
