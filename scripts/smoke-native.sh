#!/usr/bin/env bash
set -euo pipefail
cd /work

rm -rf .smoke-native
mkdir -p .smoke-native

loom() {
    cargo run -q --manifest-path /work/src/Cargo.toml -p loom-cli -- --root /work/.smoke-native "$@"
}

loom init

cat > /work/.smoke-native/task.json <<'EOF'
{
  "schema_version": "1.0",
  "task_id": "T-20261106-002",
  "goal_id": "G-20261106-native",
  "title": "native smoke",
  "role": "implement",
  "tier": "worker",
  "depends_on": [],
  "workspace": {
    "mode": "in_place",
    "base_ref": "main",
    "branch": "agent/T-20261106-002",
    "allowed_paths": ["src/**"],
    "forbidden_paths": []
  },
  "inputs": [],
  "constraints": {"max_files_touched": 4, "no_new_dependencies": true, "style": []},
  "acceptance": [{"id": "A1", "cmd": "test -f src/native.txt", "expect_exit": 0, "timeout_s": 30}],
  "budget": {"max_input_tokens": 120000, "max_output_tokens": 20000, "max_steps": 5, "max_wall_s": 300, "max_cost_usd": 0.25},
  "output": {"receipt": "4-line", "report_path": ".loom/runs/local/outbox/report.md", "required_artifacts": ["verify_log"]}
}
EOF

cat > /work/.smoke-native/script.json <<'EOF'
[
  {
    "tool_calls": [
      {"name": "write", "arguments": {"path": "src/native.txt", "content": "ok\n"}}
    ],
    "usage": {"input_tokens": 1200, "cached_input_tokens": 400, "output_tokens": 120}
  },
  {
    "tool_calls": [
      {"name": "finish", "arguments": {"status": "done", "summary": "wrote src/native.txt"}}
    ],
    "usage": {"input_tokens": 800, "cached_input_tokens": 200, "output_tokens": 40}
  }
]
EOF

loom task add --file /work/.smoke-native/task.json
loom route T-20261106-002
LOOM_NATIVE_SCRIPT=/work/.smoke-native/script.json loom dispatch T-20261106-002 --adapter native --in-place
loom verify T-20261106-002
loom route stats
loom task show T-20261106-002
loom event list --task T-20261106-002
loom adapter canary native

echo "NATIVE_SMOKE_OK"
