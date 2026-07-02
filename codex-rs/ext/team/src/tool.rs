use std::sync::Arc;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use codex_extension_api::FunctionCallError;
use codex_extension_api::JsonToolOutput;
use codex_extension_api::ResponsesApiTool;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolExecutorFuture;
use codex_extension_api::ToolName;
use codex_extension_api::ToolOutput;
use codex_extension_api::ToolSpec;
use codex_extension_api::parse_tool_input_schema_without_compaction;
use codex_tools::ToolExposure;
use serde::Deserialize;
use serde_json::Map as JsonMap;
use serde_json::Value as JsonValue;
use serde_json::json;
use tokio::sync::Mutex;

use crate::AgentId;
use crate::AgentRole;
use crate::CostBudget;
use crate::CostEstimate;
use crate::CreateTeamRequest;
use crate::FsTeamStore;
use crate::HeuristicTaskDecomposer;
use crate::MessageId;
use crate::MessagePayload;
use crate::MessageRecipient;
use crate::MessageType;
use crate::NewTask;
use crate::PlanId;
use crate::PlanImpact;
use crate::PlanReviewInput;
use crate::TaskDecompositionRequest;
use crate::TaskId;
use crate::TaskStatus;
use crate::TaskStatusTransition;
use crate::TaskUpdate;
use crate::TeamConfig;
use crate::TeamError;
use crate::TeamId;
use crate::TeamRuntime;
use crate::TeamRuntimeHandle;
use crate::TeamSizeRecommendationInput;
use crate::TeammateRunner;
use crate::TerminalTaskData;
use crate::WorkerRuntimeInfo;
use crate::prompt;

#[derive(Clone)]
pub(crate) struct TeamTool {
    runtime: TeamRuntime,
    active_team: Arc<Mutex<Option<TeamId>>>,
    runner: Arc<dyn TeammateRunner>,
}

impl TeamTool {
    pub(crate) fn new(
        store: FsTeamStore,
        active_team: Arc<Mutex<Option<TeamId>>>,
        runner: Arc<dyn TeammateRunner>,
    ) -> Self {
        Self {
            runtime: TeamRuntime::new(store),
            active_team,
            runner,
        }
    }

