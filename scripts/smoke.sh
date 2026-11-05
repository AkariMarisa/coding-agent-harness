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
loom context compile T-20261106-001 --kind worker | tail -4
loom dispatch T-20261106-001 --adapter mock --in-place
loom verify T-20261106-001
loom status
loom event list --limit 20
loom adapter canary mock

echo "SMOKE_OK"
