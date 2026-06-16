//! Workflow run progress history cell.

use super::*;
use codex_app_server_protocol::WorkflowAgentProgress;
use codex_app_server_protocol::WorkflowAgentStatus;
use codex_app_server_protocol::WorkflowPhaseProgress;
use codex_app_server_protocol::WorkflowRunStatus;
use codex_app_server_protocol::WorkflowRunUpdatedNotification;

#[derive(Debug)]
pub(crate) struct WorkflowRunCell {
    notification: WorkflowRunUpdatedNotification,
    start_time: Instant,
    animations_enabled: bool,
}

impl WorkflowRunCell {
    pub(crate) fn new(
        notification: WorkflowRunUpdatedNotification,
        animations_enabled: bool,
    ) -> Self {
        Self {
            notification,
            start_time: Instant::now(),
            animations_enabled,
        }
    }

    pub(crate) fn call_id(&self) -> &str {
        &self.notification.call_id
    }

    pub(crate) fn update(&mut self, notification: WorkflowRunUpdatedNotification) {
        self.notification = notification;
    }

    pub(crate) fn mark_failed(&mut self) {
        self.notification.status = WorkflowRunStatus::Failed;
    }

    fn is_running(&self) -> bool {
        matches!(self.notification.status, WorkflowRunStatus::Running)
    }
}

impl HistoryCell for WorkflowRunCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        workflow_run_lines(
            &self.notification,
            self.start_time,
            self.animations_enabled,
            width,
        )
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        plain_lines(workflow_run_lines(
            &self.notification,
            self.start_time,
            /*animations_enabled*/ false,
            u16::MAX,
        ))
    }

    fn transcript_animation_tick(&self) -> Option<u64> {
        if !self.animations_enabled || !self.is_running() {
            return None;
        }
        Some((self.start_time.elapsed().as_millis() / 50) as u64)
    }
}

pub(crate) fn new_workflow_run_cell(
    notification: WorkflowRunUpdatedNotification,
    animations_enabled: bool,
) -> WorkflowRunCell {
    WorkflowRunCell::new(notification, animations_enabled)
}

fn workflow_run_lines(
    notification: &WorkflowRunUpdatedNotification,
    start_time: Instant,
    animations_enabled: bool,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(workflow_header_line(
        notification,
        start_time,
        animations_enabled,
    ));
    lines.push(workflow_counts_line(notification));

    for phase in &notification.phases {
        lines.push(phase_line(phase));
        for agent in agents_for_phase(&notification.agents, Some(phase.title.as_str())) {
            push_agent_lines(&mut lines, agent, width);
        }
    }

    let phase_titles = notification
        .phases
        .iter()
        .map(|phase| phase.title.as_str())
        .collect::<Vec<_>>();
    let ungrouped_agents = notification
        .agents
        .iter()
        .filter(|agent| {
            agent
                .phase
                .as_deref()
                .is_none_or(|phase| !phase_titles.contains(&phase))
        })
        .collect::<Vec<_>>();
    if !ungrouped_agents.is_empty() {
        lines.push(Line::from(vec![
            "  ├ ".dim(),
            "Phase: ungrouped".bold(),
            " ".into(),
            agent_count_summary(ungrouped_agents.len() as u32, &ungrouped_agents).dim(),
        ]));
        for agent in ungrouped_agents {
            push_agent_lines(&mut lines, agent, width);
        }
    }

    if !notification.logs.is_empty() {
        lines.push(Line::from(vec!["  ├ ".dim(), "Logs".bold()]));
        for log in notification.logs.iter().rev().take(3).rev() {
            lines.push(Line::from(vec![
                "  │   ".dim(),
                truncate_for_width(log, width, 10).dim(),
            ]));
        }
    }

    lines
}