    async fn handle_call(&self, call: ToolCall) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let input = parse_args(&call)?;
        let output = match input {
            TeamToolInput::Decompose {
                objective,
                preferred_roles,
                include_review,
                include_security,
            } => {
                let decomposition = HeuristicTaskDecomposer.decompose(TaskDecompositionRequest {
                    objective,
                    preferred_roles: preferred_roles.unwrap_or_default(),
                    include_review: include_review.unwrap_or(true),
                    include_security: include_security.unwrap_or(false),
                });
                json!({ "decomposition": decomposition })
            }
            TeamToolInput::RecommendSize {
                tasks,
                budget_tokens,
                high_risk_roles,
            } => {
                let recommendation = crate::recommend_team_size(TeamSizeRecommendationInput {
                    tasks,
                    budget_tokens,
                    high_risk_roles: high_risk_roles.unwrap_or_default(),
                });
                json!({ "recommendation": recommendation })
            }
            TeamToolInput::Create {
                name,
                display_name,
                objective,
                lead,
                teammates,
                tasks,
                config,
                budget,
                created_at,
            } => {
                let handle = self.runtime.create_team(CreateTeamRequest {
                    name,
                    display_name,
                    objective,
                    lead: *lead,
                    teammates,
                    tasks,
                    config: config.map(|config| *config).unwrap_or_default(),
                    budget: budget.unwrap_or_default(),
                    created_at: created_at.unwrap_or_else(now_unix_timestamp_secs),
                })?;
                let team_id = handle.team().id.clone();
                *self.active_team.lock().await = Some(team_id);
                json!({ "team": handle.team() })
            }
            TeamToolInput::Status { team_id } => {
                let handle = self.load_handle(team_id).await?;
                json!({ "team": handle.team() })
            }
            TeamToolInput::ReadConfig { team_id } => {
                let handle = self.load_handle(team_id).await?;
                json!({
                    "ok": true,
                    "name": handle.team().id.0,
                    "task": handle.team().objective,
                    "config": handle.team().config,
                })
            }
            TeamToolInput::ReadManifest { team_id } => {
                let handle = self.load_handle(team_id).await?;
                json!({
                    "ok": true,
                    "schema_version": 2,
                    "name": handle.team().id.0,
                    "team": handle.team(),
                })
            }
            TeamToolInput::AddTask {
                team_id,
                actor,
                task,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                handle.add_task(&actor, task, now.unwrap_or_else(now_unix_timestamp_secs))?;
                json!({ "team": handle.team() })
            }
            TeamToolInput::ReadTask { team_id, task_id } => {
                let handle = self.load_handle(team_id).await?;
                json!({ "ok": true, "task": handle.read_task(&task_id)? })
            }
            TeamToolInput::ListTasks { team_id } => {
                let handle = self.load_handle(team_id).await?;
                json!({ "ok": true, "tasks": handle.list_tasks() })
            }
            TeamToolInput::UpdateTask {
                team_id,
                actor,
                task_id,
                update,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                let task = handle.update_task(
                    &actor,
                    &task_id,
                    update,
                    now.unwrap_or_else(now_unix_timestamp_secs),
                )?;
                json!({ "ok": true, "task": task, "team": handle.team() })
            }
            TeamToolInput::ClaimTask {
                team_id,
                agent_id,
                task_id,
                expected_version,
                lease_secs,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                let claimed = handle.claim_task_with_lease(
                    &agent_id,
                    &task_id,
                    expected_version,
                    lease_secs.unwrap_or(15 * 60),
                    now.unwrap_or_else(now_unix_timestamp_secs),
                )?;
                json!({
                    "ok": true,
                    "task": claimed.task,
                    "claimToken": claimed.claim_token,
                    "claim_token": claimed.claim_token,
                    "team": handle.team(),
                })
            }
            TeamToolInput::TransitionTaskStatus {
                team_id,
                agent_id,
                task_id,
                from,
                to,
                claim_token,
                result,
                error,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                let transition = TaskStatusTransition {
                    agent_id: &agent_id,
                    task_id: &task_id,
                    from,
                    to,
                    claim_token: &claim_token,
                    terminal: TerminalTaskData { result, error },
                };
                let task = handle.transition_task_status(
                    transition,
                    now.unwrap_or_else(now_unix_timestamp_secs),
                )?;
                json!({ "ok": true, "task": task, "team": handle.team() })
            }
            TeamToolInput::ReleaseTaskClaim {
                team_id,
                agent_id,
                task_id,
                claim_token,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                let task = handle.release_task_claim(
                    &agent_id,
                    &task_id,
                    &claim_token,
                    now.unwrap_or_else(now_unix_timestamp_secs),
                )?;
                json!({ "ok": true, "task": task, "team": handle.team() })
            }
            TeamToolInput::ReclaimExpiredTaskClaim {
                team_id,
                task_id,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                let outcome = handle.reclaim_expired_task_claim(
                    &task_id,
                    now.unwrap_or_else(now_unix_timestamp_secs),
                )?;
                json!({
                    "ok": true,
                    "task": outcome.task,
                    "reclaimed": outcome.reclaimed,
                    "team": handle.team(),
                })
            }
            TeamToolInput::CompleteTask {
                team_id,
                agent_id,
                task_id,
                tokens_used,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                handle.complete_task(
                    &agent_id,
                    &task_id,
                    tokens_used.unwrap_or_default(),
                    now.unwrap_or_else(now_unix_timestamp_secs),
                )?;
                json!({ "team": handle.team() })
            }
            TeamToolInput::RequestPlan {
                team_id,
                requester,
                task_id,
                plan,
                tests,
                impact,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                let plan_id = handle.request_plan(
                    &requester,
                    &task_id,
                    plan,
                    tests.unwrap_or_default(),
                    impact.unwrap_or(PlanImpact {
                        affects_database: false,
                        affects_api_compatibility: false,
                    }),
                    now.unwrap_or_else(now_unix_timestamp_secs),
                )?;
                json!({ "plan_id": plan_id, "team": handle.team() })
            }
            TeamToolInput::ReviewPlan {
                team_id,
                reviewer,
                plan_id,
                review,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                handle.review_plan(
                    &reviewer,
                    &plan_id,
                    review,
                    now.unwrap_or_else(now_unix_timestamp_secs),
                )?;
                json!({ "team": handle.team() })
            }
            TeamToolInput::SendMessage {
                team_id,
                from,
                to,
                message_type,
                payload,
                ack_required,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                let now = now.unwrap_or_else(now_unix_timestamp_secs);
                let message_id = handle.send_message(
                    &from,
                    to,
                    message_type.unwrap_or(MessageType::Message),
                    payload,
                    ack_required.unwrap_or(false),
                    now,
                )?;
                handle.route_pending_messages(/*max_attempts*/ 3, now)?;
                json!({ "message_id": message_id, "team": handle.team() })
            }
            TeamToolInput::SendWorkerMessage {
                team_id,
                from_worker,
                to_worker,
                body,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                let now = now.unwrap_or_else(now_unix_timestamp_secs);
                let message_id = handle.send_message(
                    &AgentId::new(from_worker),
                    MessageRecipient::Agent(AgentId::new(to_worker)),
                    MessageType::Message,
                    MessagePayload::text(body),
                    false,
                    now,
                )?;
                handle.route_pending_messages(/*max_attempts*/ 3, now)?;
                json!({ "ok": true, "message_id": message_id, "team": handle.team() })
            }
            TeamToolInput::Broadcast {
                team_id,
                from_worker,
                body,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                let now = now.unwrap_or_else(now_unix_timestamp_secs);
                let message_id = handle.send_message(
                    &AgentId::new(from_worker),
                    MessageRecipient::Broadcast,
                    MessageType::Broadcast,
                    MessagePayload::text(body),
                    false,
                    now,
                )?;
                let delivered = handle.route_pending_messages(/*max_attempts*/ 3, now)?;
                json!({
                    "ok": true,
                    "message_id": message_id,
                    "delivered": delivered,
                    "team": handle.team(),
                })
            }
            TeamToolInput::MailboxList { team_id, worker } => {
                let handle = self.load_handle(team_id).await?;
                let messages = handle
                    .team()
                    .mailbox
                    .inboxes
                    .get(&worker)
                    .map(|inbox| {
                        inbox
                            .unread
                            .iter()
                            .chain(inbox.processing.iter())
                            .chain(inbox.processed.iter())
                            .chain(inbox.failed.iter())
                            .filter_map(|message_id| handle.team().mailbox.messages.get(message_id))
                            .map(|stored| stored.message.clone())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                json!({ "ok": true, "worker": worker, "messages": messages })
            }
            TeamToolInput::MailboxMarkDelivered {
                team_id,
                worker,
                message_id,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                handle.finish_message(
                    &worker,
                    &message_id,
                    true,
                    now.unwrap_or_else(now_unix_timestamp_secs),
                )?;
                json!({ "ok": true, "team": handle.team() })
            }
            TeamToolInput::MailboxMarkNotified {
                team_id,
                worker,
                message_id,
            } => {
                let handle = self.load_handle(team_id).await?;
                json!({
                    "ok": handle.team().mailbox.messages.contains_key(&message_id),
                    "worker": worker,
                    "message_id": message_id,
                })
            }
            TeamToolInput::ReadWorkerStatus { team_id, worker } => {
                let handle = self.load_handle(team_id).await?;
                let agent = handle.team().agents.get(&worker);
                json!({
                    "ok": agent.is_some(),
                    "state": agent.map(|agent| agent.status),
                    "current_task_id": agent.and_then(|agent| agent.active_task.clone()),
                    "updated_at": agent.map(|agent| agent.last_active_at),
                })
            }
            TeamToolInput::UpdateWorkerStatus {
                team_id,
                worker,
                state,
                current_task_id,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                handle.set_agent_status(
                    &worker,
                    state,
                    current_task_id,
                    now.unwrap_or_else(now_unix_timestamp_secs),
                )?;
                json!({ "ok": true, "team": handle.team() })
            }
            TeamToolInput::ReadWorkerHeartbeat { team_id, worker } => {
                let handle = self.load_handle(team_id).await?;
                let agent = handle.team().agents.get(&worker);
                json!({
                    "ok": agent.is_some(),
                    "pid": agent.and_then(|agent| agent.worker.as_ref()).and_then(|worker| worker.pid).unwrap_or_default(),
                    "last_turn_at": agent.map(|agent| agent.last_active_at),
                    "turn_count": 0,
                    "alive": agent.map(|agent| agent.status != crate::AgentStatus::Stopped).unwrap_or(false),
                })
            }
            TeamToolInput::UpdateWorkerHeartbeat {
                team_id,
                worker,
                pid,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                let mut runtime = handle
                    .team()
                    .agents
                    .get(&worker)
                    .and_then(|agent| agent.worker.clone())
                    .unwrap_or_else(|| WorkerRuntimeInfo {
                        index: 0,
                        worker_cli: None,
                        assigned_tasks: Vec::new(),
                        pid: None,
                        pane_id: None,
                        working_dir: None,
                        worktree_repo_root: None,
                        worktree_path: None,
                        worktree_branch: None,
                        worktree_detached: false,
                        worktree_created: false,
                        team_state_root: None,
                    });
                runtime.pid = Some(pid);
                handle.update_worker_runtime(
                    &worker,
                    &worker,
                    runtime,
                    now.unwrap_or_else(now_unix_timestamp_secs),
                )?;
                json!({ "ok": true, "team": handle.team() })
            }
            TeamToolInput::WriteWorkerIdentity {
                team_id,
                worker,
                identity,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                handle.update_worker_runtime(
                    &worker,
                    &worker,
                    identity,
                    now.unwrap_or_else(now_unix_timestamp_secs),
                )?;
                json!({ "ok": true, "team": handle.team() })
            }
            TeamToolInput::WriteWorkerInbox {
                team_id,
                worker,
                prompt,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                let mut payload = MessagePayload::text(prompt);
                payload
                    .metadata
                    .insert("inbox".to_string(), "true".to_string());
                let message_id = handle.send_message(
                    &handle.team().lead_id.clone(),
                    MessageRecipient::Agent(worker),
                    MessageType::TaskAssignment,
                    payload,
                    false,
                    now.unwrap_or_else(now_unix_timestamp_secs),
                )?;
                json!({ "ok": true, "message_id": message_id, "team": handle.team() })
            }
            TeamToolInput::ReadDispatchRequests { team_id } => {
                let handle = self.load_handle(team_id).await?;
                let requests = handle
                    .team()
                    .mailbox
                    .messages
                    .values()
                    .filter(|stored| {
                        matches!(
                            stored.state,
                            crate::MessageState::Pending | crate::MessageState::Unread
                        )
                    })
                    .map(|stored| stored.message.clone())
                    .collect::<Vec<_>>();
                json!({ "ok": true, "requests": requests })
            }
            TeamToolInput::AppendEvent {
                team_id,
                actor,
                details,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                handle.append_event(
                    &actor,
                    details,
                    now.unwrap_or_else(now_unix_timestamp_secs),
                )?;
                json!({ "ok": true, "team": handle.team() })
            }
            TeamToolInput::ReadTaskApproval { team_id, task_id } => {
                let handle = self.load_handle(team_id).await?;
                let task = handle.read_task(&task_id)?;
                json!({
                    "ok": true,
                    "task_id": task_id,
                    "required": task.requires_plan_approval,
                    "status": if task.approved_plan.is_some() || !task.requires_plan_approval { "approved" } else { "pending" },
                })
            }
            TeamToolInput::WriteTaskApproval {
                team_id,
                reviewer,
                task_id,
                approved,
                comments,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                let now = now.unwrap_or_else(now_unix_timestamp_secs);
                let plan_id = handle.request_plan(
                    &reviewer,
                    &task_id,
                    "task approval record".to_string(),
                    Vec::new(),
                    PlanImpact {
                        affects_database: false,
                        affects_api_compatibility: false,
                    },
                    now,
                )?;
                handle.review_plan(
                    &handle.team().lead_id.clone(),
                    &plan_id,
                    PlanReviewInput {
                        approved,
                        comments,
                        requires_tests: false,
                        database_impact: false,
                        api_compatibility_impact: false,
                    },
                    now,
                )?;
                json!({ "ok": true, "team": handle.team() })
            }
            TeamToolInput::RouteMessages {
                team_id,
                max_attempts,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                let delivered = handle.route_pending_messages(
                    max_attempts.unwrap_or(3),
                    now.unwrap_or_else(now_unix_timestamp_secs),
                )?;
                json!({ "delivered": delivered, "team": handle.team() })
            }
            TeamToolInput::ConsumeMessage {
                team_id,
                agent_id,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                let message = handle
                    .consume_next_message(&agent_id, now.unwrap_or_else(now_unix_timestamp_secs))?;
                json!({ "message": message, "team": handle.team() })
            }
            TeamToolInput::FinishMessage {
                team_id,
                agent_id,
                message_id,
                success,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                handle.finish_message(
                    &agent_id,
                    &message_id,
                    success,
                    now.unwrap_or_else(now_unix_timestamp_secs),
                )?;
                json!({ "team": handle.team() })
            }
            TeamToolInput::RecoverMessages {
                team_id,
                agent_id,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                let recovered = handle.recover_processing_messages(
                    &agent_id,
                    now.unwrap_or_else(now_unix_timestamp_secs),
                )?;
                json!({ "recovered": recovered, "team": handle.team() })
            }
            TeamToolInput::SummarizeInbox {
                team_id,
                agent_id,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                let summary = handle.summarize_inbox_if_needed(
                    &agent_id,
                    now.unwrap_or_else(now_unix_timestamp_secs),
                )?;
                json!({ "summary": summary, "team": handle.team() })
            }
            TeamToolInput::SchedulerDecision {
                team_id,
                estimates,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                let decision = handle.scheduler_decision(
                    &estimates.unwrap_or_default(),
                    now.unwrap_or_else(now_unix_timestamp_secs),
                )?;
                json!({ "decision": decision, "team": handle.team() })
            }
            TeamToolInput::GetSummary { team_id } => {
                let handle = self.load_handle(team_id).await?;
                team_summary_json(handle.team())
            }
            TeamToolInput::ReadMonitorSnapshot { team_id } => {
                let handle = self.load_handle(team_id).await?;
                json!({ "ok": true, "summary": team_summary_json(handle.team()) })
            }
            TeamToolInput::WriteMonitorSnapshot { team_id, snapshot } => {
                let handle = self.load_handle(team_id).await?;
                json!({ "ok": true, "snapshot": snapshot, "team": handle.team() })
            }
            TeamToolInput::SleepIdleAgents { team_id, now } => {
                let mut handle = self.load_handle(team_id).await?;
                let slept =
                    handle.sleep_idle_agents(now.unwrap_or_else(now_unix_timestamp_secs))?;
                json!({ "slept": slept, "team": handle.team() })
            }
            TeamToolInput::ResumeAgent {
                team_id,
                agent_id,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                handle.resume_agent(&agent_id, now.unwrap_or_else(now_unix_timestamp_secs))?;
                json!({ "team": handle.team() })
            }
            TeamToolInput::RunReadyTasks { team_id, now } => {
                let mut handle = self.load_handle(team_id).await?;
                let results = handle
                    .run_ready_tasks(
                        Arc::clone(&self.runner),
                        now.unwrap_or_else(now_unix_timestamp_secs),
                    )
                    .await?;
                json!({ "results": results, "team": handle.team() })
            }
            TeamToolInput::RequestShutdown {
                team_id,
                lead_id,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                handle.request_shutdown(&lead_id, now.unwrap_or_else(now_unix_timestamp_secs))?;
                json!({ "team": handle.team() })
            }
            TeamToolInput::ReadShutdownAck { team_id, worker } => {
                let handle = self.load_handle(team_id).await?;
                json!({
                    "ok": handle.team().agents.contains_key(&worker),
                    "worker": worker,
                    "status": if handle.team().lifecycle == crate::TeamLifecycle::Stopped { "accept" } else { "pending" },
                })
            }
            TeamToolInput::WriteShutdownRequest {
                team_id,
                lead_id,
                worker,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                let now = now.unwrap_or_else(now_unix_timestamp_secs);
                let mut payload = MessagePayload::text("team shutdown requested");
                payload
                    .metadata
                    .insert("shutdown_worker".to_string(), worker.0);
                handle.send_message(
                    &lead_id,
                    MessageRecipient::Broadcast,
                    MessageType::Shutdown,
                    payload,
                    true,
                    now,
                )?;
                handle.request_shutdown(&lead_id, now)?;
                json!({ "ok": true, "team": handle.team() })
            }
            TeamToolInput::MarkStopped {
                team_id,
                lead_id,
                now,
            } => {
                let mut handle = self.load_handle(team_id).await?;
                handle.mark_stopped(&lead_id, now.unwrap_or_else(now_unix_timestamp_secs))?;
                json!({ "team": handle.team() })
            }
            TeamToolInput::Cleanup {
                team_id,
                lead_id,
                now,
            } => {
                let handle = self.load_handle(Some(team_id.clone())).await?;
                handle.cleanup(&lead_id, now.unwrap_or_else(now_unix_timestamp_secs))?;
                let mut active_team = self.active_team.lock().await;
                if active_team.as_ref() == Some(&team_id) {
                    *active_team = None;
                }
                json!({ "cleaned_up": team_id })
            }
        };
        Ok(Box::new(JsonToolOutput::new(output)))
    }

    async fn load_handle(
        &self,
        team_id: Option<TeamId>,
    ) -> Result<TeamRuntimeHandle, FunctionCallError> {
        let team_id = match team_id {
            Some(team_id) => team_id,
            None => self.active_team.lock().await.clone().ok_or_else(|| {
                FunctionCallError::RespondToModel(
                    "no active team; pass team_id or create a team first".to_string(),
                )
            })?,
        };
        self.runtime.load_team(&team_id).map_err(tool_error)
    }
}

impl ToolExecutor<ToolCall> for TeamTool {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(prompt::TEAM_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ResponsesApiTool {
            name: prompt::TEAM_TOOL_NAME.to_string(),
            description: prompt::TOOL_DESCRIPTION.to_string(),
            strict: false,
            defer_loading: None,
            parameters: parse_tool_input_schema_without_compaction(&team_tool_schema())
                .unwrap_or_else(|err| panic!("team schema should parse: {err}")),
            output_schema: None,
        })
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        false
    }

    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(call))
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum TeamToolInput {
    Decompose {
        objective: String,
        preferred_roles: Option<Vec<AgentRole>>,
        include_review: Option<bool>,
        include_security: Option<bool>,
    },
    RecommendSize {
        tasks: Vec<NewTask>,
        budget_tokens: Option<u64>,
        high_risk_roles: Option<Vec<AgentRole>>,
    },
    Create {
        name: Option<TeamId>,
        display_name: Option<String>,
        objective: String,
        lead: Box<crate::AgentSpec>,
        teammates: Vec<crate::AgentSpec>,
        tasks: Vec<NewTask>,
        config: Option<Box<TeamConfig>>,
        budget: Option<CostBudget>,
        created_at: Option<i64>,
    },
    Status {
        team_id: Option<TeamId>,
    },
    ReadConfig {
        team_id: Option<TeamId>,
    },
    ReadManifest {
        team_id: Option<TeamId>,
    },
    AddTask {
        team_id: Option<TeamId>,
        actor: AgentId,
        task: NewTask,
        now: Option<i64>,
    },
    ReadTask {
        team_id: Option<TeamId>,
        task_id: TaskId,
    },
    ListTasks {
        team_id: Option<TeamId>,
    },
    UpdateTask {
        team_id: Option<TeamId>,
        actor: AgentId,
        task_id: TaskId,
        update: TaskUpdate,
        now: Option<i64>,
    },
    ClaimTask {
        team_id: Option<TeamId>,
        agent_id: AgentId,
        task_id: TaskId,
        expected_version: Option<u64>,
        lease_secs: Option<i64>,
        now: Option<i64>,
    },
    TransitionTaskStatus {
        team_id: Option<TeamId>,
        agent_id: AgentId,
        task_id: TaskId,
        from: TaskStatus,
        to: TaskStatus,
        claim_token: String,
        result: Option<String>,
        error: Option<String>,
        now: Option<i64>,
    },
    ReleaseTaskClaim {
        team_id: Option<TeamId>,
        agent_id: AgentId,
        task_id: TaskId,
        claim_token: String,
        now: Option<i64>,
    },
    ReclaimExpiredTaskClaim {
        team_id: Option<TeamId>,
        task_id: TaskId,
        now: Option<i64>,
    },
    CompleteTask {
        team_id: Option<TeamId>,
        agent_id: AgentId,
        task_id: TaskId,
        tokens_used: Option<u64>,
        now: Option<i64>,
    },
    RequestPlan {
        team_id: Option<TeamId>,
        requester: AgentId,
        task_id: TaskId,
        plan: String,
        tests: Option<Vec<String>>,
        impact: Option<PlanImpact>,
        now: Option<i64>,
    },
    ReviewPlan {
        team_id: Option<TeamId>,
        reviewer: AgentId,
        plan_id: PlanId,
        review: PlanReviewInput,
        now: Option<i64>,
    },
    SendMessage {
        team_id: Option<TeamId>,
        from: AgentId,
        to: MessageRecipient,
        message_type: Option<MessageType>,
        payload: MessagePayload,
        ack_required: Option<bool>,
        now: Option<i64>,
    },
    SendWorkerMessage {
        team_id: Option<TeamId>,
        from_worker: String,
        to_worker: String,
        body: String,
        now: Option<i64>,
    },
    Broadcast {
        team_id: Option<TeamId>,
        from_worker: String,
        body: String,
        now: Option<i64>,
    },
    MailboxList {
        team_id: Option<TeamId>,
        worker: AgentId,
    },
    MailboxMarkDelivered {
        team_id: Option<TeamId>,
        worker: AgentId,
        message_id: crate::MessageId,
        now: Option<i64>,
    },
    MailboxMarkNotified {
        team_id: Option<TeamId>,
        worker: AgentId,
        message_id: crate::MessageId,
    },
    ReadWorkerStatus {
        team_id: Option<TeamId>,
        worker: AgentId,
    },
    UpdateWorkerStatus {
        team_id: Option<TeamId>,
        worker: AgentId,
        state: crate::AgentStatus,
        current_task_id: Option<TaskId>,
        now: Option<i64>,
    },
    ReadWorkerHeartbeat {
        team_id: Option<TeamId>,
        worker: AgentId,
    },
    UpdateWorkerHeartbeat {
        team_id: Option<TeamId>,
        worker: AgentId,
        pid: u32,
        now: Option<i64>,
    },
    WriteWorkerInbox {
        team_id: Option<TeamId>,
        worker: AgentId,
        prompt: String,
        now: Option<i64>,
    },
    WriteWorkerIdentity {
        team_id: Option<TeamId>,
        worker: AgentId,
        identity: WorkerRuntimeInfo,
        now: Option<i64>,
    },
    ReadDispatchRequests {
        team_id: Option<TeamId>,
    },
    AppendEvent {
        team_id: Option<TeamId>,
        actor: AgentId,
        details: String,
        now: Option<i64>,
    },
    ReadTaskApproval {
        team_id: Option<TeamId>,
        task_id: TaskId,
    },
    WriteTaskApproval {
        team_id: Option<TeamId>,
        reviewer: AgentId,
        task_id: TaskId,
        approved: bool,
        comments: String,
        now: Option<i64>,
    },
    RouteMessages {
        team_id: Option<TeamId>,
        max_attempts: Option<u32>,
        now: Option<i64>,
    },
    ConsumeMessage {
        team_id: Option<TeamId>,
        agent_id: AgentId,
        now: Option<i64>,
    },
    FinishMessage {
        team_id: Option<TeamId>,
        agent_id: AgentId,
        message_id: crate::MessageId,
        success: bool,
        now: Option<i64>,
    },
    RecoverMessages {
        team_id: Option<TeamId>,
        agent_id: AgentId,
        now: Option<i64>,
    },
    SummarizeInbox {
        team_id: Option<TeamId>,
        agent_id: AgentId,
        now: Option<i64>,
    },
    SchedulerDecision {
        team_id: Option<TeamId>,
        estimates: Option<Vec<CostEstimate>>,
        now: Option<i64>,
    },
    GetSummary {
        team_id: Option<TeamId>,
    },
    ReadMonitorSnapshot {
        team_id: Option<TeamId>,
    },
    WriteMonitorSnapshot {
        team_id: Option<TeamId>,
        snapshot: JsonValue,
    },
    SleepIdleAgents {
        team_id: Option<TeamId>,
        now: Option<i64>,
    },
    ResumeAgent {
        team_id: Option<TeamId>,
        agent_id: AgentId,
        now: Option<i64>,
    },
    RunReadyTasks {
        team_id: Option<TeamId>,
        now: Option<i64>,
    },
    RequestShutdown {
        team_id: Option<TeamId>,
        lead_id: AgentId,
        now: Option<i64>,
    },
    ReadShutdownAck {
        team_id: Option<TeamId>,
        worker: AgentId,
    },
    WriteShutdownRequest {
        team_id: Option<TeamId>,
        lead_id: AgentId,
        worker: AgentId,
        now: Option<i64>,
    },
    MarkStopped {
        team_id: Option<TeamId>,
        lead_id: AgentId,
        now: Option<i64>,
    },
    Cleanup {
        team_id: TeamId,
        lead_id: AgentId,
        now: Option<i64>,
    },
}

