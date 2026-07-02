pub(crate) const TEAM_TOOL_NAME: &str = "team";

pub(crate) const TOOL_DESCRIPTION: &str = "Coordinate a durable agent team with shared tasks, per-agent mailboxes, plan approval, scheduling, Codex teammate execution, graceful shutdown, and cleanup.";

pub(crate) const DEVELOPER_PROMPT: &str = r#"Use the `team` tool for coordinated multi-agent work that needs durable shared state, task dependencies, mailbox delivery, plan approval, or teammate execution.

Team behavior:
- Create a team before assigning work. Include one lead agent and one or more teammates with clear roles.
- Decompose broad objectives into explicit tasks with dependencies, priorities, role hints, and plan-approval requirements.
- Use plan requests for tasks that can affect architecture, APIs, storage, permissions, or broad behavior.
- Use claim/complete lifecycle actions rather than editing task state directly.
- Use run_ready_tasks only after required plans are approved; it dispatches ready tasks to isolated Codex subagent teammates and writes completion/requeue state.
- Keep team state active until tasks are terminal; then request shutdown, mark stopped, and cleanup when appropriate.
- Do not use team for tiny one-agent edits or tasks with heavy contention on the same files.

Message lifecycle (Lead ↔ Teammates):
1. Teammates can send you messages during their task via the mailbox system.
2. Before checking for messages, ALWAYS call `route_messages` first to deliver pending messages to inboxes.
3. Call `consume_message` with your `agent_id` (the lead id you specified during creation) to read the next unread message from your inbox. Repeat until there are no more messages.
4. Reply to a teammate by calling `send_message` with your agent_id as `from` and the teammate's id as `to: {"agent": "teammate_id"}`.
5. After sending, call `route_messages` again so your reply reaches the teammate's inbox.
6. The teammate will see your message when dispatch_ready_tasks routes messages before their next task run.
7. Use `mailbox_list` to inspect your inbox if you are unsure whether new messages are waiting.

The tool returns JSON details for every action. Treat those details as the source of truth for team id, task status, message ids, and lifecycle state."#;

pub(crate) fn developer_prompt_fragment() -> &'static str {
    DEVELOPER_PROMPT
}
