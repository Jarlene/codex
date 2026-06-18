pub(crate) const WORKFLOW_TOOL_NAME: &str = "workflow";

pub(crate) const TOOL_DESCRIPTION: &str = "Execute a deterministic Rust workflow script that orchestrates multiple subagents with agent(), parallel(), and pipeline(). The script is required raw text and must start with workflow! { meta: { name, description }, phases:[{title: \"phase title\", detail: \"Optional phase detail\", model: \"Optional phase model\"}]}. It must call agent() at least once; phases are optional metadata.";

pub(crate) const PROMPT_SNIPPET: &str = "Run a deterministic Rust workflow. Required script shape: workflow! { meta: { name: \"short_snake_case\", description: \"non-empty description\" },phases:[{title: \"phase title\", detail: \"Optional phase detail\", model: \"Optional phase model\"}]}. Use phase(title) at runtime to create progress groups.
## available agent_type
- **architect**: Software architecture specialist for system design, scalability, and technical decision-making. Use PROACTIVELY when planning new features, refactoring large systems, or making architectural decisions.
  *Scenario: Designing a microservices migration strategy for a monolithic e-commerce platform handling 100K+ concurrent users.*
- **coder**: Implementation specialist for writing clean, efficient code.
  *Scenario: Implementing a thread-safe connection pool with proper resource cleanup and circuit breaker patterns.*
- **planner**: Expert planning specialist for complex features and refactoring. Use PROACTIVELY when users request feature implementation, architectural changes, or complex refactoring. Automatically activated for planning tasks.
  *Scenario: Breaking down a 3-month initiative to replace a legacy payment gateway with a modern event-driven architecture.*
- **critiquer**: Expert critique specialist for comprehensive multi-perspective review using specialized judges with debate and consensus building.
  *Scenario: Evaluating a proposed API design by simulating security, scalability, and DX (developer experience) reviewers to surface hidden trade-offs.*
- **requirement**: Specification phase specialist for requirements analysis.
  *Scenario: Extracting unambiguous functional and non-functional requirements from stakeholder interviews for a real-time collaboration tool.*
- **researcher**: Deep research and information gathering specialist.
  *Scenario: Investigating state-of-the-art code database solutions and benchmarking their recall rates for a enterprise search system.*
- **reviewer**: Code review and quality assurance specialist.
  *Scenario: Conducting a line-by-line review of a critical authentication module to ensure OWASP compliance and zero trust principles.*
- **tester**: Comprehensive testing and quality assurance specialist.
  *Scenario: Designing a test matrix covering unit, integration, chaos, and load testing for a distributed transaction processing service.*
";

pub(crate) const PROMPT_GUIDELINES: [&str; 15] = [
    "Use workflow only when the user explicitly asks for a workflow, workflows, fan-out, or multi-agent orchestration.",
    "For workflow, always pass one raw Rust workflow script in the required script parameter; do not include Markdown fences or prose around the script.",
    "For workflow, the script must start with `workflow! { meta: { name: \"short_snake_case\", description: \"non-empty human description\" }, phases:[{title: \"phase title\", detail: \"Optional phase detail\", model: \"Optional phase model\"}] }`; meta.name and meta.description are required non-empty strings, and meta.phases is optional metadata for a stable upfront outline.",
    "For workflow, write Rust-like workflow code inside the workflow! block. Do not use imports, modules, macros other than workflow!, filesystem APIs, network APIs, current time, random values, or unsafe code.",
    "For workflow, available globals are agent(prompt, options), parallel(thunks), pipeline(items, stages), phase(title), log(message), args, cwd, process.cwd(), and budget. Every workflow must call agent() at least once; do not use workflow only to declare phases or return a static object.",
    "For workflow, call phase(title) when a new group of work starts. Phase names may be conditional or built in a loop; do not predeclare speculative phases just in case.",
    "For workflow, prefer it for decomposable work: repository inspection, independent research/checks, multi-perspective review, or fan-out/fan-in synthesis. Do not use it for a single quick file read/edit or when ordinary tools are enough.",
    "For workflow, parallel() takes closures, not direct agent calls: use `parallel(args.items.map(|item| || agent(\"...\", { label: \"...\" })))`, never `parallel(args.items.map(|item| agent(...)))`. Results are returned in input order.",
    "For workflow, pipeline(items, stage1, stage2) runs each item through stages sequentially, while different items may run concurrently. Each stage receives (previous_value, original_item, index).",
    "For workflow, every agent() call should include a unique short label option, 2-5 words, such as { label: \"repo inventory\" } or { label: \"source modules\" }; unique labels make live status and error reporting readable.",
    "For workflow, failed agent(), parallel(), or pipeline() branches return null and log the failure unless the workflow is aborted. Check for nulls before synthesizing conclusions.",
    "For workflow, include a final synthesis/assertion agent when combining multiple subagent results; return a compact JSON-serializable value with ok/verdict plus the important outputs.",
    "For workflow, if agent() needs machine-readable output, pass a plain JSON Schema via opts.schema; agent() will return the validated object. Use JSON Schema syntax.",
    "For workflow, do not assume the parent assistant has repository code context inside subagents; include enough task context and relevant paths in each agent prompt.",
    "For workflow, each agent() call should include the following options: { label: \"Optional label for the agent\", phase: \"phase identifier in meta informations\", schema: \"Optional schema definition as a json vules\", model: \"Optional model identifier\", agent_type: \"Optional type of the agent\", isolation: \"Optional isolation mode, currently only worktree is supported\" }.",
];

pub(crate) const SCRIPT_DESCRIPTION: &str = "Required raw Rust workflow script, with no Markdown fences. The script must start with workflow! { meta: { name: \"short_snake_case\", description: \"non-empty description\" }, phases:[{title:'phase title', detail:'Optional phase detail', model:'Optional phase model'}]}. meta.phases is optional documentation; live progress is driven by phase(title). Use phase(\"Name\"), agent(prompt, opts), parallel(array_of_closures), pipeline(items, stage1, stage2), log(message), args, cwd, and budget. The workflow must call agent() at least once. parallel() requires closures: parallel(items.map(|item| || agent(...))).";

pub(crate) const ARGS_DESCRIPTION: &str =
    "Optional JSON value exposed to the workflow script as global `args`.";

pub(crate) fn developer_prompt_fragment() -> String {
    let mut text = String::from(PROMPT_SNIPPET);
    for guideline in PROMPT_GUIDELINES {
        text.push('\n');
        text.push_str(guideline);
    }
    text
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn workflow_prompt_guides_rust_scripts() {
        assert!(TOOL_DESCRIPTION.contains("Rust workflow script"));
        assert!(TOOL_DESCRIPTION.contains("workflow! { meta: { name, description }"));
        assert!(PROMPT_SNIPPET.contains("workflow! { meta:"));
        assert_eq!(PROMPT_GUIDELINES.len(), 15);
        assert_eq!(
            PROMPT_GUIDELINES[0],
            "Use workflow only when the user explicitly asks for a workflow, workflows, fan-out, or multi-agent orchestration."
        );
        assert!(PROMPT_GUIDELINES[1].contains("raw Rust workflow script"));
        assert!(PROMPT_GUIDELINES[7].contains("parallel() takes closures"));
        assert_eq!(
            PROMPT_GUIDELINES[13],
            "For workflow, do not assume the parent assistant has repository code context inside subagents; include enough task context and relevant paths in each agent prompt."
        );
        assert!(SCRIPT_DESCRIPTION.contains("Required raw Rust workflow script"));
        assert!(SCRIPT_DESCRIPTION.contains("parallel(items.map(|item| || agent(...)))"));
    }
}
