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

# Inflight budget: a tiny parallel.max_inflight_cost_usd blocks dispatch with
# task.blocked(code=inflight_budget); `loom init --force` restores defaults.
cat > /work/.smoke/.loom/config.toml <<'EOF'
[parallel]
max_running = 2
max_inflight_cost_usd = 0.0001
EOF
if loom dispatch T-20261106-001 --adapter mock --in-place; then
    echo "expected parallel.max_inflight_cost_usd to block dispatch" >&2
    exit 1
fi
loom event list --task T-20261106-001 | grep -q "inflight_budget" || {
    echo "expected task.blocked(code=inflight_budget)" >&2
    exit 1
}
loom init --force > /dev/null

loom dispatch T-20261106-001 --adapter mock --in-place
loom verify T-20261106-001

# Merge queue: an in-place task is already on the base branch, so merge must
# reject it instead of rewriting history.
if loom merge T-20261106-001 --format pretty; then
    echo "expected in-place merge to be rejected" >&2
    exit 1
fi

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
# Merge only accepts a passed task: a pending one must be rejected.
if loom merge T-20261106-003 --format pretty; then
    echo "expected merge of a pending task to be rejected" >&2
    exit 1
fi
export LOOM_MOCK_FAIL_ALIAS=worker
loom dispatch T-20261106-003 --adapter mock --in-place
unset LOOM_MOCK_FAIL_ALIAS
loom verify T-20261106-003
loom route stats
loom event list --task T-20261106-003

loom adapter canary mock

