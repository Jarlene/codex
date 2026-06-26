use std::collections::BTreeSet;

use crate::model::AgentRole;
use crate::model::Effort;
use crate::model::NewTask;
use crate::model::TaskDecomposition;
use crate::model::TaskDecompositionRequest;
use crate::model::TaskId;
use crate::model::TaskPriority;

#[derive(Clone, Debug, Default)]
pub struct HeuristicTaskDecomposer;

impl HeuristicTaskDecomposer {
    pub fn decompose(&self, request: TaskDecompositionRequest) -> TaskDecomposition {
        let mut roles = request
            .preferred_roles
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        infer_roles(&request.objective, &mut roles);
        if roles.is_empty() {
            roles.insert(AgentRole::Architect);
            roles.insert(AgentRole::Developer);
            roles.insert(AgentRole::Tester);
            roles.insert(AgentRole::Reviewer);
        }
        if request.include_security {
            roles.insert(AgentRole::Security);
        }
        if request.include_review {
            roles.insert(AgentRole::Reviewer);
        }

        let mut tasks = Vec::new();
        if roles.contains(&AgentRole::Architect) {
            tasks.push(new_task(TaskDraft {
                id: "1",
                title: "Architecture and interface plan",
                description: "Define the implementation shape, module boundaries, compatibility risks, and test approach.",
                role: AgentRole::Architect,
                dependencies: Vec::new(),
                priority: TaskPriority::P0,
                estimated_effort: Effort::Medium,
                requires_plan_approval: true,
            }));
        }

        let mut database_id = None;
        if roles.contains(&AgentRole::Database) {
            database_id = Some(next_id(&tasks));
            tasks.push(new_task(TaskDraft {
                id: database_id.as_deref().unwrap_or("task-2"),
                title: "Database design",
                description: "Design the storage schema, migrations, and rollback considerations.",
                role: AgentRole::Database,
                dependencies: architect_dependency(&tasks),
                priority: TaskPriority::P0,
                estimated_effort: Effort::Medium,
                requires_plan_approval: true,
            }));
        }

        let backend_id = if roles.contains(&AgentRole::Backend)
            || roles.contains(&AgentRole::Developer)
            || roles.contains(&AgentRole::Custom)
        {
            let id = next_id(&tasks);
            let mut dependencies = architect_dependency(&tasks);
            if let Some(database_id) = &database_id {
                dependencies.push(TaskId::new(database_id.clone()));
            }
            tasks.push(new_task(TaskDraft {
                id: &id,
                title: "Backend implementation",
                description: "Implement the core runtime, domain logic, and integration API for the requested feature.",
                role: AgentRole::Backend,
                dependencies,
                priority: TaskPriority::P0,
                estimated_effort: Effort::Large,
                requires_plan_approval: true,
            }));
            Some(id)
        } else {
            None
        };

        let frontend_id = if roles.contains(&AgentRole::Frontend) {
            let id = next_id(&tasks);
            let dependencies = backend_id
                .as_ref()
                .map(|id| vec![TaskId::new(id.clone())])
                .unwrap_or_else(|| architect_dependency(&tasks));
            tasks.push(new_task(TaskDraft {
                id: &id,
                title: "Frontend implementation",
                description: "Build the user-facing workflow and connect it to the backend contract.",
                role: AgentRole::Frontend,
                dependencies,
                priority: TaskPriority::P1,
                estimated_effort: Effort::Large,
                requires_plan_approval: false,
            }));
            Some(id)
        } else {
            None
        };

        if roles.contains(&AgentRole::Security) {
            let id = next_id(&tasks);
            tasks.push(new_task(TaskDraft {
                id: &id,
                title: "Security review",
                description: "Review trust boundaries, permission inheritance, audit logging, and unsafe side effects.",
                role: AgentRole::Security,
                dependencies: backend_id
                    .as_ref()
                    .map(|id| vec![TaskId::new(id.clone())])
                    .unwrap_or_else(|| architect_dependency(&tasks)),
                priority: TaskPriority::P1,
                estimated_effort: Effort::Small,
                requires_plan_approval: false,
            }));
        }

        if roles.contains(&AgentRole::Performance) {
            let id = next_id(&tasks);
            tasks.push(new_task(TaskDraft {
                id: &id,
                title: "Performance review",
                description: "Estimate concurrency limits, scheduling overhead, and recovery cost.",
                role: AgentRole::Performance,
                dependencies: backend_id
                    .as_ref()
                    .map(|id| vec![TaskId::new(id.clone())])
                    .unwrap_or_else(|| architect_dependency(&tasks)),
                priority: TaskPriority::P2,
                estimated_effort: Effort::Small,
                requires_plan_approval: false,
            }));
        }

        if roles.contains(&AgentRole::Tester) {
            let id = next_id(&tasks);
            let dependencies = [backend_id, frontend_id]
                .into_iter()
                .flatten()
                .map(TaskId::new)
                .collect::<Vec<_>>();
            tasks.push(new_task(TaskDraft {
                id: &id,
                title: "Test coverage",
                description: "Add regression coverage for task scheduling, messaging, approvals, shutdown, and recovery.",
                role: AgentRole::Tester,
                dependencies: if dependencies.is_empty() {
                    architect_dependency(&tasks)
                } else {
                    dependencies
                },
                priority: TaskPriority::P1,
                estimated_effort: Effort::Medium,
                requires_plan_approval: false,
            }));
        }

        if roles.contains(&AgentRole::Reviewer) {
            let id = next_id(&tasks);
            let dependencies = tasks.iter().map(|task| task.id.clone()).collect();
            tasks.push(new_task(TaskDraft {
                id: &id,
                title: "Final code review",
                description: "Review correctness, maintainability, compatibility, and missing test risk before completion.",
                role: AgentRole::Reviewer,
                dependencies,
                priority: TaskPriority::P1,
                estimated_effort: Effort::Small,
                requires_plan_approval: false,
            }));
        }

        let warnings = if tasks.len() > 20 {
            vec!["decomposition exceeds the default maximum team size".to_string()]
        } else {
            Vec::new()
        };
        TaskDecomposition { tasks, warnings }
    }
}