fn parse_args(call: &ToolCall) -> Result<TeamToolInput, FunctionCallError> {
    serde_json::from_str(call.function_arguments()?)
        .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))
}

fn tool_error(err: TeamError) -> FunctionCallError {
    match err {
        TeamError::Io { .. } | TeamError::Json(_) => FunctionCallError::Fatal(err.to_string()),
        _ => FunctionCallError::RespondToModel(err.to_string()),
    }
}

impl From<TeamError> for FunctionCallError {
    fn from(err: TeamError) -> Self {
        tool_error(err)
    }
}

fn now_unix_timestamp_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn team_summary_json(team: &crate::Team) -> JsonValue {
    let mut counts = std::collections::BTreeMap::new();
    for status in [
        TaskStatus::Pending,
        TaskStatus::Blocked,
        TaskStatus::Ready,
        TaskStatus::InProgress,
        TaskStatus::Completed,
        TaskStatus::Failed,
    ] {
        counts.insert(format!("{status:?}"), 0usize);
    }
    for task in team.tasks.values() {
        *counts.entry(format!("{:?}", task.status)).or_insert(0) += 1;
    }
    json!({
        "teamName": team.id.0,
        "workerCount": team.agents.values().filter(|agent| agent.role != AgentRole::Lead).count(),
        "tasks": {
            "total": team.tasks.len(),
            "pending": counts.get("Pending").copied().unwrap_or_default() + counts.get("Ready").copied().unwrap_or_default(),
            "blocked": counts.get("Blocked").copied().unwrap_or_default(),
            "in_progress": counts.get("InProgress").copied().unwrap_or_default(),
            "completed": counts.get("Completed").copied().unwrap_or_default(),
            "failed": counts.get("Failed").copied().unwrap_or_default(),
        },
        "workers": team.agents.values()
            .filter(|agent| agent.role != AgentRole::Lead)
            .map(|agent| json!({
                "name": agent.id.0,
                "alive": agent.status != crate::AgentStatus::Stopped,
                "lastTurnAt": agent.last_active_at,
                "turnsWithoutProgress": 0,
            }))
            .collect::<Vec<_>>(),
        "nonReportingWorkers": [],
    })
}

