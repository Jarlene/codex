use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Weak;

use codex_core::NewThread;
use codex_core::StartThreadOptions;
use codex_core::ThreadManager;
use codex_core::config::Config;
use codex_extension_api::AgentSpawnFuture;
use codex_extension_api::AgentSpawner;
use codex_extension_api::ConfigContributor;
use codex_extension_api::ContextContributor;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::PromptFragment;
use codex_extension_api::ThreadLifecycleContributor;
use codex_extension_api::ThreadStartInput;
use codex_extension_api::ToolContributor;
use codex_protocol::ThreadId;
use codex_protocol::error::CodexErr;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::InitialHistory;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use codex_protocol::protocol::ThreadSource;
use codex_protocol::protocol::TurnEnvironmentSelection;
use codex_protocol::user_input::UserInput;
use tokio::sync::Mutex;

use crate::AgentRole;
use crate::FsTeamStore;
use crate::TeamError;
use crate::TeamId;
use crate::TeammateRunRequest;
use crate::TeammateRunResult;
use crate::TeammateRunner;
use crate::TeammateRunnerFuture;
use crate::prompt;
use crate::tool::TeamTool;

#[derive(Clone)]
struct TeamExtension<S> {
    agent_spawner: S,
}

#[derive(Clone, Debug)]
struct TeamExtensionConfig {
    enabled: bool,
    config: Config,
    forked_from_thread_id: ThreadId,
    environments: Vec<TurnEnvironmentSelection>,
    store_root: PathBuf,
    active_team: Arc<Mutex<Option<TeamId>>>,
}

impl TeamExtensionConfig {
    fn from_input(
        config: &Config,
        forked_from_thread_id: ThreadId,
        environments: &[TurnEnvironmentSelection],
        active_team: Arc<Mutex<Option<TeamId>>>,
    ) -> Self {
        Self {
            enabled: true,
            config: config.clone(),
            forked_from_thread_id,
            environments: environments.to_vec(),
            store_root: config.cwd.join(".omx/state/team").into(),
            active_team,
        }
    }
}

impl<S> ThreadLifecycleContributor<Config> for TeamExtension<S>
where
    S: Send + Sync,
{
    fn on_thread_start<'a>(
        &'a self,
        input: ThreadStartInput<'a, Config>,
    ) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            let Ok(forked_from_thread_id) = ThreadId::from_string(input.thread_store.level_id())
            else {
                return;
            };
            input.thread_store.insert(TeamExtensionConfig::from_input(
                input.config,
                forked_from_thread_id,
                input.environments,
                Arc::new(Mutex::new(None)),
            ));
        })
    }
}

impl<S> ConfigContributor<Config> for TeamExtension<S>
where
    S: Send + Sync,
{
    fn on_config_changed(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
        _previous_config: &Config,
        new_config: &Config,
    ) {
        let Some(previous) = thread_store.get::<TeamExtensionConfig>() else {
            return;
        };
        thread_store.insert(TeamExtensionConfig::from_input(
            new_config,
            previous.forked_from_thread_id,
            &previous.environments,
            Arc::clone(&previous.active_team),
        ));
    }
}

impl<S> ContextContributor for TeamExtension<S>
where
    S: Send + Sync,
{
    fn contribute<'a>(
        &'a self,
        _session_store: &'a ExtensionData,
        thread_store: &'a ExtensionData,
    ) -> ExtensionFuture<'a, Vec<PromptFragment>> {
        Box::pin(async move {
            let Some(config) = thread_store.get::<TeamExtensionConfig>() else {
                return Vec::new();
            };
            if !config.enabled {
                return Vec::new();
            }
            vec![PromptFragment::developer_capability(
                prompt::developer_prompt_fragment(),
            )]
        })
    }
}