fn infer_roles(objective: &str, roles: &mut BTreeSet<AgentRole>) {
    let lower = objective.to_ascii_lowercase();
    for (needle, role) in [
        ("architecture", AgentRole::Architect),
        ("design", AgentRole::Architect),
        ("frontend", AgentRole::Frontend),
        ("ui", AgentRole::Frontend),
        ("backend", AgentRole::Backend),
        ("api", AgentRole::Backend),
        ("database", AgentRole::Database),
        ("schema", AgentRole::Database),
        ("test", AgentRole::Tester),
        ("security", AgentRole::Security),
        ("performance", AgentRole::Performance),
        ("review", AgentRole::Reviewer),
        ("research", AgentRole::Researcher),
        ("docs", AgentRole::Writer),
    ] {
        if lower.contains(needle) {
            roles.insert(role);
        }
    }
    if lower.contains("前端") {
        roles.insert(AgentRole::Frontend);
    }
    if lower.contains("后端") || lower.contains("接口") {
        roles.insert(AgentRole::Backend);
    }
    if lower.contains("数据库") {
        roles.insert(AgentRole::Database);
    }
    if lower.contains("测试") {
        roles.insert(AgentRole::Tester);
    }
    if lower.contains("安全") {
        roles.insert(AgentRole::Security);
    }
}

fn architect_dependency(tasks: &[NewTask]) -> Vec<TaskId> {
    tasks
        .iter()
        .find(|task| task.role_hint == Some(AgentRole::Architect))
        .map(|task| vec![task.id.clone()])
        .unwrap_or_default()
}

fn next_id(tasks: &[NewTask]) -> String {
    (tasks.len() + 1).to_string()
}

struct TaskDraft<'a> {
    id: &'a str,
    title: &'a str,
    description: &'a str,
    role: AgentRole,
    dependencies: Vec<TaskId>,
    priority: TaskPriority,
    estimated_effort: Effort,
    requires_plan_approval: bool,
}

fn new_task(draft: TaskDraft<'_>) -> NewTask {
    NewTask {
        id: TaskId::new(draft.id),
        title: draft.title.to_string(),
        description: draft.description.to_string(),
        dependencies: draft.dependencies,
        priority: draft.priority,
        role_hint: Some(draft.role),
        estimated_effort: draft.estimated_effort,
        requires_plan_approval: draft.requires_plan_approval,
    }
}
