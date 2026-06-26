use std::collections::BTreeMap;
use std::collections::BTreeSet;

use crate::model::AgentStatus;
use crate::model::CostEstimate;
use crate::model::Effort;
use crate::model::NewTask;
use crate::model::SchedulerDecision;
use crate::model::TaskId;
use crate::model::TaskPriority;
use crate::model::TaskStatus;
use crate::model::Team;
use crate::model::TeamSizeRecommendation;
use crate::model::TeamSizeRecommendationInput;
use crate::model::TokenRange;
use crate::model::role_key;

pub fn recommend_team_size(input: TeamSizeRecommendationInput) -> TeamSizeRecommendation {
    let estimated_parallelism = max_parallelism(&input.tasks);
    let mut role_counts = BTreeMap::new();
    for task in &input.tasks {
        if let Some(role) = task.role_hint {
            *role_counts.entry(role_key(role)).or_insert(0) += 1;
        }
    }
    for role in input.high_risk_roles {
        role_counts
            .entry(role_key(role))
            .and_modify(|count| *count = (*count).max(2))
            .or_insert(2);
    }

    let coverage_count = role_counts.values().sum::<usize>();
    let teammate_count = coverage_count.max(estimated_parallelism).max(1);
    let token_range = estimate_token_range(&input.tasks);
    let downgrade = input.budget_tokens.and_then(|budget| {
        if token_range.min > budget {
            Some(format!(
                "estimated minimum {} tokens exceeds budget {}; use fewer reusable agents and serialize low-risk tasks",
                token_range.min, budget
            ))
        } else {
            None
        }
    });
    let total_agents = if downgrade.is_some() {
        teammate_count.min(2) + 1
    } else {
        teammate_count + 1
    };

    TeamSizeRecommendation {
        total_agents,
        lead_count: 1,
        role_counts,
        estimated_parallelism,
        estimated_token_range: token_range,
        downgrade,
    }
}

pub(crate) fn scheduler_decision(
    team: &Team,
    estimates: &[CostEstimate],
    now: i64,
) -> SchedulerDecision {
    let estimate_by_task = estimates
        .iter()
        .map(|estimate| (estimate.task_id.clone(), estimate.estimated_tokens))
        .collect::<BTreeMap<_, _>>();
    let mut budget_warning = None;
    let mut paused = false;
    if let Some(limit) = team.budget.token_limit {
        let projected = team.budget.consumed_tokens.saturating_add(
            estimates
                .iter()
                .map(|estimate| estimate.estimated_tokens)
                .sum::<u64>(),
        );
        if projected > limit {
            paused = true;
            budget_warning = Some(format!(
                "projected token use {projected} exceeds budget {limit}"
            ));
        } else if projected.saturating_mul(100) >= limit.saturating_mul(80) {
            budget_warning = Some(format!(
                "projected token use {projected} is at least 80% of budget {limit}"
            ));
        }
    }
    let runnable_tasks = if paused {
        Vec::new()
    } else {
        team.tasks
            .values()
            .filter(|task| {
                task.status == TaskStatus::Ready
                    && task.assignee.is_none()
                    && estimate_by_task
                        .get(&task.id)
                        .map(|estimate| {
                            team.budget
                                .token_limit
                                .map(|limit| {
                                    team.budget.consumed_tokens.saturating_add(*estimate) <= limit
                                })
                                .unwrap_or(true)
                        })
                        .unwrap_or(true)
            })
            .map(|task| task.id.clone())
            .collect()
    };
    let sleeping_agents = team
        .agents
        .iter()
        .filter(|(_, agent)| {
            agent.status == AgentStatus::Idle
                && now.saturating_sub(agent.last_active_at)
                    >= team.config.default_idle_timeout_secs as i64
        })
        .map(|(agent_id, _)| agent_id.clone())
        .collect();

    SchedulerDecision {
        runnable_tasks,
        sleeping_agents,
        budget_warning,
        paused,
    }
}

fn estimate_token_range(tasks: &[NewTask]) -> TokenRange {
    let mut min = 0;
    let mut max = 0;
    for task in tasks {
        let (task_min, task_max) = match task.estimated_effort {
            Effort::Small => (4_000, 12_000),
            Effort::Medium => (12_000, 35_000),
            Effort::Large => (35_000, 80_000),
        };
        let priority_multiplier = match task.priority {
            TaskPriority::P0 => 120,
            TaskPriority::P1 => 100,
            TaskPriority::P2 => 80,
            TaskPriority::P3 => 60,
        };
        min += task_min * priority_multiplier / 100;
        max += task_max * priority_multiplier / 100;
    }
    TokenRange { min, max }
}

fn max_parallelism(tasks: &[NewTask]) -> usize {
    if tasks.is_empty() {
        return 0;
    }
    let mut remaining = tasks
        .iter()
        .map(|task| (task.id.clone(), task.dependencies.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut completed = BTreeSet::new();
    let mut max_width = 0;
    while !remaining.is_empty() {
        let ready = remaining
            .iter()
            .filter(|(_, dependencies)| {
                dependencies
                    .iter()
                    .all(|dependency| completed.contains(dependency))
            })
            .map(|(task_id, _)| task_id.clone())
            .collect::<Vec<TaskId>>();
        if ready.is_empty() {
            return max_width.max(1);
        }
        max_width = max_width.max(ready.len());
        for task_id in ready {
            remaining.remove(&task_id);
            completed.insert(task_id);
        }
    }
    max_width
}