impl<S> ToolContributor for TeamExtension<S>
where
    S: AgentSpawner<StartThreadOptions, Spawned = NewThread, Error = CodexErr>
        + Clone
        + Send
        + Sync
        + 'static,
{
    fn tools(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn codex_extension_api::ToolExecutor<codex_extension_api::ToolCall>>> {
        let Some(config) = thread_store.get::<TeamExtensionConfig>() else {
            return Vec::new();
        };
        if !config.enabled {
            return Vec::new();
        }
        let runner = CodexTeamRunner {
            agent_spawner: self.agent_spawner.clone(),
            forked_from_thread_id: config.forked_from_thread_id,
            config: config.config.clone(),
            environments: config.environments.clone(),
        };
        vec![Arc::new(TeamTool::new(
            FsTeamStore::new(config.store_root.clone()),
            Arc::clone(&config.active_team),
            Arc::new(runner),
        ))]
    }
}

#[derive(Clone)]
struct CodexTeamRunner<S> {
    agent_spawner: S,
    forked_from_thread_id: ThreadId,
    config: Config,
    environments: Vec<TurnEnvironmentSelection>,
}

impl<S> TeammateRunner for CodexTeamRunner<S>
where
    S: AgentSpawner<StartThreadOptions, Spawned = NewThread, Error = CodexErr> + Send + Sync,
{
    fn run<'a>(&'a self, request: TeammateRunRequest) -> TeammateRunnerFuture<'a> {
        Box::pin(async move {
            let started_at = now_unix_timestamp_secs();
            let mut config = self.config.clone();
            let agent_type = role_agent_type(request.task.role_hint)
                .unwrap_or_else(|| role_agent_type_from_agent_id(&request.agent_id.0));
            codex_core::apply_agent_role_to_config(&mut config, Some(agent_type))
                .await
                .map_err(|err| {
                    TeamError::InvalidOperation(format!(
                        "agent {} failed to apply role: {err}",
                        request.agent_id
                    ))
                })?;
            let prompt = build_teammate_prompt(&request);
            let new_thread = self
                .agent_spawner
                .spawn_subagent(
                    self.forked_from_thread_id,
                    StartThreadOptions {
                        config,
                        initial_history: InitialHistory::New,
                        session_source: Some(SessionSource::SubAgent(SubAgentSource::Other(
                            format!("team:{}", request.agent_id),
                        ))),
                        thread_source: Some(ThreadSource::Subagent),
                        dynamic_tools: Vec::new(),
                        metrics_service_name: None,
                        parent_trace: None,
                        environments: self.environments.clone(),
                        thread_extension_init: codex_extension_api::ExtensionDataInit::default(),
                    },
                )
                .await
                .map_err(|err| {
                    TeamError::InvalidOperation(format!(
                        "agent {} failed to start: {err}",
                        request.agent_id
                    ))
                })?;
            new_thread
                .thread
                .submit(Op::UserInput {
                    items: vec![UserInput::Text {
                        text: prompt,
                        text_elements: Vec::new(),
                    }],
                    final_output_json_schema: None,
                    responsesapi_client_metadata: None,
                    additional_context: Default::default(),
                    thread_settings: Default::default(),
                })
                .await
                .map_err(|err| {
                    TeamError::InvalidOperation(format!(
                        "agent {} failed to submit task: {err}",
                        request.agent_id
                    ))
                })?;
            let output = wait_for_agent_result(&new_thread).await?;
            let _ = new_thread.thread.submit(Op::Shutdown {}).await;
            Ok(TeammateRunResult {
                agent_id: request.agent_id,
                task_id: request.task.id,
                success: true,
                output,
                tokens_used: 0,
                completed_at: now_unix_timestamp_secs().max(started_at),
            })
        })
    }
}

fn build_teammate_prompt(request: &TeammateRunRequest) -> String {
    let mut parts = Vec::new();
    parts.push(format!(
        "You are teammate `{}` in team `{}`.",
        request.agent_id, request.team_id
    ));
    if let Some(role) = request.task.role_hint {
        parts.push(format!("Role hint: {role:?}."));
    }
    parts.push("Work in an isolated context. Do not assume access to the lead's hidden conversation beyond this prompt.".to_string());
    parts.push(format!("Task id: {}", request.task.id));
    parts.push(format!("Title: {}", request.task.title));
    parts.push(format!("Description:\n{}", request.task.description));
    if !request.task.dependencies.is_empty() {
        parts.push(format!(
            "Completed dependencies: {}",
            request
                .task
                .dependencies
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    parts.push("Finish with a concise report of what you did, evidence gathered, blockers, and recommended handoff. Do not attempt to mutate team state directly; the lead runtime records completion.".to_string());
    parts.join("\n\n")
}

async fn wait_for_agent_result(new_thread: &NewThread) -> Result<String, TeamError> {
    loop {
        let event = new_thread.thread.next_event().await.map_err(|err| {
            TeamError::InvalidOperation(format!("subagent event stream failed: {err}"))
        })?;
        match event.msg {
            EventMsg::TurnComplete(complete) => {
                return Ok(complete.last_agent_message.unwrap_or_default());
            }
            EventMsg::TurnAborted(aborted) => {
                return Err(TeamError::InvalidOperation(format!(
                    "subagent aborted: {:?}",
                    aborted.reason
                )));
            }
            EventMsg::Error(error) => return Err(TeamError::InvalidOperation(error.message)),
            _ => {}
        }
    }
}

fn role_agent_type(role: Option<AgentRole>) -> Option<&'static str> {
    match role? {
        AgentRole::Lead => Some("planner"),
        AgentRole::Architect => Some("architect"),
        AgentRole::Developer | AgentRole::Backend | AgentRole::Frontend => Some("coder"),
        AgentRole::Tester => Some("tester"),
        AgentRole::Reviewer => Some("reviewer"),
        AgentRole::Security => Some("reviewer"),
        AgentRole::Performance => Some("reviewer"),
        AgentRole::Database => Some("architect"),
        AgentRole::Researcher => Some("researcher"),
        AgentRole::Writer => Some("writer"),
        AgentRole::Custom => None,
    }
}

fn role_agent_type_from_agent_id(agent_id: &str) -> &'static str {
    let lower = agent_id.to_ascii_lowercase();
    if lower.contains("architect") {
        "architect"
    } else if lower.contains("test") {
        "tester"
    } else if lower.contains("review") || lower.contains("security") {
        "reviewer"
    } else if lower.contains("research") {
        "researcher"
    } else if lower.contains("write") || lower.contains("doc") {
        "writer"
    } else {
        "coder"
    }
}

fn now_unix_timestamp_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

pub fn install<S>(registry: &mut ExtensionRegistryBuilder<Config>, agent_spawner: S)
where
    S: AgentSpawner<StartThreadOptions, Spawned = NewThread, Error = CodexErr>
        + Clone
        + Send
        + Sync
        + 'static,
{
    let extension = Arc::new(TeamExtension { agent_spawner });
    registry.thread_lifecycle_contributor(extension.clone());
    registry.config_contributor(extension.clone());
    registry.prompt_contributor(extension.clone());
    registry.tool_contributor(extension);
}

pub fn team_agent_spawner(
    thread_manager: Weak<ThreadManager>,
) -> impl AgentSpawner<StartThreadOptions, Spawned = NewThread, Error = CodexErr> + Clone {
    move |forked_from_thread_id: ThreadId,
          options: StartThreadOptions|
          -> AgentSpawnFuture<'static, NewThread, CodexErr> {
        let thread_manager = thread_manager.clone();
        Box::pin(async move {
            let thread_manager = thread_manager.upgrade().ok_or_else(|| {
                CodexErr::UnsupportedOperation("thread manager dropped".to_string())
            })?;
            thread_manager
                .spawn_subagent(forked_from_thread_id, options)
                .await
        })
    }
}

