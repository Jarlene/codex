use std::collections::BTreeMap;
use std::collections::BTreeSet;

use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TeamId(pub String);

impl TeamId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn slug_from_objective(objective: &str) -> Self {
        Self(safe_slug(objective, /*max_len*/ 30))
    }
}

impl std::fmt::Display for TeamId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AgentId(pub String);

impl AgentId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TaskId(pub String);

impl TaskId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MessageId(pub String);

impl MessageId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl std::fmt::Display for MessageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PlanId(pub String);

impl PlanId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl std::fmt::Display for PlanId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SummaryId(pub String);

impl SummaryId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    Lead,
    Architect,
    Developer,
    Tester,
    Reviewer,
    Security,
    Performance,
    Frontend,
    Backend,
    Database,
    Researcher,
    Writer,
    Custom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Idle,
    Running,
    Sleeping,
    ShuttingDown,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamAgent {
    pub id: AgentId,
    pub display_name: String,
    pub role: AgentRole,
    pub status: AgentStatus,
    pub active_task: Option<TaskId>,
    pub context: AgentContext,
    #[serde(default)]
    pub worker: Option<WorkerRuntimeInfo>,
    pub last_active_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentContext {
    pub project_config_loaded: bool,
    pub tool_capabilities_loaded: bool,
    pub inherited_lead_history: bool,
    pub shared_context_keys: Vec<String>,
}

impl AgentContext {
    pub fn isolated(shared_context_keys: Vec<String>) -> Self {
        Self {
            project_config_loaded: true,
            tool_capabilities_loaded: true,
            inherited_lead_history: false,
            shared_context_keys,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Blocked,
    Ready,
    InProgress,
    Completed,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPriority {
    P0,
    P1,
    P2,
    P3,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub title: String,
    pub description: String,
    pub status: TaskStatus,
    pub assignee: Option<AgentId>,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub claim: Option<TaskClaim>,
    #[serde(default = "default_task_version")]
    pub version: u64,
    pub dependencies: Vec<TaskId>,
    pub priority: TaskPriority,
    pub role_hint: Option<AgentRole>,
    pub estimated_effort: Effort,
    pub requires_plan_approval: bool,
    pub approved_plan: Option<PlanId>,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub completed_at: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Small,
    Medium,
    Large,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    Message,
    Broadcast,
    PlanRequest,
    PlanResponse,
    PermissionRequest,
    PermissionResponse,
    SandboxPermissionRequest,
    SandboxPermissionResponse,
    ModeSetRequest,
    TeamPermissionUpdate,
    TaskAssignment,
    Shutdown,
    ShutdownResponse,
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageState {
    Pending,
    Sent,
    Unread,
    Processing,
    Processed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamMessage {
    pub id: MessageId,
    pub from: AgentId,
    pub to: MessageRecipient,
    pub message_type: MessageType,
    pub timestamp: i64,
    pub payload: MessagePayload,
    pub attempts: u32,
    pub ack_required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRecipient {
    Agent(AgentId),
    Broadcast,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessagePayload {
    pub content: String,
    pub task_id: Option<TaskId>,
    pub plan_id: Option<PlanId>,
    pub metadata: BTreeMap<String, String>,
}

impl MessagePayload {
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            task_id: None,
            plan_id: None,
            metadata: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mailbox {
    pub inboxes: BTreeMap<AgentId, Inbox>,
    pub outboxes: BTreeMap<AgentId, Outbox>,
    pub messages: BTreeMap<MessageId, StoredMessage>,
    pub next_sequence: u64,
}

impl Mailbox {
    pub fn empty() -> Self {
        Self {
            inboxes: BTreeMap::new(),
            outboxes: BTreeMap::new(),
            messages: BTreeMap::new(),
            next_sequence: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredMessage {
    pub message: TeamMessage,
    pub state: MessageState,
    pub sequence: u64,
    pub inbox_owner: Option<AgentId>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inbox {
    pub unread: Vec<MessageId>,
    pub processing: Vec<MessageId>,
    pub processed: Vec<MessageId>,
    pub failed: Vec<MessageId>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outbox {
    pub pending: Vec<MessageId>,
    pub sent: Vec<MessageId>,
    pub failed: Vec<MessageId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanRequest {
    pub id: PlanId,
    pub task_id: TaskId,
    pub requester: AgentId,
    pub plan: String,
    pub tests: Vec<String>,
    pub affects_database: bool,
    pub affects_api_compatibility: bool,
    pub status: PlanStatus,
    pub review: Option<PlanReview>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Pending,
    Approved,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanReview {
    pub reviewer: AgentId,
    pub approved: bool,
    pub comments: String,
    pub requires_tests: bool,
    pub database_impact: bool,
    pub api_compatibility_impact: bool,
    pub reviewed_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Team {
    pub id: TeamId,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub requested_name: Option<String>,
    pub objective: String,
    pub lead_id: AgentId,
    pub agents: BTreeMap<AgentId, TeamAgent>,
    pub tasks: BTreeMap<TaskId, Task>,
    pub mailbox: Mailbox,
    pub plans: BTreeMap<PlanId, PlanRequest>,
    pub summaries: BTreeMap<SummaryId, MessageSummary>,
    pub audit_log: Vec<AuditEvent>,
    pub lifecycle: TeamLifecycle,
    pub config: TeamConfig,
    pub budget: CostBudget,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamLifecycle {
    Active,
    ShuttingDown,
    Stopped,
    CleanedUp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamConfig {
    pub max_agents: usize,
    pub default_idle_timeout_secs: u64,
    pub message_summary_threshold: usize,
    pub long_message_threshold_chars: usize,
    pub shared_context_keys: Vec<String>,
    #[serde(default = "default_worker_launch_mode")]
    pub worker_launch_mode: WorkerLaunchMode,
    #[serde(default = "default_team_display_mode")]
    pub display_mode: TeamDisplayMode,
    #[serde(default = "default_dispatch_ack_timeout_ms")]
    pub dispatch_ack_timeout_ms: u64,
    #[serde(default)]
    pub leader_cwd: Option<String>,
    #[serde(default)]
    pub team_state_root: Option<String>,
    #[serde(default)]
    pub tmux_session: Option<String>,
    #[serde(default)]
    pub leader_pane_id: Option<String>,
    #[serde(default)]
    pub hud_pane_id: Option<String>,
    #[serde(default)]
    pub resize_hook_name: Option<String>,
    #[serde(default)]
    pub resize_hook_target: Option<String>,
    #[serde(default)]
    pub permissions: PermissionsSnapshot,
    #[serde(default)]
    pub governance: TeamGovernance,
}

impl Default for TeamConfig {
    fn default() -> Self {
        Self {
            max_agents: 20,
            default_idle_timeout_secs: 300,
            message_summary_threshold: 20,
            long_message_threshold_chars: 8_000,
            shared_context_keys: Vec::new(),
            worker_launch_mode: WorkerLaunchMode::Prompt,
            display_mode: TeamDisplayMode::Auto,
            dispatch_ack_timeout_ms: 2_000,
            leader_cwd: None,
            team_state_root: None,
            tmux_session: None,
            leader_pane_id: None,
            hud_pane_id: None,
            resize_hook_name: None,
            resize_hook_target: None,
            permissions: PermissionsSnapshot::default(),
            governance: TeamGovernance::default(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostBudget {
    pub token_limit: Option<u64>,
    pub consumed_tokens: u64,
    pub warned_at_80_percent: bool,
    pub paused: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub timestamp: i64,
    pub actor: AgentId,
    pub action: AuditAction,
    pub target: String,
    pub details: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    TeamCreated,
    AgentSpawned,
    TaskCreated,
    TaskClaimed,
    TaskUpdated,
    MessageSent,
    MessageDelivered,
    MessageConsumed,
    PlanRequested,
    PlanReviewed,
    SummaryCreated,
    BudgetUpdated,
    AgentSlept,
    AgentResumed,
    ShutdownRequested,
    CleanupCompleted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateTeamRequest {
    #[serde(default)]
    pub name: Option<TeamId>,
    #[serde(default)]
    pub display_name: Option<String>,
    pub objective: String,
    pub lead: AgentSpec,
    pub teammates: Vec<AgentSpec>,
    pub tasks: Vec<NewTask>,
    pub config: TeamConfig,
    pub budget: CostBudget,
    pub created_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSpec {
    pub id: AgentId,
    pub display_name: String,
    pub role: AgentRole,
    #[serde(default)]
    pub worker: Option<WorkerRuntimeInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewTask {
    pub id: TaskId,
    pub title: String,
    pub description: String,
    pub dependencies: Vec<TaskId>,
    pub priority: TaskPriority,
    pub role_hint: Option<AgentRole>,
    pub estimated_effort: Effort,
    pub requires_plan_approval: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskClaim {
    pub owner: AgentId,
    pub token: String,
    pub leased_until: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerRuntimeInfo {
    pub index: usize,
    #[serde(default)]
    pub worker_cli: Option<String>,
    #[serde(default)]
    pub assigned_tasks: Vec<TaskId>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub pane_id: Option<String>,
    #[serde(default)]
    pub working_dir: Option<String>,
    #[serde(default)]
    pub worktree_repo_root: Option<String>,
    #[serde(default)]
    pub worktree_path: Option<String>,
    #[serde(default)]
    pub worktree_branch: Option<String>,
    #[serde(default)]
    pub worktree_detached: bool,
    #[serde(default)]
    pub worktree_created: bool,
    #[serde(default)]
    pub team_state_root: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerLaunchMode {
    Interactive,
    Prompt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamDisplayMode {
    SplitPane,
    Auto,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionsSnapshot {
    pub approval_mode: String,
    pub sandbox_mode: String,
    pub network_access: bool,
}

impl Default for PermissionsSnapshot {
    fn default() -> Self {
        Self {
            approval_mode: "unknown".to_string(),
            sandbox_mode: "unknown".to_string(),
            network_access: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamGovernance {
    pub delegation_only: bool,
    pub plan_approval_required: bool,
    pub nested_teams_allowed: bool,
    pub one_team_per_leader_session: bool,
    pub cleanup_requires_all_workers_inactive: bool,
}

impl Default for TeamGovernance {
    fn default() -> Self {
        Self {
            delegation_only: false,
            plan_approval_required: false,
            nested_teams_allowed: false,
            one_team_per_leader_session: true,
            cleanup_requires_all_workers_inactive: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDecompositionRequest {
    pub objective: String,
    pub preferred_roles: Vec<AgentRole>,
    pub include_review: bool,
    pub include_security: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDecomposition {
    pub tasks: Vec<NewTask>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamSizeRecommendationInput {
    pub tasks: Vec<NewTask>,
    pub budget_tokens: Option<u64>,
    pub high_risk_roles: Vec<AgentRole>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamSizeRecommendation {
    pub total_agents: usize,
    pub lead_count: usize,
    pub role_counts: BTreeMap<String, usize>,
    pub estimated_parallelism: usize,
    pub estimated_token_range: TokenRange,
    pub downgrade: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenRange {
    pub min: u64,
    pub max: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageSummary {
    pub id: SummaryId,
    pub source_messages: Vec<MessageId>,
    pub compressed_at: i64,
    pub content: SummaryContent,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryContent {
    pub key_decisions: Vec<String>,
    pub risks: Vec<String>,
    pub blockers: Vec<String>,
    pub action_items: Vec<String>,
    pub context_for_lead: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostEstimate {
    pub task_id: TaskId,
    pub estimated_tokens: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerDecision {
    pub runnable_tasks: Vec<TaskId>,
    pub sleeping_agents: Vec<AgentId>,
    pub budget_warning: Option<String>,
    pub paused: bool,
}

pub(crate) fn role_key(role: AgentRole) -> String {
    match role {
        AgentRole::Lead => "lead",
        AgentRole::Architect => "architect",
        AgentRole::Developer => "developer",
        AgentRole::Tester => "tester",
        AgentRole::Reviewer => "reviewer",
        AgentRole::Security => "security",
        AgentRole::Performance => "performance",
        AgentRole::Frontend => "frontend",
        AgentRole::Backend => "backend",
        AgentRole::Database => "database",
        AgentRole::Researcher => "researcher",
        AgentRole::Writer => "writer",
        AgentRole::Custom => "custom",
    }
    .to_string()
}

fn default_task_version() -> u64 {
    1
}

fn default_worker_launch_mode() -> WorkerLaunchMode {
    WorkerLaunchMode::Prompt
}

fn default_team_display_mode() -> TeamDisplayMode {
    TeamDisplayMode::Auto
}

fn default_dispatch_ack_timeout_ms() -> u64 {
    2_000
}

pub(crate) fn task_dependency_set(tasks: &[NewTask]) -> BTreeMap<TaskId, BTreeSet<TaskId>> {
    tasks
        .iter()
        .map(|task| {
            (
                task.id.clone(),
                task.dependencies.iter().cloned().collect::<BTreeSet<_>>(),
            )
        })
        .collect()
}

pub(crate) fn safe_slug(value: &str, max_len: usize) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;
    for ch in value.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_was_dash = false;
        } else if !last_was_dash && !slug.is_empty() {
            slug.push('-');
            last_was_dash = true;
        }
        if slug.len() >= max_len {
            break;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "team".to_string()
    } else {
        slug
    }
}