fn team_tool_schema() -> JsonValue {
    let actions = vec![
        "decompose",
        "recommend_size",
        "create",
        "status",
        "read_config",
        "read_manifest",
        "add_task",
        "read_task",
        "list_tasks",
        "update_task",
        "claim_task",
        "transition_task_status",
        "release_task_claim",
        "reclaim_expired_task_claim",
        "complete_task",
        "request_plan",
        "review_plan",
        "send_message",
        "send_worker_message",
        "broadcast",
        "mailbox_list",
        "mailbox_mark_delivered",
        "mailbox_mark_notified",
        "read_worker_status",
        "update_worker_status",
        "read_worker_heartbeat",
        "update_worker_heartbeat",
        "write_worker_inbox",
        "write_worker_identity",
        "read_dispatch_requests",
        "append_event",
        "read_task_approval",
        "write_task_approval",
        "route_messages",
        "consume_message",
        "finish_message",
        "recover_messages",
        "summarize_inbox",
        "scheduler_decision",
        "get_summary",
        "read_monitor_snapshot",
        "write_monitor_snapshot",
        "sleep_idle_agents",
        "resume_agent",
        "run_ready_tasks",
        "request_shutdown",
        "read_shutdown_ack",
        "write_shutdown_request",
        "mark_stopped",
        "cleanup",
    ];
    let mut properties = JsonMap::new();
    properties.insert(
        "action".to_string(),
        json!({
            "type": "string",
            "enum": actions,
            "description": "Team operation to execute."
        }),
    );
    for (name, value) in [
        (
            "team_id",
            json!({ "type": "string", "description": "Team id. Optional for most actions after create because the newest created team becomes active." }),
        ),
        (
            "name",
            json!({ "type": "string", "description": "OMX-compatible safe team name ([a-z0-9][a-z0-9-]{0,29}). If omitted, one is derived from the objective." }),
        ),
        ("display_name", json!({ "type": "string" })),
        ("objective", json!({ "type": "string" })),
        ("preferred_roles", string_array_schema()),
        ("include_review", json!({ "type": "boolean" })),
        ("include_security", json!({ "type": "boolean" })),
        ("lead", json!({ "type": "object" })),
        ("teammates", object_array_schema()),
        ("tasks", object_array_schema()),
        ("task", json!({ "type": "object" })),
        ("update", json!({ "type": "object" })),
        ("config", json!({ "type": "object" })),
        ("budget", json!({ "type": "object" })),
        ("budget_tokens", json!({ "type": "integer", "minimum": 0 })),
        ("high_risk_roles", string_array_schema()),
        ("actor", json!({ "type": "string" })),
        ("agent_id", json!({ "type": "string" })),
        ("lead_id", json!({ "type": "string" })),
        ("requester", json!({ "type": "string" })),
        ("reviewer", json!({ "type": "string" })),
        ("task_id", json!({ "type": "string" })),
        (
            "expected_version",
            json!({ "type": "integer", "minimum": 1 }),
        ),
        ("lease_secs", json!({ "type": "integer", "minimum": 1 })),
        ("claim_token", json!({ "type": "string" })),
        ("result", json!({ "type": "string" })),
        ("error", json!({ "type": "string" })),
        ("plan_id", json!({ "type": "string" })),
        ("plan", json!({ "type": "string" })),
        ("tests", string_array_schema()),
        ("impact", json!({ "type": "object" })),
        ("review", json!({ "type": "object" })),
        ("from", json!({ "type": "string" })),
        ("from_worker", json!({ "type": "string" })),
        ("to_worker", json!({ "type": "string" })),
        ("worker", json!({ "type": "string" })),
        ("state", json!({ "type": "string" })),
        ("current_task_id", json!({ "type": "string" })),
        ("pid", json!({ "type": "integer", "minimum": 0 })),
        ("prompt", json!({ "type": "string" })),
        ("identity", json!({ "type": "object" })),
        ("details", json!({ "type": "string" })),
        ("approved", json!({ "type": "boolean" })),
        ("comments", json!({ "type": "string" })),
        ("snapshot", json!({ "type": "object" })),
        ("body", json!({ "type": "string" })),
        (
            "to",
            json!({ "description": "Message recipient: {\"agent\":\"id\"} or \"broadcast\" depending on serde enum representation." }),
        ),
        ("message_type", json!({ "type": "string" })),
        ("payload", json!({ "type": "object" })),
        ("ack_required", json!({ "type": "boolean" })),
        ("message_id", json!({ "type": "string" })),
        ("success", json!({ "type": "boolean" })),
        ("tokens_used", json!({ "type": "integer", "minimum": 0 })),
        ("estimates", object_array_schema()),
        ("max_attempts", json!({ "type": "integer", "minimum": 1 })),
        ("now", json!({ "type": "integer" })),
        ("created_at", json!({ "type": "integer" })),
    ] {
        properties.insert(name.to_string(), value);
    }
    let mut schema = JsonMap::new();
    schema.insert("type".to_string(), json!("object"));
    schema.insert("properties".to_string(), JsonValue::Object(properties));
    schema.insert("required".to_string(), json!(["action"]));
    schema.insert("additionalProperties".to_string(), json!(false));
    JsonValue::Object(schema)
}

