use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use codex_protocol::ThreadId;
use codex_protocol::protocol::WorkflowAgentProgress;
use codex_protocol::protocol::WorkflowAgentStatus;
use codex_protocol::protocol::WorkflowPhaseProgress;
use codex_protocol::protocol::WorkflowRunStatus;
use codex_protocol::protocol::WorkflowRunUpdatedEvent;
use serde::Serialize;
use serde_json::Value as JsonValue;
use tokio::sync::Mutex;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::parser::WorkflowMeta;
use crate::parser::parse_workflow_script;
use crate::runtime_value::FunctionValue;
use crate::runtime_value::RuntimeValue;
use crate::script::BinaryOp;
use crate::script::Expr;
use crate::script::FunctionBody;
use crate::script::ObjectProperty;
use crate::script::Stmt;
use crate::script::UnaryOp;
use crate::script::parse_workflow_body;

pub type AgentRunnerFuture<'a> =
    Pin<Box<dyn Future<Output = Result<AgentRunResponse, String>> + Send + 'a>>;

pub trait AgentRunner: Send + Sync {
    fn run_agent<'a>(&'a self, request: AgentRunRequest) -> AgentRunnerFuture<'a>;
}

impl<F> AgentRunner for F
where
    F: Fn(AgentRunRequest) -> AgentRunnerFuture<'static> + Send + Sync,
{
    fn run_agent<'a>(&'a self, request: AgentRunRequest) -> AgentRunnerFuture<'a> {
        self(request)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentRunRequest {
    pub prompt: String,
    pub label: String,
    pub phase: Option<String>,
    pub schema: Option<JsonValue>,
    pub instructions: Option<String>,
    pub model: Option<String>,
    pub agent_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentRunResponse {
    pub value: JsonValue,
}

pub type WorkflowProgressEmitterFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

pub trait WorkflowProgressEmitter: Send + Sync {
    fn emit<'a>(&'a self, event: WorkflowRunUpdatedEvent) -> WorkflowProgressEmitterFuture<'a>;
}

impl<F> WorkflowProgressEmitter for F
where
    F: Fn(WorkflowRunUpdatedEvent) -> WorkflowProgressEmitterFuture<'static> + Send + Sync,
{
    fn emit<'a>(&'a self, event: WorkflowRunUpdatedEvent) -> WorkflowProgressEmitterFuture<'a> {
        self(event)
    }
}

#[derive(Clone)]
pub(crate) struct WorkflowProgressContext {
    pub(crate) thread_id: ThreadId,
    pub(crate) turn_id: String,
    pub(crate) call_id: String,
    pub(crate) emitter: Arc<dyn WorkflowProgressEmitter>,
}

impl fmt::Debug for WorkflowProgressContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkflowProgressContext")
            .field("thread_id", &self.thread_id)
            .field("turn_id", &self.turn_id)
            .field("call_id", &self.call_id)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct WorkflowRunResult {
    pub(crate) meta: WorkflowMeta,
    pub(crate) result: JsonValue,
    pub(crate) logs: Vec<String>,
    pub(crate) phases: Vec<String>,
    #[serde(rename = "agentCount")]
    pub(crate) agent_count: usize,
    #[serde(rename = "durationMs")]
    pub(crate) duration_ms: u128,
}

struct WorkflowInterpreter {
    args: JsonValue,
    cwd: String,
    runner: Arc<dyn AgentRunner>,
    limiter: Arc<Semaphore>,
    shared: Arc<SharedRuntimeState>,
    scopes: Vec<HashMap<String, RuntimeValue>>,
}

#[derive(Debug)]
struct SharedRuntimeState {
    meta: WorkflowMeta,
    progress: Option<WorkflowProgressContext>,
    started_at_ms: i64,
    logs: Mutex<Vec<String>>,
    phases: Mutex<Vec<String>>,
    current_phase: Mutex<Option<String>>,
    agents: Mutex<Vec<WorkflowAgentProgress>>,
    agent_count: Mutex<usize>,
    spent: Mutex<usize>,
    token_budget: Option<usize>,
}

struct SharedRuntimeSnapshot {
    logs: Vec<String>,
    phases: Vec<String>,
    agent_count: usize,
}

