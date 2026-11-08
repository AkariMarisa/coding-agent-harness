#!/usr/bin/env bash
# Offline churn-projection check (doc/15 §1.3A, doc/16 §8.2): oversized old
# tool payloads must be elided from later requests while the most recent turn
# stays verbatim. Scripted provider => no network, no tokens.
#
#   ./scripts/dev.sh bash /work/scripts/smoke-churn.sh
set -euo pipefail
cd /work

rm -rf .smoke-churn
mkdir -p .smoke-churn
(
    cd /work/.smoke-churn
    git init -q
    git symbolic-ref HEAD refs/heads/main
    git config user.email "smoke@example.com"
    git config user.name "loom-smoke"
    echo churn > README.md
    git add README.md
    git commit -qm "init"
)

cargo build -q --manifest-path /work/src/Cargo.toml -p loom-cli -p loom-native
LOOM_BIN=/work/src/target/debug/loom
loom() {
    "$LOOM_BIN" --root /work/.smoke-churn "$@"
}
loom init > /dev/null

cat > /work/.smoke-churn/task.json <<'EOF'
{
  "schema_version": "1.0",
  "task_id": "T-20261108-030",
  "goal_id": "G-20261108-churn",
  "title": "churn 投影冒烟：旧的大输出必须被投影，最近一轮保留原文",
  "role": "implement",
  "tier": "worker",
  "depends_on": [],
  "workspace": {
    "mode": "in_place",
    "base_ref": "main",
    "branch": "agent/T-20261108-030",
    "allowed_paths": ["churn.txt"],
    "forbidden_paths": []
  },
  "inputs": [],
  "constraints": {"max_files_touched": 2, "no_new_dependencies": true, "style": []},
  "acceptance": [
    {"id": "a1", "cmd": "test -f churn.txt", "expect_exit": 0, "timeout_s": 30}
  ],
  "budget": {"max_input_tokens": 10000, "max_output_tokens": 5000, "max_steps": 8, "max_wall_s": 300, "max_cost_usd": 0.25},
  "output": {"receipt": "4-line", "report_path": ".loom/runs/local/outbox/report.md", "required_artifacts": []}
}
EOF
loom task add --file /work/.smoke-churn/task.json > /dev/null

# Scripted sequence: write, two bulky outputs (EARLY), one bulky output (LATE),
# finish. Projection knobs are tightened so the tiny scripted history still
# crosses the threshold; assertions below use content that only exists inside
# command OUTPUT (never in the command itself).
cat > /work/.smoke-churn/script.json <<'EOF'
[
  {"tool_calls": [{"name": "write", "arguments": {"path": "churn.txt", "content": "ok\n"}}], "usage": {"input_tokens": 400, "cached_input_tokens": 100, "output_tokens": 40}},
  {"tool_calls": [{"name": "bash", "arguments": {"cmd": "printf 'EARLY-%04d\\n' $(seq 1 200)"}}], "usage": {"input_tokens": 500, "cached_input_tokens": 100, "output_tokens": 40}},
  {"tool_calls": [{"name": "bash", "arguments": {"cmd": "printf 'EARLY2-%04d\\n' $(seq 1 200)"}}], "usage": {"input_tokens": 600, "cached_input_tokens": 100, "output_tokens": 40}},
  {"tool_calls": [{"name": "bash", "arguments": {"cmd": "printf 'LATE-%04d\\n' $(seq 1 200)"}}], "usage": {"input_tokens": 700, "cached_input_tokens": 100, "output_tokens": 40}},
  {"tool_calls": [{"name": "finish", "arguments": {"status": "done", "summary": "churn projection smoke"}}], "usage": {"input_tokens": 500, "cached_input_tokens": 100, "output_tokens": 30}}
]
EOF

export LOOM_NATIVE_SCRIPT=/work/.smoke-churn/script.json
export LOOM_NATIVE_DEBUG=1
export LOOM_NATIVE_KEEP_TURNS=1
export LOOM_NATIVE_HISTORY_BYTES=1024
export LOOM_NATIVE_MIN_CLEAR=50

loom --format pretty dispatch T-20261108-030 --adapter native
loom --format pretty verify T-20261108-030

DBG=$(ls -t /work/.smoke-churn/.loom/runs/*/logs/provider.debug.log | head -1)
echo "debug: $DBG"
N=$(grep -c "=== request ===" "$DBG")
LAST=$(awk -v want="$N" '/=== request ===/{c++; next} c==want{print; exit}' "$DBG")

echo "$LAST" | grep -q "\[elided" || { echo "FAIL: last request has no elision marker" >&2; exit 1; }
echo "$LAST" | grep -q "LATE-0001" || { echo "FAIL: recent turn must stay verbatim" >&2; exit 1; }
if echo "$LAST" | grep -q "EARLY-0001"; then
    echo "FAIL: old bulky output was resent in the last request" >&2
    exit 1
fi
grep -q "EARLY-0001" "$DBG" || { echo "FAIL: EARLY output never entered history?" >&2; exit 1; }

echo "CHURN_SMOKE_OK (requests=$N, old payload elided, recent turn kept)"