fn string_array_schema() -> JsonValue {
    json!({ "type": "array", "items": { "type": "string" } })
}

fn object_array_schema() -> JsonValue {
    json!({ "type": "array", "items": { "type": "object" } })
}

// ---------------------------------------------------------------------------
// TeamMemberTool — restricted tool exposed to sub-agent teammates
// ---------------------------------------------------------------------------

const MEMBER_TOOL_NAME: &str = "team_member";
const MEMBER_TOOL_DESCRIPTION: &str = "Communicate with your team via the shared mailbox. Check your inbox, send messages to the lead or broadcast to all teammates, and manage message lifecycle.";

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
#[allow(dead_code)]
enum TeamMemberToolInput {
    Status,
    ReadTask {
        task_id: TaskId,
    },
    ListTasks,
    SendMessage {
        to: MessageRecipient,
        #[serde(default)]
        message_type: Option<MessageType>,
        payload: MessagePayload,
        #[serde(default)]
        ack_required: Option<bool>,
        #[serde(default)]
        now: Option<i64>,
    },
    ConsumeMessage {
        #[serde(default)]
        now: Option<i64>,
    },
    FinishMessage {
        message_id: MessageId,
        success: bool,
        #[serde(default)]
        now: Option<i64>,
    },
    RouteMessages {
        #[serde(default)]
        max_attempts: Option<u32>,
        #[serde(default)]
        now: Option<i64>,
    },
    MailboxList,
    RecoverMessages {
        #[serde(default)]
        now: Option<i64>,
    },
    SummarizeInbox {
        #[serde(default)]
        now: Option<i64>,
    },
}

