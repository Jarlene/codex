use std::sync::Arc;
use std::sync::Mutex;

use pretty_assertions::assert_eq;
use serde_json::Value as JsonValue;
use serde_json::json;
use tokio::sync::Barrier;
use tokio::time::Duration;
use tokio::time::timeout;

use super::*;

struct FakeAgent {
    prompts: Mutex<Vec<AgentRunRequest>>,
}

impl AgentRunner for FakeAgent {
    fn run_agent<'a>(&'a self, request: AgentRunRequest) -> AgentRunnerFuture<'a> {
        Box::pin(async move {
            self.prompts
                .lock()
                .expect("not poisoned")
                .push(request.clone());
            Ok(AgentRunResponse {
                value: JsonValue::String(format!("result:{}", request.prompt)),
            })
        })
    }
}

struct BarrierAgent {
    barrier: Arc<Barrier>,
}

impl AgentRunner for BarrierAgent {
    fn run_agent<'a>(&'a self, request: AgentRunRequest) -> AgentRunnerFuture<'a> {
        Box::pin(async move {
            self.barrier.wait().await;
            Ok(AgentRunResponse {
                value: JsonValue::String(format!("result:{}", request.prompt)),
            })
        })
    }
}

#[tokio::test]
async fn accepts_metadata_without_phases_and_records_runtime_phases() {
    let agent = Arc::new(FakeAgent {
        prompts: Mutex::new(Vec::new()),
    });
    let result = run_workflow(
        r#"workflow! {
  meta: {
    name: "dynamic_demo",
    description: "Use runtime phases",
  }

  phase("Scan");
  let scan = agent("scan", { label: "scan" });
  return { scan };
}"#,
        None,
        "/tmp".to_string(),
        4,
        agent,
        None,
    )
    .await
    .expect("workflow succeeds");

    assert_eq!(result.phases, vec!["Scan".to_string()]);
    assert_eq!(result.agent_count, 1);
    assert_eq!(result.result, json!({ "scan": "result:scan" }));
}

#[tokio::test]
async fn records_loop_created_phases_without_skipped_conditional_phases() {
    let agent = Arc::new(FakeAgent {
        prompts: Mutex::new(Vec::new()),
    });
    let result = run_workflow(
        r#"workflow! {
  meta: {
    name: "loop_demo",
    description: "Create phases from work items",
    phases: [{ title: "Review" }],
  }

  if args.needsReview {
    phase("Review");
    agent("review", { label: "review" });
  }

  for area in args.areas {
    phase("Inspect " + area);
    agent("inspect " + area, { label: "inspect " + area });
  }

  return { ok: true };
}"#,
        Some(json!({ "needsReview": false, "areas": ["API", "UI"] })),
        "/tmp".to_string(),
        4,
        agent,
        None,
    )
    .await
    .expect("workflow succeeds");

    assert_eq!(
        result.phases,
        vec!["Inspect API".to_string(), "Inspect UI".to_string()]
    );
    assert_eq!(result.agent_count, 2);
}

#[tokio::test]
async fn emits_progress_snapshots_for_frontend_status() {
    let agent = Arc::new(FakeAgent {
        prompts: Mutex::new(Vec::new()),
    });
    let events = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let captured_events = Arc::clone(&events);
    let thread_id = ThreadId::new();
    let progress = WorkflowProgressContext {
        thread_id,
        turn_id: "turn-1".to_string(),
        call_id: "call-1".to_string(),
        emitter: Arc::new(
            move |event: WorkflowRunUpdatedEvent| -> WorkflowProgressEmitterFuture<'static> {
                let captured_events = Arc::clone(&captured_events);
                Box::pin(async move {
                    captured_events.lock().await.push(event);
                })
            },
        ),
    };

    run_workflow(
        r#"workflow! {
  meta: {
    name: "visible_workflow",
    description: "Expose runtime progress",
  }

  phase("Plan");
  let plan = agent("plan", { label: "plan", agentType: "architect" });
  phase("Build");
  let build = agent("build", { label: "build", model: "gpt-workflow" });
  log("workflow ready");
  return { plan, build };
}"#,
        None,
        "/tmp".to_string(),
        4,
        agent,
        Some(progress),
    )
    .await
    .expect("workflow succeeds");

    let events = events.lock().await.clone();
    let first = events.first().expect("emits initial snapshot");
    assert_eq!(first.thread_id, thread_id);
    assert_eq!(first.turn_id, "turn-1");
    assert_eq!(first.call_id, "call-1");
    assert_eq!(first.workflow_name, "visible_workflow");
    assert_eq!(first.workflow_description, "Expose runtime progress");
    assert_eq!(first.status, WorkflowRunStatus::Running);

    let last = events.last().expect("emits final snapshot");
    assert_eq!(last.status, WorkflowRunStatus::Completed);
    assert_eq!(last.agent_count, 2);
    assert_eq!(last.running_agent_count, 0);
    assert_eq!(last.completed_agent_count, 2);
    assert_eq!(last.failed_agent_count, 0);
    assert_eq!(last.logs, vec!["workflow ready".to_string()]);
    assert_eq!(
        last.phases,
        vec![
            WorkflowPhaseProgress {
                title: "Plan".to_string(),
                agent_count: 1,
                running_agent_count: 0,
                completed_agent_count: 1,
                failed_agent_count: 0,
            },
            WorkflowPhaseProgress {
                title: "Build".to_string(),
                agent_count: 1,
                running_agent_count: 0,
                completed_agent_count: 1,
                failed_agent_count: 0,
            },
        ]
    );
    assert_eq!(
        last.agents
            .iter()
            .map(|agent| {
                (
                    agent.label.as_str(),
                    agent.phase.as_deref(),
                    agent.status,
                    agent.model.as_deref(),
                    agent.agent_type.as_deref(),
                )
            })
            .collect::<Vec<_>>(),
        vec![
            (
                "plan",
                Some("Plan"),
                WorkflowAgentStatus::Completed,
                None,
                Some("architect"),
            ),
            (
                "build",
                Some("Build"),
                WorkflowAgentStatus::Completed,
                Some("gpt-workflow"),
                None,
            ),
        ]
    );
    assert!(last.completed_at_ms.is_some());
}

