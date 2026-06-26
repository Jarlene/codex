use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use chrono::SecondsFormat;
use chrono::Utc;
use codex_utils_path::write_atomically;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;

use crate::AgentId;
use crate::AgentRole;
use crate::AgentStatus;
use crate::MessageRecipient;
use crate::MessageState;
use crate::MessageType;
use crate::StoredMessage;
use crate::Task;
use crate::TaskId;
use crate::TaskStatus;
use crate::Team;
use crate::TeamDisplayMode;
use crate::TeamError;
use crate::TeamLifecycle;
use crate::WorkerLaunchMode;
use crate::WorkerRuntimeInfo;

const LEADER_WORKER_ID: &str = "leader-fixed";

pub(crate) fn materialize_team(root: &Path, team: &Team) -> Result<(), TeamError> {
    let team_dir = root.join(&team.id.0);
    create_dirs(&team_dir)?;
    write_json(&team_dir.join("config.json"), &team_config(team, root))?;
    write_json(
        &team_dir.join("manifest.v2.json"),
        &team_manifest(team, root),
    )?;
    write_worker_agents(&team_dir, team)?;
    write_phase(&team_dir, team)?;
    write_tasks(&team_dir, team)?;
    write_task_approvals(&team_dir, team)?;
    write_workers(&team_dir, team, root)?;
    write_mailboxes(&team_dir, team)?;
    write_dispatch_queue(&team_dir, team)?;
    write_events(&team_dir, team)?;
    write_monitor_snapshot(&team_dir, team)?;
    Ok(())
}

fn create_dirs(team_dir: &Path) -> Result<(), TeamError> {
    for dir in [
        team_dir.join("workers"),
        team_dir.join("tasks"),
        team_dir.join("claims"),
        team_dir.join("mailbox"),
        team_dir.join("dispatch"),
        team_dir.join("events"),
        team_dir.join("approvals"),
    ] {
        fs::create_dir_all(&dir).map_err(|err| TeamError::io(&dir, err))?;
    }
    Ok(())
}

fn team_config(team: &Team, root: &Path) -> TeamConfig {
    let workers = worker_infos(team, root);
    TeamConfig {
        name: team.id.0.clone(),
        task: team.objective.clone(),
        agent_type: workers
            .first()
            .map(|worker| worker.role.clone())
            .unwrap_or_else(|| "executor".to_string()),
        worker_launch_mode: worker_launch_mode_key(team.config.worker_launch_mode).to_string(),
        lifecycle_profile: "default".to_string(),
        worker_count: workers.len(),
        max_workers: team.config.max_agents,
        workers,
        created_at: iso(team.created_at),
        tmux_session: team
            .config
            .tmux_session
            .clone()
            .unwrap_or_else(|| format!("omx-team-{}", team.id.0)),
        next_task_id: next_task_id(team),
        leader_cwd: team.config.leader_cwd.clone(),
        team_state_root: team
            .config
            .team_state_root
            .clone()
            .or_else(|| root.parent().map(|path| path.display().to_string())),
        workspace_mode: Some("single".to_string()),
        worktree_mode: None,
        leader_pane_id: team.config.leader_pane_id.clone(),
        hud_pane_id: team.config.hud_pane_id.clone(),
        resize_hook_name: team.config.resize_hook_name.clone(),
        resize_hook_target: team.config.resize_hook_target.clone(),
        next_worker_index: Some(next_worker_index(team)),
        display_name: team.display_name.clone(),
        requested_name: team.requested_name.clone(),
        identity_source: Some("codex-rs-ext-team".to_string()),
    }
}

