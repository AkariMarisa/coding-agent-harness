#!/usr/bin/env bash
# M4 exit check (`doc/05` §6): 10 tasks, 2 concurrent dispatches, serial
# merge queue, no lost updates. Runs against a throw-away git repo so merging
# never touches the Loom checkout.
set -euo pipefail
cd /work

rm -rf .smoke-parallel
mkdir -p .smoke-parallel

cd /work/.smoke-parallel
git init -q
git symbolic-ref HEAD refs/heads/main
git config user.email "smoke@example.com"
git config user.name "loom-smoke"
echo "parallel smoke" > README.md
git add README.md
git commit -qm "init"
cd /work

cargo build -q --manifest-path /work/src/Cargo.toml -p loom-cli -p loom-native
LOOM_BIN=/work/src/target/debug/loom

loom() {
    "$LOOM_BIN" --root /work/.smoke-parallel "$@"
}

loom init

gen() {
    local n="$1" id="T-20261108-$1"
    mkdir -p /work/.smoke-parallel/.loom/gen
    cat > "/work/.smoke-parallel/.loom/gen/task-$n.json" <<EOF
{
  "schema_version": "1.0",
  "task_id": "$id",
  "goal_id": "G-20261108-parallel",
  "title": "parallel task $n",
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
  "constraints": {"max_files_touched": 4, "no_new_dependencies": true, "style": []},
  "acceptance": [{"id": "A1", "cmd": "test -f out/$n.txt", "expect_exit": 0, "timeout_s": 30}],
  "budget": {"max_input_tokens": 120000, "max_output_tokens": 20000, "max_steps": 8, "max_wall_s": 300, "max_cost_usd": 0.25},
  "output": {"receipt": "4-line", "report_path": ".loom/runs/local/outbox/report.md", "required_artifacts": ["verify_log"]}
}
EOF
    loom task add --file "/work/.smoke-parallel/.loom/gen/task-$n.json"

    # Scripted native run: write the file, commit it on the task branch, spend
    # a second so two dispatches visibly overlap, then finish.
    cat > "/work/.smoke-parallel/.loom/gen/script-$n.json" <<EOF
[
  {"tool_calls": [{"name": "write", "arguments": {"path": "out/$n.txt", "content": "$n\n"}}], "usage": {"input_tokens": 1200, "cached_input_tokens": 400, "output_tokens": 60}},
  {"tool_calls": [{"name": "bash", "arguments": {"cmd": "git add out/$n.txt && git commit -qm 'add $n'", "timeout_s": 30}}], "usage": {"input_tokens": 900, "cached_input_tokens": 200, "output_tokens": 50}},
  {"tool_calls": [{"name": "bash", "arguments": {"cmd": "sleep 1"}}], "usage": {"input_tokens": 700, "cached_input_tokens": 100, "output_tokens": 20}},
  {"tool_calls": [{"name": "finish", "arguments": {"status": "done", "summary": "wrote out/$n.txt"}}], "usage": {"input_tokens": 500, "cached_input_tokens": 100, "output_tokens": 30}}
]
EOF
}

for n in 001 002 003 004 005 006 007 008 009 010; do
    gen "$n"
done

# Sample `running` while dispatches go out, to prove 2 really ran at once.
counts=/work/.smoke-parallel/.loom/gen/running.counts
: > "$counts"
(
    while :; do
        "$LOOM_BIN" --root /work/.smoke-parallel --format json board 2>/dev/null \
            | sed -n 's/^ *"running": \([0-9][0-9]*\),*$/\1/p' >> "$counts" || true
        sleep 0.1
    done
) &
sampler=$!

dispatch_one() {
    local n="$1"
    LOOM_NATIVE_SCRIPT="/work/.smoke-parallel/.loom/gen/script-$n.json" \
        loom --format pretty dispatch "T-20261108-$n" --adapter native
}