#[allow(dead_code)]
fn parse_member_args(call: &ToolCall) -> Result<TeamMemberToolInput, FunctionCallError> {
    serde_json::from_str(call.function_arguments()?)
        .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))
}

fn team_member_tool_schema() -> JsonValue {
    let actions = vec![
        "status",
        "read_task",
        "list_tasks",
        "send_message",
        "consume_message",
        "finish_message",
        "route_messages",
        "mailbox_list",
        "recover_messages",
        "summarize_inbox",
    ];
    let mut properties = JsonMap::new();
    properties.insert(
        "action".to_string(),
        json!({
            "type": "string",
            "enum": actions,
            "description": "Team member operation to execute."
        }),
    );
    for (name, value) in [
        ("task_id", json!({ "type": "string" })),
        (
            "to",
            json!({ "description": "Recipient: {\"agent\":\"id\"} for a specific teammate/lead, or \"broadcast\" for everyone." }),
        ),
        ("message_type", json!({ "type": "string" })),
        ("payload", json!({ "type": "object" })),
        ("ack_required", json!({ "type": "boolean" })),
        ("message_id", json!({ "type": "string" })),
        ("success", json!({ "type": "boolean" })),
        ("max_attempts", json!({ "type": "integer", "minimum": 1 })),
        ("now", json!({ "type": "integer" })),
    ] {
        properties.insert(name.to_string(), value);
    }
    let mut schema = JsonMap::new();
    schema.insert("type".to_string(), json!("object"));
    schema.insert("properties".to_string(), JsonValue::Object(properties));
    schema.insert("required".to_string(), json!(["action"]));
    schema.insert("additionalProperties".to_string(), json!(false));
    JsonValue::Object(schema)
}

