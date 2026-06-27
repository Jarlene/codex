use std::sync::Arc;

use pretty_assertions::assert_eq;
use serde_json::Value as JsonValue;

use crate::AgentId;
use crate::AgentRole;
use crate::AgentSpec;
use crate::AgentStatus;
use crate::CostBudget;
use crate::CostEstimate;
use crate::CreateTeamRequest;
use crate::Effort;
use crate::FsTeamStore;
use crate::HeuristicTaskDecomposer;
use crate::MessagePayload;
use crate::MessageRecipient;
use crate::MessageState;
use crate::MessageType;
use crate::NewTask;
use crate::PlanImpact;
use crate::PlanReviewInput;
use crate::TaskDecompositionRequest;
use crate::TaskId;
use crate::TaskPriority;
use crate::TaskStatus;
use crate::TaskStatusTransition;
use crate::TeamConfig;
use crate::TeamError;
use crate::TeamLifecycle;
use crate::TeamRuntime;
use crate::TeamSizeRecommendationInput;
use crate::TeammateRunResult;
use crate::TeammateRunner;
use crate::TeammateRunnerFuture;
use crate::TerminalTaskData;
use crate::recommend_team_size;
use crate::runtime::TeammateRunRequest;

#[test]
fn creates_team_with_isolated_contexts_and_persistent_mailboxes() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let runtime = TeamRuntime::new(FsTeamStore::new(temp.path()));
    let handle = runtime.create_team(sample_request(1_000))?;

    assert_eq!(handle.team().agents.len(), 4);
    assert_eq!(handle.team().tasks.len(), 3);
    assert_eq!(handle.team().lifecycle, TeamLifecycle::Active);
    assert!(
        handle
            .team()
            .agents
            .values()
            .all(|agent| !agent.context.inherited_lead_history)
    );
    assert!(
        temp.path()
            .join(&handle.team().id.0)
            .join("team.json")
            .exists()
    );
    assert!(
        temp.path()
            .join(&handle.team().id.0)
            .join("agents/dev")
            .join("inbox/unread")
            .exists()
    );

    let loaded = runtime.load_team(&handle.team().id)?;
    assert_eq!(loaded.team(), handle.team());
    Ok(())
}

#[test]
fn task_dependencies_claim_and_plan_approval_gate_execution() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let runtime = TeamRuntime::new(FsTeamStore::new(temp.path()));
    let mut handle = runtime.create_team(sample_request(1_000))?;
    let dev = AgentId::new("dev");
    let backend = TaskId::new("backend");

    assert!(matches!(
        handle.claim_task(&dev, &backend, 1_001),
        Err(TeamError::DependenciesIncomplete { .. })
    ));

    let architect = AgentId::new("architect");
    let design = TaskId::new("design");
    let plan_id = handle.request_plan(
        &architect,
        &design,
        "Design team runtime API first".to_string(),
        vec!["unit tests".to_string()],
        PlanImpact {
            affects_database: false,
            affects_api_compatibility: true,
        },
        1_002,
    )?;
    handle.review_plan(
        &AgentId::new("lead"),
        &plan_id,
        PlanReviewInput {
            approved: true,
            comments: "approved with tests".to_string(),
            requires_tests: true,
            database_impact: false,
            api_compatibility_impact: true,
        },
        1_003,
    )?;
    handle.claim_task(&architect, &design, 1_004)?;
    handle.complete_task(&architect, &design, 1_000, 1_005)?;

    assert!(matches!(
        handle.claim_task(&dev, &backend, 1_006),
        Err(TeamError::PlanApprovalRequired { .. })
    ));
    let plan_id = handle.request_plan(
        &dev,
        &backend,
        "Implement backend and tests".to_string(),
        vec!["runtime tests".to_string()],
        PlanImpact {
            affects_database: false,
            affects_api_compatibility: false,
        },
        1_007,
    )?;
    handle.review_plan(
        &AgentId::new("lead"),
        &plan_id,
        PlanReviewInput {
            approved: true,
            comments: "go".to_string(),
            requires_tests: true,
            database_impact: false,
            api_compatibility_impact: false,
        },
        1_008,
    )?;
    let claimed = handle.claim_task(&dev, &backend, 1_009)?;
    assert_eq!(claimed.status, TaskStatus::InProgress);
    assert_eq!(claimed.assignee, Some(dev));
    Ok(())
}

