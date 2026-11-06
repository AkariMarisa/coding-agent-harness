use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{EventType, Tier};

pub const TASK_VERSION: &str = "1.0";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskRole {
    Survey,
    SpecTests,
    #[default]
    Implement,
    Review,
    Judgment,
    Docs,
}

impl TaskRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Survey => "survey",
            Self::SpecTests => "spec-tests",
            Self::Implement => "implement",
            Self::Review => "review",
            Self::Judgment => "judgment",
            Self::Docs => "docs",
        }
    }
}

impl FromStr for TaskRole {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.trim().to_ascii_lowercase().as_str() {
            "survey" => Ok(Self::Survey),
            "spec-tests" | "spec_tests" => Ok(Self::SpecTests),
            "implement" => Ok(Self::Implement),
            "review" => Ok(Self::Review),
            "judgment" | "judge" => Ok(Self::Judgment),
            "docs" => Ok(Self::Docs),
            other => Err(format!("unknown task role: {other}")),
        }
    }
}

/// Status is reconstructed from `events.jsonl`; `tasks.json` is only a view.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    #[default]
    Pending,
    Ready,
    Dispatched,
    Running,
    Receipt,
    Passed,
    Failed,
    Blocked,
    Escalated,
    Merged,
    Cancelled,
}

impl TaskStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Dispatched => "dispatched",
            Self::Running => "running",
            Self::Receipt => "receipt",
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
            Self::Escalated => "escalated",
            Self::Merged => "merged",
            Self::Cancelled => "cancelled",
        }
    }

    /// Dependencies only count as satisfied after independent verification.
    pub const fn satisfies_dependency(self) -> bool {
        matches!(self, Self::Passed | Self::Merged)
    }

    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Passed | Self::Failed | Self::Cancelled | Self::Merged
        )
    }

    pub const fn is_open(self) -> bool {
        matches!(
            self,
            Self::Ready | Self::Dispatched | Self::Running | Self::Receipt | Self::Escalated
        )
    }
}

impl FromStr for TaskStatus {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.trim().to_ascii_lowercase().as_str() {
            "pending" => Ok(Self::Pending),
            "ready" => Ok(Self::Ready),
            "dispatched" => Ok(Self::Dispatched),
            "running" => Ok(Self::Running),
            "receipt" => Ok(Self::Receipt),
            "passed" => Ok(Self::Passed),
            "failed" => Ok(Self::Failed),
            "blocked" => Ok(Self::Blocked),
            "escalated" => Ok(Self::Escalated),
            "merged" => Ok(Self::Merged),
            "cancelled" | "canceled" => Ok(Self::Cancelled),
            other => Err(format!("unknown task status: {other}")),
        }
    }
}

/// Apply one event to a task status. Events are the only state transitions.
pub fn status_after_event(current: TaskStatus, event: EventType) -> TaskStatus {
    match event {
        EventType::TaskCreated => TaskStatus::Pending,
        EventType::TaskReady => TaskStatus::Ready,
        EventType::TaskDispatched => TaskStatus::Dispatched,
        EventType::TaskStarted => TaskStatus::Running,
        EventType::TaskReceipt => TaskStatus::Receipt,
        EventType::TaskPassed => TaskStatus::Passed,
        EventType::TaskFailed => TaskStatus::Failed,
        EventType::TaskBlocked => TaskStatus::Blocked,
        EventType::TaskEscalated => TaskStatus::Escalated,
        EventType::TaskMerged => TaskStatus::Merged,
        EventType::HumanAborted => TaskStatus::Cancelled,
        EventType::RunStarted
        | EventType::GoalLoaded
        | EventType::TaskVerify
        | EventType::TaskFallback
        | EventType::BudgetWarning
        | EventType::BudgetExceeded
        | EventType::HumanApproved
        | EventType::RunFinished => current,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMode {
    Worktree,
    InPlace,
}

impl WorkspaceMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Worktree => "worktree",
            Self::InPlace => "in_place",
        }
    }
}

