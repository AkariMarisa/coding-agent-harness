#!/usr/bin/env bash
set -euo pipefail
cd /work

rm -rf .smoke
mkdir -p .smoke

loom() {
    cargo run -q --manifest-path /work/src/Cargo.toml -p loom-cli -- --root /work/.smoke "$@"
}

loom init

cat > /work/.smoke/task.json <<'EOF'
{
  "schema_version": "1.0",
  "task_id": "T-20261106-001",
  "goal_id": "G-20261106-smoke",
  "title": "smoke task",
  "role": "implement",
  "tier": "worker",
  "depends_on": [],
  "workspace": {
    "mode": "in_place",
    "base_ref": "main",
    "branch": "agent/T-20261106-001",
    "allowed_paths": ["src/**"],
    "forbidden_paths": []
  },
  "inputs": [],
  "constraints": {"max_files_touched": 4, "no_new_dependencies": true, "style": []},
  "acceptance": [{"id": "A1", "cmd": "true", "expect_exit": 0, "timeout_s": 30}],
  "budget": {"max_input_tokens": 120000, "max_output_tokens": 20000, "max_steps": 25, "max_wall_s": 300, "max_cost_usd": 0.25},
  "output": {"receipt": "4-line", "report_path": ".loom/runs/local/outbox/report.md", "required_artifacts": ["verify_log"]}
}
EOF

loom task add --file /work/.smoke/task.json
loom task list
loom route T-20261106-001
loom context compile T-20261106-001 --kind worker | tail -4

# M4 path locks: a foreign lock on the same write set blocks dispatch with
# task.blocked(code=lock_owner); removing it lets the task through.
lock_until=$(( $(date +%s) * 1000 + 3600000 ))
foreign_lock="/work/.smoke/.loom/locks/$(printf '%s' 'src/**' | sha256sum | cut -c1-12).lock"
printf '{"v":1,"path":"src/**","mode":"write","owner_task":"T-20261106-009","owner_run":"R-foreign","owner_pid":%s,"lease_until_ms":%s}\n' \
    "$$" "$lock_until" > "$foreign_lock"
if loom dispatch T-20261106-001 --adapter mock --in-place; then
    echo "expected the foreign path lock to block dispatch" >&2
    exit 1
fi
loom event list --task T-20261106-001 | grep -q "lock_owner" || {
    echo "expected task.blocked(code=lock_owner)" >&2
    exit 1
}
rm -f "$foreign_lock"

# Parallel cap: two other live write sets exhaust parallel.max_running (=2),
# so the third dispatch blocks with task.blocked(code=parallel_cap).
cap_a="/work/.smoke/.loom/locks/$(printf '%s' 'a/**' | sha256sum | cut -c1-12).lock"
cap_b="/work/.smoke/.loom/locks/$(printf '%s' 'b/**' | sha256sum | cut -c1-12).lock"
printf '{"v":1,"path":"a/**","mode":"write","owner_task":"T-20261106-010","owner_run":"R-foreign","owner_pid":%s,"lease_until_ms":%s}\n' \
    "$$" "$lock_until" > "$cap_a"
printf '{"v":1,"path":"b/**","mode":"write","owner_task":"T-20261106-011","owner_run":"R-foreign","owner_pid":%s,"lease_until_ms":%s}\n' \
    "$$" "$lock_until" > "$cap_b"
if loom dispatch T-20261106-001 --adapter mock --in-place; then
    echo "expected parallel.max_running to block dispatch" >&2
    exit 1
fi
loom event list --task T-20261106-001 | grep -q "parallel_cap" || {
    echo "expected task.blocked(code=parallel_cap)" >&2
    exit 1
}
rm -f "$cap_a" "$cap_b"

loom dispatch T-20261106-001 --adapter mock --in-place
loom verify T-20261106-001
loom route stats
loom status
loom event list --limit 20

# Failover: the primary alias fails with exit 4, the configured same-tier
# fallback takes over and the task still passes.
cat > /work/.smoke/.loom/config.toml <<'EOF'
[[models]]
alias = "worker"
tier = "worker"
model_id = "deepseek-v4-flash"

[[models]]
alias = "worker-b"
tier = "worker"
model_id = "deepseek-v4-flash-b"

[[models]]
alias = "frontier"
tier = "frontier"
model_id = "gpt-6.1-sol"

[routing.fallbacks]
worker = ["worker-b"]
EOF

cat > /work/.smoke/task-fallback.json <<'EOF'
{
  "schema_version": "1.0",
  "task_id": "T-20261106-003",
  "goal_id": "G-20261106-smoke",
  "title": "smoke failover task",
  "role": "implement",
  "tier": "worker",
  "depends_on": [],
  "workspace": {
    "mode": "in_place",
    "base_ref": "main",
    "branch": "agent/T-20261106-003",
    "allowed_paths": ["src/**"],
    "forbidden_paths": []
  },
  "inputs": [],
  "constraints": {"max_files_touched": 4, "no_new_dependencies": true, "style": []},
  "acceptance": [{"id": "A1", "cmd": "true", "expect_exit": 0, "timeout_s": 30}],
  "budget": {"max_input_tokens": 120000, "max_output_tokens": 20000, "max_steps": 25, "max_wall_s": 300, "max_cost_usd": 0.25},
  "output": {"receipt": "4-line", "report_path": ".loom/runs/local/outbox/report.md", "required_artifacts": ["verify_log"]}
}
EOF

loom task add --file /work/.smoke/task-fallback.json
loom route T-20261106-003
export LOOM_MOCK_FAIL_ALIAS=worker
loom dispatch T-20261106-003 --adapter mock --in-place
unset LOOM_MOCK_FAIL_ALIAS
loom verify T-20261106-003
loom route stats
loom event list --task T-20261106-003

loom adapter canary mock

echo "SMOKE_OK"
