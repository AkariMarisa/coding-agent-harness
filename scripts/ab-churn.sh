#!/usr/bin/env bash
# D: step-level projection A/B on a real model (`doc/16` §8.4).
#
# One small read -> write -> finish task, run twice in throw-away repos:
#   off  LOOM_NATIVE_HISTORY_BYTES=0            (history replayed verbatim)
#   on   LOOM_NATIVE_HISTORY_BYTES=4096         (projection active)
#        LOOM_NATIVE_KEEP_TURNS=1
#        LOOM_NATIVE_MIN_CLEAR=200
#
# The "on" knobs are deliberately aggressive: shipped defaults (24KB / keep 4
# / clear 2000) would not trigger inside a 4-step task, and proving the lever
# should not cost 60k tokens. Numbers are estimates; usage.jsonl is the truth.
#
#   ./scripts/dev.sh bash /work/scripts/ab-churn.sh --arm off
#   ./scripts/dev.sh bash /work/scripts/ab-churn.sh --arm on
#   ./scripts/dev.sh bash /work/scripts/ab-churn.sh --report
#
# Default model is official deepseek-flash (trustworthy usage numbers).
# Token cap: LOOM_AB_MAX_TOKENS (default 30000, input+output across both arms),
# checked after every arm so the first arm cannot spend the whole budget.
set -euo pipefail
cd /work

TOKEN_CAP="${LOOM_AB_MAX_TOKENS:-30000}"
. /work/scripts/model-prices.sh
AB_MODEL="${LOOM_AB_MODEL:-deepseek-flash}"
AB_WIRE="${LOOM_AB_WIRE:-chat}"
AB_BASE="${LOOM_AB_BASE:-${LOOM_DEEPSEEK_API_BASE:-}}"
AB_KEY_ENV="${LOOM_AB_KEY_ENV:-LOOM_DEEPSEEK_API_KEY}"
ROOT=/work/.smoke-abchurn

arm=""
report=0
while [ $# -gt 0 ]; do
    case "$1" in
        --arm) arm="$2"; shift 2 ;;
        --report) report=1; shift ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

sum_field() {
    local file="$1" field="$2"
    grep -oE "\"$field\":[0-9]+" "$file" 2>/dev/null \
        | grep -oE '[0-9]+' | awk '{ s += $1 } END { print s + 0 }' || true
}

arm_usage() {
    cat "$ROOT/$1"/.loom/runs/*/outbox/usage.jsonl 2>/dev/null || true
}

token_total() {
    local total=0 dir
    for dir in off on; do
        local rows
        rows=$(arm_usage "$dir")
        total=$(( total
            + $(printf '%s\n' "$rows" | grep -oE '"(input|output)_tokens":[0-9]+' \
                | grep -oE '[0-9]+' | awk '{ s += $1 } END { print s + 0 }') ))
    done
    echo "$total"
}

if [ "$report" = 1 ]; then
    printf '%-4s %-8s %-6s %-8s %-8s %-8s %-8s %-9s %-8s %-8s\n' \
        arm steps input cached output avoided waste ratio_runs wall_s
    for dir in off on; do
        usage=$(arm_usage "$dir")
        churn=$(cat "$ROOT/$dir"/.loom/runs/*/logs/churn.json 2>/dev/null || echo '{}')
        steps=$(printf '%s\n' "$usage" | grep -c '"step":' || true)
        input=$(printf '%s\n' "$usage" | grep -oE '"input_tokens":[0-9]+' | grep -oE '[0-9]+' | awk '{s+=$1} END {print s+0}')
        cached=$(printf '%s\n' "$usage" | grep -oE '"cached_input_tokens":[0-9]+' | grep -oE '[0-9]+' | awk '{s+=$1} END {print s+0}')
        output=$(printf '%s\n' "$usage" | grep -oE '"output_tokens":[0-9]+' | grep -oE '[0-9]+' | awk '{s+=$1} END {print s+0}')
        avoided=$(printf '%s' "$churn" | grep -oE '"avoided_input_tokens": [0-9]+' | grep -oE '[0-9]+' || true)
        waste=$(printf '%s' "$churn" | grep -oE '"wasted_tokens_est": [0-9]+' | grep -oE '[0-9]+' || true)
        ratio=$(printf '%s' "$churn" | grep -oE '"churn_ratio": [0-9.]+' | grep -oE '[0-9.]+' || true)
        wall=$(cat "$ROOT/$dir/wall_s" 2>/dev/null || echo 0)
        printf '%-4s %-8s %-6s %-8s %-8s %-9s %-8s %-8s %-8s\n' \
            "$dir" "${steps:-0}" "${input:-0}" "${cached:-0}" "${output:-0}" \
            "${avoided:-0}" "${waste:-0}" "${ratio:-0}" "${wall:-0}"
    done
    echo
    echo "per-step (input tokens; on arm also sent history bytes):"
    for dir in off on; do
        echo "  [$dir]"
        arm_usage "$dir" | while IFS= read -r line; do
            [ -n "$line" ] || continue
            step=$(printf '%s' "$line" | grep -oE '"step":[0-9]+' | grep -oE '[0-9]+')
            input=$(printf '%s' "$line" | grep -oE '"input_tokens":[0-9]+' | grep -oE '[0-9]+')
            cached=$(printf '%s' "$line" | grep -oE '"cached_input_tokens":[0-9]+' | grep -oE '[0-9]+')
            full=$(printf '%s' "$line" | grep -oE '"history_bytes":[0-9]+' | grep -oE '[0-9]+')
            sent=$(printf '%s' "$line" | grep -oE '"projected_bytes":[0-9]+' | grep -oE '[0-9]+')
            tools=$(printf '%s' "$line" | grep -oE '"tool_names":\[[^]]*\]' || true)
            printf '    step %-2s input=%-6s cached=%-6s history=%-6s sent=%-6s %s\n' \
                "${step:-?}" "${input:-0}" "${cached:-0}" "${full:-0}" "${sent:-0}" "$tools"
        done
    done
    exit 0
