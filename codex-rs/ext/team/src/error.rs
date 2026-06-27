use std::path::PathBuf;

use crate::TaskStatus;

#[derive(Debug, thiserror::Error)]
pub enum TeamError {
    #[error("team has {requested} agents, but the configured maximum is {max}")]
    TooManyAgents { requested: usize, max: usize },

    #[error("team {team_id} was not found")]
    TeamNotFound { team_id: String },

    #[error("team {team_id} is not active")]
    TeamNotActive { team_id: String },

    #[error("agent {agent_id} was not found")]
    AgentNotFound { agent_id: String },

    #[error("agent {agent_id} is not the team lead")]
    NotTeamLead { agent_id: String },

    #[error("task {task_id} was not found")]
    TaskNotFound { task_id: String },

    #[error("task {task_id} is waiting on incomplete dependencies: {dependencies:?}")]
    DependenciesIncomplete {
        task_id: String,
        dependencies: Vec<String>,
    },

    #[error("task {task_id} is already claimed by {assignee}")]
    TaskAlreadyClaimed { task_id: String, assignee: String },

    #[error("claim conflict for task {task_id}; owner={owner:?}")]
    ClaimConflict {
        task_id: String,
        owner: Option<String>,
    },

    #[error("task {task_id} is already terminal")]
    AlreadyTerminal { task_id: String },

    #[error("task {task_id} claim lease expired")]
    LeaseExpired { task_id: String },

    #[error("task {task_id} claim lease is still active")]
    LeaseActive { task_id: String },

    #[error("task {task_id} cannot transition from {from:?} to {to:?}")]
    InvalidTransition {
        task_id: String,
        from: TaskStatus,
        to: TaskStatus,
    },

    #[error("task {task_id} has no approved plan")]
    PlanApprovalRequired { task_id: String },

    #[error("plan {plan_id} was not found")]
    PlanNotFound { plan_id: String },

    #[error("message {message_id} was not found")]
    MessageNotFound { message_id: String },

    #[error("dependency cycle detected involving {task_id}")]
    DependencyCycle { task_id: String },

    #[error("token budget exceeded: consumed {consumed_tokens}, limit {token_limit}")]
    BudgetExceeded {
        consumed_tokens: u64,
        token_limit: u64,
    },

    #[error("invalid team operation: {0}")]
    InvalidOperation(String),

    #[error("failed to persist path {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to serialize team state: {0}")]
    Json(#[from] serde_json::Error),
}

impl TeamError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