pub(crate) async fn run_workflow(
    script: &str,
    args: Option<JsonValue>,
    cwd: String,
    concurrency: usize,
    runner: Arc<dyn AgentRunner>,
    progress: Option<WorkflowProgressContext>,
) -> Result<WorkflowRunResult, String> {
    let started = Instant::now();
    let parsed = parse_workflow_script(script)?;
    let statements = parse_workflow_body(&parsed.body)?;
    let shared = Arc::new(SharedRuntimeState::new(
        parsed.meta.clone(),
        progress,
        /*token_budget*/ None,
    ));
    shared.emit_progress(WorkflowRunStatus::Running).await;
    let mut interpreter = WorkflowInterpreter {
        args: args.unwrap_or(JsonValue::Null),
        cwd,
        runner,
        limiter: Arc::new(Semaphore::new(concurrency.clamp(1, 16))),
        shared: Arc::clone(&shared),
        scopes: vec![HashMap::new()],
    };
    let result = match interpreter.exec_block(&statements).await {
        Ok(result) => result.unwrap_or(RuntimeValue::Null),
        Err(err) => {
            shared.emit_progress(WorkflowRunStatus::Failed).await;
            return Err(err);
        }
    };
    let snapshot = shared.snapshot().await;
    if snapshot.agent_count == 0 {
        shared.emit_progress(WorkflowRunStatus::Failed).await;
        return Err("workflow scripts must call agent() at least once; this workflow declared phases but did not run any subagents".to_string());
    }
    let result = match result.to_json() {
        Ok(result) => result,
        Err(err) => {
            shared.emit_progress(WorkflowRunStatus::Failed).await;
            return Err(err);
        }
    };
    shared.emit_progress(WorkflowRunStatus::Completed).await;
    Ok(WorkflowRunResult {
        meta: parsed.meta,
        result,
        logs: snapshot.logs,
        phases: snapshot.phases,
        agent_count: snapshot.agent_count,
        duration_ms: started.elapsed().as_millis(),
    })
}

impl SharedRuntimeState {
    fn new(
        meta: WorkflowMeta,
        progress: Option<WorkflowProgressContext>,
        token_budget: Option<usize>,
    ) -> Self {
        Self {
            meta,
            progress,
            started_at_ms: now_unix_timestamp_ms(),
            logs: Mutex::new(Vec::new()),
            phases: Mutex::new(Vec::new()),
            current_phase: Mutex::new(None),
            agents: Mutex::new(Vec::new()),
            agent_count: Mutex::new(0),
            spent: Mutex::new(0),
            token_budget,
        }
    }

    async fn log(&self, message: String) {
        self.logs.lock().await.push(message);
        self.emit_progress(WorkflowRunStatus::Running).await;
    }

    async fn record_phase(&self, title: String) {
        *self.current_phase.lock().await = Some(title.clone());
        {
            let mut phases = self.phases.lock().await;
            if !phases.contains(&title) {
                phases.push(title);
            }
        }
        self.emit_progress(WorkflowRunStatus::Running).await;
    }

    async fn current_phase(&self) -> Option<String> {
        self.current_phase.lock().await.clone()
    }

    async fn next_agent_index(&self) -> usize {
        let mut agent_count = self.agent_count.lock().await;
        *agent_count += 1;
        *agent_count
    }

    async fn record_agent_started(
        &self,
        id: String,
        label: String,
        prompt: String,
        phase: Option<String>,
        options: &AgentOptions,
    ) {
        self.agents.lock().await.push(WorkflowAgentProgress {
            id,
            label,
            prompt,
            phase,
            status: WorkflowAgentStatus::Running,
            started_at_ms: now_unix_timestamp_ms(),
            completed_at_ms: None,
            error: None,
            model: options.model.clone(),
            agent_type: options.agent_type.clone(),
        });
        self.emit_progress(WorkflowRunStatus::Running).await;
    }

    async fn record_agent_completed(&self, id: &str) {
        self.update_agent_finished(id, WorkflowAgentStatus::Completed, None)
            .await;
    }

    async fn record_agent_failed(&self, id: &str, error: String) {
        self.update_agent_finished(id, WorkflowAgentStatus::Failed, Some(error))
            .await;
    }

