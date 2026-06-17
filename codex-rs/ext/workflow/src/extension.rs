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
use codex_extension_api::ExtensionEventSink;
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
use codex_utils_absolute_path::AbsolutePathBuf;
use serde_json::Value as JsonValue;

use crate::prompt;
use crate::runtime::AgentRunRequest;
use crate::runtime::AgentRunResponse;
use crate::runtime::AgentRunner;
use crate::runtime::AgentRunnerFuture;
use crate::tool::WorkflowTool;

#[derive(Clone)]
struct WorkflowExtension<S> {
    agent_spawner: S,
    event_sink: Arc<dyn ExtensionEventSink>,
}

#[derive(Clone, Debug)]
struct WorkflowExtensionConfig {
    enabled: bool,
    config: Config,
    cwd: AbsolutePathBuf,
    concurrency: usize,
    forked_from_thread_id: ThreadId,
    environments: Vec<TurnEnvironmentSelection>,
}

impl WorkflowExtensionConfig {
    fn from_input(
        config: &Config,
        forked_from_thread_id: ThreadId,
        environments: &[TurnEnvironmentSelection],
    ) -> Self {
        Self {
            enabled: true,
            config: config.clone(),
            cwd: config.cwd.clone(),
            concurrency: 4,
            forked_from_thread_id,
            environments: environments.to_vec(),
        }
    }

    fn from_config(config: &Config, forked_from_thread_id: ThreadId) -> Self {
        Self::from_input(config, forked_from_thread_id, &[])
    }
}

impl<S> ThreadLifecycleContributor<Config> for WorkflowExtension<S>
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
            input
                .thread_store
                .insert(WorkflowExtensionConfig::from_input(
                    input.config,
                    forked_from_thread_id,
                    input.environments,
                ));
        })
    }
}

impl<S> ConfigContributor<Config> for WorkflowExtension<S>
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
        let forked_from_thread_id = thread_store
            .get::<WorkflowExtensionConfig>()
            .map(|config| config.forked_from_thread_id)
            .or_else(|| ThreadId::from_string(thread_store.level_id()).ok());
        if let Some(forked_from_thread_id) = forked_from_thread_id {
            thread_store.insert(WorkflowExtensionConfig::from_config(
                new_config,
                forked_from_thread_id,
            ));
        }
    }
}