#[test]
fn mailbox_routes_ordered_direct_messages_broadcast_and_recovery() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let runtime = TeamRuntime::new(FsTeamStore::new(temp.path()));
    let mut handle = runtime.create_team(sample_request(1_000))?;
    let lead = AgentId::new("lead");
    let dev = AgentId::new("dev");

    let first = handle.send_message(
        &lead,
        MessageRecipient::Agent(dev.clone()),
        MessageType::Message,
        MessagePayload::text("first"),
        false,
        1_001,
    )?;
    let second = handle.send_message(
        &lead,
        MessageRecipient::Agent(dev.clone()),
        MessageType::Message,
        MessagePayload::text("second"),
        false,
        1_002,
    )?;
    assert_eq!(handle.route_pending_messages(/*max_attempts*/ 3, 1_003)?, 2);

    let message = handle.consume_next_message(&dev, 1_004)?.expect("message");
    assert_eq!(message.id, first);
    assert_eq!(message.message.payload.content, "first");
    assert_eq!(handle.recover_processing_messages(&dev, 1_005)?, 1);
    let message = handle.consume_next_message(&dev, 1_006)?.expect("message");
    assert_eq!(message.id, first);
    handle.finish_message(&dev, &first, true, 1_007)?;
    let message = handle.consume_next_message(&dev, 1_008)?.expect("message");
    assert_eq!(message.id, second);

    handle.send_message(
        &lead,
        MessageRecipient::Broadcast,
        MessageType::Broadcast,
        MessagePayload::text("all hands"),
        false,
        1_009,
    )?;
    assert_eq!(handle.route_pending_messages(/*max_attempts*/ 3, 1_010)?, 1);
    let unread_recipients = handle
        .team()
        .mailbox
        .inboxes
        .iter()
        .filter(|(agent_id, inbox)| **agent_id != lead && !inbox.unread.is_empty())
        .count();
    assert_eq!(unread_recipients, 3);
    assert!(
        handle
            .team()
            .mailbox
            .messages
            .values()
            .any(|stored| stored.state == MessageState::Unread)
    );
    Ok(())
}

#[test]
fn writes_omx_compatible_state_tree_for_named_numeric_team() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let runtime = TeamRuntime::new(FsTeamStore::new(temp.path()));
    let mut request = sample_numeric_request(1_700_000_000);
    request.name = Some(crate::TeamId::new("e2e-team-demo"));
    request.display_name = Some("E2E Team Demo".to_string());
    let handle = runtime.create_team(request)?;
    let team_root = temp.path().join("e2e-team-demo");

    assert!(team_root.join("team.json").exists());
    assert!(team_root.join("config.json").exists());
    assert!(team_root.join("manifest.v2.json").exists());
    assert!(team_root.join("worker-agents.md").exists());
    assert!(team_root.join("workers/worker-1/identity.json").exists());
    assert!(team_root.join("workers/worker-1/status.json").exists());
    assert!(team_root.join("workers/worker-1/heartbeat.json").exists());
    assert!(team_root.join("workers/worker-1/inbox.md").exists());
    assert!(team_root.join("tasks/task-1.json").exists());
    assert!(team_root.join("tasks/task-2.json").exists());
    assert!(team_root.join("mailbox/leader-fixed.json").exists());
    assert!(team_root.join("mailbox/worker-1.json").exists());
    assert!(team_root.join("dispatch/requests.json").exists());
    assert!(team_root.join("events/events.ndjson").exists());
    assert!(team_root.join("monitor-snapshot.json").exists());
    assert!(team_root.join("phase.json").exists());

    let manifest = read_json(team_root.join("manifest.v2.json"))?;
    assert_eq!(manifest["schema_version"], 2);
    assert_eq!(manifest["name"], "e2e-team-demo");
    assert_eq!(manifest["leader"]["worker_id"], "leader-fixed");
    assert_eq!(
        manifest["policy"]["dispatch_mode"],
        "hook_preferred_with_fallback"
    );
    assert_eq!(manifest["worker_count"], 2);

    let task = read_json(team_root.join("tasks/task-1.json"))?;
    assert_eq!(task["id"], "1");
    assert_eq!(task["subject"], "Design");
    assert_eq!(task["status"], "pending");
    assert_eq!(task["version"], 1);

    let mailbox = read_json(team_root.join("mailbox/leader-fixed.json"))?;
    assert_eq!(mailbox["worker"], "leader-fixed");
    assert!(mailbox["messages"].as_array().expect("messages").is_empty());
    assert_eq!(handle.team().id.0, "e2e-team-demo");
    Ok(())
}