    async fn update_agent_finished(
        &self,
        id: &str,
        status: WorkflowAgentStatus,
        error: Option<String>,
    ) {
        {
            let mut agents = self.agents.lock().await;
            if let Some(agent) = agents.iter_mut().find(|agent| agent.id == id) {
                agent.status = status;
                agent.completed_at_ms = Some(now_unix_timestamp_ms());
                agent.error = error;
            }
        }
        self.emit_progress(WorkflowRunStatus::Running).await;
    }

    async fn add_spent(&self, amount: usize) {
        let mut spent = self.spent.lock().await;
        *spent = spent.saturating_add(amount);
    }

    async fn spent(&self) -> usize {
        *self.spent.lock().await
    }

    async fn remaining(&self) -> Option<usize> {
        let total = self.token_budget?;
        Some(total.saturating_sub(self.spent().await))
    }

    async fn is_budget_exhausted(&self) -> bool {
        self.remaining().await == Some(0)
    }

    async fn snapshot(&self) -> SharedRuntimeSnapshot {
        let logs = self.logs.lock().await.clone();
        let phases = self.phases.lock().await.clone();
        let agent_count = *self.agent_count.lock().await;
        SharedRuntimeSnapshot {
            logs,
            phases,
            agent_count,
        }
    }

    async fn emit_progress(&self, status: WorkflowRunStatus) {
        let Some(progress) = self.progress.as_ref() else {
            return;
        };
        let logs = self.logs.lock().await.clone();
        let mut phases = self.phases.lock().await.clone();
        let agents = self.agents.lock().await.clone();
        for agent in &agents {
            if let Some(phase) = agent.phase.as_ref()
                && !phases.contains(phase)
            {
                phases.push(phase.clone());
            }
        }

        let phase_progress = phases
            .into_iter()
            .map(|title| {
                let phase_agents = agents
                    .iter()
                    .filter(|agent| agent.phase.as_deref() == Some(title.as_str()));
                let mut agent_count = 0;
                let mut running_agent_count = 0;
                let mut completed_agent_count = 0;
                let mut failed_agent_count = 0;
                for agent in phase_agents {
                    agent_count += 1;
                    match agent.status {
                        WorkflowAgentStatus::Running => running_agent_count += 1,
                        WorkflowAgentStatus::Completed => completed_agent_count += 1,
                        WorkflowAgentStatus::Failed => failed_agent_count += 1,
                    }
                }
                WorkflowPhaseProgress {
                    title,
                    agent_count,
                    running_agent_count,
                    completed_agent_count,
                    failed_agent_count,
                }
            })
            .collect::<Vec<_>>();

        let agent_count = u32::try_from(agents.len()).unwrap_or(u32::MAX);
        let running_agent_count = count_agents_with_status(&agents, WorkflowAgentStatus::Running);
        let completed_agent_count =
            count_agents_with_status(&agents, WorkflowAgentStatus::Completed);
        let failed_agent_count = count_agents_with_status(&agents, WorkflowAgentStatus::Failed);
        let completed_at_ms = match status {
            WorkflowRunStatus::Running => None,
            WorkflowRunStatus::Completed | WorkflowRunStatus::Failed => {
                Some(now_unix_timestamp_ms())
            }
        };

        progress
            .emitter
            .emit(WorkflowRunUpdatedEvent {
                thread_id: progress.thread_id,
                turn_id: progress.turn_id.clone(),
                call_id: progress.call_id.clone(),
                workflow_name: self.meta.name.clone(),
                workflow_description: self.meta.description.clone(),
                status,
                phases: phase_progress,
                agents,
                logs,
                agent_count,
                running_agent_count,
                completed_agent_count,
                failed_agent_count,
                started_at_ms: self.started_at_ms,
                updated_at_ms: now_unix_timestamp_ms(),
                completed_at_ms,
            })
            .await;
    }
}

impl WorkflowInterpreter {
    fn branch_interpreter(&self) -> Self {
        Self {
            args: self.args.clone(),
            cwd: self.cwd.clone(),
            runner: Arc::clone(&self.runner),
            limiter: Arc::clone(&self.limiter),
            shared: Arc::clone(&self.shared),
            scopes: self.scopes.clone(),
        }
    }