/// A restricted team tool for sub-agent members.
///
/// Only exposes communication and read-only actions. The agent_id is fixed
/// at construction so members cannot impersonate other agents.
///
/// Note: currently unused because the extension system shares tools across all
/// threads in a session and `dynamic_tools` only accepts spec-level descriptors
/// (not in-process executors). This struct is kept for when per-thread tool
/// injection becomes available, or when a future refactor adds a dedicated
/// sub-agent extension path.
#[derive(Clone)]
#[allow(dead_code)]
pub(crate) struct TeamMemberTool {
    runtime: TeamRuntime,
    team_id: TeamId,
    agent_id: AgentId,
}

impl TeamMemberTool {
    #[allow(dead_code)]
    pub(crate) fn new(store: FsTeamStore, team_id: TeamId, agent_id: AgentId) -> Self {
        Self {
            runtime: TeamRuntime::new(store),
            team_id,
            agent_id,
        }
    }

    #[allow(dead_code)]
    async fn handle_call(&self, call: ToolCall) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let input = parse_member_args(&call)?;
        let mut handle = self.runtime.load_team(&self.team_id).map_err(tool_error)?;
        let now = now_unix_timestamp_secs();
        let output = match input {
            TeamMemberToolInput::Status => team_summary_json(handle.team()),
            TeamMemberToolInput::ReadTask { task_id } => {
                let task = handle.team().tasks.get(&task_id).cloned();
                json!({ "task": task })
            }
            TeamMemberToolInput::ListTasks => {
                let tasks: Vec<_> = handle.team().tasks.values().collect();
                json!({ "tasks": tasks })
            }
            TeamMemberToolInput::SendMessage {
                to,
                message_type,
                payload,
                ack_required,
                now: input_now,
            } => {
                let ts = input_now.unwrap_or(now);
                let message_id = handle.send_message(
                    &self.agent_id,
                    to,
                    message_type.unwrap_or(MessageType::Message),
                    payload,
                    ack_required.unwrap_or(false),
                    ts,
                )?;
                let delivered = handle.route_pending_messages(/*max_attempts*/ 3, ts)?;
                json!({ "message_id": message_id, "delivered": delivered })
            }
            TeamMemberToolInput::ConsumeMessage { now: input_now } => {
                let ts = input_now.unwrap_or(now);
                let message = handle
                    .consume_next_message(&self.agent_id, ts)
                    .map_err(tool_error)?;
                json!({ "message": message })
            }
            TeamMemberToolInput::FinishMessage {
                message_id,
                success,
                now: input_now,
            } => {
                let ts = input_now.unwrap_or(now);
                handle
                    .finish_message(&self.agent_id, &message_id, success, ts)
                    .map_err(tool_error)?;
                json!({ "ok": true })
            }
            TeamMemberToolInput::RouteMessages {
                max_attempts,
                now: input_now,
            } => {
                let ts = input_now.unwrap_or(now);
                let delivered = handle.route_pending_messages(max_attempts.unwrap_or(3), ts)?;
                json!({ "delivered": delivered })
            }
            TeamMemberToolInput::MailboxList => {
                let inbox = handle.team().mailbox.inboxes.get(&self.agent_id);
                let messages = inbox
                    .map(|inbox| {
                        let unread: Vec<_> = inbox
                            .unread
                            .iter()
                            .map(|id| json!({ "id": id, "state": "unread" }))
                            .collect();
                        let processing: Vec<_> = inbox
                            .processing
                            .iter()
                            .map(|id| json!({ "id": id, "state": "processing" }))
                            .collect();
                        let processed: Vec<_> = inbox
                            .processed
                            .iter()
                            .map(|id| json!({ "id": id, "state": "processed" }))
                            .collect();
                        [unread, processing, processed].concat()
                    })
                    .unwrap_or_default();
                json!({ "messages": messages, "count": messages.len() })
            }
            TeamMemberToolInput::RecoverMessages { now: input_now } => {
                let ts = input_now.unwrap_or(now);
                let recovered = handle
                    .recover_processing_messages(&self.agent_id, ts)
                    .map_err(tool_error)?;
                json!({ "recovered": recovered })
            }
            TeamMemberToolInput::SummarizeInbox { now: input_now } => {
                let ts = input_now.unwrap_or(now);
                let summarized = handle
                    .summarize_inbox_if_needed(&self.agent_id, ts)
                    .map_err(tool_error)?;
                json!({ "summarized": summarized })
            }
        };
        Ok(Box::new(JsonToolOutput::new(output)))
    }
}