fi

case "$arm" in
    off) PROJECTION_ENV=(LOOM_NATIVE_HISTORY_BYTES=0) ;;
    on)  PROJECTION_ENV=(
            LOOM_NATIVE_HISTORY_BYTES=4096
            LOOM_NATIVE_KEEP_TURNS=1
            LOOM_NATIVE_MIN_CLEAR=200
         ) ;;
    *) echo "usage: ab-churn.sh --arm off|on | --report" >&2; exit 2 ;;
esac
if [ -z "$AB_BASE" ]; then
    echo "error: no endpoint; set LOOM_DEEPSEEK_API_BASE (or LOOM_AB_BASE)" >&2
    exit 2
fi

cargo build -q --manifest-path /work/src/Cargo.toml -p loom-cli -p loom-native
LOOM_BIN=/work/src/target/debug/loom

# Fixed starting state per arm: same repo, same fixture bytes, same task.
rm -rf "$ROOT/$arm"
mkdir -p "$ROOT/$arm"
(
    cd "$ROOT/$arm"
    git init -q
    git symbolic-ref HEAD refs/heads/main
    git config user.email "ab@example.com"
    git config user.name "loom-ab"
    mkdir -p src
    # ~6KB of read-only fixture: one `read` is enough to cross 4096 bytes and
    # to save well over MIN_CLEAR once the old turn is projected.
    for i in $(seq 1 160); do
        printf 'fixture line %03d: beta gamma delta epsilon zeta eta theta\n' "$i"
    done > src/data.txt
    echo ab > README.md
    git add README.md src/data.txt
    git commit -qm "init"
)

loom() {
    "$LOOM_BIN" --root "$ROOT/$arm" "$@"
}
loom init > /dev/null

cat > "$ROOT/$arm/.loom/config.toml" <<EOF
[[models]]
alias = "ab"
tier = "worker"
model_id = "$AB_MODEL"
tool_calls = true
wire = "$AB_WIRE"
base_url = "$AB_BASE"
key_env = "$AB_KEY_ENV"
EOF
price_toml "$AB_MODEL" >> "$ROOT/$arm/.loom/config.toml"

cat > "$ROOT/$arm/task.json" <<'EOF'
{
  "schema_version": "1.0",
  "task_id": "T-20261109-901",
  "goal_id": "G-20261109-churn",
  "title": "read src/data.txt 1-160，write out/first.txt 第一行，finish(done)，禁止 bash",
  "role": "implement",
  "tier": "worker",
  "depends_on": [],
  "workspace": {
    "mode": "in_place",
    "base_ref": "main",
    "branch": "agent/T-20261109-901",
    "allowed_paths": ["src/**", "out/**"],
    "forbidden_paths": []
  },
  "inputs": [],
  "constraints": {"max_files_touched": 2, "no_new_dependencies": true, "style": []},
  "acceptance": [{"id": "A1", "cmd": "test -f out/first.txt", "expect_exit": 0, "timeout_s": 30}],
  "budget": {"max_input_tokens": 12000, "max_output_tokens": 1200, "max_steps": 6, "max_wall_s": 300, "max_cost_usd": 0.1},
  "output": {"receipt": "4-line", "report_path": ".loom/runs/local/outbox/report.md", "required_artifacts": []}
}
EOF

loom task add --file "$ROOT/$arm/task.json" > /dev/null

export LOOM_NATIVE_DEBUG=1
export LOOM_API_TIMEOUT_S="${LOOM_API_TIMEOUT_S:-60}"
export "${PROJECTION_ENV[@]}"

echo "== arm $arm ($AB_MODEL wire=$AB_WIRE, ${PROJECTION_ENV[*]}) =="
started=$SECONDS
status=0
loom dispatch T-20261109-901 --adapter native --in-place || status=$?
wall=$(( SECONDS - started ))
echo "$wall" > "$ROOT/$arm/wall_s"
loom verify T-20261109-901 || status=$?

run_dir=$(ls -dt "$ROOT/$arm"/.loom/runs/*/ 2>/dev/null | head -1)
echo "--- usage ---"
cat "$run_dir/outbox/usage.jsonl" 2>/dev/null || echo "(no usage rows)"
echo "--- churn ---"
cat "$run_dir/logs/churn.json" 2>/dev/null || echo "(no churn report)"
echo "--- finish ---"
cat "$run_dir/outbox/final.md" 2>/dev/null || true

echo "tokens so far (both arms): $(token_total) (cap $TOKEN_CAP)"
if [ "$(token_total)" -gt "$TOKEN_CAP" ]; then
    echo "token cap exceeded" >&2
    exit 1
fi
exit "$status"