fn team_manifest(team: &Team, root: &Path) -> TeamManifestV2 {
    let config = team_config(team, root);
    TeamManifestV2 {
        schema_version: 2,
        name: config.name,
        task: config.task,
        leader: TeamLeader {
            session_id: String::new(),
            thread_id: None,
            worker_id: LEADER_WORKER_ID.to_string(),
            role: "coordinator".to_string(),
        },
        policy: TeamPolicy {
            display_mode: display_mode_key(team.config.display_mode).to_string(),
            worker_launch_mode: config.worker_launch_mode,
            dispatch_mode: "hook_preferred_with_fallback".to_string(),
            dispatch_ack_timeout_ms: team.config.dispatch_ack_timeout_ms,
        },
        governance: TeamGovernance {
            delegation_only: team.config.governance.delegation_only,
            plan_approval_required: team.config.governance.plan_approval_required,
            nested_teams_allowed: team.config.governance.nested_teams_allowed,
            one_team_per_leader_session: team.config.governance.one_team_per_leader_session,
            cleanup_requires_all_workers_inactive: team
                .config
                .governance
                .cleanup_requires_all_workers_inactive,
        },
        lifecycle_profile: config.lifecycle_profile,
        permissions_snapshot: TeamPermissionsSnapshot {
            approval_mode: team.config.permissions.approval_mode.clone(),
            sandbox_mode: team.config.permissions.sandbox_mode.clone(),
            network_access: team.config.permissions.network_access,
        },
        team_decomposition: None,
        tmux_session: config.tmux_session,
        worker_count: config.worker_count,
        workers: config.workers,
        next_task_id: config.next_task_id,
        created_at: config.created_at,
        leader_cwd: config.leader_cwd,
        team_state_root: config.team_state_root,
        workspace_mode: config.workspace_mode,
        worktree_mode: config.worktree_mode,
        leader_pane_id: config.leader_pane_id,
        hud_pane_id: config.hud_pane_id,
        resize_hook_name: config.resize_hook_name,
        resize_hook_target: config.resize_hook_target,
        next_worker_index: config.next_worker_index,
        display_name: config.display_name,
        requested_name: config.requested_name,
        identity_source: config.identity_source,
    }
}

fn write_worker_agents(team_dir: &Path, team: &Team) -> Result<(), TeamError> {
    let content = worker_agents_content(team);
    write_atomically(&team_dir.join("worker-agents.md"), &content)
        .map_err(|err| TeamError::io(team_dir.join("worker-agents.md"), err))
}

fn write_worker_root_agents(worker_dir: &Path, team: &Team) -> Result<(), TeamError> {
    let content = worker_agents_content(team);
    write_atomically(&worker_dir.join("AGENTS.md"), &content)
        .map_err(|err| TeamError::io(worker_dir.join("AGENTS.md"), err))
}

fn worker_agents_content(team: &Team) -> String {
    let mut content = String::new();
    content.push_str("# OMX Team Worker Overlay\n\n");
    content.push_str("Workers in this team coordinate through this directory:\n\n");
    content.push_str("- Read `workers/<worker>/inbox.md` for assignments.\n");
    content.push_str("- Claim tasks through the team API before working.\n");
    content.push_str("- Task files use `tasks/task-<id>.json`; API task ids are bare ids.\n");
    content.push_str("- Send ACK and progress messages to `mailbox/leader-fixed.json`.\n\n");
    content.push_str("Team objective:\n\n");
    content.push_str(&team.objective);
    content.push('\n');
    content
}

fn write_phase(team_dir: &Path, team: &Team) -> Result<(), TeamError> {
    let phase = match team.lifecycle {
        TeamLifecycle::Active => "team-exec",
        TeamLifecycle::ShuttingDown => "shutdown",
        TeamLifecycle::Stopped | TeamLifecycle::CleanedUp => "complete",
    };
    write_json(
        &team_dir.join("phase.json"),
        &json!({
            "current_phase": phase,
            "max_fix_attempts": 3,
            "current_fix_attempt": 0,
            "transitions": [],
            "updated_at": iso(team.updated_at),
        }),
    )
}

