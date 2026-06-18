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
        r#"workflow! { meta: { name: "flea_market_platform", description: "Build a C2C flea market platform end-to-end with requirements, design, plan, implementation, tests, audit documents, and evaluation", phases:[{title: "Discovery", detail: "Inspect
  repository and constraints", model: "gpt-5.4"}, {title: "Requirements", detail: "Produce PRD and acceptance criteria", model: "gpt-5.4"}, {title: "Design", detail: "Produce architecture, data model, API, security, and UX design", model:
  "gpt-5.4"}, {title: "Planning", detail: "Produce implementation and verification plan", model: "gpt-5.4"}, {title: "Implementation", detail: "Implement frontend, backend, database, storage, realtime, docs", model: "gpt-5.4"}, {title:
  "Verification", detail: "Run tests, lint/type checks, and integration checks", model: "gpt-5.4"}, {title: "Audit", detail: "Code review, security review, and final evaluation", model: "gpt-5.4"}] }

  phase("Discovery");
  let repo = agent("You are the repository discovery agent. Work in cwd. Inspect the project structure, existing files, package/tooling state, AGENTS.md instructions, git status, and available build tools. Do not modify files. Return a concise
  machine-readable inventory with: repo_type, existing_files, likely_stack, constraints, commands_available, risks, recommended_scaffold. The user asks to build from 0 to 1 a C2C second-hand marketplace with Vue 3/Vite/Vue Router/Pinia, Go/Gin/
  GORM/JWT, PostgreSQL/Redis, and MinIO. Use rg/ls/git status as needed.", { label: "repo discovery", phase: "Discovery", agent_type: "researcher" });

  phase("Requirements");
  let requirements = agent("Using this repository inventory and the user request, create project documentation for requirements. You may edit files. Create docs/requirements.md with a full PRD in Chinese covering: product goals, personas,
  feature scope, functional requirements for auth/account, product publishing/browsing/category, realtime IM, comments, escrow transaction, admin or moderation boundaries if needed, non-functional requirements, acceptance criteria, out-of-scope,
  assumptions, and risk evaluation. Keep it practical for a first complete implementation. Repository inventory: " + repo, { label: "requirements doc", phase: "Requirements", agent_type: "requirement" });

  phase("Design");
  let design_results = parallel([
    || agent("Create technical architecture documentation in Chinese for the C2C flea market platform. You may edit files. Based on repo inventory and requirements result, create docs/design.md covering system architecture, frontend module
    layout, backend service/module layout, storage strategy, Redis usage, MinIO usage, realtime chat approach, escrow transaction state machine, deployment topology, and design tradeoffs. Inventory: " + repo + " Requirements: " + requirements,
    { label: "architecture doc", phase: "Design", agent_type: "architect" }),
    || agent("Create API and data model documentation in Chinese. You may edit files. Create docs/api.md with REST endpoint contracts and websocket message contracts, and docs/data-model.md with PostgreSQL tables/entities, important indexes,
    Redis keys, MinIO buckets/object naming, and migration strategy. Cover auth/account, products/categories/images, chats/messages, comments, orders/escrow/payments simulation. Inventory: " + repo + " Requirements: " + requirements, { label:
    "api data docs", phase: "Design", agent_type: "architect" }),
    || agent("Create UX and security design documentation in Chinese. You may edit files. Create docs/ux.md covering main screens and user flows for buyer/seller messaging/comment/order flows, and docs/security.md covering JWT, password hashing,
    authorization, upload validation, chat/order access control, rate limit considerations, escrow risk model, and privacy/audit logging. Inventory: " + repo + " Requirements: " + requirements, { label: "ux security docs", phase: "Design",
    agent_type: "architect" })
  ]);

  phase("Planning");
  let plan = agent("Create implementation and verification plan documentation in Chinese. You may edit files. Create docs/implementation-plan.md and docs/test-plan.md. The plan must sequence scaffolding, backend modules, frontend modules, infra,
  tests, and review. Include parallelization opportunities and acceptance mapping. Use the repository inventory, requirements, and design outputs. Inventory: " + repo + " Requirements: " + requirements + " Design outputs: " + design_results,
  { label: "plan docs", phase: "Planning", agent_type: "planner" });

  phase("Implementation");
  let impl_results = parallel([
    || agent("Implement the Go backend for the flea market platform in this repository. You may edit files. Follow existing repo/AGENTS instructions. Create a backend with Go + Gin + GORM + JWT, PostgreSQL, Redis, MinIO integration points. If
    there is no existing backend, scaffold under backend/. Implement: config loading, DB models/migrations/auto-migrate, auth register/login/me, account profile, categories, product publish/list/detail/update/status, comments, conversations/
    messages via websocket and REST history, escrow order lifecycle endpoints. Include middleware for JWT auth and ownership checks. Include unit or handler tests where practical. Avoid adding dependencies beyond the requested stack unless
    unavoidable. Document env vars. Return changed files and test commands. Requirements: " + requirements + " Design: " + design_results + " Plan: " + plan + " Repo: " + repo, { label: "backend implementation", phase: "Implementation",
    agent_type: "coder" }),
    || agent("Implement the Vue 3 frontend for the flea market platform in this repository. You may edit files. Follow existing repo/AGENTS instructions. If there is no existing frontend, scaffold under frontend/ with Vite + Vue 3 + Vue Router +
    Pinia. Implement usable first screen and routes for login/register, product browse/category filters/search, product detail with comments, publish/edit product, conversations/chat, orders/escrow dashboard, profile/account. Provide API client,
    auth store, product store, chat store, order store, route guards, responsive UI. Use existing patterns if any. Include tests where practical. Return changed files and test commands. Requirements: " + requirements + " Design: " +
    design_results + " Plan: " + plan + " Repo: " + repo, { label: "frontend implementation", phase: "Implementation", agent_type: "coder" }),
    || agent("Implement project-level infrastructure and developer experience files. You may edit files. Create docker-compose.yml for PostgreSQL, Redis, MinIO, backend, frontend if practical; create .env.example files; create Makefile or
    scripts for setup/test/run; create README.md in Chinese with quick start, architecture summary, env vars, and verification commands. Add docs/evaluation.md initial evaluation skeleton if not created elsewhere. Coordinate with backend/
    frontend paths without overwriting their work; inspect before editing shared files. Requirements: " + requirements + " Design: " + design_results + " Plan: " + plan + " Repo: " + repo, { label: "infra docs implementation", phase:
    "Implementation", agent_type: "coder" })
  ]);

  let integration = agent("Integrate the parallel implementation results. You may edit files. Inspect the repository after the backend, frontend, and infra lanes. Resolve conflicts/inconsistencies between frontend API paths and backend routes,
  env vars, README, docker-compose, and tests. Add or update docs/evaluation.md with a feature completion matrix, quality assessment, known risks, and next milestones. Keep diffs scoped. Return changed files and commands to verify.
  Implementation lane outputs: " + impl_results, { label: "integration pass", phase: "Implementation", agent_type: "coder" });

  phase("Verification");
  let verification = agent("Run verification for the implemented project. You may edit files only to fix verification failures. Inspect README/Makefile/package/go files to determine commands. Run backend tests, frontend tests/build/typecheck if
  available, gofmt/go test, npm install only if dependencies are missing and network/cache allow it, and docker compose config if possible. Fix issues that are clearly in scope, then rerun relevant checks. Create docs/test-report.md in Chinese
  with commands, outputs summarized, pass/fail status, fixes made, and residual gaps. Return verification status, commands run, fixed files, and blockers. Integration output: " + integration, { label: "verification run", phase: "Verification",
  agent_type: "tester" });

  phase("Audit");
  let audit_results = parallel([
    || agent("Perform a code review of the completed implementation. Do not make code changes unless a critical obvious issue can be safely fixed; if you fix anything, report it. Focus on bugs, integration mismatches, missing tests,
    maintainability, and correctness. Create docs/code-audit.md in Chinese with findings ordered by severity, file references, recommendations, and final verdict. Verification output: " + verification + " Implementation: " + integration,
    { label: "code audit", phase: "Audit", agent_type: "reviewer" }),
    || agent("Perform a security review of the completed implementation. Do not make code changes unless a critical obvious issue can be safely fixed; if you fix anything, report it. Focus on auth/JWT, password hashing, authorization, file
    upload, websocket access control, order/escrow state transitions, secrets/config, CORS, rate limiting, and privacy. Update or create docs/security-audit.md in Chinese with findings, severity, evidence, and recommendations. Verification
    output: " + verification + " Implementation: " + integration, { label: "security audit", phase: "Audit", agent_type: "reviewer" })
  ]);

  let final_eval = agent("Produce the final workflow synthesis. You may edit files. Ensure docs/evaluation.md reflects the whole process: requirements, design, plan, implementation, testing, audit, feature completion matrix, technical debt,
  risks, and recommended next steps. Inspect git status and key files. Return a compact JSON-serializable final report with changed_files, docs_created, verification_summary, audit_verdict, risks, and recommended_next_steps. Inputs: repo=" +
  repo + " requirements=" + requirements + " design=" + design_results + " plan=" + plan + " implementation=" + impl_results + " integration=" + integration + " verification=" + verification + " audit=" + audit_results, { label: "final
  synthesis", phase: "Audit", agent_type: "critiquer" });

  return final_eval;
  }"#,
        None,
        "/tmp".to_string(),
        4,
        agent,
        None,
    )
    .await
    .expect("workflow succeeds");

    assert_eq!(
        result.phases,
        vec![
            "Discovery",
            "Requirements",
            "Design",
            "Planning",
            "Implementation",
            "Verification",
            "Audit",
        ]
    );
    assert_eq!(result.agent_count, 14);
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
