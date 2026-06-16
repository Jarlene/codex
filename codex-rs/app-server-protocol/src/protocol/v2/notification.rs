use super::TurnError;
use crate::RequestId;
use codex_protocol::protocol::WorkflowAgentStatus as CoreWorkflowAgentStatus;
use codex_protocol::protocol::WorkflowRunStatus as CoreWorkflowRunStatus;
use codex_protocol::protocol::WorkflowRunUpdatedEvent as CoreWorkflowRunUpdatedEvent;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct DeprecationNoticeNotification {
    /// Concise summary of what is deprecated.
    pub summary: String,
    /// Optional extra guidance, such as migration steps or rationale.
    pub details: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WarningNotification {
    /// Optional thread target when the warning applies to a specific thread.
    pub thread_id: Option<String>,
    /// Concise warning message for the user.
    pub message: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct GuardianWarningNotification {
    /// Thread target for the guardian warning.
    pub thread_id: String,
    /// Concise guardian warning message for the user.
    pub message: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ErrorNotification {
    pub error: TurnError,
    // Set to true if the error is transient and the app-server process will automatically retry.
    // If true, this will not interrupt a turn.
    pub will_retry: bool,
    pub thread_id: String,
    pub turn_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ServerRequestResolvedNotification {
    pub thread_id: String,
    pub request_id: RequestId,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum WorkflowRunStatus {
    Running,
    Completed,
    Failed,
}

impl From<CoreWorkflowRunStatus> for WorkflowRunStatus {
    fn from(value: CoreWorkflowRunStatus) -> Self {
        match value {
            CoreWorkflowRunStatus::Running => Self::Running,
            CoreWorkflowRunStatus::Completed => Self::Completed,
            CoreWorkflowRunStatus::Failed => Self::Failed,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum WorkflowAgentStatus {
    Running,
    Completed,
    Failed,
}

impl From<CoreWorkflowAgentStatus> for WorkflowAgentStatus {
    fn from(value: CoreWorkflowAgentStatus) -> Self {
        match value {
            CoreWorkflowAgentStatus::Running => Self::Running,
            CoreWorkflowAgentStatus::Completed => Self::Completed,
            CoreWorkflowAgentStatus::Failed => Self::Failed,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkflowPhaseProgress {
    pub title: String,
    pub agent_count: u32,
    pub running_agent_count: u32,
    pub completed_agent_count: u32,
    pub failed_agent_count: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkflowAgentProgress {
    pub id: String,
    pub label: String,
    pub prompt: String,
    pub phase: Option<String>,
    pub status: WorkflowAgentStatus,
    pub started_at_ms: i64,
    pub completed_at_ms: Option<i64>,
    pub error: Option<String>,
    pub model: Option<String>,
    pub agent_type: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkflowRunUpdatedNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub call_id: String,
    pub workflow_name: String,
    pub workflow_description: String,
    pub status: WorkflowRunStatus,
    pub phases: Vec<WorkflowPhaseProgress>,
    pub agents: Vec<WorkflowAgentProgress>,
    pub logs: Vec<String>,
    pub agent_count: u32,
    pub running_agent_count: u32,
    pub completed_agent_count: u32,
    pub failed_agent_count: u32,
    pub started_at_ms: i64,
    pub updated_at_ms: i64,
    pub completed_at_ms: Option<i64>,
}

impl From<CoreWorkflowRunUpdatedEvent> for WorkflowRunUpdatedNotification {
    fn from(value: CoreWorkflowRunUpdatedEvent) -> Self {
        Self {
            thread_id: value.thread_id.to_string(),
            turn_id: value.turn_id,
            call_id: value.call_id,
            workflow_name: value.workflow_name,
            workflow_description: value.workflow_description,
            status: value.status.into(),
            phases: value
                .phases
                .into_iter()
                .map(|phase| WorkflowPhaseProgress {
                    title: phase.title,
                    agent_count: phase.agent_count,
                    running_agent_count: phase.running_agent_count,
                    completed_agent_count: phase.completed_agent_count,
                    failed_agent_count: phase.failed_agent_count,
                })
                .collect(),
            agents: value
                .agents
                .into_iter()
                .map(|agent| WorkflowAgentProgress {
                    id: agent.id,
                    label: agent.label,
                    prompt: agent.prompt,
                    phase: agent.phase,
                    status: agent.status.into(),
                    started_at_ms: agent.started_at_ms,
                    completed_at_ms: agent.completed_at_ms,
                    error: agent.error,
                    model: agent.model,
                    agent_type: agent.agent_type,
                })
                .collect(),
            logs: value.logs,
            agent_count: value.agent_count,
            running_agent_count: value.running_agent_count,
            completed_agent_count: value.completed_agent_count,
            failed_agent_count: value.failed_agent_count,
            started_at_ms: value.started_at_ms,
            updated_at_ms: value.updated_at_ms,
            completed_at_ms: value.completed_at_ms,
        }
    }
}
