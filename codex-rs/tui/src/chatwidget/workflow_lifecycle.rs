//! Workflow run lifecycle rendering for `ChatWidget`.

use super::*;

impl ChatWidget {
    pub(super) fn on_workflow_run_updated(
        &mut self,
        notification: codex_app_server_protocol::WorkflowRunUpdatedNotification,
    ) {
        self.record_visible_turn_activity();
        self.flush_answer_stream_with_separator();

        let call_id = notification.call_id.clone();
        self.latest_workflow_run = Some(notification.clone());
        let is_terminal = matches!(
            notification.status,
            WorkflowRunStatus::Completed | WorkflowRunStatus::Failed
        );
        let mut handled = false;
        if let Some(cell) = self
            .transcript
            .active_cell
            .as_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<WorkflowRunCell>())
            && cell.call_id() == call_id
        {
            cell.update(notification.clone());
            self.bump_active_cell_revision();
            handled = true;
        }

        if !handled {
            self.flush_active_cell();
            self.transcript.active_cell = Some(Box::new(history_cell::new_workflow_run_cell(
                notification,
                self.config.animations,
            )));
            self.bump_active_cell_revision();
        }

        if is_terminal {
            self.flush_active_cell();
        }
        self.refresh_workflow_popup_if_open();
        self.request_redraw();
    }
}