#[allow(dead_code)]
impl ToolExecutor<ToolCall> for TeamMemberTool {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(MEMBER_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ResponsesApiTool {
            name: MEMBER_TOOL_NAME.to_string(),
            description: MEMBER_TOOL_DESCRIPTION.to_string(),
            strict: false,
            defer_loading: None,
            parameters: parse_tool_input_schema_without_compaction(&team_member_tool_schema())
                .unwrap_or_else(|err| panic!("team member schema should parse: {err}")),
            output_schema: None,
        })
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        false
    }

    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(call))
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn schema_lists_team_actions() {
        let schema = team_tool_schema();
        let actions = schema["properties"]["action"]["enum"]
            .as_array()
            .expect("action enum");
        assert!(actions.contains(&json!("claim_task")));
        assert!(actions.contains(&json!("transition_task_status")));
        assert!(actions.contains(&json!("release_task_claim")));
        assert!(actions.contains(&json!("mailbox_list")));
        assert!(actions.contains(&json!("get_summary")));
        assert_eq!(schema["required"], json!(["action"]));
    }

    #[test]
    fn member_schema_lists_communication_actions() {
        let schema = team_member_tool_schema();
        let actions = schema["properties"]["action"]["enum"]
            .as_array()
            .expect("action enum");
        assert!(actions.contains(&json!("send_message")));
        assert!(actions.contains(&json!("consume_message")));
        assert!(actions.contains(&json!("finish_message")));
        assert!(actions.contains(&json!("route_messages")));
        assert!(actions.contains(&json!("status")));
        assert!(actions.contains(&json!("read_task")));
        assert!(actions.contains(&json!("list_tasks")));
        assert!(!actions.contains(&json!("run_ready_tasks")));
        assert!(!actions.contains(&json!("request_shutdown")));
        assert!(!actions.contains(&json!("create")));
        assert_eq!(schema["required"], json!(["action"]));
    }
}
