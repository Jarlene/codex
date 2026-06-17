use std::sync::Arc;

use codex_extension_api::ExtensionEventSink;
use codex_extension_api::FunctionCallError;
use codex_extension_api::JsonToolOutput;
use codex_extension_api::ResponsesApiTool;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolExecutorFuture;
use codex_extension_api::ToolName;
use codex_extension_api::ToolOutput;
use codex_extension_api::ToolSpec;
use codex_extension_api::parse_tool_input_schema_without_compaction;
use codex_protocol::ThreadId;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_tools::ToolExposure;
use codex_utils_absolute_path::AbsolutePathBuf;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use serde_json::json;

use crate::prompt;
use crate::runtime;
use crate::runtime::AgentRunner;
use crate::runtime::WorkflowProgressContext;

#[derive(Clone)]
pub(crate) struct WorkflowTool {
    pub(crate) thread_id: ThreadId,
    pub(crate) cwd: AbsolutePathBuf,
    pub(crate) concurrency: usize,
    pub(crate) runner: Arc<dyn AgentRunner>,
    pub(crate) event_sink: Arc<dyn ExtensionEventSink>,
}

#[derive(Debug, Deserialize)]
struct WorkflowToolInput {
    script: String,
    args: Option<JsonValue>,
}

impl ToolExecutor<ToolCall> for WorkflowTool {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(prompt::WORKFLOW_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ResponsesApiTool {
            name: prompt::WORKFLOW_TOOL_NAME.to_string(),
            description: prompt::developer_prompt_fragment(),
            strict: false,
            defer_loading: None,
            parameters: parse_tool_input_schema_without_compaction(&workflow_tool_schema())
                .unwrap_or_else(|err| panic!("workflow schema should parse: {err}")),
            output_schema: None,
        })
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        false
    }

    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(call))
    }
}

impl WorkflowTool {
    async fn handle_call(&self, call: ToolCall) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let input = parse_args(&call)?;
        let script = normalize_workflow_script(&input.script);
        let result = runtime::run_workflow(
            &script,
            input.args,
            self.cwd.display().to_string(),
            self.concurrency,
            Arc::clone(&self.runner),
            Some(WorkflowProgressContext {
                thread_id: self.thread_id,
                turn_id: call.turn_id.clone(),
                call_id: call.call_id.clone(),
                emitter: workflow_progress_emitter(
                    Arc::clone(&self.event_sink),
                    call.turn_id.clone(),
                ),
            }),
        )
        .await
        .map_err(FunctionCallError::RespondToModel)?;
        Ok(Box::new(JsonToolOutput::new(json!({
            "content": [{
                "type": "text",
                "text": format!(
                    "Workflow {} completed with {} agent(s).\n\nResult:\n{}",
                    result.meta.name,
                    result.agent_count,
                    serde_json::to_string_pretty(&result.result).unwrap_or_else(|_| result.result.to_string())
                )
            }],
            "details": result,
        }))))
    }
}

fn workflow_progress_emitter(
    event_sink: Arc<dyn ExtensionEventSink>,
    event_id: String,
) -> Arc<dyn runtime::WorkflowProgressEmitter> {
    Arc::new(
        move |event| -> runtime::WorkflowProgressEmitterFuture<'static> {
            let event_sink = Arc::clone(&event_sink);
            let event_id = event_id.clone();
            Box::pin(async move {
                event_sink.emit(Event {
                    id: event_id,
                    msg: EventMsg::WorkflowRunUpdated(event),
                });
            })
        },
    )
}

fn parse_args(call: &ToolCall) -> Result<WorkflowToolInput, FunctionCallError> {
    serde_json::from_str(call.function_arguments()?)
        .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))
}

fn normalize_workflow_script(script: &str) -> String {
    let trimmed = script.trim();
    if let Some(body) = trimmed
        .strip_prefix("```")
        .and_then(|rest| rest.strip_suffix("```"))
    {
        let body = body
            .strip_prefix("js\n")
            .or_else(|| body.strip_prefix("javascript\n"))
            .or_else(|| body.strip_prefix("rust\n"))
            .or_else(|| body.strip_prefix("rs\n"))
            .unwrap_or(body);
        return body.trim().to_string();
    }
    trimmed.to_string()
}

fn workflow_tool_schema() -> JsonValue {
    json!({
        "type": "object",
        "properties": {
            "script": {
                "type": "string",
                "description": prompt::SCRIPT_DESCRIPTION,
            },
            "args": {
                "description": prompt::ARGS_DESCRIPTION,
            },
        },
        "required": ["script"],
        "additionalProperties": false,
    })
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;

    #[test]
    fn exposes_pi_schema_descriptions() {
        let schema = workflow_tool_schema();
        assert_eq!(
            schema["properties"]["script"]["description"],
            prompt::SCRIPT_DESCRIPTION
        );
        assert_eq!(
            schema["properties"]["args"]["description"],
            prompt::ARGS_DESCRIPTION
        );
    }

    #[test]
    fn normalizes_markdown_fences() {
        assert_eq!(
            normalize_workflow_script(
                "```rust\nworkflow! { meta: { name: \"x\", description: \"y\" } }\n```"
            ),
            "workflow! { meta: { name: \"x\", description: \"y\" } }"
        );
    }

    #[test]
    fn spec_uses_exact_description() {
        let tool = WorkflowTool {
            thread_id: ThreadId::new(),
            cwd: AbsolutePathBuf::from_absolute_path("/tmp").expect("absolute path"),
            concurrency: 4,
            runner: Arc::new(|_request| -> crate::runtime::AgentRunnerFuture<'static> {
                Box::pin(async { Ok(crate::runtime::AgentRunResponse { value: json!("ok") }) })
            }),
            event_sink: Arc::new(codex_extension_api::NoopExtensionEventSink),
        };
        let ToolSpec::Function(spec) = tool.spec() else {
            panic!("workflow should be a function tool");
        };
        assert_eq!(spec.name, "workflow");
        assert_eq!(spec.description, prompt::developer_prompt_fragment());
    }
}
