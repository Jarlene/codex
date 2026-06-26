pub(crate) const TEAM_TOOL_NAME: &str = "team";

pub(crate) const TOOL_DESCRIPTION: &str = "Coordinate a durable agent team with shared tasks, per-agent mailboxes, plan approval, scheduling, Codex teammate execution, graceful shutdown, and cleanup.";

pub(crate) const DEVELOPER_PROMPT: &str = r#"Use the `team` tool coordinated multi-agent work that needs durable shared state, task dependencies, mailbox delivery, plan approval, or teammate execution.

Team behavior:
- Create a team before assigning work. Include one lead agent and one or more teammates with clear roles.
- Decompose broad objectives into explicit tasks with dependencies, priorities, role hints, and plan-approval requirements.
- Use plan requests for tasks that can affect architecture, APIs, storage, permissions, or broad behavior.
- Use mailbox actions for direct messages, broadcasts, shutdown notices, and plan responses. Route pending messages before expecting teammates to consume them.
- Use claim/complete lifecycle actions rather than editing task state directly.
- Use run_ready_tasks only after required plans are approved; it dispatches ready tasks to isolated Codex subagent teammates and writes completion/requeue state.
- Keep team state active until tasks are terminal; then request shutdown, mark stopped, and cleanup when appropriate.
- Do not use team for tiny one-agent edits or tasks with heavy contention on the same files.

The tool returns JSON details for every action. Treat those details as the source of truth for team id, task status, message ids, and lifecycle state."#;

pub(crate) fn developer_prompt_fragment() -> &'static str {
    DEVELOPER_PROMPT
}