fn write_tasks(team_dir: &Path, team: &Team) -> Result<(), TeamError> {
    let tasks_dir = team_dir.join("tasks");
    clear_matching_files(&tasks_dir, "task-", "json")?;
    for task in team.tasks.values() {
        write_json(
            &tasks_dir.join(format!("task-{}.json", bare_task_id(&task.id))),
            &omx_task(task),
        )?;
    }
    Ok(())
}

fn write_task_approvals(team_dir: &Path, team: &Team) -> Result<(), TeamError> {
    let approvals_dir = team_dir.join("approvals");
    clear_matching_files(&approvals_dir, "task-", "json")?;
    for task in team
        .tasks
        .values()
        .filter(|task| task.requires_plan_approval)
    {
        let (status, reviewer, decision_reason, decided_at) =
            if let Some(plan_id) = &task.approved_plan {
                let plan = team.plans.get(plan_id);
                (
                    "approved",
                    plan.and_then(|plan| plan.review.as_ref())
                        .map(|review| review.reviewer.0.clone())
                        .unwrap_or_else(|| team.lead_id.0.clone()),
                    plan.and_then(|plan| plan.review.as_ref())
                        .map(|review| review.comments.clone())
                        .unwrap_or_default(),
                    task.updated_at,
                )
            } else {
                (
                    "pending",
                    team.lead_id.0.clone(),
                    String::new(),
                    task.updated_at,
                )
            };
        write_json(
            &approvals_dir.join(format!("task-{}.json", bare_task_id(&task.id))),
            &json!({
                "task_id": bare_task_id(&task.id),
                "required": true,
                "status": status,
                "reviewer": reviewer,
                "decision_reason": decision_reason,
                "decided_at": iso(decided_at),
            }),
        )?;
    }
    Ok(())
}

fn write_workers(team_dir: &Path, team: &Team, root: &Path) -> Result<(), TeamError> {
    for worker in worker_infos(team, root) {
        let worker_dir = team_dir.join("workers").join(&worker.name);
        fs::create_dir_all(&worker_dir).map_err(|err| TeamError::io(&worker_dir, err))?;
        write_json(&worker_dir.join("identity.json"), &worker)?;
        write_json(
            &worker_dir.join("status.json"),
            &worker_status(team, &worker.name),
        )?;
        write_json(
            &worker_dir.join("heartbeat.json"),
            &json!({
                "pid": worker.pid.unwrap_or(0),
                "last_turn_at": iso(team.updated_at),
                "turn_count": 0,
                "alive": !matches!(team.lifecycle, TeamLifecycle::Stopped | TeamLifecycle::CleanedUp),
            }),
        )?;
        write_worker_inbox(&worker_dir, team, &worker)?;
        write_worker_agents(&worker_dir, team)?;
        write_worker_root_agents(&worker_dir, team)?;
        if team.lifecycle == TeamLifecycle::ShuttingDown {
            write_json(
                &worker_dir.join("shutdown-request.json"),
                &json!({
                    "requested_at": iso(team.updated_at),
                    "requested_by": team.lead_id.0,
                }),
            )?;
        }
    }
    Ok(())
}

fn write_worker_inbox(
    worker_dir: &Path,
    team: &Team,
    worker: &WorkerInfo,
) -> Result<(), TeamError> {
    let mut content = String::new();
    content.push_str("# Team Assignment\n\n");
    content.push_str(&format!("Team: {}\n", team.id.0));
    content.push_str(&format!("Worker: {}\n\n", worker.name));
    content.push_str("Read the worker skill, ACK to leader-fixed, then claim the first unblocked assigned task.\n\n");
    content.push_str("Assigned tasks:\n");
    for task_id in &worker.assigned_tasks {
        if let Some(task) = team.tasks.get(&TaskId::new(task_id.clone())) {
            content.push_str(&format!(
                "- {}: {} [{}]\n",
                bare_task_id(&task.id),
                task.title,
                omx_task_status(task.status)
            ));
        } else {
            content.push_str(&format!("- {task_id}\n"));
        }
    }
    if worker.assigned_tasks.is_empty() {
        content.push_str("- none yet\n");
    }
    write_atomically(&worker_dir.join("inbox.md"), &content)
        .map_err(|err| TeamError::io(worker_dir.join("inbox.md"), err))
}