    fn exec_block<'a>(
        &'a mut self,
        statements: &'a [Stmt],
    ) -> Pin<Box<dyn Future<Output = Result<Option<RuntimeValue>, String>> + Send + 'a>> {
        Box::pin(async move {
            for statement in statements {
                if let Some(value) = self.exec_stmt(statement).await? {
                    return Ok(Some(value));
                }
            }
            Ok(None)
        })
    }

    fn exec_stmt<'a>(
        &'a mut self,
        statement: &'a Stmt,
    ) -> Pin<Box<dyn Future<Output = Result<Option<RuntimeValue>, String>> + Send + 'a>> {
        Box::pin(async move {
            match statement {
                Stmt::Let { name, expr } => {
                    let value = self.eval_expr(expr).await?;
                    self.set_local(name.clone(), value);
                    Ok(None)
                }
                Stmt::Expr(expr) => {
                    self.eval_expr(expr).await?;
                    Ok(None)
                }
                Stmt::Return(expr) => Ok(Some(self.eval_expr(expr).await?)),
                Stmt::If {
                    condition,
                    then_branch,
                    else_branch,
                } => {
                    if self.eval_expr(condition).await?.is_truthy() {
                        self.push_scope();
                        let result = self.exec_block(then_branch).await;
                        self.pop_scope();
                        result
                    } else {
                        self.push_scope();
                        let result = self.exec_block(else_branch).await;
                        self.pop_scope();
                        result
                    }
                }
                Stmt::ForOf {
                    item,
                    iterable,
                    body,
                } => {
                    let values = match self.eval_expr(iterable).await? {
                        RuntimeValue::Array(values) => values,
                        RuntimeValue::Null => Vec::new(),
                        _ => return Err("for...of expects an array".to_string()),
                    };
                    for value in values {
                        self.push_scope();
                        self.set_local(item.clone(), value);
                        let result = self.exec_block(body).await;
                        self.pop_scope();
                        if result.as_ref().is_ok_and(Option::is_some) {
                            return result;
                        }
                        result?;
                    }
                    Ok(None)
                }
            }
        })
    }

    fn eval_expr<'a>(
        &'a mut self,
        expr: &'a Expr,
    ) -> Pin<Box<dyn Future<Output = Result<RuntimeValue, String>> + Send + 'a>> {
        Box::pin(async move {
            match expr {
                Expr::Null => Ok(RuntimeValue::Null),
                Expr::Bool(value) => Ok(RuntimeValue::Bool(*value)),
                Expr::Number(value) => Ok(RuntimeValue::Number(*value)),
                Expr::String(value) => Ok(RuntimeValue::String(value.clone())),
                Expr::Identifier(name) => self.resolve_identifier(name),
                Expr::Array(values) => {
                    let mut out = Vec::with_capacity(values.len());
                    for value in values {
                        out.push(self.eval_expr(value).await?);
                    }
                    Ok(RuntimeValue::Array(out))
                }
                Expr::Object(properties) => self.eval_object(properties).await,
                Expr::Member(object, property) => {
                    let object = self.eval_expr(object).await?;
                    Ok(object.member(property).unwrap_or(RuntimeValue::Null))
                }
                Expr::Index(object, index) => {
                    let object = self.eval_expr(object).await?;
                    let index = self.eval_expr(index).await?;
                    Ok(object.index(&index).unwrap_or(RuntimeValue::Null))
                }
                Expr::Call(callee, args) => self.eval_call(callee, args).await,
                Expr::Await(inner) => self.eval_expr(inner).await,
                Expr::Unary { op, expr } => {
                    let value = self.eval_expr(expr).await?;
                    match op {
                        UnaryOp::Not => Ok(RuntimeValue::Bool(!value.is_truthy())),
                        UnaryOp::Negate => Ok(RuntimeValue::Number(-value.as_number())),
                    }
                }
                Expr::Binary { left, op, right } => self.eval_binary(left, *op, right).await,
                Expr::ArrowFunction { params, body } => {
                    Ok(RuntimeValue::Function(Arc::new(FunctionValue {
                        params: params.clone(),
                        body: body.clone(),
                        captured_scopes: self.scopes.clone(),
                    })))
                }
            }
        })
    }

    async fn eval_object(&mut self, properties: &[ObjectProperty]) -> Result<RuntimeValue, String> {
        let mut out = HashMap::new();
        for property in properties {
            let value = self.eval_expr(&property.value).await?;
            out.insert(property.key.clone(), value);
        }
        Ok(RuntimeValue::Object(out))
    }

    async fn eval_call(&mut self, callee: &Expr, args: &[Expr]) -> Result<RuntimeValue, String> {
        if let Expr::Identifier(name) = callee {
            return self.call_builtin(name, args).await;
        }
        if let Expr::Member(object, method) = callee {
            return self.call_method(object, method, args).await;
        }
        let callee = self.eval_expr(callee).await?;
        let RuntimeValue::Function(function) = callee else {
            return Err("workflow call target is not a function".to_string());
        };
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            values.push(self.eval_expr(arg).await?);
        }
        self.invoke_function(&function, values).await
    }

    async fn call_builtin(&mut self, name: &str, args: &[Expr]) -> Result<RuntimeValue, String> {
        match name {
            "phase" => {
                let title = self.required_string_arg(args, 0, "phase title").await?;
                self.shared.record_phase(title).await;
                Ok(RuntimeValue::Null)
            }
            "log" => {
                let message = if let Some(arg) = args.first() {
                    self.eval_expr(arg).await?.to_display_string()
                } else {
                    "undefined".to_string()
                };
                self.shared.log(message).await;
                Ok(RuntimeValue::Null)
            }
            "agent" => {
                let prompt = self.required_string_arg(args, 0, "agent prompt").await?;
                let options = if let Some(arg) = args.get(1) {
                    AgentOptions::from_value(self.eval_expr(arg).await?)?
                } else {
                    AgentOptions::default()
                };
                self.run_agent(prompt, options).await
            }
            "parallel" => {
                let thunks =
                    match self
                        .eval_expr(args.first().ok_or_else(|| {
                            "parallel() expects an array of functions".to_string()
                        })?)
                        .await?
                    {
                        RuntimeValue::Array(values) => values,
                        _ => return Err("parallel() expects an array of functions".to_string()),
                    };
                let mut functions = Vec::with_capacity(thunks.len());
                for thunk in thunks {
                    let RuntimeValue::Function(function) = thunk else {
                        return Err(
                            "parallel() expects an array of functions, not promises. Wrap each call: () => agent(...)"
                                .to_string(),
                        );
                    };
                    functions.push(function);
                }
                let count = functions.len();
                let mut tasks = JoinSet::new();
                for (index, function) in functions.into_iter().enumerate() {
                    let mut branch = self.branch_interpreter();
                    tasks.spawn(async move {
                        let result = branch.invoke_function(&function, Vec::new()).await;
                        (index, result)
                    });
                }
                let mut results = vec![RuntimeValue::Null; count];
                while let Some(joined) = tasks.join_next().await {
                    let (index, result) =
                        joined.map_err(|err| format!("parallel branch failed to join: {err}"))?;
                    match result {
                        Ok(value) => results[index] = value,
                        Err(err) => {
                            self.shared
                                .log(format!("parallel[{index}] failed: {err}"))
                                .await;
                        }
                    }
                }
                Ok(RuntimeValue::Array(results))
            }
            "pipeline" => self.run_pipeline(args).await,
            "String" => {
                let value = if let Some(arg) = args.first() {
                    self.eval_expr(arg).await?.to_display_string()
                } else {
                    String::new()
                };
                Ok(RuntimeValue::String(value))
            }
            _ => Err(format!("unsupported workflow function `{name}`")),
        }
    }

    async fn call_method(
        &mut self,
        object: &Expr,
        method: &str,
        args: &[Expr],
    ) -> Result<RuntimeValue, String> {
        if method == "cwd"
            && let Expr::Identifier(name) = object
            && name == "process"
        {
            if !args.is_empty() {
                return Err("process.cwd() does not accept arguments".to_string());
            }
            return Ok(RuntimeValue::String(self.cwd.clone()));
        }

        if method == "stringify"
            && let Expr::Identifier(name) = object
            && name == "JSON"
        {
            let value = if let Some(arg) = args.first() {
                self.eval_expr(arg).await?.to_json()?
            } else {
                JsonValue::Null
            };
            return serde_json::to_string(&value)
                .map(RuntimeValue::String)
                .map_err(|err| format!("failed to stringify JSON: {err}"));
        }

        if let Expr::Identifier(name) = object
            && name == "budget"
        {
            if !args.is_empty() {
                return Err(format!("budget.{method}() does not accept arguments"));
            }
            match method {
                "spent" => return Ok(RuntimeValue::Number(self.shared.spent().await as f64)),
                "remaining" => {
                    return Ok(RuntimeValue::Number(
                        self.shared
                            .remaining()
                            .await
                            .map(|remaining| remaining as f64)
                            .unwrap_or(f64::INFINITY),
                    ));
                }
                _ => {}
            }
        }

        if let Expr::Identifier(name) = object
            && matches!(name.as_str(), "Promise" | "Date" | "Math")
        {
            return Ok(RuntimeValue::Null);
        }

        let object_value = self.eval_expr(object).await?;
        if method == "map" {
            let RuntimeValue::Array(items) = object_value else {
                return Err("map() expects an array receiver".to_string());
            };
            let mapper = match self
                .eval_expr(
                    args.first()
                        .ok_or_else(|| "map() expects a mapper function".to_string())?,
                )
                .await?
            {
                RuntimeValue::Function(function) => function,
                _ => return Err("map() expects a mapper function".to_string()),
            };
            let mut mapped = Vec::with_capacity(items.len());
            for (index, item) in items.into_iter().enumerate() {
                mapped.push(
                    self.invoke_function(
                        &mapper,
                        vec![item.clone(), RuntimeValue::Number(index as f64)],
                    )
                    .await?,
                );
            }
            return Ok(RuntimeValue::Array(mapped));
        }

        Err(format!("unsupported workflow method `{method}`"))
    }

    async fn run_pipeline(&mut self, args: &[Expr]) -> Result<RuntimeValue, String> {
        let items =
            match self
                .eval_expr(args.first().ok_or_else(|| {
                    "pipeline() expects an array as the first argument".to_string()
                })?)
                .await?
            {
                RuntimeValue::Array(values) => values,
                _ => return Err("pipeline() expects an array as the first argument".to_string()),
            };
        let mut stages = Vec::new();
        for arg in &args[1..] {
            match self.eval_expr(arg).await? {
                RuntimeValue::Function(function) => stages.push(function),
                _ => {
                    return Err(
                        "pipeline() stages must be functions: pipeline(items, item => ..., result => ...)"
                            .to_string(),
                    );
                }
            }
        }

        let count = items.len();
        let mut tasks = JoinSet::new();
        for (index, item) in items.into_iter().enumerate() {
            let stages = stages.clone();
            let mut branch = self.branch_interpreter();
            tasks.spawn(async move {
                let original = item.clone();
                let mut value = item;
                for stage in &stages {
                    match branch
                        .invoke_function(
                            stage,
                            vec![
                                value.clone(),
                                original.clone(),
                                RuntimeValue::Number(index as f64),
                            ],
                        )
                        .await
                    {
                        Ok(next) => value = next,
                        Err(err) => return (index, Err(err)),
                    }
                }
                (index, Ok(value))
            });
        }
        let mut results = vec![RuntimeValue::Null; count];
        while let Some(joined) = tasks.join_next().await {
            let (index, result) =
                joined.map_err(|err| format!("pipeline branch failed to join: {err}"))?;
            match result {
                Ok(value) => results[index] = value,
                Err(err) => {
                    self.shared
                        .log(format!("pipeline[{index}] failed: {err}"))
                        .await;
                }
            }
        }
        Ok(RuntimeValue::Array(results))
    }

    async fn invoke_function(
        &mut self,
        function: &FunctionValue,
        args: Vec<RuntimeValue>,
    ) -> Result<RuntimeValue, String> {
        let saved_scopes = std::mem::replace(&mut self.scopes, function.captured_scopes.clone());
        self.push_scope();
        for (index, param) in function.params.iter().enumerate() {
            self.set_local(
                param.clone(),
                args.get(index).cloned().unwrap_or(RuntimeValue::Null),
            );
        }
        let result = match &function.body {
            FunctionBody::Expr(expr) => self.eval_expr(expr).await,
            FunctionBody::Block(statements) => self
                .exec_block(statements)
                .await
                .map(|value| value.unwrap_or(RuntimeValue::Null)),
        };
        self.scopes = saved_scopes;
        result
    }

    async fn run_agent(
        &mut self,
        prompt: String,
        options: AgentOptions,
    ) -> Result<RuntimeValue, String> {
        if self.shared.is_budget_exhausted().await {
            return Err("workflow token budget exhausted".to_string());
        }
        let assigned_phase = match options.phase.clone() {
            Some(phase) => Some(phase),
            None => self.shared.current_phase().await,
        };
        let agent_index = self.shared.next_agent_index().await;
        let label = options
            .label
            .as_deref()
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| default_agent_label(assigned_phase.as_deref(), agent_index));
        let agent_id = format!("agent-{agent_index}");
        self.shared
            .record_agent_started(
                agent_id.clone(),
                label.clone(),
                prompt.clone(),
                assigned_phase.clone(),
                &options,
            )
            .await;
        let request = AgentRunRequest {
            prompt,
            label: label.clone(),
            phase: assigned_phase.clone(),
            schema: options.schema.clone(),
            instructions: build_agent_instructions(assigned_phase.as_deref(), &options),
            model: options.model.clone(),
            agent_type: options.agent_type.clone(),
        };
        let permit = self
            .limiter
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "workflow concurrency limiter closed".to_string())?;
        let result = self.runner.run_agent(request).await;
        drop(permit);
        match result {
            Ok(response) => {
                self.shared
                    .add_spent(estimate_tokens(&response.value))
                    .await;
                self.shared.record_agent_completed(&agent_id).await;
                Ok(RuntimeValue::from_json(response.value))
            }
            Err(err) => {
                self.shared
                    .log(format!("agent {label} failed: {err}"))
                    .await;
                self.shared.record_agent_failed(&agent_id, err).await;
                Ok(RuntimeValue::Null)
            }
        }
    }

    async fn required_string_arg(
        &mut self,
        args: &[Expr],
        index: usize,
        name: &str,
    ) -> Result<String, String> {
        let value = self
            .eval_expr(
                args.get(index)
                    .ok_or_else(|| format!("{name} must be a string"))?,
            )
            .await?;
        value
            .as_string()
            .ok_or_else(|| format!("{name} must be a string"))
    }

    async fn eval_binary(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
    ) -> Result<RuntimeValue, String> {
        if op == BinaryOp::And {
            let left = self.eval_expr(left).await?;
            if !left.is_truthy() {
                return Ok(left);
            }
            return self.eval_expr(right).await;
        }
        if op == BinaryOp::Or {
            let left = self.eval_expr(left).await?;
            if left.is_truthy() {
                return Ok(left);
            }
            return self.eval_expr(right).await;
        }
        let left = self.eval_expr(left).await?;
        let right = self.eval_expr(right).await?;
        match op {
            BinaryOp::Add => {
                if left.is_string_like() || right.is_string_like() {
                    Ok(RuntimeValue::String(format!(
                        "{}{}",
                        left.to_display_string(),
                        right.to_display_string()
                    )))
                } else {
                    Ok(RuntimeValue::Number(left.as_number() + right.as_number()))
                }
            }
            BinaryOp::Subtract => Ok(RuntimeValue::Number(left.as_number() - right.as_number())),
            BinaryOp::Multiply => Ok(RuntimeValue::Number(left.as_number() * right.as_number())),
            BinaryOp::Divide => Ok(RuntimeValue::Number(left.as_number() / right.as_number())),
            BinaryOp::Remainder => Ok(RuntimeValue::Number(left.as_number() % right.as_number())),
            BinaryOp::Equal => Ok(RuntimeValue::Bool(left.to_json()? == right.to_json()?)),
            BinaryOp::NotEqual => Ok(RuntimeValue::Bool(left.to_json()? != right.to_json()?)),
            BinaryOp::Less => Ok(RuntimeValue::Bool(left.as_number() < right.as_number())),
            BinaryOp::LessEqual => Ok(RuntimeValue::Bool(left.as_number() <= right.as_number())),
            BinaryOp::Greater => Ok(RuntimeValue::Bool(left.as_number() > right.as_number())),
            BinaryOp::GreaterEqual => Ok(RuntimeValue::Bool(left.as_number() >= right.as_number())),
            BinaryOp::And | BinaryOp::Or => unreachable!("short-circuit handled above"),
        }
    }

    fn resolve_identifier(&self, name: &str) -> Result<RuntimeValue, String> {
        for scope in self.scopes.iter().rev() {
            if let Some(value) = scope.get(name) {
                return Ok(value.clone());
            }
        }
        match name {
            "args" => Ok(RuntimeValue::from_json(self.args.clone())),
            "cwd" => Ok(RuntimeValue::String(self.cwd.clone())),
            "process" | "JSON" => Ok(RuntimeValue::Object(HashMap::new())),
            "budget" => {
                let mut budget = HashMap::new();
                budget.insert(
                    "total".to_string(),
                    self.shared
                        .token_budget
                        .map(|value| RuntimeValue::Number(value as f64))
                        .unwrap_or(RuntimeValue::Null),
                );
                Ok(RuntimeValue::Object(budget))
            }
            _ => Err(format!("unknown workflow identifier `{name}`")),
        }
    }

    fn set_local(&mut self, name: String, value: RuntimeValue) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, value);
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
        if self.scopes.is_empty() {
            self.scopes.push(HashMap::new());
        }
    }
}