# needs_input must not be recorded as task.failed (doc/15 §2.1): the task
# stays re-dispatchable and the escalation counter must not see a failure.
cat > /work/.smoke/task-needs-input.json <<'EOF'
{
  "schema_version": "1.0",
  "task_id": "T-20261106-004",
  "goal_id": "G-20261106-smoke",
  "title": "smoke needs_input task",
  "role": "implement",
  "tier": "worker",
  "depends_on": [],
  "workspace": {
    "mode": "in_place",
    "base_ref": "main",
    "branch": "agent/T-20261106-004",
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
loom task add --file /work/.smoke/task-needs-input.json > /dev/null
LOOM_MOCK_MODE=needs_input loom dispatch T-20261106-004 --adapter mock --in-place > /dev/null || true
# Verification cannot pass a needs_input claim, but it must move the task to
# needs_input instead of failed.
if loom verify T-20261106-004 --format json > /dev/null; then
    echo "expected verify of a needs_input claim to be non-passing" >&2
    exit 1
fi
loom event list --task T-20261106-004 --format json | grep -q '"task.needs_input"' || {
    echo "expected task.needs_input event" >&2
    exit 1
}
if loom event list --task T-20261106-004 --format json | grep -q '"task.failed"'; then
    echo "needs_input must not emit task.failed" >&2
    exit 1
fi
loom board --format json | grep -q '"T-20261106-004"' || {
    echo "expected needs_input task on the board" >&2
    exit 1
}
# task.needs_input opens a resident question item (doc/15 §2.2), generic or
# per-question — either way the view records one blocking todo.
grep -q '"blocking": 1' /work/.smoke/.loom/inbox.json || {
    echo "expected one blocking inbox item after needs_input" >&2
    exit 1
}
grep -q '"kind": "question"' /work/.smoke/.loom/inbox.json || {
    echo "expected a question item in the inbox" >&2
    exit 1
}
# `loom inbox --check` gates scripts on blocking items (doc/15 §2.2).
if loom inbox --check > /dev/null 2>&1; then
    echo "expected loom inbox --check to exit 2 while blocking" >&2
    exit 1
fi
loom inbox --line | grep -q '^\[!\] 1 blocking'
# The footer is a TTY-only stderr line (doc/15 §2.2): silent under a pipe,
# and `loom status` shares the same view with counts + top titles.
err=$(loom status 2>&1 >/dev/null)
case "$err" in
    *"[loom]"*)
        echo "footer must stay off on non-TTY" >&2
        exit 1
        ;;
esac
loom status | grep -q 'inbox:' || {
    echo "expected loom status to share the inbox view" >&2
    exit 1
}
loom status | grep -q '\[blocking\]' || {
    echo "expected loom status to show the top item title" >&2
    exit 1
}

# `loom top` is a read-only watcher (doc/15 §12.1): the needs_input task is
# in flight until answered, and JSON always prints exactly one snapshot.
loom top --format json | grep -q '"state": "needs_input"' || {
    echo "expected loom top to show the needs_input run" >&2
    exit 1
}
loom top --format pretty | grep -q '^loom top' || {
    echo "expected loom top pretty header when piped" >&2
    exit 1
}

# --- Declarative skills (doc/15 §12.3): loaded by declaration, never discovery ---
# Skills resolve under the project root, so the fixture lives in the smoke root.
mkdir -p /work/.smoke/.opencode/skills/smoke-skill
cat > /work/.smoke/.opencode/skills/smoke-skill/SKILL.md <<'EOF'
---
name: Smoke Skill
description: fixture skill for the smoke run
---
# Smoke Skill

SKILL_BODY_MARKER: lead with the answer; cut throat-clearing.
EOF

cat > /work/.smoke/task-skills.json <<'EOF'
{
  "schema_version": "1.0",
  "task_id": "T-20261106-009",
  "goal_id": "G-20261106-smoke",
  "title": "skills injection",
  "role": "implement",
  "tier": "worker",
  "depends_on": [],
  "workspace": {
    "mode": "in_place",
    "base_ref": "main",
    "branch": "agent/T-20261106-009",
    "allowed_paths": ["src/**"],
    "forbidden_paths": []
  },
  "inputs": [],
  "skills": ["smoke-skill"],
  "constraints": {"max_files_touched": 4, "no_new_dependencies": true, "style": []},
  "acceptance": [{"id": "A1", "cmd": "true", "expect_exit": 0, "timeout_s": 30}],
  "budget": {"max_input_tokens": 120000, "max_output_tokens": 20000, "max_steps": 5, "max_wall_s": 300, "max_cost_usd": 0.25},
  "output": {"receipt": "4-line", "report_path": ".loom/runs/local/outbox/report.md", "required_artifacts": []}
}
EOF
loom task add --file /work/.smoke/task-skills.json > /dev/null

# Compile injects the body and records the ids in the manifest.
loom context compile T-20261106-009 --out /work/.smoke/skills-ctx --format json \
    | grep -q '"skills"' || {
    echo "expected manifest to record injected skills" >&2; exit 1;
}
grep -q 'SKILL_BODY_MARKER' /work/.smoke/skills-ctx/packet.md || {
    echo "expected skill body injected into the packet" >&2; exit 1;
}

# Declared but absent → compile fails fast, never ships a skill-less packet.
cat > /work/.smoke/task-bad-skill.json <<'EOF'
{
  "schema_version": "1.0",
  "task_id": "T-20261106-010",
  "goal_id": "G-20261106-smoke",
  "title": "missing skill",
  "role": "implement",
  "tier": "worker",
  "depends_on": [],
  "workspace": {
    "mode": "in_place",
    "base_ref": "main",
    "branch": "agent/T-20261106-010",
    "allowed_paths": ["src/**"],
    "forbidden_paths": []
  },
  "inputs": [],
  "skills": ["no-such-skill"],
  "constraints": {"max_files_touched": 4, "no_new_dependencies": true, "style": []},
  "acceptance": [{"id": "A1", "cmd": "true", "expect_exit": 0, "timeout_s": 30}],
  "budget": {"max_input_tokens": 120000, "max_output_tokens": 20000, "max_steps": 5, "max_wall_s": 300, "max_cost_usd": 0.25},
  "output": {"receipt": "4-line", "report_path": ".loom/runs/local/outbox/report.md", "required_artifacts": []}
}
EOF
loom task add --file /work/.smoke/task-bad-skill.json > /dev/null
if loom context compile T-20261106-010 --format json > /dev/null 2>&1; then
    echo "expected compile to fail for a missing skill" >&2
    exit 1
fi

# Dispatch + verify: the receipt records what was injected (mechanically checkable).
loom dispatch T-20261106-009 --adapter mock > /dev/null
loom verify T-20261106-009 --format json > /work/.smoke/verify-skills.json || true
skills_receipt=$(grep -o '"receipt": "[^"]*"' /work/.smoke/verify-skills.json | head -1 | sed 's/.*"receipt": "//; s/"$//')
grep -q '"skills"' "$skills_receipt" || {
    echo "expected receipt to record injected skills" >&2; exit 1;
}
grep -q 'smoke-skill' "$skills_receipt" || {
    echo "expected receipt to name the injected skill" >&2; exit 1;
}

# --- Review verdict gate (doc/03): parse → fail fast → block/allow merge ---
# The scenario needs git (worktree mode + a committed report fixture); the
# rest of this file stays in-place and is unaffected.
git -C /work/.smoke init -q
git -C /work/.smoke symbolic-ref HEAD refs/heads/main
git -C /work/.smoke config user.email smoke@example.com
git -C /work/.smoke config user.name loom-smoke
mkdir -p /work/.smoke/docs
printf 'verdict: request_changes\n\nreasons: fixture\n' > /work/.smoke/docs/review.md
printf 'verdict: pass\n\nreasons: fixture\n' > /work/.smoke/docs/review-pass.md
printf 'no verdict in this report\n' > /work/.smoke/docs/review-empty.md
git -C /work/.smoke add docs
git -C /work/.smoke commit -qm "fixture: review reports"

cat > /work/.smoke/task-g1.json <<'EOF'
{
  "schema_version": "1.0",
  "task_id": "T-20261106-005",
  "goal_id": "G-20261106-smoke",
  "title": "gate fixture implement",
  "role": "implement",
  "tier": "worker",
  "depends_on": [],
  "workspace": {"mode": "worktree", "base_ref": "main", "branch": "agent/T-20261106-005", "allowed_paths": ["docs/**"], "forbidden_paths": []},
  "inputs": [],
  "constraints": {"max_files_touched": 2, "no_new_dependencies": true, "style": []},
  "acceptance": [{"id": "A1", "cmd": "true", "expect_exit": 0, "timeout_s": 30}],
  "budget": {"max_input_tokens": 120000, "max_output_tokens": 20000, "max_steps": 5, "max_wall_s": 300, "max_cost_usd": 0.25},
  "output": {"receipt": "4-line", "report_path": ".loom/runs/local/outbox/report.md", "required_artifacts": ["verify_log"]}
}
EOF
cat > /work/.smoke/task-g2.json <<'EOF'
{
  "schema_version": "1.0",
  "task_id": "T-20261106-006",
  "goal_id": "G-20261106-smoke",
  "title": "gate fixture review",
  "role": "review",
  "tier": "worker",
  "depends_on": ["T-20261106-005"],
  "workspace": {"mode": "worktree", "base_ref": "main", "branch": "agent/T-20261106-006", "allowed_paths": ["docs/**"], "forbidden_paths": []},
  "inputs": [],
  "constraints": {"max_files_touched": 2, "no_new_dependencies": true, "style": []},
  "acceptance": [{"id": "A1", "cmd": "grep -q verdict docs/review.md", "expect_exit": 0, "timeout_s": 30}],
  "budget": {"max_input_tokens": 120000, "max_output_tokens": 20000, "max_steps": 5, "max_wall_s": 300, "max_cost_usd": 0.25},
  "output": {"receipt": "4-line", "report_path": "docs/review.md", "required_artifacts": ["verify_log"]}
}
EOF
cat > /work/.smoke/task-g3.json <<'EOF'
{
  "schema_version": "1.0",
  "task_id": "T-20261106-007",
  "goal_id": "G-20261106-smoke",
  "title": "gate fixture review without verdict",
  "role": "review",
  "tier": "worker",
  "depends_on": [],
  "workspace": {"mode": "worktree", "base_ref": "main", "branch": "agent/T-20261106-007", "allowed_paths": ["docs/**"], "forbidden_paths": []},
  "inputs": [],
  "constraints": {"max_files_touched": 2, "no_new_dependencies": true, "style": []},
  "acceptance": [{"id": "A1", "cmd": "test -f docs/review-empty.md", "expect_exit": 0, "timeout_s": 30}],
  "budget": {"max_input_tokens": 120000, "max_output_tokens": 20000, "max_steps": 5, "max_wall_s": 300, "max_cost_usd": 0.25},
  "output": {"receipt": "4-line", "report_path": "docs/review-empty.md", "required_artifacts": ["verify_log"]}
}
EOF
cat > /work/.smoke/task-g2b.json <<'EOF'
{
  "schema_version": "1.0",
  "task_id": "T-20261106-008",
  "goal_id": "G-20261106-smoke",
  "title": "gate fixture approving review",
  "role": "review",
  "tier": "worker",
  "depends_on": ["T-20261106-005"],
  "workspace": {"mode": "worktree", "base_ref": "main", "branch": "agent/T-20261106-008", "allowed_paths": ["docs/**"], "forbidden_paths": []},
  "inputs": [],
  "constraints": {"max_files_touched": 2, "no_new_dependencies": true, "style": []},
  "acceptance": [{"id": "A1", "cmd": "grep -q verdict docs/review-pass.md", "expect_exit": 0, "timeout_s": 30}],
  "budget": {"max_input_tokens": 120000, "max_output_tokens": 20000, "max_steps": 5, "max_wall_s": 300, "max_cost_usd": 0.25},
  "output": {"receipt": "4-line", "report_path": "docs/review-pass.md", "required_artifacts": ["verify_log"]}
}
EOF
loom task add --file /work/.smoke/task-g1.json > /dev/null
loom task add --file /work/.smoke/task-g2.json > /dev/null
loom task add --file /work/.smoke/task-g3.json > /dev/null
loom task add --file /work/.smoke/task-g2b.json > /dev/null

# g1: implement (worktree mode so `loom merge` can reach the review gate).
loom dispatch T-20261106-005 --adapter mock > /dev/null
# Between dispatch and verify the task sits in `receipt`: top must show it as
# awaiting verify, with the mock's usage row feeding step/tokens/latency.
loom top --format json > /work/.smoke/top-receipt.json
grep -q '"state": "receipt"' /work/.smoke/top-receipt.json || {
    echo "expected loom top to show the awaiting-verify run" >&2; exit 1;
}
grep -q '"task_id": "T-20261106-005"' /work/.smoke/top-receipt.json || {
    echo "expected loom top to name the dispatched task" >&2; exit 1;
}
grep -q '"input_tokens": 12000' /work/.smoke/top-receipt.json || {
    echo "expected loom top to aggregate mock usage tokens" >&2; exit 1;
}
loom verify T-20261106-005 --format json | grep -q '"passed": true' || {
    echo "expected g1 to verify as passed" >&2; exit 1;
}
# Once passed the task leaves the in-flight view.
if loom top --format json | grep -q '"task_id": "T-20261106-005"'; then
    echo "loom top must not show a settled task" >&2; exit 1;
fi

# g2: review whose fixture report says request_changes → parsed into the receipt.
# verify's stdout is a summary envelope; verdict/next_hint live in the receipt
# file it points at.
loom dispatch T-20261106-006 --adapter mock > /dev/null
loom verify T-20261106-006 --format json > /work/.smoke/verify-g2.json || true
grep -q '"passed": true' /work/.smoke/verify-g2.json || {
    echo "a review with a parseable verdict must still pass its own acceptance" >&2; exit 1;
}
g2_receipt=$(grep -o '"receipt": "[^"]*"' /work/.smoke/verify-g2.json | head -1 | sed 's/.*"receipt": "//; s/"$//')
grep -q '"verdict": "request_changes"' "$g2_receipt" || {
    echo "expected request_changes verdict in receipt" >&2; exit 1;
}

# The gate: g1 cannot merge while its review says request_changes.
if loom merge T-20261106-005 --format json > /work/.smoke/merge-blocked.json; then
    echo "expected the review gate to block merge of g1" >&2; exit 1;
fi
grep -q 'review_changes' /work/.smoke/.loom/events.jsonl || {
    echo "expected task.blocked(code=review_changes)" >&2; exit 1;
}

# The block moved g1 to `blocked`; re-verify restores `passed` (its event
# replays after the blocked one) so the approving review can be dispatched —
# dependencies must be passed.
loom verify T-20261106-005 --format json | grep -q '"passed": true' || {
    echo "expected re-verify of g1 to restore passed" >&2; exit 1;
}

# A second review approves: one pass clears standing objections (a passed
# task cannot be re-dispatched, so a re-review is always a new task).
loom dispatch T-20261106-008 --adapter mock > /dev/null
loom verify T-20261106-008 --format json > /work/.smoke/verify-g2b.json || true
g2b_receipt=$(grep -o '"receipt": "[^"]*"' /work/.smoke/verify-g2b.json | head -1 | sed 's/.*"receipt": "//; s/"$//')
grep -q '"verdict": "pass"' "$g2b_receipt" || {
    echo "expected pass verdict from the approving review" >&2; exit 1;
}

# Retry after review_changes is allowed; the gate now sees pass.
loom merge T-20261106-005 --format json | grep -q '"merged": true' || {
    echo "expected g1 to merge after the review passed" >&2; exit 1;
}

# Fail fast: a review report without a verdict must fail verification.
loom dispatch T-20261106-007 --adapter mock > /dev/null
if loom verify T-20261106-007 --format json > /work/.smoke/verify-g3.json; then
    echo "expected verify to fail when the report has no verdict" >&2; exit 1;
fi
g3_receipt=$(grep -o '"receipt": "[^"]*"' /work/.smoke/verify-g3.json | head -1 | sed 's/.*"receipt": "//; s/"$//')
grep -q 'no parseable verdict' "$g3_receipt" || {
    echo "expected a verdict hint in the failed receipt" >&2; exit 1;
}

echo "SMOKE_OK"
