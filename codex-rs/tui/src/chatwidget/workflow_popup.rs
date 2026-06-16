//! Interactive workflow progress popup for `ChatWidget`.

use super::*;
use codex_app_server_protocol::WorkflowAgentProgress;
use codex_app_server_protocol::WorkflowAgentStatus;
use codex_app_server_protocol::WorkflowPhaseProgress;

pub(super) const WORKFLOW_PHASES_VIEW_ID: &str = "workflow-phases";
pub(super) const WORKFLOW_AGENTS_VIEW_ID: &str = "workflow-agents";

impl ChatWidget {
    pub(crate) fn open_workflow_popup(&mut self) {
        let Some(notification) = self.latest_workflow_run.as_ref() else {
            self.add_info_message(
                "No workflow has run in this thread yet.".to_string(),
                Some(
                    "Run a workflow, then use /workflow to inspect phases and agents.".to_string(),
                ),
            );
            return;
        };

        self.workflow_popup_phase = None;
        self.bottom_pane
            .show_selection_view(workflow_phases_params(notification));
        self.request_redraw();
    }

    pub(crate) fn open_workflow_phase_agents(&mut self, phase_title: &str) {
        if phase_title.is_empty() {
            self.open_workflow_popup();
            return;
        }
        let Some(notification) = self.latest_workflow_run.as_ref() else {
            self.add_info_message(
                "No workflow has run in this thread yet.".to_string(),
                /*hint*/ None,
            );
            return;
        };

        self.workflow_popup_phase = Some(phase_title.to_string());
        self.bottom_pane
            .show_selection_view(workflow_agents_params(notification, phase_title));
        self.request_redraw();
    }

    pub(super) fn refresh_workflow_popup_if_open(&mut self) {
        let Some(notification) = self.latest_workflow_run.as_ref() else {
            return;
        };
        let Some(view_id) = self.bottom_pane.active_view_id() else {
            return;
        };

        match view_id {
            WORKFLOW_PHASES_VIEW_ID => {
                let phases = workflow_phase_rows(notification);
                let selected_phase = self
                    .bottom_pane
                    .selected_index_for_active_view(WORKFLOW_PHASES_VIEW_ID)
                    .and_then(|idx| phases.get(idx))
                    .map(|phase| phase.title.as_str());
                let params = workflow_phases_params_with_selected(notification, selected_phase);
                let _ = self
                    .bottom_pane
                    .replace_selection_view_if_active(WORKFLOW_PHASES_VIEW_ID, params);
            }
            WORKFLOW_AGENTS_VIEW_ID => {
                let Some(phase_title) = self.workflow_popup_phase.as_deref() else {
                    return;
                };
                let params = workflow_agents_params(notification, phase_title);
                let _ = self
                    .bottom_pane
                    .replace_selection_view_if_active(WORKFLOW_AGENTS_VIEW_ID, params);
            }
            _ => {}
        }
    }
}

fn workflow_phases_params(notification: &WorkflowRunUpdatedNotification) -> SelectionViewParams {
    workflow_phases_params_with_selected(notification, /*selected_phase*/ None)
}

fn workflow_phases_params_with_selected(
    notification: &WorkflowRunUpdatedNotification,
    selected_phase: Option<&str>,
) -> SelectionViewParams {
    let phases = workflow_phase_rows(notification);
    let initial_selected_idx =
        selected_phase.and_then(|selected| phases.iter().position(|phase| phase.title == selected));

    let items = phases.iter().map(workflow_phase_item).collect::<Vec<_>>();
    SelectionViewParams {
        view_id: Some(WORKFLOW_PHASES_VIEW_ID),
        title: Some(format!("Workflow: {}", notification.workflow_name)),
        subtitle: Some(format!(
            "{} · {} agent(s): {} running, {} done, {} failed",
            workflow_status_label(notification.status),
            notification.agent_count,
            notification.running_agent_count,
            notification.completed_agent_count,
            notification.failed_agent_count
        )),
        footer_hint: Some(Line::from("enter agents · esc close")),
        items,
        initial_selected_idx,
        ..Default::default()
    }
}

fn workflow_phase_item(phase: &WorkflowPhaseProgress) -> SelectionItem {
    let phase_title = phase.title.clone();
    let description = phase_count_description(phase);
    SelectionItem {
        name: phase.title.clone(),
        description: Some(description.clone()),
        selected_description: Some(format!("{description}. Press Enter to inspect agents.")),
        actions: vec![Box::new(move |tx| {
            tx.send(AppEvent::OpenWorkflowPhaseAgents {
                phase_title: phase_title.clone(),
            });
        })],
        dismiss_on_select: true,
        ..Default::default()
    }
}