#[test]
fn claim_safe_lifecycle_uses_tokens_versions_and_leases() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let runtime = TeamRuntime::new(FsTeamStore::new(temp.path()));
    let mut handle = runtime.create_team(sample_numeric_request(1_000))?;
    let worker = AgentId::new("worker-1");
    let task_id = TaskId::new("1");

    let claimed = handle.claim_task_with_lease(
        &worker,
        &task_id,
        Some(/*expected_version*/ 1),
        /*lease_secs*/ 60,
        1_001,
    )?;
    assert_eq!(claimed.task.version, 2);
    assert_eq!(claimed.task.status, TaskStatus::InProgress);
    assert!(claimed.task.claim.is_some());

    assert!(matches!(
        handle.transition_task_status(
            TaskStatusTransition {
                agent_id: &worker,
                task_id: &task_id,
                from: TaskStatus::InProgress,
                to: TaskStatus::Completed,
                claim_token: "wrong-token",
                terminal: TerminalTaskData::default(),
            },
            1_002,
        ),
        Err(TeamError::ClaimConflict { .. })
    ));

    let task = handle.transition_task_status(
        TaskStatusTransition {
            agent_id: &worker,
            task_id: &task_id,
            from: TaskStatus::InProgress,
            to: TaskStatus::Completed,
            claim_token: &claimed.claim_token,
            terminal: TerminalTaskData {
                result: Some("done".to_string()),
                error: None,
            },
        },
        1_002,
    )?;
    assert_eq!(task.status, TaskStatus::Completed);
    assert_eq!(task.version, 3);
    assert_eq!(task.result, Some("done".to_string()));
    assert!(task.claim.is_none());

    let mut request = sample_numeric_request(2_000);
    request.name = Some(crate::TeamId::new("lease-reclaim"));
    let mut handle = runtime.create_team(request)?;
    let claimed = handle.claim_task_with_lease(
        &worker,
        &task_id,
        Some(/*expected_version*/ 1),
        /*lease_secs*/ 5,
        2_001,
    )?;
    assert!(matches!(
        handle.release_task_claim(&worker, &task_id, &claimed.claim_token, 2_007),
        Err(TeamError::LeaseExpired { .. })
    ));
    let outcome = handle.reclaim_expired_task_claim(&task_id, 2_007)?;
    assert!(outcome.reclaimed);
    assert_eq!(outcome.task.status, TaskStatus::Ready);
    assert!(outcome.task.claim.is_none());
    Ok(())
}

#[test]
fn task_and_worker_updates_rematerialize_omx_files() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let runtime = TeamRuntime::new(FsTeamStore::new(temp.path()));
    let mut request = sample_numeric_request(3_000);
    request.name = Some(crate::TeamId::new("update-files"));
    let mut handle = runtime.create_team(request)?;

    let task = handle.update_task(
        &AgentId::new("leader-fixed"),
        &TaskId::new("1"),
        crate::TaskUpdate {
            title: Some("Design v2".to_string()),
            description: None,
            dependencies: None,
            priority: Some(TaskPriority::P1),
            role_hint: Some(Some(AgentRole::Architect)),
            requires_plan_approval: Some(true),
        },
        3_001,
    )?;
    assert_eq!(task.title, "Design v2");
    assert_eq!(task.version, 2);

    handle.set_agent_status(
        &AgentId::new("worker-1"),
        AgentStatus::Running,
        Some(TaskId::new("1")),
        3_002,
    )?;
    handle.append_event(
        &AgentId::new("worker-1"),
        "manual progress".to_string(),
        3_003,
    )?;

    let team_root = temp.path().join("update-files");
    let task_json = read_json(team_root.join("tasks/task-1.json"))?;
    assert_eq!(task_json["subject"], "Design v2");
    assert_eq!(task_json["version"], 2);
    let status_json = read_json(team_root.join("workers/worker-1/status.json"))?;
    assert_eq!(status_json["state"], "working");
    assert_eq!(status_json["current_task_id"], "1");
    let events = std::fs::read_to_string(team_root.join("events/events.ndjson"))?;
    assert!(events.contains("manual progress"));
    Ok(())
}

#[test]
fn graceful_shutdown_and_cleanup_are_lead_only() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let store = FsTeamStore::new(temp.path());
    let runtime = TeamRuntime::new(store.clone());
    let mut handle = runtime.create_team(sample_request(1_000))?;
    let team_id = handle.team().id.clone();

    assert!(matches!(
        handle.request_shutdown(&AgentId::new("dev"), 1_001),
        Err(TeamError::NotTeamLead { .. })
    ));
    handle.request_shutdown(&AgentId::new("lead"), 1_002)?;
    assert_eq!(handle.team().lifecycle, TeamLifecycle::ShuttingDown);
    assert!(
        handle
            .team()
            .agents
            .values()
            .all(|agent| agent.status == AgentStatus::ShuttingDown)
    );
    handle.mark_stopped(&AgentId::new("lead"), 1_003)?;
    handle.cleanup(&AgentId::new("lead"), 1_004)?;
    assert!(!store.team_exists(&team_id));
    Ok(())
}