impl FromStr for WorkspaceMode {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.trim().to_ascii_lowercase().as_str() {
            "worktree" => Ok(Self::Worktree),
            "in_place" | "in-place" => Ok(Self::InPlace),
            other => Err(format!("unknown workspace mode: {other}")),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TaskSpec {
    pub schema_version: String,
    pub task_id: String,
    pub goal_id: String,
    pub title: String,
    pub role: TaskRole,
    pub tier: Tier,
    pub depends_on: Vec<String>,
    pub workspace: TaskWorkspace,
    pub inputs: Vec<TaskInput>,
    pub constraints: TaskConstraints,
    pub acceptance: Vec<Acceptance>,
    pub budget: TaskBudget,
    pub output: TaskOutput,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_policy: Option<ContextPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl TaskSpec {
    pub fn role(&self) -> TaskRole {
        self.role
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != TASK_VERSION {
            return Err(format!(
                "unsupported task schema_version: {} (expected {TASK_VERSION})",
                self.schema_version
            ));
        }
        if !valid_task_id(&self.task_id) {
            return Err(format!(
                "task_id must match T-YYYYMMDD-NNN: {}",
                self.task_id
            ));
        }
        if !valid_goal_id(&self.goal_id) {
            return Err(format!(
                "goal_id must match G-[A-Za-z0-9-]+: {}",
                self.goal_id
            ));
        }
        if self.title.trim().is_empty() {
            return Err("title must not be empty".to_string());
        }
        if self.title.chars().count() > 80 {
            return Err("title must be <= 80 characters".to_string());
        }
        if self.depends_on.iter().any(|id| id == &self.task_id) {
            return Err("task must not depend on itself".to_string());
        }
        ensure_unique("depends_on", &self.depends_on)?;

        self.workspace.validate()?;
        self.constraints.validate()?;
        self.budget.validate()?;
        self.output.validate()?;

        if self.inputs.len() > 64 {
            return Err("inputs must contain at most 64 entries".to_string());
        }
        for input in &self.inputs {
            input.validate()?;
        }

        if self.acceptance.is_empty() {
            return Err("at least one acceptance command is required".to_string());
        }
        let mut ids = HashSet::new();
        for acceptance in &self.acceptance {
            acceptance.validate()?;
            if !ids.insert(acceptance.id.as_str()) {
                return Err(format!("duplicate acceptance id: {}", acceptance.id));
            }
        }

        let policy = self.context_policy.clone().unwrap_or_default();
        policy.validate()?;
        if let Some(notes) = &self.notes {
            if notes.chars().count() > 500 {
                return Err("notes must be <= 500 characters".to_string());
            }
        }
        Ok(())
    }

    pub fn must_read_inputs(&self) -> impl Iterator<Item = &TaskInput> {
        self.inputs.iter().filter(|input| input.must_read)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TaskWorkspace {
    pub mode: WorkspaceMode,
    pub base_ref: String,
    pub branch: String,
    pub allowed_paths: Vec<String>,
    pub forbidden_paths: Vec<String>,
}

impl Default for TaskWorkspace {
    fn default() -> Self {
        Self {
            mode: WorkspaceMode::Worktree,
            base_ref: "main".to_string(),
            branch: String::new(),
            allowed_paths: Vec::new(),
            forbidden_paths: Vec::new(),
        }
    }
}

impl TaskWorkspace {
    fn validate(&self) -> Result<(), String> {
        if self.base_ref.trim().is_empty() {
            return Err("workspace.base_ref must not be empty".to_string());
        }
        if self.branch.trim().is_empty() {
            return Err("workspace.branch must not be empty".to_string());
        }
        if self.allowed_paths.is_empty() {
            return Err("workspace.allowed_paths must contain at least one path".to_string());
        }
        for pattern in self.allowed_paths.iter().chain(self.forbidden_paths.iter()) {
            if pattern.trim().is_empty() {
                return Err("workspace path patterns must not be empty".to_string());
            }
            if pattern.starts_with('/') || pattern.contains("..") {
                return Err(format!("workspace path must be relative: {pattern}"));
            }
        }
        Ok(())
    }

    /// Returns `true` when a relative path is allowed. Forbidden patterns win.
    pub fn allows(&self, path: &str) -> bool {
        if self
            .forbidden_paths
            .iter()
            .any(|pattern| path_matches(pattern, path))
        {
            return false;
        }
        self.allowed_paths
            .iter()
            .any(|pattern| path_matches(pattern, path))
    }
}

/// Minimal glob matching for `*` and `**`. No regex dependency is needed.
fn path_matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim_start_matches("./");
    let path = path.trim_start_matches("./");
    glob_match(pattern.as_bytes(), path.as_bytes())
}

fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    if pattern.is_empty() {
        return text.is_empty();
    }
    match pattern[0] {
        b'*' => {
            if pattern.get(1) == Some(&b'*') {
                let rest = &pattern[2..];
                if rest.first() == Some(&b'/') && glob_match(&rest[1..], text) {
                    // `**/` may match zero path components.
                    return true;
                }
                for index in 0..=text.len() {
                    if glob_match(rest, &text[index..]) {
                        return true;
                    }
                }
                false
            } else {
                let rest = &pattern[1..];
                for index in 0..=text.len() {
                    // A single `*` never crosses a path separator.
                    if index > 0 && text[index - 1] == b'/' {
                        break;
                    }
                    if glob_match(rest, &text[index..]) {
                        return true;
                    }
                }
                false
            }
        }
        b'?' => !text.is_empty() && text[0] != b'/' && glob_match(&pattern[1..], &text[1..]),
        byte => !text.is_empty() && byte == text[0] && glob_match(&pattern[1..], &text[1..]),
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TaskInput {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<[u32; 2]>,
    pub must_read: bool,
}

impl TaskInput {
    fn validate(&self) -> Result<(), String> {
        if self.path.trim().is_empty() {
            return Err("input path must not be empty".to_string());
        }
        if self.path.starts_with('/') || self.path.contains("..") {
            return Err(format!(
                "input path must be project-relative: {}",
                self.path
            ));
        }
        if let Some(hash) = &self.sha256 {
            if hash.len() != 64 || !hash.chars().all(|ch| ch.is_ascii_hexdigit()) {
                return Err(format!(
                    "input sha256 must be 64 hex characters: {}",
                    self.path
                ));
            }
        }
        if let Some([start, end]) = self.range {
            if start == 0 {
                return Err(format!("input range must be 1-based: {}", self.path));
            }
            if end < start {
                return Err(format!(
                    "input range end must be >= start: {} [{start},{end}]",
                    self.path
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TaskConstraints {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_files_touched: Option<u32>,
    pub no_new_dependencies: bool,
    pub style: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl TaskConstraints {
    fn validate(&self) -> Result<(), String> {
        if self.max_files_touched == Some(0) {
            return Err("constraints.max_files_touched must be > 0".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Acceptance {
    pub id: String,
    pub cmd: String,
    pub expect_exit: i32,
    pub timeout_s: u64,
}

impl Default for Acceptance {
    fn default() -> Self {
        Self {
            id: String::new(),
            cmd: String::new(),
            expect_exit: 0,
            timeout_s: 300,
        }
    }
}

impl Acceptance {
    fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("acceptance id must not be empty".to_string());
        }
        if !self
            .id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        {
            return Err(format!("invalid acceptance id: {}", self.id));
        }
        if self.cmd.trim().is_empty() {
            return Err(format!("acceptance {} command must not be empty", self.id));
        }
        if !(0..=255).contains(&self.expect_exit) {
            return Err(format!("acceptance {} expect_exit must be 0..255", self.id));
        }
        if self.timeout_s == 0 {
            return Err(format!("acceptance {} timeout_s must be > 0", self.id));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TaskBudget {
    pub max_input_tokens: u64,
    pub max_output_tokens: u64,
    pub max_steps: u32,
    pub max_wall_s: u64,
    pub max_cost_usd: f64,
}

impl Default for TaskBudget {
    fn default() -> Self {
        Self {
            max_input_tokens: 120_000,
            max_output_tokens: 20_000,
            max_steps: 25,
            max_wall_s: 1_800,
            max_cost_usd: 0.25,
        }
    }
}

impl TaskBudget {
    fn validate(&self) -> Result<(), String> {
        if self.max_input_tokens == 0 {
            return Err("budget.max_input_tokens must be > 0".to_string());
        }
        if self.max_output_tokens == 0 {
            return Err("budget.max_output_tokens must be > 0".to_string());
        }
        if self.max_steps == 0 {
            return Err("budget.max_steps must be > 0".to_string());
        }
        if self.max_wall_s == 0 {
            return Err("budget.max_wall_s must be > 0".to_string());
        }
        if !self.max_cost_usd.is_finite() || self.max_cost_usd <= 0.0 {
            return Err("budget.max_cost_usd must be finite and > 0".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TaskOutput {
    pub receipt: String,
    pub report_path: String,
    pub required_artifacts: Vec<String>,
}

impl Default for TaskOutput {
    fn default() -> Self {
        Self {
            receipt: "4-line".to_string(),
            report_path: String::new(),
            required_artifacts: Vec::new(),
        }
    }
}

impl TaskOutput {
    fn validate(&self) -> Result<(), String> {
        if self.receipt != "4-line" {
            return Err(format!(
                "output.receipt must be \"4-line\": {}",
                self.receipt
            ));
        }
        if self.report_path.trim().is_empty() {
            return Err("output.report_path must not be empty".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ContextPolicy {
    pub max_must_read_files: usize,
    pub max_must_read_tokens: usize,
    pub allow_extra_reads: bool,
    pub forbid_full_repo_scan: bool,
}

impl Default for ContextPolicy {
    fn default() -> Self {
        Self {
            max_must_read_files: 5,
            max_must_read_tokens: 40_000,
            allow_extra_reads: true,
            forbid_full_repo_scan: true,
        }
    }
}

impl ContextPolicy {
    fn validate(&self) -> Result<(), String> {
        if self.max_must_read_files == 0 {
            return Err("context_policy.max_must_read_files must be > 0".to_string());
        }
        if self.max_must_read_tokens == 0 {
            return Err("context_policy.max_must_read_tokens must be > 0".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TaskIndex {
    pub schema_version: String,
    pub goal_id: String,
    pub updated_at: String,
    pub tasks: Vec<TaskIndexEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TaskIndexEntry {
    pub task_id: String,
    pub title: String,
    pub status: TaskStatus,
    pub tier: Tier,
    pub depends_on: Vec<String>,
    pub spec: String,
}

impl TaskIndex {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != TASK_VERSION {
            return Err(format!(
                "unsupported tasks index schema_version: {}",
                self.schema_version
            ));
        }
        if !valid_goal_id(&self.goal_id) {
            return Err(format!("invalid index goal_id: {}", self.goal_id));
        }
        ensure_unique(
            "task ids",
            &self
                .tasks
                .iter()
                .map(|entry| entry.task_id.clone())
                .collect::<Vec<_>>(),
        )?;
        Ok(())
    }
}

/// Validate a whole task graph: unique ids, same goal, existing deps, no cycles.
pub fn validate_graph(tasks: &[TaskSpec]) -> Result<(), String> {
    let mut by_id = HashMap::new();
    for task in tasks {
        task.validate()?;
        if by_id.insert(task.task_id.as_str(), task).is_some() {
            return Err(format!("duplicate task_id: {}", task.task_id));
        }
    }

    let goals: HashSet<&str> = tasks.iter().map(|task| task.goal_id.as_str()).collect();
    if goals.len() > 1 {
        return Err("tasks in one index must share the same goal_id".to_string());
    }

    for task in tasks {
        for dependency in &task.depends_on {
            let Some(dep) = by_id.get(dependency.as_str()) else {
                return Err(format!(
                    "{} depends on missing task {}",
                    task.task_id, dependency
                ));
            };
            if dep.goal_id != task.goal_id {
                return Err(format!(
                    "{} depends on {} from a different goal",
                    task.task_id, dependency
                ));
            }
        }
    }

    let mut visiting = HashSet::new();
    let mut done = HashSet::new();
    for task in tasks {
        visit(task, &by_id, &mut visiting, &mut done)?;
    }
    Ok(())
}

fn visit<'a>(
    task: &'a TaskSpec,
    by_id: &HashMap<&'a str, &'a TaskSpec>,
    visiting: &mut HashSet<&'a str>,
    done: &mut HashSet<&'a str>,
) -> Result<(), String> {
    if done.contains(task.task_id.as_str()) {
        return Ok(());
    }
    if !visiting.insert(task.task_id.as_str()) {
        return Err(format!("dependency cycle involving {}", task.task_id));
    }
    for dependency in &task.depends_on {
        if let Some(dep) = by_id.get(dependency.as_str()) {
            visit(dep, by_id, visiting, done)?;
        }
    }
    visiting.remove(task.task_id.as_str());
    done.insert(task.task_id.as_str());
    Ok(())
}

fn ensure_unique(name: &str, values: &[String]) -> Result<(), String> {
    let mut seen = HashSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(format!("duplicate {name}: {value}"));
        }
    }
    Ok(())
}

fn valid_task_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    bytes.len() == 14
        && bytes.starts_with(b"T-")
        && bytes[2..10].iter().all(u8::is_ascii_digit)
        && bytes[10] == b'-'
        && bytes[11..].iter().all(u8::is_ascii_digit)
}

fn valid_goal_id(id: &str) -> bool {
    id.starts_with("G-")
        && id.len() > 2
        && id[2..]
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EventType;

    fn task(id: &str) -> TaskSpec {
        TaskSpec {
            schema_version: TASK_VERSION.into(),
            task_id: id.into(),
            goal_id: "G-20261104-auth".into(),
            title: "实现登录".into(),
            role: TaskRole::Implement,
            tier: Tier::Worker,
            workspace: TaskWorkspace {
                mode: WorkspaceMode::InPlace,
                base_ref: "main".into(),
                branch: "agent/test".into(),
                allowed_paths: vec!["src/**".into()],
                forbidden_paths: vec!["migrations/**".into()],
            },
            acceptance: vec![Acceptance {
                id: "A1".into(),
                cmd: "true".into(),
                expect_exit: 0,
                timeout_s: 30,
            }],
            output: TaskOutput {
                report_path: ".loom/reports/test.md".into(),
                ..TaskOutput::default()
            },
            ..TaskSpec::default()
        }
    }

    #[test]
    fn validates_task_spec() {
        let spec = task("T-20261104-001");
        assert!(spec.validate().is_ok());

        let mut bad = spec.clone();
        bad.task_id = "T-1".into();
        assert!(bad.validate().is_err());

        let mut bad = spec.clone();
        bad.title = "x".repeat(81);
        assert!(bad.validate().is_err());

        let mut bad = spec.clone();
        bad.acceptance.clear();
        assert!(bad.validate().is_err());

        let mut bad = spec.clone();
        bad.notes = Some("x".repeat(501));
        assert!(bad.validate().is_err());
    }

    #[test]
    fn rejects_bad_inputs_and_paths() {
        let mut spec = task("T-20261104-001");
        spec.inputs.push(TaskInput {
            path: "../secret".into(),
            ..TaskInput::default()
        });
        assert!(spec.validate().is_err());

        let mut spec = task("T-20261104-001");
        spec.inputs.push(TaskInput {
            path: "src/lib.rs".into(),
            sha256: Some("abc".into()),
            ..TaskInput::default()
        });
        assert!(spec.validate().is_err());

        let mut spec = task("T-20261104-001");
        spec.inputs.push(TaskInput {
            path: "src/lib.rs".into(),
            range: Some([10, 3]),
            ..TaskInput::default()
        });
        assert!(spec.validate().is_err());
    }

    #[test]
    fn path_allowlist_and_forbidden_win() {
        let workspace = task("T-20261104-001").workspace;
        assert!(workspace.allows("src/api/auth/login.rs"));
        assert!(!workspace.allows("migrations/001.sql"));
        assert!(!workspace.allows("docs/readme.md"));
    }

    #[test]
    fn double_star_matches_nested_paths() {
        assert!(path_matches("src/**", "src/a/b/c.rs"));
        assert!(path_matches("**/*.rs", "a/b/c.rs"));
        assert!(path_matches("src/*.rs", "src/lib.rs"));
        assert!(!path_matches("src/*.rs", "src/a/lib.rs"));
    }

    #[test]
    fn validates_graph_and_cycles() {
        let one = task("T-20261104-001");
        let mut two = task("T-20261104-002");
        two.depends_on = vec![one.task_id.clone()];
        assert!(validate_graph(&[one.clone(), two.clone()]).is_ok());

        let mut cycle = one.clone();
        cycle.depends_on = vec![two.task_id.clone()];
        assert!(validate_graph(&[cycle, two]).is_err());
    }

    #[test]
    fn missing_dependency_is_rejected() {
        let mut spec = task("T-20261104-001");
        spec.depends_on = vec!["T-20261104-999".into()];
        assert!(validate_graph(&[spec]).is_err());
    }

    #[test]
    fn status_follows_events() {
        let mut status = TaskStatus::Pending;
        for (event, expected) in [
            (EventType::TaskReady, TaskStatus::Ready),
            (EventType::TaskDispatched, TaskStatus::Dispatched),
            (EventType::TaskStarted, TaskStatus::Running),
            (EventType::TaskReceipt, TaskStatus::Receipt),
            (EventType::TaskPassed, TaskStatus::Passed),
        ] {
            status = status_after_event(status, event);
            assert_eq!(status, expected);
        }
        assert!(status.satisfies_dependency());
    }

    #[test]
    fn fallback_does_not_change_status() {
        for current in [
            TaskStatus::Dispatched,
            TaskStatus::Running,
            TaskStatus::Receipt,
        ] {
            assert_eq!(
                status_after_event(current, EventType::TaskFallback),
                current,
                "task.fallback must not move a task"
            );
        }
    }

    #[test]
    fn index_requires_goal_and_unique_ids() {
        let index = TaskIndex {
            schema_version: TASK_VERSION.into(),
            goal_id: "G-x".into(),
            updated_at: "2026-11-04T00:00:00Z".into(),
            tasks: vec![TaskIndexEntry {
                task_id: "T-20261104-001".into(),
                ..TaskIndexEntry::default()
            }],
        };
        assert!(index.validate().is_ok());

        let mut bad = index.clone();
        bad.goal_id = "x".into();
        assert!(bad.validate().is_err());

        let mut bad = index;
        bad.tasks.push(bad.tasks[0].clone());
        assert!(bad.validate().is_err());
    }
}