impl<S> ContextContributor for WorkflowExtension<S>
where
    S: Send + Sync,
{
    fn contribute<'a>(
        &'a self,
        _session_store: &'a ExtensionData,
        thread_store: &'a ExtensionData,
    ) -> ExtensionFuture<'a, Vec<PromptFragment>> {
        Box::pin(async move {
            let Some(config) = thread_store.get::<WorkflowExtensionConfig>() else {
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

impl<S> ToolContributor for WorkflowExtension<S>
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
        let Some(config) = thread_store.get::<WorkflowExtensionConfig>() else {
            return Vec::new();
        };
        if !config.enabled {
            return Vec::new();
        }
        vec![Arc::new(WorkflowTool {
            thread_id: config.forked_from_thread_id,
            cwd: config.cwd.clone(),
            concurrency: config.concurrency,
            runner: Arc::new(WorkflowCodexAgentRunner {
                agent_spawner: self.agent_spawner.clone(),
                forked_from_thread_id: config.forked_from_thread_id,
                config: config.config.clone(),
                environments: config.environments.clone(),
            }),
            event_sink: Arc::clone(&self.event_sink),
        })]
    }
}

#[derive(Clone)]
struct WorkflowCodexAgentRunner<S> {
    agent_spawner: S,
    forked_from_thread_id: ThreadId,
    config: Config,
    environments: Vec<TurnEnvironmentSelection>,
}

impl<S> AgentRunner for WorkflowCodexAgentRunner<S>
where
    S: AgentSpawner<StartThreadOptions, Spawned = NewThread, Error = CodexErr> + Send + Sync,
{
    fn run_agent<'a>(&'a self, request: AgentRunRequest) -> AgentRunnerFuture<'a> {
        Box::pin(async move {
            let mut config = self.config.clone();
            codex_core::apply_agent_role_to_config(&mut config, request.agent_type.as_deref())
                .await
                .map_err(|err| format!("agent {} failed to apply role: {err}", request.label))?;
            if let Some(model) = request.model.as_ref() {
                config.model = Some(model.clone());
            }
            let prompt = build_subagent_prompt(&request);
            let new_thread = self
                .agent_spawner
                .spawn_subagent(
                    self.forked_from_thread_id,
                    StartThreadOptions {
                        config: config.clone(),
                        initial_history: InitialHistory::New,
                        session_source: Some(SessionSource::SubAgent(SubAgentSource::Other(
                            format!("workflow:{}", request.label),
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
                .map_err(|err| format!("agent {} failed to start: {err}", request.label))?;
            new_thread
                .thread
                .submit(Op::UserInput {
                    items: vec![UserInput::Text {
                        text: prompt,
                        text_elements: Vec::new(),
                    }],
                    final_output_json_schema: request.schema.clone(),
                    responsesapi_client_metadata: None,
                    additional_context: Default::default(),
                    thread_settings: Default::default(),
                })
                .await
                .map_err(|err| format!("agent {} failed to submit: {err}", request.label))?;
            let value = wait_for_agent_result(&new_thread, request.schema.is_some()).await?;
            let _ = new_thread.thread.submit(Op::Shutdown {}).await;
            Ok(AgentRunResponse { value })
        })
    }
}

fn build_subagent_prompt(request: &AgentRunRequest) -> String {
    let mut parts = Vec::new();
    if let Some(instructions) = request.instructions.as_deref() {
        parts.push(instructions.to_string());
    }
    parts.push(format!("Task label: {}", request.label));
    parts.push(request.prompt.clone());
    if request.schema.is_some() {
        parts.push(
            [
                "Final output contract:",
                "- Your final response MUST be valid JSON matching the provided schema.",
                "- Do not emit prose outside the JSON value.",
                "- If you need to inspect files or run commands first, do so, then finish with JSON exactly once.",
            ]
            .join("\n"),
        );
    }
    parts.join("\n\n")
}

async fn wait_for_agent_result(
    new_thread: &NewThread,
    structured: bool,
) -> Result<JsonValue, String> {
    loop {
        let event = new_thread
            .thread
            .next_event()
            .await
            .map_err(|err| format!("subagent event stream failed: {err}"))?;
        match event.msg {
            EventMsg::TurnComplete(complete) => {
                let text = complete.last_agent_message.unwrap_or_default();
                if structured {
                    return serde_json::from_str(&text)
                        .map_err(|err| format!("subagent structured output was not JSON: {err}"));
                }
                return Ok(JsonValue::String(text));
            }
            EventMsg::TurnAborted(aborted) => {
                return Err(format!("subagent aborted: {:?}", aborted.reason));
            }
            EventMsg::Error(error) => {
                return Err(error.message);
            }
            _ => {}
        }
    }
}

pub fn install<S>(registry: &mut ExtensionRegistryBuilder<Config>, agent_spawner: S)
where
    S: AgentSpawner<StartThreadOptions, Spawned = NewThread, Error = CodexErr>
        + Clone
        + Send
        + Sync
        + 'static,
{
    let extension = Arc::new(WorkflowExtension {
        agent_spawner,
        event_sink: registry.event_sink(),
    });
    registry.thread_lifecycle_contributor(extension.clone());
    registry.config_contributor(extension.clone());
    registry.prompt_contributor(extension.clone());
    registry.tool_contributor(extension);
}

pub fn workflow_agent_spawner(
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
    use codex_extension_api::ExtensionData;
    use codex_extension_api::ExtensionRegistryBuilder;
    use codex_extension_api::ToolName;
    use codex_protocol::openai_models::ReasoningEffort;
    use pretty_assertions::assert_eq;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::sync::Mutex;

    use crate::runtime::AgentRunner;

    use super::*;

    #[tokio::test]
    async fn installed_extension_contributes_workflow_tool_and_prompt() {
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
        let config = ConfigBuilder::default()
            .codex_home(codex_home.path().to_path_buf())
            .build()
            .await
            .expect("build test config");
        thread_store.insert(WorkflowExtensionConfig {
            enabled: true,
            cwd: config.cwd.clone(),
            config,
            concurrency: 4,
            forked_from_thread_id: thread_id,
            environments: Vec::new(),
        });

        let tools = registry.tool_contributors()[0].tools(&session_store, &thread_store);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].tool_name(), ToolName::plain("workflow"));
        let fragments = registry.context_contributors()[0]
            .contribute(&session_store, &thread_store)
            .await;
        assert_eq!(fragments.len(), 1);
        assert!(fragments[0].text().contains(prompt::PROMPT_SNIPPET));
    }

    #[tokio::test]
    async fn workflow_agent_type_applies_role_config_before_spawn() {
        let codex_home = tempdir().expect("create temp codex home");
        let mut config = ConfigBuilder::default()
            .codex_home(codex_home.path().to_path_buf())
            .build()
            .await
            .expect("build test config");
        config.model_reasoning_effort = Some(ReasoningEffort::High);
        let captured_config = Arc::new(Mutex::new(None));
        let captured_config_for_spawner = Arc::clone(&captured_config);
        let runner = WorkflowCodexAgentRunner {
            agent_spawner: move |_thread_id: ThreadId,
                                 options: StartThreadOptions|
                  -> AgentSpawnFuture<'static, NewThread, CodexErr> {
                let captured_config_for_spawner = Arc::clone(&captured_config_for_spawner);
                Box::pin(async move {
                    *captured_config_for_spawner.lock().await = Some(options.config);
                    Err::<NewThread, CodexErr>(CodexErr::UnsupportedOperation(
                        "test stop".to_string(),
                    ))
                })
            },
            forked_from_thread_id: ThreadId::default(),
            config,
            environments: Vec::new(),
        };

        let result = runner
            .run_agent(AgentRunRequest {
                prompt: "inspect architecture".to_string(),
                label: "role check".to_string(),
                phase: None,
                schema: None,
                instructions: None,
                model: Some("gpt-explicit".to_string()),
                agent_type: Some("architect".to_string()),
            })
            .await;

        assert!(result.is_err());
        let captured = captured_config
            .lock()
            .await
            .clone()
            .expect("spawner should receive config");
        assert_eq!(captured.model.as_deref(), Some("gpt-explicit"));
        assert_eq!(
            captured.model_reasoning_effort,
            Some(ReasoningEffort::Custom("middle".to_string()))
        );
        assert!(
            captured
                .developer_instructions
                .as_deref()
                .is_some_and(|instructions| instructions.contains("software architect"))
        );
    }
}