fn workflow_agents_params(
    notification: &WorkflowRunUpdatedNotification,
    phase_title: &str,
) -> SelectionViewParams {
    let agents = agents_for_phase(&notification.agents, phase_title)
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let mut items = Vec::with_capacity(agents.len().saturating_add(1));
    items.push(SelectionItem {
        name: "Back to phases".to_string(),
        description: Some("Return to workflow phase list".to_string()),
        actions: vec![Box::new(|tx| {
            tx.send(AppEvent::OpenWorkflowPhaseAgents {
                phase_title: String::new(),
            });
        })],
        dismiss_on_select: true,
        ..Default::default()
    });

    if agents.is_empty() {
        items.push(SelectionItem {
            name: "No agents in this phase yet".to_string(),
            is_disabled: true,
            ..Default::default()
        });
    } else {
        items.extend(agents.iter().map(workflow_agent_item));
    }

    let phase_counts = notification
        .phases
        .iter()
        .find(|phase| phase.title == phase_title)
        .map(phase_count_description)
        .unwrap_or_else(|| phase_count_description_from_agents(&agents));

    SelectionViewParams {
        view_id: Some(WORKFLOW_AGENTS_VIEW_ID),
        title: Some(format!("Workflow: {}", notification.workflow_name)),
        subtitle: Some(format!("Phase: {phase_title} · {phase_counts}")),
        footer_hint: Some(Line::from("enter back · esc close")),
        items,
        ..Default::default()
    }
}

fn workflow_phase_rows(
    notification: &WorkflowRunUpdatedNotification,
) -> Vec<WorkflowPhaseProgress> {
    let mut phases = notification.phases.clone();
    let ungrouped_agents = agents_for_phase(&notification.agents, "ungrouped");
    if phases.is_empty() {
        phases.push(WorkflowPhaseProgress {
            title: "ungrouped".to_string(),
            agent_count: notification.agent_count,
            running_agent_count: notification.running_agent_count,
            completed_agent_count: notification.completed_agent_count,
            failed_agent_count: notification.failed_agent_count,
        });
    } else if !ungrouped_agents.is_empty() {
        phases.push(WorkflowPhaseProgress {
            title: "ungrouped".to_string(),
            agent_count: u32::try_from(ungrouped_agents.len()).unwrap_or(u32::MAX),
            running_agent_count: count_agents(&ungrouped_agents, WorkflowAgentStatus::Running),
            completed_agent_count: count_agents(&ungrouped_agents, WorkflowAgentStatus::Completed),
            failed_agent_count: count_agents(&ungrouped_agents, WorkflowAgentStatus::Failed),
        });
    }
    phases
}

fn count_agents(agents: &[&WorkflowAgentProgress], status: WorkflowAgentStatus) -> u32 {
    u32::try_from(agents.iter().filter(|agent| agent.status == status).count()).unwrap_or(u32::MAX)
}

fn workflow_agent_item(agent: &WorkflowAgentProgress) -> SelectionItem {
    SelectionItem {
        name: format!("{} {}", agent_status_label(agent.status), agent.label),
        description: Some(agent_description(agent)),
        selected_description: Some(agent_selected_description(agent)),
        search_value: Some(format!(
            "{} {} {} {}",
            agent.label,
            agent.prompt,
            agent.agent_type.as_deref().unwrap_or_default(),
            agent.model.as_deref().unwrap_or_default()
        )),
        ..Default::default()
    }
}

fn agents_for_phase<'a>(
    agents: &'a [WorkflowAgentProgress],
    phase_title: &str,
) -> Vec<&'a WorkflowAgentProgress> {
    if phase_title == "ungrouped" {
        agents
            .iter()
            .filter(|agent| agent.phase.as_deref().is_none_or(str::is_empty))
            .collect()
    } else {
        agents
            .iter()
            .filter(|agent| agent.phase.as_deref() == Some(phase_title))
            .collect()
    }
}

fn agent_description(agent: &WorkflowAgentProgress) -> String {
    let mut parts = Vec::new();
    if let Some(agent_type) = agent.agent_type.as_ref().filter(|value| !value.is_empty()) {
        parts.push(agent_type.as_str());
    }
    if let Some(model) = agent.model.as_ref().filter(|value| !value.is_empty()) {
        parts.push(model.as_str());
    }
    if parts.is_empty() {
        truncate_text(agent.prompt.as_str(), 90)
    } else {
        format!(
            "{} · {}",
            parts.join(" · "),
            truncate_text(agent.prompt.as_str(), 72)
        )
    }
}

fn agent_selected_description(agent: &WorkflowAgentProgress) -> String {
    if let Some(error) = agent.error.as_ref().filter(|value| !value.is_empty()) {
        return format!("error: {}", truncate_text(error.as_str(), 120));
    }
    truncate_text(agent.prompt.as_str(), 120)
}

fn phase_count_description(phase: &WorkflowPhaseProgress) -> String {
    format!(
        "{} agent(s): {} running, {} done, {} failed",
        phase.agent_count,
        phase.running_agent_count,
        phase.completed_agent_count,
        phase.failed_agent_count
    )
}

fn phase_count_description_from_agents(agents: &[WorkflowAgentProgress]) -> String {
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
    format!(
        "{} agent(s): {} running, {} done, {} failed",
        agents.len(),
        running,
        completed,
        failed
    )
}

fn workflow_status_label(status: WorkflowRunStatus) -> &'static str {
    match status {
        WorkflowRunStatus::Running => "running",
        WorkflowRunStatus::Completed => "completed",
        WorkflowRunStatus::Failed => "failed",
    }
}

fn agent_status_label(status: WorkflowAgentStatus) -> &'static str {
    match status {
        WorkflowAgentStatus::Running => "running",
        WorkflowAgentStatus::Completed => "done",
        WorkflowAgentStatus::Failed => "failed",
    }
}
