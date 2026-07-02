use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;

use crate::FsTeamStore;
use crate::TeamError;
use crate::model::AgentContext;
use crate::model::AgentId;
use crate::model::AgentRole;
use crate::model::AgentSpec;
use crate::model::AgentStatus;
use crate::model::AuditAction;
use crate::model::AuditEvent;
use crate::model::CostEstimate;
use crate::model::CreateTeamRequest;
use crate::model::Inbox;
use crate::model::Mailbox;
use crate::model::MessageId;
use crate::model::MessagePayload;
use crate::model::MessageRecipient;
use crate::model::MessageState;
use crate::model::MessageSummary;
use crate::model::MessageType;
use crate::model::NewTask;
use crate::model::Outbox;
use crate::model::PlanId;
use crate::model::PlanRequest;
use crate::model::PlanReview;
use crate::model::PlanStatus;
use crate::model::SchedulerDecision;
use crate::model::StoredMessage;
use crate::model::SummaryId;
use crate::model::Task;
use crate::model::TaskClaim;
use crate::model::TaskId;
use crate::model::TaskStatus;
use crate::model::Team;
use crate::model::TeamAgent;
use crate::model::TeamId;
use crate::model::TeamLifecycle;
use crate::model::WorkerRuntimeInfo;
use crate::scheduler;
use crate::summary;

#[derive(Clone, Debug)]
pub struct TeamRuntime {
    store: FsTeamStore,
}

impl TeamRuntime {
    pub fn new(store: FsTeamStore) -> Self {
        Self { store }
    }

    pub fn create_team(&self, request: CreateTeamRequest) -> Result<TeamRuntimeHandle, TeamError> {
        let requested = 1 + request.teammates.len();
        if requested > request.config.max_agents {
            return Err(TeamError::TooManyAgents {
                requested,
                max: request.config.max_agents,
            });
        }
        validate_new_tasks(&request.tasks)?;

        let team_id = self.allocate_team_id(request.name.as_ref(), &request.objective)?;
        let mut agents = std::collections::BTreeMap::new();
        let mut mailbox = Mailbox::empty();
        let mut audit_log = Vec::new();
        let lead = request.lead.clone();
        insert_agent(
            &mut agents,
            &mut mailbox,
            request.lead,
            AgentRole::Lead,
            request.created_at,
            &request.config.shared_context_keys,
        );
        audit_log.push(AuditEvent {
            timestamp: request.created_at,
            actor: lead.id.clone(),
            action: AuditAction::TeamCreated,
            target: team_id.0.clone(),
            details: request.objective.clone(),
        });

        for spec in request.teammates {
            let id = spec.id.clone();
            insert_agent(
                &mut agents,
                &mut mailbox,
                spec,
                id_role_hint(&id),
                request.created_at,
                &request.config.shared_context_keys,
            );
            audit_log.push(AuditEvent {
                timestamp: request.created_at,
                actor: lead.id.clone(),
                action: AuditAction::AgentSpawned,
                target: id.0,
                details: "teammate spawned".to_string(),
            });
        }

        let mut tasks = std::collections::BTreeMap::new();
        for new_task in request.tasks {
            let task = task_from_new(new_task, request.created_at);
            audit_log.push(AuditEvent {
                timestamp: request.created_at,
                actor: lead.id.clone(),
                action: AuditAction::TaskCreated,
                target: task.id.0.clone(),
                details: task.title.clone(),
            });
            tasks.insert(task.id.clone(), task);
        }

        let mut team = Team {
            id: team_id,
            display_name: request.display_name,
            requested_name: request.name.as_ref().map(|name| name.0.clone()),
            objective: request.objective,
            lead_id: lead.id,
            agents,
            tasks,
            mailbox,
            plans: std::collections::BTreeMap::new(),
            summaries: std::collections::BTreeMap::new(),
            audit_log,
            lifecycle: TeamLifecycle::Active,
            config: request.config,
            budget: request.budget,
            created_at: request.created_at,
            updated_at: request.created_at,
        };
        update_ready_tasks(&mut team.tasks);
        self.store.save_team(&team)?;
        Ok(TeamRuntimeHandle {
            store: self.store.clone(),
            team,
        })
    }

    pub fn load_team(&self, team_id: &TeamId) -> Result<TeamRuntimeHandle, TeamError> {
        if !self.store.team_exists(team_id) {
            return Err(TeamError::TeamNotFound {
                team_id: team_id.0.clone(),
            });
        }
        Ok(TeamRuntimeHandle {
            store: self.store.clone(),
            team: self.store.load_team(team_id)?,
        })
    }

    fn allocate_team_id(
        &self,
        requested_name: Option<&TeamId>,
        objective: &str,
    ) -> Result<TeamId, TeamError> {
        if let Some(team_id) = requested_name {
            validate_team_id(team_id)?;
            if self.store.team_exists(team_id) {
                return Err(TeamError::InvalidOperation(format!(
                    "team {team_id} already exists"
                )));
            }
            return Ok(team_id.clone());
        }

        let base = TeamId::slug_from_objective(objective);
        if !self.store.team_exists(&base) {
            return Ok(base);
        }
        for suffix in 2..=999 {
            let suffix = format!("-{suffix}");
            let keep = 30usize.saturating_sub(suffix.len());
            let mut value = base.0.chars().take(keep).collect::<String>();
            value = value.trim_matches('-').to_string();
            value.push_str(&suffix);
            let candidate = TeamId::new(value);
            if !self.store.team_exists(&candidate) {
                return Ok(candidate);
            }
        }
        Ok(TeamId::new(Uuid::new_v4().to_string()))
    }
}

#[derive(Clone, Debug)]
pub struct TeamRuntimeHandle {
    store: FsTeamStore,
    team: Team,
}

impl TeamRuntimeHandle {
    pub fn team(&self) -> &Team {
        &self.team
    }

    pub fn add_task(&mut self, actor: &AgentId, task: NewTask, now: i64) -> Result<(), TeamError> {
        self.ensure_active()?;
        self.ensure_agent(actor)?;
        if self.team.tasks.contains_key(&task.id) {
            return Err(TeamError::InvalidOperation(format!(
                "task {} already exists",
                task.id
            )));
        }
        for dependency in &task.dependencies {
            if !self.team.tasks.contains_key(dependency) {
                return Err(TeamError::TaskNotFound {
                    task_id: dependency.0.clone(),
                });
            }
        }

        let mut all_tasks = self
            .team
            .tasks
            .values()
            .map(new_task_from_task)
            .collect::<Vec<_>>();
        all_tasks.push(task.clone());
        validate_new_tasks(&all_tasks)?;

        let inserted = task_from_new(task, now);
        self.team
            .tasks
            .insert(inserted.id.clone(), inserted.clone());
        self.audit(
            actor,
            AuditAction::TaskCreated,
            inserted.id.0,
            inserted.title,
            now,
        );
        self.touch(now);
        update_ready_tasks(&mut self.team.tasks);
        self.save()
    }