fn workflow_header_line(
    notification: &WorkflowRunUpdatedNotification,
    start_time: Instant,
    animations_enabled: bool,
) -> Line<'static> {
    let bullet = match notification.status {
        WorkflowRunStatus::Running => activity_indicator(
            Some(start_time),
            MotionMode::from_animations_enabled(animations_enabled),
            ReducedMotionIndicator::StaticBullet,
        )
        .unwrap_or_else(|| "•".dim()),
        WorkflowRunStatus::Completed => "•".green().bold(),
        WorkflowRunStatus::Failed => "•".red().bold(),
    };
    Line::from(vec![
        bullet,
        " ".into(),
        workflow_status_label(notification.status).bold(),
        " workflow ".bold(),
        notification.workflow_name.clone().cyan().bold(),
    ])
}

fn workflow_counts_line(notification: &WorkflowRunUpdatedNotification) -> Line<'static> {
    Line::from(vec![
        "  └ ".dim(),
        format!(
            "{} agent(s): {} running, {} done, {} failed",
            notification.agent_count,
            notification.running_agent_count,
            notification.completed_agent_count,
            notification.failed_agent_count
        )
        .dim(),
    ])
}

fn phase_line(phase: &WorkflowPhaseProgress) -> Line<'static> {
    Line::from(vec![
        "  ├ ".dim(),
        "Phase: ".bold(),
        phase.title.clone().bold(),
        " ".into(),
        format!(
            "({}/{} done, {} running, {} failed)",
            phase.completed_agent_count,
            phase.agent_count,
            phase.running_agent_count,
            phase.failed_agent_count
        )
        .dim(),
    ])
}

fn push_agent_lines(lines: &mut Vec<Line<'static>>, agent: &WorkflowAgentProgress, width: u16) {
    let mut spans = vec![
        "  │   ".dim(),
        agent_status_label(agent.status),
        " ".into(),
        agent.label.clone().bold(),
    ];
    if let Some(agent_type) = agent.agent_type.as_ref().filter(|value| !value.is_empty()) {
        spans.push(" · ".dim());
        spans.push(agent_type.clone().dim());
    }
    if let Some(model) = agent.model.as_ref().filter(|value| !value.is_empty()) {
        spans.push(" · ".dim());
        spans.push(model.clone().dim());
    }
    lines.push(Line::from(spans));

    if let Some(error) = agent.error.as_ref().filter(|value| !value.is_empty()) {
        lines.push(Line::from(vec![
            "  │     ".dim(),
            "error: ".red(),
            truncate_for_width(error, width, 12).red(),
        ]));
    } else if !agent.prompt.is_empty() {
        lines.push(Line::from(vec![
            "  │     ".dim(),
            truncate_for_width(&agent.prompt, width, 12).dim(),
        ]));
    }
}

fn agents_for_phase<'a>(
    agents: &'a [WorkflowAgentProgress],
    phase: Option<&str>,
) -> impl Iterator<Item = &'a WorkflowAgentProgress> {
    agents
        .iter()
        .filter(move |agent| agent.phase.as_deref() == phase)
}

fn agent_count_summary(count: u32, agents: &[&WorkflowAgentProgress]) -> String {
    let running = agents
        .iter()
        .filter(|agent| matches!(agent.status, WorkflowAgentStatus::Running))
        .count();
    let completed = agents
        .iter()
        .filter(|agent| matches!(agent.status, WorkflowAgentStatus::Completed))
        .count();
    let failed = agents
        .iter()
        .filter(|agent| matches!(agent.status, WorkflowAgentStatus::Failed))
        .count();
    format!("({completed}/{count} done, {running} running, {failed} failed)")
}

fn workflow_status_label(status: WorkflowRunStatus) -> &'static str {
    match status {
        WorkflowRunStatus::Running => "Running",
        WorkflowRunStatus::Completed => "Completed",
        WorkflowRunStatus::Failed => "Failed",
    }
}

fn agent_status_label(status: WorkflowAgentStatus) -> Span<'static> {
    match status {
        WorkflowAgentStatus::Running => "running".cyan(),
        WorkflowAgentStatus::Completed => "done".green(),
        WorkflowAgentStatus::Failed => "failed".red(),
    }
}

fn truncate_for_width(text: &str, width: u16, reserved: usize) -> String {
    let available = usize::from(width).saturating_sub(reserved).max(16);
    truncate_text(text, available.min(120))
}