#[cfg(test)]
mod tests {
    use codex_core::config::ConfigBuilder;
    use codex_extension_api::AgentSpawnFuture;
    use codex_extension_api::ConversationHistory;
    use codex_extension_api::ExtensionData;
    use codex_extension_api::ExtensionRegistryBuilder;
    use codex_extension_api::NoopTurnItemEmitter;
    use codex_extension_api::ToolName;
    use codex_extension_api::ToolPayload;
    use codex_tools::ToolExposure;
    use codex_utils_output_truncation::TruncationPolicy;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn installed_extension_contributes_team_tool_prompt_and_persistent_state() {
        let mut builder = ExtensionRegistryBuilder::<Config>::new();
        install(
            &mut builder,
            |_thread_id, _options| -> AgentSpawnFuture<'static, NewThread, CodexErr> {
                Box::pin(async { Err(CodexErr::UnsupportedOperation("test".to_string())) })
            },
        );
        let registry = builder.build();
        let session_store = ExtensionData::new("session");
        let thread_id = ThreadId::default();
        let thread_store = ExtensionData::new(thread_id.to_string());
        let codex_home = tempdir().expect("create temp codex home");
        let cwd = tempdir().expect("create temp cwd");
        let config = ConfigBuilder::default()
            .codex_home(codex_home.path().to_path_buf())
            .fallback_cwd(Some(cwd.path().to_path_buf()))
            .build()
            .await
            .expect("build test config");
        thread_store.insert(TeamExtensionConfig::from_input(
            &config,
            thread_id,
            &[],
            Arc::new(Mutex::new(None)),
        ));

        let fragments = registry.context_contributors()[0]
            .contribute(&session_store, &thread_store)
            .await;
        assert_eq!(fragments.len(), 1);
        assert!(fragments[0].text().contains("coordinated"));

        let tools = registry.tool_contributors()[0].tools(&session_store, &thread_store);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].tool_name(), ToolName::plain("team"));
        assert_eq!(tools[0].exposure(), ToolExposure::Direct);
        let output = tools[0]
            .handle(team_tool_call(json!({
                "action": "create",
                "objective": "coordinate tests",
                "lead": { "id": "lead", "display_name": "Lead", "role": "lead" },
                "teammates": [
                    { "id": "dev", "display_name": "Developer", "role": "backend" }
                ],
                "tasks": [
                    {
                        "id": "implementation",
                        "title": "Implementation",
                        "description": "Implement the task",
                        "dependencies": [],
                        "priority": "p0",
                        "role_hint": "backend",
                        "estimated_effort": "small",
                        "requires_plan_approval": false
                    }
                ],
                "created_at": 123
            })))
            .await
            .expect("team create should succeed");
        let result = output.code_mode_result(&ToolPayload::Function {
            arguments: String::new(),
        });
        let team_id = result["team"]["id"]
            .as_str()
            .expect("team id should be returned");
        assert!(
            cwd.path()
                .join(".omx/state/team")
                .join(team_id)
                .join("team.json")
                .exists()
        );
    }

    fn team_tool_call(arguments: serde_json::Value) -> codex_extension_api::ToolCall {
        codex_extension_api::ToolCall {
            turn_id: "turn-1".to_string(),
            call_id: "call-1".to_string(),
            tool_name: ToolName::plain("team"),
            model: "test-model".to_string(),
            truncation_policy: TruncationPolicy::Bytes(1024 * 1024),
            conversation_history: ConversationHistory::default(),
            turn_item_emitter: Arc::new(NoopTurnItemEmitter),
            environments: Vec::new(),
            payload: ToolPayload::Function {
                arguments: arguments.to_string(),
            },
        }
    }
}