    pub fn read_task(&self, task_id: &TaskId) -> Result<Task, TeamError> {
        self.team
            .tasks
            .get(task_id)
            .cloned()
            .ok_or_else(|| TeamError::TaskNotFound {
                task_id: task_id.0.clone(),
            })
    }

    pub fn list_tasks(&self) -> Vec<Task> {
        self.team.tasks.values().cloned().collect()
    }

    pub fn update_task(
        &mut self,
        actor: &AgentId,
        task_id: &TaskId,
        update: TaskUpdate,
        now: i64,
    ) -> Result<Task, TeamError> {
        self.ensure_active()?;
        self.ensure_agent(actor)?;
        if let Some(dependencies) = &update.dependencies {
            for dependency in dependencies {
                if !self.team.tasks.contains_key(dependency) {
                    return Err(TeamError::TaskNotFound {
                        task_id: dependency.0.clone(),
                    });
                }
            }
        }
        let mut all_tasks = self
            .team
            .tasks
            .values()
            .map(new_task_from_task)
            .collect::<Vec<_>>();
        if let Some(candidate) = all_tasks.iter_mut().find(|task| &task.id == task_id)
            && let Some(dependencies) = &update.dependencies
        {
            candidate.dependencies.clone_from(dependencies);
        }
        validate_new_tasks(&all_tasks)?;

        let task = self
            .team
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| TeamError::TaskNotFound {
                task_id: task_id.0.clone(),
            })?;
        if let Some(title) = update.title {
            task.title = title;
        }
        if let Some(description) = update.description {
            task.description = description;
        }
        if let Some(priority) = update.priority {
            task.priority = priority;
        }
        if let Some(role_hint) = update.role_hint {
            task.role_hint = role_hint;
        }
        if let Some(dependencies) = update.dependencies {
            task.dependencies = dependencies;
        }
        if let Some(requires_plan_approval) = update.requires_plan_approval {
            task.requires_plan_approval = requires_plan_approval;
        }
        task.version = task.version.saturating_add(1);
        task.updated_at = now;
        update_ready_tasks(&mut self.team.tasks);
        let cloned = self.read_task(task_id)?;
        self.audit(
            actor,
            AuditAction::TaskUpdated,
            task_id.0.clone(),
            "task updated".to_string(),
            now,
        );
        self.touch(now);
        self.save()?;
        Ok(cloned)
    }

    pub fn claim_task(
        &mut self,
        agent_id: &AgentId,
        task_id: &TaskId,
        now: i64,
    ) -> Result<Task, TeamError> {
        Ok(self
            .claim_task_with_lease(agent_id, task_id, None, DEFAULT_CLAIM_LEASE_SECS, now)?
            .task)
    }

    pub fn claim_task_with_lease(
        &mut self,
        agent_id: &AgentId,
        task_id: &TaskId,
        expected_version: Option<u64>,
        lease_secs: i64,
        now: i64,
    ) -> Result<ClaimedTask, TeamError> {
        self.ensure_active()?;
        self.ensure_agent(agent_id)?;
        update_ready_tasks(&mut self.team.tasks);
        if !dependencies_completed(&self.team.tasks, task_id) {
            let dependencies = incomplete_dependencies(&self.team.tasks, task_id);
            return Err(TeamError::DependenciesIncomplete {
                task_id: task_id.0.clone(),
                dependencies,
            });
        }
        let task = self
            .team
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| TeamError::TaskNotFound {
                task_id: task_id.0.clone(),
            })?;
        if let Some(expected_version) = expected_version
            && task.version != expected_version
        {
            return Err(TeamError::ClaimConflict {
                task_id: task_id.0.clone(),
                owner: task.assignee.as_ref().map(|agent| agent.0.clone()),
            });
        }
        if matches!(task.status, TaskStatus::Completed | TaskStatus::Failed) {
            return Err(TeamError::AlreadyTerminal {
                task_id: task_id.0.clone(),
            });
        }
        if let Some(claim) = &task.claim
            && claim.leased_until > now
            && claim.owner != *agent_id
        {
            return Err(TeamError::ClaimConflict {
                task_id: task_id.0.clone(),
                owner: Some(claim.owner.0.clone()),
            });
        }
        if let Some(assignee) = &task.assignee
            && assignee != agent_id
        {
            return Err(TeamError::TaskAlreadyClaimed {
                task_id: task_id.0.clone(),
                assignee: assignee.0.clone(),
            });
        }
        if task.requires_plan_approval && task.approved_plan.is_none() {
            return Err(TeamError::PlanApprovalRequired {
                task_id: task_id.0.clone(),
            });
        }
        let claim_token = Uuid::new_v4().to_string();
        task.status = TaskStatus::InProgress;
        task.assignee = Some(agent_id.clone());
        task.claim = Some(TaskClaim {
            owner: agent_id.clone(),
            token: claim_token.clone(),
            leased_until: now.saturating_add(lease_secs.max(1)),
        });
        task.version = task.version.saturating_add(1);
        task.updated_at = now;
        let cloned = task.clone();
        if let Some(agent) = self.team.agents.get_mut(agent_id) {
            agent.status = AgentStatus::Running;
            agent.active_task = Some(task_id.clone());
            agent.last_active_at = now;
        }
        self.audit(
            agent_id,
            AuditAction::TaskClaimed,
            task_id.0.clone(),
            "task claimed".to_string(),
            now,
        );
        self.touch(now);
        self.save()?;
        Ok(ClaimedTask {
            task: cloned,
            claim_token,
        })
    }

    pub fn complete_task(
        &mut self,
        agent_id: &AgentId,
        task_id: &TaskId,
        tokens_used: u64,
        now: i64,
    ) -> Result<(), TeamError> {
        self.ensure_active()?;
        self.ensure_agent(agent_id)?;
        let task = self
            .team
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| TeamError::TaskNotFound {
                task_id: task_id.0.clone(),
            })?;
        if task.assignee.as_ref() != Some(agent_id) {
            return Err(TeamError::InvalidOperation(format!(
                "task {task_id} is not assigned to {agent_id}"
            )));
        }
        task.status = TaskStatus::Completed;
        task.result.get_or_insert_with(|| "completed".to_string());
        task.error = None;
        task.claim = None;
        task.version = task.version.saturating_add(1);
        task.updated_at = now;
        task.completed_at = Some(now);
        if let Some(agent) = self.team.agents.get_mut(agent_id) {
            agent.status = AgentStatus::Idle;
            agent.active_task = None;
            agent.last_active_at = now;
        }
        self.record_token_usage(tokens_used, now)?;
        update_ready_tasks(&mut self.team.tasks);
        self.audit(
            agent_id,
            AuditAction::TaskUpdated,
            task_id.0.clone(),
            "task completed".to_string(),
            now,
        );
        self.touch(now);
        self.save()
    }

    pub fn transition_task_status(
        &mut self,
        transition: TaskStatusTransition,
        now: i64,
    ) -> Result<Task, TeamError> {
        self.ensure_active()?;
        let TaskStatusTransition {
            agent_id,
            task_id,
            from,
            to,
            claim_token,
            terminal,
        } = transition;
        self.ensure_agent(agent_id)?;
        if !can_transition_task_status(from, to) {
            return Err(TeamError::InvalidTransition {
                task_id: task_id.0.clone(),
                from,
                to,
            });
        }
        let task = self
            .team
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| TeamError::TaskNotFound {
                task_id: task_id.0.clone(),
            })?;
        if matches!(task.status, TaskStatus::Completed | TaskStatus::Failed) {
            return Err(TeamError::AlreadyTerminal {
                task_id: task_id.0.clone(),
            });
        }
        if task.status != from {
            return Err(TeamError::InvalidTransition {
                task_id: task_id.0.clone(),
                from: task.status,
                to,
            });
        }
        let Some(claim) = task.claim.as_ref() else {
            return Err(TeamError::ClaimConflict {
                task_id: task_id.0.clone(),
                owner: task.assignee.as_ref().map(|agent| agent.0.clone()),
            });
        };
        if &claim.owner != agent_id || claim.token != claim_token {
            return Err(TeamError::ClaimConflict {
                task_id: task_id.0.clone(),
                owner: Some(claim.owner.0.clone()),
            });
        }
        if claim.leased_until <= now {
            return Err(TeamError::LeaseExpired {
                task_id: task_id.0.clone(),
            });
        }

        task.status = to;
        task.result = if to == TaskStatus::Completed {
            terminal.result
        } else {
            None
        };
        task.error = if to == TaskStatus::Failed {
            terminal.error
        } else {
            None
        };
        task.claim = None;
        task.version = task.version.saturating_add(1);
        task.updated_at = now;
        task.completed_at = Some(now);
        let cloned = task.clone();
        if let Some(agent) = self.team.agents.get_mut(agent_id) {
            agent.status = AgentStatus::Idle;
            agent.active_task = None;
            agent.last_active_at = now;
        }
        update_ready_tasks(&mut self.team.tasks);
        self.audit(
            agent_id,
            AuditAction::TaskUpdated,
            task_id.0.clone(),
            format!("task transitioned to {}", task_status_key(to)),
            now,
        );
        self.touch(now);
        self.save()?;
        Ok(cloned)
    }

    pub fn release_task_claim(
        &mut self,
        agent_id: &AgentId,
        task_id: &TaskId,
        claim_token: &str,
        now: i64,
    ) -> Result<Task, TeamError> {
        self.ensure_active()?;
        self.ensure_agent(agent_id)?;
        let task = self
            .team
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| TeamError::TaskNotFound {
                task_id: task_id.0.clone(),
            })?;
        if matches!(task.status, TaskStatus::Completed | TaskStatus::Failed) {
            return Err(TeamError::AlreadyTerminal {
                task_id: task_id.0.clone(),
            });
        }
        let Some(claim) = task.claim.as_ref() else {
            return Err(TeamError::ClaimConflict {
                task_id: task_id.0.clone(),
                owner: task.assignee.as_ref().map(|agent| agent.0.clone()),
            });
        };
        if &claim.owner != agent_id || claim.token != claim_token {
            return Err(TeamError::ClaimConflict {
                task_id: task_id.0.clone(),
                owner: Some(claim.owner.0.clone()),
            });
        }
        if claim.leased_until <= now {
            return Err(TeamError::LeaseExpired {
                task_id: task_id.0.clone(),
            });
        }
        task.status = TaskStatus::Pending;
        task.assignee = None;
        task.claim = None;
        task.version = task.version.saturating_add(1);
        task.updated_at = now;
        if let Some(agent) = self.team.agents.get_mut(agent_id) {
            agent.status = AgentStatus::Idle;
            agent.active_task = None;
            agent.last_active_at = now;
        }
        update_ready_tasks(&mut self.team.tasks);
        let cloned =
            self.team
                .tasks
                .get(task_id)
                .cloned()
                .ok_or_else(|| TeamError::TaskNotFound {
                    task_id: task_id.0.clone(),
                })?;
        self.touch(now);
        self.save()?;
        Ok(cloned)
    }

    pub fn reclaim_expired_task_claim(
        &mut self,
        task_id: &TaskId,
        now: i64,
    ) -> Result<ReclaimTaskOutcome, TeamError> {
        self.ensure_active()?;
        let task = self
            .team
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| TeamError::TaskNotFound {
                task_id: task_id.0.clone(),
            })?;
        if matches!(task.status, TaskStatus::Completed | TaskStatus::Failed) {
            return Err(TeamError::AlreadyTerminal {
                task_id: task_id.0.clone(),
            });
        }
        let Some(claim) = task.claim.as_ref() else {
            return Ok(ReclaimTaskOutcome {
                task: task.clone(),
                reclaimed: false,
            });
        };
        if claim.leased_until > now {
            return Err(TeamError::LeaseActive {
                task_id: task_id.0.clone(),
            });
        }
        let owner = claim.owner.clone();
        task.status = TaskStatus::Pending;
        task.assignee = None;
        task.claim = None;
        task.version = task.version.saturating_add(1);
        task.updated_at = now;
        if let Some(agent) = self.team.agents.get_mut(&owner) {
            agent.status = AgentStatus::Idle;
            agent.active_task = None;
            agent.last_active_at = now;
        }
        update_ready_tasks(&mut self.team.tasks);
        let cloned =
            self.team
                .tasks
                .get(task_id)
                .cloned()
                .ok_or_else(|| TeamError::TaskNotFound {
                    task_id: task_id.0.clone(),
                })?;
        self.touch(now);
        self.save()?;
        Ok(ReclaimTaskOutcome {
            task: cloned,
            reclaimed: true,
        })
    }

    pub fn request_plan(
        &mut self,
        requester: &AgentId,
        task_id: &TaskId,
        plan: String,
        tests: Vec<String>,
        impact: PlanImpact,
        now: i64,
    ) -> Result<PlanId, TeamError> {
        self.ensure_active()?;
        self.ensure_agent(requester)?;
        if !self.team.tasks.contains_key(task_id) {
            return Err(TeamError::TaskNotFound {
                task_id: task_id.0.clone(),
            });
        }
        let plan_id = PlanId::new(Uuid::new_v4().to_string());
        let request = PlanRequest {
            id: plan_id.clone(),
            task_id: task_id.clone(),
            requester: requester.clone(),
            plan,
            tests,
            affects_database: impact.affects_database,
            affects_api_compatibility: impact.affects_api_compatibility,
            status: PlanStatus::Pending,
            review: None,
            created_at: now,
            updated_at: now,
        };
        self.team.plans.insert(plan_id.clone(), request);
        let mut payload = MessagePayload::text("plan approval requested");
        payload.task_id = Some(task_id.clone());
        payload.plan_id = Some(plan_id.clone());
        self.enqueue_message(
            requester,
            MessageRecipient::Agent(self.team.lead_id.clone()),
            MessageType::PlanRequest,
            payload,
            true,
            now,
        )?;
        self.audit(
            requester,
            AuditAction::PlanRequested,
            plan_id.0.clone(),
            "plan approval requested".to_string(),
            now,
        );
        self.touch(now);
        self.save()?;
        Ok(plan_id)
    }

    pub fn review_plan(
        &mut self,
        reviewer: &AgentId,
        plan_id: &PlanId,
        review: PlanReviewInput,
        now: i64,
    ) -> Result<(), TeamError> {
        self.ensure_lead(reviewer)?;
        let (requester, task_id, approved) = {
            let plan = self
                .team
                .plans
                .get_mut(plan_id)
                .ok_or_else(|| TeamError::PlanNotFound {
                    plan_id: plan_id.0.clone(),
                })?;
            plan.status = if review.approved {
                PlanStatus::Approved
            } else {
                PlanStatus::Rejected
            };
            plan.review = Some(PlanReview {
                reviewer: reviewer.clone(),
                approved: review.approved,
                comments: review.comments.clone(),
                requires_tests: review.requires_tests,
                database_impact: review.database_impact,
                api_compatibility_impact: review.api_compatibility_impact,
                reviewed_at: now,
            });
            plan.updated_at = now;
            (
                plan.requester.clone(),
                plan.task_id.clone(),
                review.approved,
            )
        };
        if approved {
            let task =
                self.team
                    .tasks
                    .get_mut(&task_id)
                    .ok_or_else(|| TeamError::TaskNotFound {
                        task_id: task_id.0.clone(),
                    })?;
            task.approved_plan = Some(plan_id.clone());
            task.updated_at = now;
        }
        let mut payload = MessagePayload::text(review.comments);
        payload.task_id = Some(task_id);
        payload.plan_id = Some(plan_id.clone());
        payload
            .metadata
            .insert("approved".to_string(), approved.to_string());
        self.enqueue_message(
            reviewer,
            MessageRecipient::Agent(requester),
            MessageType::PlanResponse,
            payload,
            true,
            now,
        )?;
        self.audit(
            reviewer,
            AuditAction::PlanReviewed,
            plan_id.0.clone(),
            format!("approved={approved}"),
            now,
        );
        self.touch(now);
        update_ready_tasks(&mut self.team.tasks);
        self.save()
    }

    pub fn send_message(
        &mut self,
        from: &AgentId,
        to: MessageRecipient,
        message_type: MessageType,
        payload: MessagePayload,
        ack_required: bool,
        now: i64,
    ) -> Result<MessageId, TeamError> {
        self.ensure_active()?;
        self.ensure_agent(from)?;
        let id = self.enqueue_message(from, to, message_type, payload, ack_required, now)?;
        self.touch(now);
        self.save()?;
        Ok(id)
    }

    pub fn route_pending_messages(
        &mut self,
        max_attempts: u32,
        now: i64,
    ) -> Result<usize, TeamError> {
        self.ensure_active()?;
        let mut delivered = 0;
        let outbox_owners = self
            .team
            .mailbox
            .outboxes
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for owner in outbox_owners {
            let pending = self
                .team
                .mailbox
                .outboxes
                .get(&owner)
                .map(|outbox| outbox.pending.clone())
                .unwrap_or_default();
            for message_id in pending {
                if self.deliver_message(&owner, &message_id, max_attempts, now)? {
                    delivered += 1;
                }
            }
        }
        self.touch(now);
        self.save()?;
        Ok(delivered)
    }

    pub fn consume_next_message(
        &mut self,
        agent_id: &AgentId,
        now: i64,
    ) -> Result<Option<TeamMessageSnapshot>, TeamError> {
        self.ensure_agent(agent_id)?;
        let Some(inbox) = self.team.mailbox.inboxes.get_mut(agent_id) else {
            return Ok(None);
        };
        if inbox.unread.is_empty() {
            return Ok(None);
        }
        let message_id = inbox.unread.remove(0);
        inbox.processing.push(message_id.clone());
        let message = {
            let stored = self
                .team
                .mailbox
                .messages
                .get_mut(&message_id)
                .ok_or_else(|| TeamError::MessageNotFound {
                    message_id: message_id.0.clone(),
                })?;
            stored.state = MessageState::Processing;
            stored.message.clone()
        };
        self.audit(
            agent_id,
            AuditAction::MessageConsumed,
            message_id.0.clone(),
            "message moved to processing".to_string(),
            now,
        );
        self.touch(now);
        self.save()?;
        Ok(Some(TeamMessageSnapshot {
            id: message_id,
            message,
        }))
    }

    pub fn finish_message(
        &mut self,
        agent_id: &AgentId,
        message_id: &MessageId,
        success: bool,
        now: i64,
    ) -> Result<(), TeamError> {
        self.ensure_agent(agent_id)?;
        let inbox = self.team.mailbox.inboxes.get_mut(agent_id).ok_or_else(|| {
            TeamError::AgentNotFound {
                agent_id: agent_id.0.clone(),
            }
        })?;
        remove_id(&mut inbox.processing, message_id);
        if success {
            push_unique(&mut inbox.processed, message_id.clone());
        } else {
            push_unique(&mut inbox.failed, message_id.clone());
        }
        let stored = self
            .team
            .mailbox
            .messages
            .get_mut(message_id)
            .ok_or_else(|| TeamError::MessageNotFound {
                message_id: message_id.0.clone(),
            })?;
        stored.state = if success {
            MessageState::Processed
        } else {
            MessageState::Failed
        };
        self.audit(
            agent_id,
            AuditAction::MessageConsumed,
            message_id.0.clone(),
            format!("message finished success={success}"),
            now,
        );
        self.touch(now);
        self.save()
    }

    pub fn recover_processing_messages(
        &mut self,
        agent_id: &AgentId,
        now: i64,
    ) -> Result<usize, TeamError> {
        self.ensure_agent(agent_id)?;
        let inbox = self.team.mailbox.inboxes.get_mut(agent_id).ok_or_else(|| {
            TeamError::AgentNotFound {
                agent_id: agent_id.0.clone(),
            }
        })?;
        let recovered = inbox.processing.len();
        let processing = std::mem::take(&mut inbox.processing);
        for message_id in processing.into_iter().rev() {
            if let Some(stored) = self.team.mailbox.messages.get_mut(&message_id) {
                stored.state = MessageState::Unread;
            }
            inbox.unread.insert(0, message_id);
        }
        self.touch(now);
        self.save()?;
        Ok(recovered)
    }

    pub fn summarize_inbox_if_needed(
        &mut self,
        agent_id: &AgentId,
        now: i64,
    ) -> Result<Option<MessageSummary>, TeamError> {
        self.ensure_agent(agent_id)?;
        let Some(inbox) = self.team.mailbox.inboxes.get(agent_id) else {
            return Ok(None);
        };
        let source_ids = inbox.unread.clone();
        if source_ids.is_empty() {
            return Ok(None);
        }
        let has_long_message = source_ids.iter().any(|message_id| {
            self.team
                .mailbox
                .messages
                .get(message_id)
                .map(|stored| {
                    stored.message.payload.content.len()
                        >= self.team.config.long_message_threshold_chars
                })
                .unwrap_or(false)
        });
        if source_ids.len() < self.team.config.message_summary_threshold && !has_long_message {
            return Ok(None);
        }
        let messages = source_ids
            .iter()
            .filter_map(|id| self.team.mailbox.messages.get(id))
            .map(|stored| stored.message.clone())
            .collect::<Vec<_>>();
        let summary = summary::summarize_messages(
            SummaryId::new(Uuid::new_v4().to_string()),
            &source_ids,
            &messages,
            now,
        );
        self.team
            .summaries
            .insert(summary.id.clone(), summary.clone());
        self.audit(
            agent_id,
            AuditAction::SummaryCreated,
            summary.id.0.clone(),
            "inbox summary created".to_string(),
            now,
        );
        self.touch(now);
        self.save()?;
        Ok(Some(summary))
    }

    pub fn scheduler_decision(
        &mut self,
        estimates: &[CostEstimate],
        now: i64,
    ) -> Result<SchedulerDecision, TeamError> {
        update_ready_tasks(&mut self.team.tasks);
        let decision = scheduler::scheduler_decision(&self.team, estimates, now);
        if decision.paused {
            self.team.budget.paused = true;
        }
        if decision.budget_warning.is_some() {
            self.team.budget.warned_at_80_percent = true;
        }
        self.touch(now);
        self.save()?;
        Ok(decision)
    }

    pub fn sleep_idle_agents(&mut self, now: i64) -> Result<Vec<AgentId>, TeamError> {
        let mut slept = Vec::new();
        for (agent_id, agent) in &mut self.team.agents {
            if agent.role == AgentRole::Lead {
                continue;
            }
            if agent.status == AgentStatus::Idle
                && now.saturating_sub(agent.last_active_at)
                    >= self.team.config.default_idle_timeout_secs as i64
            {
                agent.status = AgentStatus::Sleeping;
                slept.push(agent_id.clone());
            }
        }
        for agent_id in &slept {
            self.audit(
                agent_id,
                AuditAction::AgentSlept,
                agent_id.0.clone(),
                "agent slept after idle timeout".to_string(),
                now,
            );
        }
        self.touch(now);
        self.save()?;
        Ok(slept)
    }

    pub fn resume_agent(&mut self, agent_id: &AgentId, now: i64) -> Result<(), TeamError> {
        self.ensure_agent(agent_id)?;
        if let Some(agent) = self.team.agents.get_mut(agent_id) {
            agent.status = AgentStatus::Idle;
            agent.last_active_at = now;
        }
        self.audit(
            agent_id,
            AuditAction::AgentResumed,
            agent_id.0.clone(),
            "agent resumed".to_string(),
            now,
        );
        self.touch(now);
        self.save()
    }

    pub fn update_worker_runtime(
        &mut self,
        actor: &AgentId,
        agent_id: &AgentId,
        worker: WorkerRuntimeInfo,
        now: i64,
    ) -> Result<(), TeamError> {
        self.ensure_agent(actor)?;
        let agent = self
            .team
            .agents
            .get_mut(agent_id)
            .ok_or_else(|| TeamError::AgentNotFound {
                agent_id: agent_id.0.clone(),
            })?;
        agent.worker = Some(worker);
        agent.last_active_at = now;
        self.touch(now);
        self.save()
    }

    pub fn set_agent_status(
        &mut self,
        agent_id: &AgentId,
        status: AgentStatus,
        active_task: Option<TaskId>,
        now: i64,
    ) -> Result<(), TeamError> {
        let agent = self
            .team
            .agents
            .get_mut(agent_id)
            .ok_or_else(|| TeamError::AgentNotFound {
                agent_id: agent_id.0.clone(),
            })?;
        agent.status = status;
        agent.active_task = active_task;
        agent.last_active_at = now;
        self.touch(now);
        self.save()
    }

    pub fn append_event(
        &mut self,
        actor: &AgentId,
        details: String,
        now: i64,
    ) -> Result<(), TeamError> {
        self.ensure_agent(actor)?;
        self.audit(
            actor,
            AuditAction::TaskUpdated,
            self.team.id.0.clone(),
            details,
            now,
        );
        self.touch(now);
        self.save()
    }

    pub async fn run_ready_tasks(
        &mut self,
        runner: Arc<dyn TeammateRunner>,
        now: i64,
    ) -> Result<Vec<TeammateRunResult>, TeamError> {
        // Route any pending messages so members see communications before starting.
        self.route_pending_messages(/*max_attempts*/ 3, now)?;
        let assignments = self.dispatch_ready_tasks(now)?;
        let team_id = self.team.id.clone();
        let requests = assignments
            .into_iter()
            .map(|(agent_id, task)| TeammateRunRequest {
                team_id: team_id.clone(),
                agent_id,
                task,
            });
        let results = futures::future::join_all(requests.map(|request| {
            let runner = Arc::clone(&runner);
            async move { runner.run(request).await }
        }))
        .await;
        let mut completed = Vec::new();
        for result in results {
            let result = result?;
            if result.success {
                self.complete_task(
                    &result.agent_id,
                    &result.task_id,
                    result.tokens_used,
                    result.completed_at,
                )?;
            } else {
                self.requeue_task(&result.agent_id, &result.task_id, result.completed_at)?;
            }
            completed.push(result);
        }
        Ok(completed)
    }

    pub fn request_shutdown(&mut self, lead_id: &AgentId, now: i64) -> Result<(), TeamError> {
        self.ensure_lead(lead_id)?;
        self.team.lifecycle = TeamLifecycle::ShuttingDown;
        let recipients = self
            .team
            .agents
            .keys()
            .filter(|agent_id| *agent_id != lead_id)
            .cloned()
            .collect::<Vec<_>>();
        for agent_id in recipients {
            self.enqueue_message(
                lead_id,
                MessageRecipient::Agent(agent_id),
                MessageType::Shutdown,
                MessagePayload::text("team shutdown requested"),
                true,
                now,
            )?;
        }
        for agent in self.team.agents.values_mut() {
            agent.status = AgentStatus::ShuttingDown;
        }
        self.audit(
            lead_id,
            AuditAction::ShutdownRequested,
            self.team.id.0.clone(),
            "shutdown requested".to_string(),
            now,
        );
        self.touch(now);
        self.save()
    }

    pub fn mark_stopped(&mut self, lead_id: &AgentId, now: i64) -> Result<(), TeamError> {
        self.ensure_lead(lead_id)?;
        self.team.lifecycle = TeamLifecycle::Stopped;
        for agent in self.team.agents.values_mut() {
            agent.status = AgentStatus::Stopped;
        }
        self.touch(now);
        self.save()
    }

    pub fn cleanup(mut self, lead_id: &AgentId, now: i64) -> Result<(), TeamError> {
        self.ensure_lead(lead_id)?;
        self.team.lifecycle = TeamLifecycle::CleanedUp;
        self.audit(
            lead_id,
            AuditAction::CleanupCompleted,
            self.team.id.0.clone(),
            "team cleaned up".to_string(),
            now,
        );
        self.touch(now);
        self.save()?;
        self.store.delete_team(&self.team.id)
    }

    fn dispatch_ready_tasks(&mut self, now: i64) -> Result<Vec<(AgentId, Task)>, TeamError> {
        update_ready_tasks(&mut self.team.tasks);
        let idle_agents = self
            .team
            .agents
            .iter()
            .filter(|(_, agent)| agent.role != AgentRole::Lead && agent.status == AgentStatus::Idle)
            .map(|(agent_id, agent)| (agent_id.clone(), agent.role))
            .collect::<Vec<_>>();
        let ready_tasks = self
            .team
            .tasks
            .values()
            .filter(|task| {
                task.status == TaskStatus::Ready
                    && task.assignee.is_none()
                    && (!task.requires_plan_approval || task.approved_plan.is_some())
            })
            .cloned()
            .collect::<Vec<_>>();

        let mut assignments = Vec::new();
        let mut used_tasks = BTreeSet::new();
        for (agent_id, role) in idle_agents {
            let Some(task) = ready_tasks
                .iter()
                .find(|task| {
                    !used_tasks.contains(&task.id)
                        && task
                            .role_hint
                            .map(|role_hint| role_hint == role)
                            .unwrap_or(true)
                })
                .cloned()
            else {
                continue;
            };
            used_tasks.insert(task.id.clone());
            self.claim_task(&agent_id, &task.id, now)?;
            assignments.push((agent_id, task));
        }
        Ok(assignments)
    }

    fn requeue_task(
        &mut self,
        agent_id: &AgentId,
        task_id: &TaskId,
        now: i64,
    ) -> Result<(), TeamError> {
        let task = self
            .team
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| TeamError::TaskNotFound {
                task_id: task_id.0.clone(),
            })?;
        task.status = TaskStatus::Ready;
        task.assignee = None;
        task.claim = None;
        task.version = task.version.saturating_add(1);
        task.updated_at = now;
        if let Some(agent) = self.team.agents.get_mut(agent_id) {
            agent.status = AgentStatus::Idle;
            agent.active_task = None;
            agent.last_active_at = now;
        }
        self.touch(now);
        self.save()
    }

    fn enqueue_message(
        &mut self,
        from: &AgentId,
        to: MessageRecipient,
        message_type: MessageType,
        payload: MessagePayload,
        ack_required: bool,
        now: i64,
    ) -> Result<MessageId, TeamError> {
        validate_recipient(&self.team, &to)?;
        let id = MessageId::new(Uuid::new_v4().to_string());
        let message = crate::model::TeamMessage {
            id: id.clone(),
            from: from.clone(),
            to,
            message_type,
            timestamp: now,
            payload,
            attempts: 0,
            ack_required,
        };
        let sequence = next_sequence(&mut self.team.mailbox);
        self.team.mailbox.messages.insert(
            id.clone(),
            StoredMessage {
                message,
                state: MessageState::Pending,
                sequence,
                inbox_owner: None,
            },
        );
        self.team
            .mailbox
            .outboxes
            .entry(from.clone())
            .or_default()
            .pending
            .push(id.clone());
        self.audit(
            from,
            AuditAction::MessageSent,
            id.0.clone(),
            "message enqueued".to_string(),
            now,
        );
        Ok(id)
    }

    fn deliver_message(
        &mut self,
        owner: &AgentId,
        message_id: &MessageId,
        max_attempts: u32,
        now: i64,
    ) -> Result<bool, TeamError> {
        let stored = self
            .team
            .mailbox
            .messages
            .get(message_id)
            .cloned()
            .ok_or_else(|| TeamError::MessageNotFound {
                message_id: message_id.0.clone(),
            })?;
        let recipients = match &stored.message.to {
            MessageRecipient::Agent(agent_id) => vec![agent_id.clone()],
            MessageRecipient::Broadcast => self
                .team
                .agents
                .keys()
                .filter(|agent_id| *agent_id != owner)
                .cloned()
                .collect::<Vec<_>>(),
        };
        if recipients.is_empty() {
            self.mark_outbox_failed(owner, message_id, max_attempts)?;
            return Ok(false);
        }
        match stored.message.to {
            MessageRecipient::Agent(_) => {
                self.deliver_existing_message(message_id, &recipients[0], now)?;
            }
            MessageRecipient::Broadcast => {
                for recipient in recipients {
                    self.deliver_broadcast_copy(&stored, &recipient, now)?;
                }
            }
        }
        if let Some(outbox) = self.team.mailbox.outboxes.get_mut(owner) {
            remove_id(&mut outbox.pending, message_id);
            push_unique(&mut outbox.sent, message_id.clone());
        }
        if let Some(stored) = self.team.mailbox.messages.get_mut(message_id) {
            stored.state = MessageState::Sent;
        }
        self.audit(
            owner,
            AuditAction::MessageDelivered,
            message_id.0.clone(),
            "message delivered".to_string(),
            now,
        );
        Ok(true)
    }

    fn deliver_existing_message(
        &mut self,
        message_id: &MessageId,
        recipient: &AgentId,
        now: i64,
    ) -> Result<(), TeamError> {
        let inbox = self
            .team
            .mailbox
            .inboxes
            .get_mut(recipient)
            .ok_or_else(|| TeamError::AgentNotFound {
                agent_id: recipient.0.clone(),
            })?;
        push_unique(&mut inbox.unread, message_id.clone());
        let stored = self
            .team
            .mailbox
            .messages
            .get_mut(message_id)
            .ok_or_else(|| TeamError::MessageNotFound {
                message_id: message_id.0.clone(),
            })?;
        stored.state = MessageState::Unread;
        stored.inbox_owner = Some(recipient.clone());
        stored.message.attempts += 1;
        self.audit(
            recipient,
            AuditAction::MessageDelivered,
            message_id.0.clone(),
            "message delivered to inbox".to_string(),
            now,
        );
        Ok(())
    }

    fn deliver_broadcast_copy(
        &mut self,
        source: &StoredMessage,
        recipient: &AgentId,
        now: i64,
    ) -> Result<(), TeamError> {
        let id = MessageId::new(Uuid::new_v4().to_string());
        let mut message = source.message.clone();
        message.id = id.clone();
        message.to = MessageRecipient::Agent(recipient.clone());
        message.attempts += 1;
        let sequence = next_sequence(&mut self.team.mailbox);
        self.team.mailbox.messages.insert(
            id.clone(),
            StoredMessage {
                message,
                state: MessageState::Unread,
                sequence,
                inbox_owner: Some(recipient.clone()),
            },
        );
        let inbox = self
            .team
            .mailbox
            .inboxes
            .get_mut(recipient)
            .ok_or_else(|| TeamError::AgentNotFound {
                agent_id: recipient.0.clone(),
            })?;
        inbox.unread.push(id.clone());
        self.audit(
            recipient,
            AuditAction::MessageDelivered,
            id.0,
            "broadcast delivered to inbox".to_string(),
            now,
        );
        Ok(())
    }

    fn mark_outbox_failed(
        &mut self,
        owner: &AgentId,
        message_id: &MessageId,
        max_attempts: u32,
    ) -> Result<(), TeamError> {
        let stored = self
            .team
            .mailbox
            .messages
            .get_mut(message_id)
            .ok_or_else(|| TeamError::MessageNotFound {
                message_id: message_id.0.clone(),
            })?;
        stored.message.attempts += 1;
        if stored.message.attempts >= max_attempts {
            stored.state = MessageState::Failed;
            if let Some(outbox) = self.team.mailbox.outboxes.get_mut(owner) {
                remove_id(&mut outbox.pending, message_id);
                push_unique(&mut outbox.failed, message_id.clone());
            }
        }
        Ok(())
    }

    fn record_token_usage(&mut self, tokens: u64, now: i64) -> Result<(), TeamError> {
        self.team.budget.consumed_tokens = self.team.budget.consumed_tokens.saturating_add(tokens);
        if let Some(limit) = self.team.budget.token_limit {
            if self.team.budget.consumed_tokens > limit {
                self.team.budget.paused = true;
                return Err(TeamError::BudgetExceeded {
                    consumed_tokens: self.team.budget.consumed_tokens,
                    token_limit: limit,
                });
            }
            if self.team.budget.consumed_tokens.saturating_mul(100) >= limit.saturating_mul(80) {
                self.team.budget.warned_at_80_percent = true;
            }
        }
        self.audit(
            &self.team.lead_id.clone(),
            AuditAction::BudgetUpdated,
            self.team.id.0.clone(),
            format!("consumed_tokens={}", self.team.budget.consumed_tokens),
            now,
        );
        Ok(())
    }

    fn ensure_active(&self) -> Result<(), TeamError> {
        if self.team.lifecycle != TeamLifecycle::Active {
            return Err(TeamError::TeamNotActive {
                team_id: self.team.id.0.clone(),
            });
        }
        Ok(())
    }

    fn ensure_agent(&self, agent_id: &AgentId) -> Result<(), TeamError> {
        if !self.team.agents.contains_key(agent_id) {
            return Err(TeamError::AgentNotFound {
                agent_id: agent_id.0.clone(),
            });
        }
        Ok(())
    }

    fn ensure_lead(&self, agent_id: &AgentId) -> Result<(), TeamError> {
        self.ensure_agent(agent_id)?;
        if &self.team.lead_id != agent_id {
            return Err(TeamError::NotTeamLead {
                agent_id: agent_id.0.clone(),
            });
        }
        Ok(())
    }

    fn audit(
        &mut self,
        actor: &AgentId,
        action: AuditAction,
        target: String,
        details: String,
        now: i64,
    ) {
        self.team.audit_log.push(AuditEvent {
            timestamp: now,
            actor: actor.clone(),
            action,
            target,
            details,
        });
    }

    fn touch(&mut self, now: i64) {
        self.team.updated_at = now;
    }

    fn save(&self) -> Result<(), TeamError> {
        self.store.save_team(&self.team)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanImpact {
    pub affects_database: bool,
    pub affects_api_compatibility: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanReviewInput {
    pub approved: bool,
    pub comments: String,
    pub requires_tests: bool,
    pub database_impact: bool,
    pub api_compatibility_impact: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskUpdate {
    pub title: Option<String>,
    pub description: Option<String>,
    pub dependencies: Option<Vec<TaskId>>,
    pub priority: Option<crate::TaskPriority>,
    pub role_hint: Option<Option<AgentRole>>,
    pub requires_plan_approval: Option<bool>,
}

const DEFAULT_CLAIM_LEASE_SECS: i64 = 15 * 60;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimedTask {
    pub task: Task,
    pub claim_token: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalTaskData {
    pub result: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskStatusTransition<'a> {
    pub agent_id: &'a AgentId,
    pub task_id: &'a TaskId,
    pub from: TaskStatus,
    pub to: TaskStatus,
    pub claim_token: &'a str,
    pub terminal: TerminalTaskData,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReclaimTaskOutcome {
    pub task: Task,
    pub reclaimed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamMessageSnapshot {
    pub id: MessageId,
    pub message: crate::model::TeamMessage,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeammateRunRequest {
    pub team_id: TeamId,
    pub agent_id: AgentId,
    pub task: Task,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeammateRunResult {
    pub agent_id: AgentId,
    pub task_id: TaskId,
    pub success: bool,
    pub output: String,
    pub tokens_used: u64,
    pub completed_at: i64,
}

pub type TeammateRunnerFuture<'a> =
    Pin<Box<dyn Future<Output = Result<TeammateRunResult, TeamError>> + Send + 'a>>;

pub trait TeammateRunner: Send + Sync {
    fn run<'a>(&'a self, request: TeammateRunRequest) -> TeammateRunnerFuture<'a>;
}

fn insert_agent(
    agents: &mut std::collections::BTreeMap<AgentId, TeamAgent>,
    mailbox: &mut Mailbox,
    spec: AgentSpec,
    fallback_role: AgentRole,
    now: i64,
    shared_context_keys: &[String],
) {
    let role = if spec.role == AgentRole::Custom {
        fallback_role
    } else {
        spec.role
    };
    mailbox.inboxes.insert(spec.id.clone(), Inbox::default());
    mailbox.outboxes.insert(spec.id.clone(), Outbox::default());
    agents.insert(
        spec.id.clone(),
        TeamAgent {
            id: spec.id,
            display_name: spec.display_name,
            role,
            status: AgentStatus::Idle,
            active_task: None,
            context: AgentContext::isolated(shared_context_keys.to_vec()),
            worker: spec.worker,
            last_active_at: now,
        },
    );
}

fn id_role_hint(agent_id: &AgentId) -> AgentRole {
    let id = agent_id.0.to_ascii_lowercase();
    if id.contains("architect") {
        AgentRole::Architect
    } else if id.contains("test") {
        AgentRole::Tester
    } else if id.contains("review") {
        AgentRole::Reviewer
    } else if id.contains("security") {
        AgentRole::Security
    } else if id.contains("front") {
        AgentRole::Frontend
    } else if id.contains("back") {
        AgentRole::Backend
    } else {
        AgentRole::Developer
    }
}

fn task_from_new(new_task: NewTask, now: i64) -> Task {
    Task {
        id: new_task.id,
        title: new_task.title,
        description: new_task.description,
        status: TaskStatus::Pending,
        assignee: None,
        result: None,
        error: None,
        claim: None,
        version: 1,
        dependencies: new_task.dependencies,
        priority: new_task.priority,
        role_hint: new_task.role_hint,
        estimated_effort: new_task.estimated_effort,
        requires_plan_approval: new_task.requires_plan_approval,
        approved_plan: None,
        created_at: now,
        updated_at: now,
        completed_at: None,
    }
}

fn new_task_from_task(task: &Task) -> NewTask {
    NewTask {
        id: task.id.clone(),
        title: task.title.clone(),
        description: task.description.clone(),
        dependencies: task.dependencies.clone(),
        priority: task.priority,
        role_hint: task.role_hint,
        estimated_effort: task.estimated_effort,
        requires_plan_approval: task.requires_plan_approval,
    }
}

fn validate_new_tasks(tasks: &[NewTask]) -> Result<(), TeamError> {
    let graph = crate::model::task_dependency_set(tasks);
    for task in tasks {
        for dependency in &task.dependencies {
            if !graph.contains_key(dependency) {
                return Err(TeamError::TaskNotFound {
                    task_id: dependency.0.clone(),
                });
            }
        }
    }
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for task_id in graph.keys() {
        visit_task(task_id, &graph, &mut visiting, &mut visited)?;
    }
    Ok(())
}

fn validate_team_id(team_id: &TeamId) -> Result<(), TeamError> {
    let value = &team_id.0;
    let valid = !value.is_empty()
        && value.len() <= 30
        && value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        && value
            .chars()
            .next()
            .map(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
            .unwrap_or(false);
    if valid {
        Ok(())
    } else {
        Err(TeamError::InvalidOperation(format!(
            "team name {value:?} must match [a-z0-9][a-z0-9-]{{0,29}}"
        )))
    }
}

fn visit_task(
    task_id: &TaskId,
    graph: &std::collections::BTreeMap<TaskId, BTreeSet<TaskId>>,
    visiting: &mut BTreeSet<TaskId>,
    visited: &mut BTreeSet<TaskId>,
) -> Result<(), TeamError> {
    if visited.contains(task_id) {
        return Ok(());
    }
    if !visiting.insert(task_id.clone()) {
        return Err(TeamError::DependencyCycle {
            task_id: task_id.0.clone(),
        });
    }
    if let Some(dependencies) = graph.get(task_id) {
        for dependency in dependencies {
            visit_task(dependency, graph, visiting, visited)?;
        }
    }
    visiting.remove(task_id);
    visited.insert(task_id.clone());
    Ok(())
}

fn update_ready_tasks(tasks: &mut std::collections::BTreeMap<TaskId, Task>) {
    let completed = tasks
        .iter()
        .filter(|(_, task)| task.status == TaskStatus::Completed)
        .map(|(task_id, _)| task_id.clone())
        .collect::<BTreeSet<_>>();
    for task in tasks.values_mut() {
        if matches!(
            task.status,
            TaskStatus::Pending | TaskStatus::Blocked | TaskStatus::Ready
        ) {
            task.status = if task
                .dependencies
                .iter()
                .all(|dependency| completed.contains(dependency))
            {
                TaskStatus::Ready
            } else {
                TaskStatus::Blocked
            };
        }
    }
}

fn can_transition_task_status(from: TaskStatus, to: TaskStatus) -> bool {
    matches!(
        (from, to),
        (TaskStatus::InProgress, TaskStatus::Completed)
            | (TaskStatus::InProgress, TaskStatus::Failed)
    )
}

fn task_status_key(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Pending => "pending",
        TaskStatus::Blocked => "blocked",
        TaskStatus::Ready => "ready",
        TaskStatus::InProgress => "in_progress",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
    }
}

fn dependencies_completed(
    tasks: &std::collections::BTreeMap<TaskId, Task>,
    task_id: &TaskId,
) -> bool {
    tasks
        .get(task_id)
        .map(|task| {
            task.dependencies.iter().all(|dependency| {
                tasks
                    .get(dependency)
                    .map(|dependency_task| dependency_task.status == TaskStatus::Completed)
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn incomplete_dependencies(
    tasks: &std::collections::BTreeMap<TaskId, Task>,
    task_id: &TaskId,
) -> Vec<String> {
    tasks
        .get(task_id)
        .map(|task| {
            task.dependencies
                .iter()
                .filter(|dependency| {
                    tasks
                        .get(dependency)
                        .map(|dependency_task| dependency_task.status != TaskStatus::Completed)
                        .unwrap_or(true)
                })
                .map(|dependency| dependency.0.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn validate_recipient(team: &Team, recipient: &MessageRecipient) -> Result<(), TeamError> {
    if let MessageRecipient::Agent(agent_id) = recipient
        && !team.agents.contains_key(agent_id)
    {
        return Err(TeamError::AgentNotFound {
            agent_id: agent_id.0.clone(),
        });
    }
    Ok(())
}

fn next_sequence(mailbox: &mut Mailbox) -> u64 {
    let sequence = mailbox.next_sequence;
    mailbox.next_sequence += 1;
    sequence
}

fn remove_id(ids: &mut Vec<MessageId>, id: &MessageId) {
    ids.retain(|candidate| candidate != id);
}

fn push_unique(ids: &mut Vec<MessageId>, id: MessageId) {
    if !ids.contains(&id) {
        ids.push(id);
    }
}
