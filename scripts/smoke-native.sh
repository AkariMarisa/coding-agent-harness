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
# Step-level attribution (`doc/16` §8.4): every native run leaves a churn
# report, and the aggregate command sees it.
find /work/.smoke-native/.loom/runs -name churn.json | grep -q .
loom churn stats --by model --format pretty
loom churn stats --format json | grep -q '"estimated": true'
loom task show T-20261106-002
loom event list --task T-20261106-002
loom adapter canary native

# --- Q&A protocol (`doc/15` §2.4/§2.6): ask → needs_input → answer → ready ---
# Acceptance is `true` so verify keeps the `needs_input` claim instead of
# recording a failure (an all-pass acceptance + non-done claim stays claims').
cat > /work/.smoke-native/task-q.json <<'EOF'
{
  "schema_version": "1.0",
  "task_id": "T-20261106-003",
  "goal_id": "G-20261106-native",
  "title": "qa handshake",
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
  "budget": {"max_input_tokens": 120000, "max_output_tokens": 20000, "max_steps": 5, "max_wall_s": 300, "max_cost_usd": 0.25},
  "output": {"receipt": "4-line", "report_path": ".loom/runs/local/outbox/report.md", "required_artifacts": ["verify_log"]}
}
EOF

cat > /work/.smoke-native/script-ask.json <<'EOF'
[
  {
    "tool_calls": [
      {"name": "ask", "arguments": {"questions": [
        {"id": "Q1",
         "question": "Widen allowed paths to docs/**?",
         "why_now": "the report belongs in docs/ but the spec says src/**",
         "blocking": true,
         "scope": "scope",
         "options": [
           {"id": "A", "label": "keep src/** only", "consequence": "report stays out of the tree"},
           {"id": "B", "label": "allow docs/**", "consequence": "docs can change"}
         ],
         "recommended": "A",
         "default": "A",
         "reversible": true,
         "evidence": ["src/native.txt"]}
      ]}}
    ],
    "usage": {"input_tokens": 900, "cached_input_tokens": 300, "output_tokens": 60}
  }
]
EOF

cat > /work/.smoke-native/script-done.json <<'EOF'
[
  {
    "tool_calls": [
      {"name": "finish", "arguments": {"status": "done", "summary": "answered, proceeding on A"}}
    ],
    "usage": {"input_tokens": 700, "cached_input_tokens": 250, "output_tokens": 40}
  }
]
EOF

loom task add --file /work/.smoke-native/task-q.json > /dev/null

# The model asks: exit 2, questions.json lands in the run outbox.
LOOM_NATIVE_SCRIPT=/work/.smoke-native/script-ask.json \
    loom dispatch T-20261106-003 --adapter native --in-place > /dev/null || true
qfile=$(ls -t /work/.smoke-native/.loom/runs/*/outbox/questions.json | head -1)
grep -q '"task_id": "T-20261106-003"' "$qfile"
grep -q '"blocking": true' "$qfile"
grep -q '"scope": "scope"' "$qfile"

# Verify keeps the claim and enriches the event with the Q&A.
loom verify T-20261106-003 > /dev/null || true
loom event list --task T-20261106-003 | grep -q 'task.needs_input'
loom event list --task T-20261106-003 | grep -q 'question_ids'
loom task show T-20261106-003 | grep -q 'needs_input'

# Answering the blocking question emits task.answered → ready.
loom answer T-20261106-003 -q Q1 -c A --rationale "keep the tree small" --format json \
    | grep -q '"status": "ready"'
loom event list --task T-20261106-003 | grep -q 'task.answered'
loom task show T-20261106-003 | grep -q 'ready'

# The next packet carries the answer (failure-pack channel).
LOOM_NATIVE_SCRIPT=/work/.smoke-native/script-done.json \
    loom dispatch T-20261106-003 --adapter native --in-place > /dev/null
packet=$(ls -t /work/.smoke-native/.loom/runs/*/context/packet.md | head -1)
grep -q '## questions' "$packet"
grep -q 'answered: A — keep the tree small' "$packet"

loom verify T-20261106-003 > /dev/null
loom task show T-20261106-003 | grep -q 'passed'

echo "NATIVE_SMOKE_OK"