#[test]
fn v2_decomposes_tasks_recommends_team_size_summarizes_and_controls_budget() -> anyhow::Result<()> {
    let decomposer = HeuristicTaskDecomposer;
    let decomposition = decomposer.decompose(TaskDecompositionRequest {
        objective: "实现订单管理模块，包括前端、后端 API、数据库、测试和安全审计".to_string(),
        preferred_roles: Vec::new(),
        include_review: true,
        include_security: true,
    });
    assert!(
        decomposition
            .tasks
            .iter()
            .any(|task| task.role_hint == Some(AgentRole::Backend))
    );
    assert!(
        decomposition
            .tasks
            .iter()
            .any(|task| task.role_hint == Some(AgentRole::Security))
    );
    let recommendation = recommend_team_size(TeamSizeRecommendationInput {
        tasks: decomposition.tasks,
        budget_tokens: Some(1),
        high_risk_roles: vec![AgentRole::Security],
    });
    assert!(recommendation.total_agents >= 2);
    assert!(recommendation.downgrade.is_some());
    assert_eq!(recommendation.role_counts["security"], 2);

    let temp = tempfile::tempdir()?;
    let runtime = TeamRuntime::new(FsTeamStore::new(temp.path()));
    let mut request = sample_request(1_000);
    request.config.message_summary_threshold = 2;
    request.budget.token_limit = Some(2_000);
    let mut handle = runtime.create_team(request)?;
    let lead = AgentId::new("lead");
    let dev = AgentId::new("dev");
    handle.send_message(
        &lead,
        MessageRecipient::Agent(dev.clone()),
        MessageType::Message,
        MessagePayload::text("Decision: use filesystem state for recovery"),
        false,
        1_001,
    )?;
    handle.send_message(
        &lead,
        MessageRecipient::Agent(dev.clone()),
        MessageType::Message,
        MessagePayload::text("Risk: API integration is not wired yet\nAction: add tests"),
        false,
        1_002,
    )?;
    handle.route_pending_messages(/*max_attempts*/ 3, 1_003)?;
    let summary = handle
        .summarize_inbox_if_needed(&dev, 1_004)?
        .expect("summary");
    assert_eq!(
        summary.content.key_decisions,
        vec!["Decision: use filesystem state for recovery".to_string()]
    );
    assert_eq!(
        summary.content.risks,
        vec!["Risk: API integration is not wired yet".to_string()]
    );
    assert_eq!(
        summary.content.action_items,
        vec!["Action: add tests".to_string()]
    );

    let decision = handle.scheduler_decision(
        &[CostEstimate {
            task_id: TaskId::new("design"),
            estimated_tokens: 1_700,
        }],
        1_005,
    )?;
    assert!(decision.budget_warning.is_some());

    let slept = handle.sleep_idle_agents(1_400)?;
    assert!(slept.contains(&AgentId::new("dev")));
    handle.resume_agent(&AgentId::new("dev"), 1_401)?;
    assert_eq!(
        handle.team().agents[&AgentId::new("dev")].status,
        AgentStatus::Idle
    );
    Ok(())
}

#[tokio::test]
async fn run_ready_tasks_executes_teammates_in_parallel_and_updates_tasks() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let runtime = TeamRuntime::new(FsTeamStore::new(temp.path()));
    let mut request = sample_request(1_000);
    request.tasks = vec![new_task(
        "implementation",
        "Implementation",
        "Implement isolated task",
        Vec::new(),
        TaskPriority::P0,
        Some(AgentRole::Backend),
        false,
    )];
    let mut handle = runtime.create_team(request)?;
    let results = handle
        .run_ready_tasks(Arc::new(RecordingRunner), 1_001)
        .await?;

    assert_eq!(results.len(), 1);
    assert_eq!(
        handle.team().tasks[&TaskId::new("implementation")].status,
        TaskStatus::Completed
    );
    assert_eq!(
        results
            .iter()
            .map(|result| result.task_id.clone())
            .collect::<Vec<_>>(),
        vec![TaskId::new("implementation")]
    );
    Ok(())
}

struct RecordingRunner;

impl TeammateRunner for RecordingRunner {
    fn run<'a>(&'a self, request: TeammateRunRequest) -> TeammateRunnerFuture<'a> {
        Box::pin(async move {
            Ok(TeammateRunResult {
                agent_id: request.agent_id,
                task_id: request.task.id,
                success: true,
                output: "ok".to_string(),
                tokens_used: 25,
                completed_at: 1_002,
            })
        })
    }
}