# Two dispatch processes at a time: M4 starts at parallel.max_running = 2.
n=1
while [ "$n" -le 10 ]; do
    a=$(printf '%03d' "$n")
    b=$(printf '%03d' "$((n + 1))")
    dispatch_one "$a" &
    p1=$!
    dispatch_one "$b" &
    p2=$!
    wait "$p1" || { echo "dispatch $a failed" >&2; exit 1; }
    wait "$p2" || { echo "dispatch $b failed" >&2; exit 1; }
    n=$((n + 2))
done

kill "$sampler" 2>/dev/null || true
max_seen=$(grep -E '^[0-9]+$' "$counts" | sort -n | tail -1)
if [ "${max_seen:-0}" -lt 2 ]; then
    echo "expected 2 concurrent dispatches, saw max ${max_seen:-0}" >&2
    exit 1
fi

for n in 001 002 003 004 005 006 007 008 009 010; do
    loom --format pretty verify "T-20261108-$n" || { echo "verify $n failed" >&2; exit 1; }
done

# The merge queue is global: a foreign `**` lock must hold merging back.
queue_lock="/work/.smoke-parallel/.loom/locks/$(printf '%s' '**' | sha256sum | cut -c1-12).lock"
printf '{"v":1,"path":"**","mode":"write","owner_task":"T-20261108-099","owner_run":"R-foreign","owner_pid":%s,"lease_until_ms":%s}\n' \
    "$$" "$(( $(date +%s) * 1000 + 3600000 ))" > "$queue_lock"
if loom --format pretty merge T-20261108-001; then
    echo "expected the merge queue to wait for the foreign writer" >&2
    exit 1
fi
rm -f "$queue_lock"

merge_start=$(date +%s)
for n in 001 002 003 004 005 006 007 008 009 010; do
    loom --format pretty merge "T-20261108-$n" || { echo "merge $n failed" >&2; exit 1; }
done
merge_secs=$(( $(date +%s) - merge_start ))
if [ "$merge_secs" -ge 300 ]; then
    echo "merges took ${merge_secs}s, expected < 300s" >&2
    exit 1
fi

events=$(loom --format pretty event list)
merged=$(printf '%s\n' "$events" | grep -c "task.merged" || true)
failed=$(printf '%s\n' "$events" | grep -c "task.failed" || true)
blocked=$(printf '%s\n' "$events" | grep -c "task.blocked" || true)
[ "$merged" -eq 10 ] || { echo "expected 10 task.merged events, got $merged" >&2; exit 1; }
[ "$failed" -eq 0 ] || { echo "expected no task.failed events, got $failed" >&2; exit 1; }
[ "$blocked" -eq 0 ] || { echo "expected no task.blocked events, got $blocked" >&2; exit 1; }

board_json=$(loom --format json board)
board_ids=$(printf '%s' "$board_json" | grep -o '"T-20261108-[0-9]*"' | wc -l)
[ "$board_ids" -eq 10 ] || { echo "expected 10 tasks in the merged column, got $board_ids" >&2; exit 1; }

for n in 001 002 003 004 005 006 007 008 009 010; do
    content=$(cat "/work/.smoke-parallel/out/$n.txt")
    [ "$content" = "$n" ] || { echo "out/$n.txt lost or corrupted: '$content'" >&2; exit 1; }
done

if [ -n "$(ls -A /work/.smoke-parallel/.loom/locks 2>/dev/null)" ]; then
    echo "locks were not released after the run" >&2
    ls -A /work/.smoke-parallel/.loom/locks >&2
    exit 1
fi
if [ -n "$(ls -A /work/.smoke-parallel/.worktrees 2>/dev/null)" ]; then
    echo "worktrees were not cleaned up after merge" >&2
    exit 1
fi
leftover_branches=$(git -C /work/.smoke-parallel branch --list 'agent/*')
if [ -n "$leftover_branches" ]; then
    echo "task branches were not deleted: $leftover_branches" >&2
    exit 1
fi

loom --format pretty doctor > /dev/null

echo "max concurrent running: $max_seen"
echo "10 merges in ${merge_secs}s"
echo "PARALLEL_SMOKE_OK"