fn write_mailboxes(team_dir: &Path, team: &Team) -> Result<(), TeamError> {
    let mailbox_dir = team_dir.join("mailbox");
    let mut by_worker = BTreeMap::<String, Vec<MailboxMessage>>::new();
    by_worker.entry(LEADER_WORKER_ID.to_string()).or_default();
    for worker in worker_infos(team, team_dir) {
        by_worker.entry(worker.name).or_default();
    }
    for stored in team.mailbox.messages.values() {
        for (worker, message) in omx_mailbox_messages(stored, team) {
            by_worker.entry(worker).or_default().push(message);
        }
    }
    for (worker, messages) in by_worker {
        let path = mailbox_dir.join(format!("{worker}.json"));
        let merged = merge_existing_mailbox(path.as_path(), worker, messages);
        write_json(&path, &merged)?;
    }
    Ok(())
}

fn write_dispatch_queue(team_dir: &Path, team: &Team) -> Result<(), TeamError> {
    let requests_path = team_dir.join("dispatch").join("requests.json");
    let mut requests = read_existing_json_array::<DispatchRequest>(&requests_path);
    let mut known = requests
        .iter()
        .map(|request| request.request_id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    for stored in team.mailbox.messages.values() {
        if !matches!(stored.state, MessageState::Pending | MessageState::Unread) {
            continue;
        }
        for (worker, message) in omx_mailbox_messages(stored, team) {
            let request_id = format!("msg-{}", message.message_id);
            if !known.insert(request_id.clone()) {
                continue;
            }
            requests.push(DispatchRequest {
                request_id,
                kind: "mailbox".to_string(),
                team_name: team.id.0.clone(),
                to_worker: worker,
                worker_index: None,
                pane_id: None,
                trigger_message: trigger_message(&message.body),
                intent: None,
                message_id: Some(message.message_id),
                inbox_correlation_key: None,
                transport_preference: "hook_preferred_with_fallback".to_string(),
                fallback_allowed: true,
                status: "pending".to_string(),
                attempt_count: 0,
                created_at: message.created_at.clone(),
                updated_at: message.created_at,
                notified_at: None,
                delivered_at: None,
                failed_at: None,
                last_reason: None,
            });
        }
    }
    write_json(&requests_path, &requests)
}

fn write_events(team_dir: &Path, team: &Team) -> Result<(), TeamError> {
    let path = team_dir.join("events").join("events.ndjson");
    let mut lines = String::new();
    for event in &team.audit_log {
        let event_type = match event.action {
            crate::AuditAction::TaskUpdated if event.details.contains("failed") => "task_failed",
            crate::AuditAction::TaskUpdated if event.details.contains("completed") => {
                "task_completed"
            }
            crate::AuditAction::MessageDelivered | crate::AuditAction::MessageSent => {
                "message_received"
            }
            crate::AuditAction::ShutdownRequested => "shutdown_gate",
            _ => "worker_state_changed",
        };
        let record = json!({
            "event_id": format!("{}-{}-{}", team.id.0, event.timestamp, event.target),
            "team": team.id.0,
            "type": event_type,
            "worker": event.actor.0,
            "task_id": task_id_for_event(&event.target),
            "message_id": null,
            "reason": event.details,
            "created_at": iso(event.timestamp),
        });
        lines.push_str(&serde_json::to_string(&record)?);
        lines.push('\n');
    }
    write_atomically(&path, &lines).map_err(|err| TeamError::io(path, err))
}

fn write_monitor_snapshot(team_dir: &Path, team: &Team) -> Result<(), TeamError> {
    let task_status_by_id = team
        .tasks
        .values()
        .map(|task| {
            (
                bare_task_id(&task.id),
                omx_task_status(task.status).to_string(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let worker_infos = worker_infos(team, team_dir);
    let worker_alive_by_name = worker_infos
        .iter()
        .map(|worker| {
            (
                worker.name.clone(),
                !matches!(
                    team.lifecycle,
                    TeamLifecycle::Stopped | TeamLifecycle::CleanedUp
                ),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let worker_state_by_name = worker_infos
        .iter()
        .map(|worker| {
            (
                worker.name.clone(),
                worker_status_key(team, &worker.name).to_string(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let worker_task_id_by_name = worker_infos
        .iter()
        .filter_map(|worker| {
            team.agents
                .get(&AgentId::new(worker.name.clone()))
                .and_then(|agent| agent.active_task.as_ref())
                .map(|task_id| (worker.name.clone(), bare_task_id(task_id)))
        })
        .collect::<BTreeMap<_, _>>();
    let completed_event_task_ids = team
        .tasks
        .values()
        .filter(|task| task.status == TaskStatus::Completed)
        .map(|task| (bare_task_id(&task.id), true))
        .collect::<BTreeMap<_, _>>();
    write_json(
        &team_dir.join("monitor-snapshot.json"),
        &json!({
            "taskStatusById": task_status_by_id,
            "workerAliveByName": worker_alive_by_name,
            "workerStateByName": worker_state_by_name,
            "workerTurnCountByName": BTreeMap::<String, u64>::new(),
            "workerTaskIdByName": worker_task_id_by_name,
            "mailboxNotifiedByMessageId": BTreeMap::<String, String>::new(),
            "completedEventTaskIds": completed_event_task_ids,
        }),
    )
}

fn worker_infos(team: &Team, root: &Path) -> Vec<WorkerInfo> {
    let mut next_index = 1;
    team.agents
        .iter()
        .filter(|(_, agent)| agent.role != AgentRole::Lead)
        .map(|(agent_id, agent)| {
            let worker = agent.worker.as_ref();
            let index = worker.map(|worker| worker.index).unwrap_or_else(|| {
                let index = next_index;
                next_index += 1;
                index
            });
            WorkerInfo {
                name: agent_id.0.clone(),
                index,
                role: agent_role_key(agent.role).to_string(),
                worker_cli: worker.and_then(|worker| worker.worker_cli.clone()),
                assigned_tasks: assigned_tasks(team, agent_id, worker),
                pid: worker.and_then(|worker| worker.pid),
                pane_id: worker.and_then(|worker| worker.pane_id.clone()),
                working_dir: worker.and_then(|worker| worker.working_dir.clone()),
                worktree_repo_root: worker.and_then(|worker| worker.worktree_repo_root.clone()),
                worktree_path: worker.and_then(|worker| worker.worktree_path.clone()),
                worktree_branch: worker.and_then(|worker| worker.worktree_branch.clone()),
                worktree_detached: worker.map(|worker| worker.worktree_detached),
                worktree_created: worker.map(|worker| worker.worktree_created),
                team_state_root: worker
                    .and_then(|worker| worker.team_state_root.clone())
                    .or_else(|| root.parent().map(|path| path.display().to_string())),
            }
        })
        .collect()
}

fn assigned_tasks(
    team: &Team,
    agent_id: &AgentId,
    worker: Option<&WorkerRuntimeInfo>,
) -> Vec<String> {
    let mut assigned = worker
        .map(|worker| {
            worker
                .assigned_tasks
                .iter()
                .map(bare_task_id)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for task in team.tasks.values() {
        if task.assignee.as_ref() == Some(agent_id) {
            let id = bare_task_id(&task.id);
            if !assigned.contains(&id) {
                assigned.push(id);
            }
        }
    }
    assigned
}

fn worker_status(team: &Team, worker_name: &str) -> Value {
    let agent = team.agents.get(&AgentId::new(worker_name.to_string()));
    let current_task_id = agent
        .and_then(|agent| agent.active_task.as_ref())
        .map(bare_task_id);
    json!({
        "state": worker_status_key(team, worker_name),
        "current_task_id": current_task_id,
        "updated_at": iso(team.updated_at),
    })
}

fn worker_status_key(team: &Team, worker_name: &str) -> &'static str {
    let Some(agent) = team.agents.get(&AgentId::new(worker_name.to_string())) else {
        return "unknown";
    };
    match agent.status {
        AgentStatus::Idle | AgentStatus::Sleeping => "idle",
        AgentStatus::Running => "working",
        AgentStatus::ShuttingDown => "draining",
        AgentStatus::Stopped => "done",
    }
}

fn omx_task(task: &Task) -> TeamTask {
    TeamTask {
        id: bare_task_id(&task.id),
        subject: task.title.clone(),
        description: task.description.clone(),
        status: omx_task_status(task.status).to_string(),
        requires_code_change: Some(true),
        role: task.role_hint.map(|role| agent_role_key(role).to_string()),
        owner: task.assignee.as_ref().map(|agent| agent.0.clone()),
        result: task.result.clone(),
        error: task.error.clone(),
        blocked_by: if task.dependencies.is_empty() {
            None
        } else {
            Some(task.dependencies.iter().map(bare_task_id).collect())
        },
        depends_on: Some(task.dependencies.iter().map(bare_task_id).collect()),
        version: task.version.max(1),
        claim: task.claim.as_ref().map(|claim| TaskClaim {
            owner: claim.owner.0.clone(),
            token: claim.token.clone(),
            leased_until: iso(claim.leased_until),
        }),
        created_at: iso(task.created_at),
        completed_at: task.completed_at.map(iso),
    }
}

fn omx_mailbox_messages(stored: &StoredMessage, team: &Team) -> Vec<(String, MailboxMessage)> {
    let recipients = match &stored.message.to {
        MessageRecipient::Agent(agent_id) => vec![agent_id.0.clone()],
        MessageRecipient::Broadcast => team
            .agents
            .keys()
            .filter(|agent_id| *agent_id != &stored.message.from)
            .map(|agent_id| agent_id.0.clone())
            .collect(),
    };
    recipients
        .into_iter()
        .map(|to_worker| {
            let body = match stored.message.message_type {
                MessageType::Shutdown => "shutdown requested".to_string(),
                _ => stored.message.payload.content.clone(),
            };
            (
                to_worker.clone(),
                MailboxMessage {
                    message_id: stored.message.id.0.clone(),
                    from_worker: stored.message.from.0.clone(),
                    to_worker,
                    body,
                    created_at: iso(stored.message.timestamp),
                    notified_at: None,
                    delivered_at: if matches!(
                        stored.state,
                        MessageState::Processing | MessageState::Processed
                    ) {
                        Some(iso(stored.message.timestamp))
                    } else {
                        None
                    },
                },
            )
        })
        .collect()
}

fn merge_existing_mailbox(path: &Path, worker: String, messages: Vec<MailboxMessage>) -> Mailbox {
    let existing = fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Mailbox>(&raw).ok());
    let mut by_id = existing
        .map(|mailbox| {
            mailbox
                .messages
                .into_iter()
                .map(|message| (message.message_id.clone(), message))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    for message in messages {
        by_id
            .entry(message.message_id.clone())
            .and_modify(|existing| {
                existing.body = message.body.clone();
                existing.from_worker = message.from_worker.clone();
                existing.to_worker = message.to_worker.clone();
                existing.created_at = message.created_at.clone();
            })
            .or_insert(message);
    }
    Mailbox {
        worker,
        messages: by_id.into_values().collect(),
    }
}

fn read_existing_json_array<T>(path: &Path) -> Vec<T>
where
    T: for<'de> Deserialize<'de>,
{
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Vec<T>>(&raw).ok())
        .unwrap_or_default()
}

fn trigger_message(body: &str) -> String {
    let mut trigger = body
        .lines()
        .next()
        .unwrap_or("check team state")
        .to_string();
    if trigger.len() > 200 {
        trigger.truncate(200);
    }
    trigger
}

fn next_task_id(team: &Team) -> u64 {
    team.tasks
        .keys()
        .filter_map(|task_id| bare_task_id(task_id).parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

fn next_worker_index(team: &Team) -> usize {
    team.agents
        .values()
        .filter_map(|agent| agent.worker.as_ref().map(|worker| worker.index))
        .max()
        .unwrap_or_else(|| team.agents.len().saturating_sub(1))
        .saturating_add(1)
}

fn clear_matching_files(dir: &Path, prefix: &str, extension: &str) -> Result<(), TeamError> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir).map_err(|err| TeamError::io(dir, err))? {
        let entry = entry.map_err(|err| TeamError::io(dir, err))?;
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if file_name.starts_with(prefix)
            && path.extension().and_then(|ext| ext.to_str()) == Some(extension)
        {
            fs::remove_file(&path).map_err(|err| TeamError::io(path, err))?;
        }
    }
    Ok(())
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), TeamError> {
    let json = serde_json::to_string_pretty(value)?;
    write_atomically(path, &json).map_err(|err| TeamError::io(path, err))
}

fn iso(secs: i64) -> String {
    chrono::DateTime::<Utc>::from_timestamp(secs, 0)
        .unwrap_or_else(Utc::now)
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn bare_task_id(task_id: &TaskId) -> String {
    task_id
        .0
        .strip_prefix("task-")
        .unwrap_or(&task_id.0)
        .to_string()
}

fn task_id_for_event(target: &str) -> Option<String> {
    let bare = target.strip_prefix("task-").unwrap_or(target);
    if bare.chars().all(|ch| ch.is_ascii_digit()) {
        Some(bare.to_string())
    } else {
        None
    }
}

fn omx_task_status(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Pending | TaskStatus::Ready => "pending",
        TaskStatus::Blocked => "blocked",
        TaskStatus::InProgress => "in_progress",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
    }
}

fn agent_role_key(role: AgentRole) -> &'static str {
    match role {
        AgentRole::Lead => "coordinator",
        AgentRole::Architect => "architect",
        AgentRole::Developer => "executor",
        AgentRole::Tester => "test-engineer",
        AgentRole::Reviewer => "code-reviewer",
        AgentRole::Security => "security-reviewer",
        AgentRole::Performance => "performance-reviewer",
        AgentRole::Frontend | AgentRole::Backend | AgentRole::Database | AgentRole::Custom => {
            "executor"
        }
        AgentRole::Researcher => "researcher",
        AgentRole::Writer => "writer",
    }
}

fn worker_launch_mode_key(mode: WorkerLaunchMode) -> &'static str {
    match mode {
        WorkerLaunchMode::Interactive => "interactive",
        WorkerLaunchMode::Prompt => "prompt",
    }
}

fn display_mode_key(mode: TeamDisplayMode) -> &'static str {
    match mode {
        TeamDisplayMode::SplitPane => "split_pane",
        TeamDisplayMode::Auto => "auto",
    }
}

#[derive(Serialize)]
struct TeamConfig {
    name: String,
    task: String,
    agent_type: String,
    worker_launch_mode: String,
    lifecycle_profile: String,
    worker_count: usize,
    max_workers: usize,
    workers: Vec<WorkerInfo>,
    created_at: String,
    tmux_session: String,
    next_task_id: u64,
    leader_cwd: Option<String>,
    team_state_root: Option<String>,
    workspace_mode: Option<String>,
    worktree_mode: Option<String>,
    leader_pane_id: Option<String>,
    hud_pane_id: Option<String>,
    resize_hook_name: Option<String>,
    resize_hook_target: Option<String>,
    next_worker_index: Option<usize>,
    display_name: Option<String>,
    requested_name: Option<String>,
    identity_source: Option<String>,
}

#[derive(Serialize)]
struct TeamManifestV2 {
    schema_version: u8,
    name: String,
    task: String,
    leader: TeamLeader,
    policy: TeamPolicy,
    governance: TeamGovernance,
    lifecycle_profile: String,
    permissions_snapshot: TeamPermissionsSnapshot,
    team_decomposition: Option<Value>,
    tmux_session: String,
    worker_count: usize,
    workers: Vec<WorkerInfo>,
    next_task_id: u64,
    created_at: String,
    leader_cwd: Option<String>,
    team_state_root: Option<String>,
    workspace_mode: Option<String>,
    worktree_mode: Option<String>,
    leader_pane_id: Option<String>,
    hud_pane_id: Option<String>,
    resize_hook_name: Option<String>,
    resize_hook_target: Option<String>,
    next_worker_index: Option<usize>,
    display_name: Option<String>,
    requested_name: Option<String>,
    identity_source: Option<String>,
}

#[derive(Serialize)]
struct TeamLeader {
    session_id: String,
    thread_id: Option<String>,
    worker_id: String,
    role: String,
}

#[derive(Serialize)]
struct TeamPolicy {
    display_mode: String,
    worker_launch_mode: String,
    dispatch_mode: String,
    dispatch_ack_timeout_ms: u64,
}

#[derive(Serialize)]
struct TeamGovernance {
    delegation_only: bool,
    plan_approval_required: bool,
    nested_teams_allowed: bool,
    one_team_per_leader_session: bool,
    cleanup_requires_all_workers_inactive: bool,
}

#[derive(Serialize)]
struct TeamPermissionsSnapshot {
    approval_mode: String,
    sandbox_mode: String,
    network_access: bool,
}

#[derive(Clone, Debug, Serialize)]
struct WorkerInfo {
    name: String,
    index: usize,
    role: String,
    worker_cli: Option<String>,
    assigned_tasks: Vec<String>,
    pid: Option<u32>,
    pane_id: Option<String>,
    working_dir: Option<String>,
    worktree_repo_root: Option<String>,
    worktree_path: Option<String>,
    worktree_branch: Option<String>,
    worktree_detached: Option<bool>,
    worktree_created: Option<bool>,
    team_state_root: Option<String>,
}

#[derive(Serialize)]
struct TeamTask {
    id: String,
    subject: String,
    description: String,
    status: String,
    requires_code_change: Option<bool>,
    role: Option<String>,
    owner: Option<String>,
    result: Option<String>,
    error: Option<String>,
    blocked_by: Option<Vec<String>>,
    depends_on: Option<Vec<String>>,
    version: u64,
    claim: Option<TaskClaim>,
    created_at: String,
    completed_at: Option<String>,
}

#[derive(Serialize)]
struct TaskClaim {
    owner: String,
    token: String,
    leased_until: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Mailbox {
    worker: String,
    messages: Vec<MailboxMessage>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct MailboxMessage {
    message_id: String,
    from_worker: String,
    to_worker: String,
    body: String,
    created_at: String,
    notified_at: Option<String>,
    delivered_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct DispatchRequest {
    request_id: String,
    kind: String,
    team_name: String,
    to_worker: String,
    worker_index: Option<usize>,
    pane_id: Option<String>,
    trigger_message: String,
    intent: Option<String>,
    message_id: Option<String>,
    inbox_correlation_key: Option<String>,
    transport_preference: String,
    fallback_allowed: bool,
    status: String,
    attempt_count: u64,
    created_at: String,
    updated_at: String,
    notified_at: Option<String>,
    delivered_at: Option<String>,
    failed_at: Option<String>,
    last_reason: Option<String>,
}