#[tokio::test]
async fn rejects_non_string_runtime_phase_titles() {
    let agent = Arc::new(FakeAgent {
        prompts: Mutex::new(Vec::new()),
    });
    let err = run_workflow(
        r#"workflow! {
  meta: {
    name: "bad_phase",
    description: "Use a non-string phase title",
  }

  phase({ title: "Scan" });
  return { ok: true };
}"#,
        None,
        "/tmp".to_string(),
        4,
        agent,
        None,
    )
    .await
    .expect_err("workflow fails");

    assert!(err.contains("phase title must be a string"), "{err}");
}

#[tokio::test]
async fn runs_parallel_map_thunks() {
    let agent = Arc::new(FakeAgent {
        prompts: Mutex::new(Vec::new()),
    });
    let result = run_workflow(
        r#"workflow! {
  meta: {
    name: "parallel_demo",
    description: "Run mapped agent thunks",
  }

  phase("Scan");
  let scans = parallel(args.areas.map(|area| || agent("scan " + area, { label: "scan " + area })));
  return { scans };
}"#,
        Some(json!({ "areas": ["API", "UI"] })),
        "/tmp".to_string(),
        4,
        Arc::clone(&agent) as Arc<dyn AgentRunner>,
        None,
    )
    .await
    .expect("workflow succeeds");

    assert_eq!(result.agent_count, 2);
    assert_eq!(
        result.result,
        json!({ "scans": ["result:scan API", "result:scan UI"] })
    );
    assert_eq!(
        agent.prompts.lock().expect("not poisoned").clone(),
        vec![
            AgentRunRequest {
                prompt: "scan API".to_string(),
                label: "scan API".to_string(),
                phase: Some("Scan".to_string()),
                schema: None,
                instructions: Some("Workflow phase: Scan".to_string()),
                model: None,
                agent_type: None,
            },
            AgentRunRequest {
                prompt: "scan UI".to_string(),
                label: "scan UI".to_string(),
                phase: Some("Scan".to_string()),
                schema: None,
                instructions: Some("Workflow phase: Scan".to_string()),
                model: None,
                agent_type: None,
            },
        ]
    );
}

#[tokio::test]
async fn parallel_runs_agent_thunks_concurrently() {
    let agent = Arc::new(BarrierAgent {
        barrier: Arc::new(Barrier::new(2)),
    });

    let result = timeout(
        Duration::from_secs(1),
        run_workflow(
            r#"workflow! {
  meta: {
    name: "parallel_concurrency",
    description: "Run branches concurrently",
  }

  let scans = parallel([
    || agent("one", { label: "one" }),
    || agent("two", { label: "two" }),
  ]);
  return { scans };
}"#,
            None,
            "/tmp".to_string(),
            4,
            agent,
            None,
        ),
    )
    .await
    .expect("parallel branches should not deadlock")
    .expect("workflow succeeds");

    assert_eq!(
        result.result,
        json!({ "scans": ["result:one", "result:two"] })
    );
}

#[tokio::test]
async fn pipeline_runs_items_concurrently() {
    let agent = Arc::new(BarrierAgent {
        barrier: Arc::new(Barrier::new(2)),
    });

    let result = timeout(
        Duration::from_secs(1),
        run_workflow(
            r#"workflow! {
  meta: {
    name: "pipeline_concurrency",
    description: "Run pipeline items concurrently",
  }

  let scans = pipeline(
    args.items,
    |item| agent("scan " + item, { label: "scan " + item }),
    |previous| agent("review " + previous, { label: "review " + previous }),
  );
  return { scans };
}"#,
            Some(json!({ "items": ["API", "UI"] })),
            "/tmp".to_string(),
            4,
            agent,
            None,
        ),
    )
    .await
    .expect("pipeline items should not deadlock")
    .expect("workflow succeeds");

    assert_eq!(
        result.result,
        json!({
            "scans": [
                "result:review result:scan API",
                "result:review result:scan UI",
            ],
        })
    );
}