fn sample_request(now: i64) -> CreateTeamRequest {
    CreateTeamRequest {
        name: None,
        display_name: None,
        objective: "Build agent team runtime".to_string(),
        lead: AgentSpec {
            id: AgentId::new("lead"),
            display_name: "Lead".to_string(),
            role: AgentRole::Lead,
            worker: None,
        },
        teammates: vec![
            AgentSpec {
                id: AgentId::new("architect"),
                display_name: "Architect".to_string(),
                role: AgentRole::Architect,
                worker: None,
            },
            AgentSpec {
                id: AgentId::new("dev"),
                display_name: "Developer".to_string(),
                role: AgentRole::Backend,
                worker: None,
            },
            AgentSpec {
                id: AgentId::new("reviewer"),
                display_name: "Reviewer".to_string(),
                role: AgentRole::Reviewer,
                worker: None,
            },
        ],
        tasks: vec![
            new_task(
                "design",
                "Design",
                "Design the runtime",
                Vec::new(),
                TaskPriority::P0,
                Some(AgentRole::Architect),
                true,
            ),
            new_task(
                "backend",
                "Backend",
                "Implement runtime",
                vec![TaskId::new("design")],
                TaskPriority::P0,
                Some(AgentRole::Backend),
                true,
            ),
            new_task(
                "review",
                "Review",
                "Review implementation",
                vec![TaskId::new("backend")],
                TaskPriority::P1,
                Some(AgentRole::Reviewer),
                false,
            ),
        ],
        config: TeamConfig {
            default_idle_timeout_secs: 300,
            ..TeamConfig::default()
        },
        budget: CostBudget::default(),
        created_at: now,
    }
}

fn sample_numeric_request(now: i64) -> CreateTeamRequest {
    CreateTeamRequest {
        name: None,
        display_name: None,
        objective: "Build numeric team runtime".to_string(),
        lead: AgentSpec {
            id: AgentId::new("leader-fixed"),
            display_name: "Lead".to_string(),
            role: AgentRole::Lead,
            worker: None,
        },
        teammates: vec![
            AgentSpec {
                id: AgentId::new("worker-1"),
                display_name: "Worker 1".to_string(),
                role: AgentRole::Backend,
                worker: Some(crate::WorkerRuntimeInfo {
                    index: 1,
                    worker_cli: Some("codex".to_string()),
                    assigned_tasks: vec![TaskId::new("1")],
                    pid: Some(123),
                    pane_id: Some("%1".to_string()),
                    working_dir: None,
                    worktree_repo_root: None,
                    worktree_path: None,
                    worktree_branch: None,
                    worktree_detached: false,
                    worktree_created: false,
                    team_state_root: None,
                }),
            },
            AgentSpec {
                id: AgentId::new("worker-2"),
                display_name: "Worker 2".to_string(),
                role: AgentRole::Reviewer,
                worker: Some(crate::WorkerRuntimeInfo {
                    index: 2,
                    worker_cli: Some("codex".to_string()),
                    assigned_tasks: vec![TaskId::new("2")],
                    pid: Some(456),
                    pane_id: Some("%2".to_string()),
                    working_dir: None,
                    worktree_repo_root: None,
                    worktree_path: None,
                    worktree_branch: None,
                    worktree_detached: false,
                    worktree_created: false,
                    team_state_root: None,
                }),
            },
        ],
        tasks: vec![
            new_task(
                "1",
                "Design",
                "Design the runtime",
                Vec::new(),
                TaskPriority::P0,
                Some(AgentRole::Backend),
                false,
            ),
            new_task(
                "2",
                "Review",
                "Review the runtime",
                vec![TaskId::new("1")],
                TaskPriority::P1,
                Some(AgentRole::Reviewer),
                false,
            ),
        ],
        config: TeamConfig::default(),
        budget: CostBudget::default(),
        created_at: now,
    }
}

fn read_json(path: impl AsRef<std::path::Path>) -> anyhow::Result<JsonValue> {
    let raw = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

fn new_task(
    id: &str,
    title: &str,
    description: &str,
    dependencies: Vec<TaskId>,
    priority: TaskPriority,
    role_hint: Option<AgentRole>,
    requires_plan_approval: bool,
) -> NewTask {
    NewTask {
        id: TaskId::new(id),
        title: title.to_string(),
        description: description.to_string(),
        dependencies,
        priority,
        role_hint,
        estimated_effort: Effort::Medium,
        requires_plan_approval,
    }
}