#[derive(Default)]
struct AgentOptions {
    label: Option<String>,
    phase: Option<String>,
    schema: Option<JsonValue>,
    model: Option<String>,
    isolation: Option<String>,
    agent_type: Option<String>,
}

impl AgentOptions {
    fn from_value(value: RuntimeValue) -> Result<Self, String> {
        let RuntimeValue::Object(mut object) = value else {
            return Err("agent options must be an object".to_string());
        };
        Ok(Self {
            label: optional_string(object.remove("label"), "agent label")?,
            phase: optional_string(object.remove("phase"), "agent phase")?,
            schema: object
                .remove("schema")
                .map(|value| value.to_json())
                .transpose()?,
            model: optional_string(object.remove("model"), "agent model")?,
            isolation: optional_string(object.remove("isolation"), "agent isolation")?,
            agent_type: optional_string(
                object
                    .remove("agentType")
                    .or_else(|| object.remove("agent_type")),
                "agent type",
            )?,
        })
    }
}

fn optional_string(value: Option<RuntimeValue>, name: &str) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    value
        .as_string()
        .map(Some)
        .ok_or_else(|| format!("{name} must be a string"))
}

fn default_agent_label(phase: Option<&str>, index: usize) -> String {
    phase
        .map(|phase| format!("{phase} agent {index}"))
        .unwrap_or_else(|| format!("agent {index}"))
}

fn build_agent_instructions(phase: Option<&str>, options: &AgentOptions) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(phase) = phase {
        lines.push(format!("Workflow phase: {phase}"));
    }
    if let Some(agent_type) = options.agent_type.as_deref() {
        lines.push(format!("Act as workflow subagent type: {agent_type}"));
    }
    if let Some(isolation) = options.isolation.as_deref() {
        lines.push(format!("Requested isolation: {isolation}"));
    }
    if let Some(model) = options.model.as_deref() {
        lines.push(format!("Requested model: {model}"));
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn estimate_tokens(value: &JsonValue) -> usize {
    serde_json::to_string(value)
        .map(|text| text.len().div_ceil(4))
        .unwrap_or_default()
}

fn count_agents_with_status(agents: &[WorkflowAgentProgress], status: WorkflowAgentStatus) -> u32 {
    u32::try_from(agents.iter().filter(|agent| agent.status == status).count()).unwrap_or(u32::MAX)
}

fn now_unix_timestamp_ms() -> i64 {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    duration.as_millis().try_into().unwrap_or(i64::MAX)
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
