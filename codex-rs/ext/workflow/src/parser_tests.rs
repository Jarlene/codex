use pretty_assertions::assert_eq;

use super::*;

const VALID_SCRIPT: &str = r#"workflow! {
  meta: {
    name: "demo_workflow",
    description: "A useful workflow",
    when_to_use: "When testing parser behavior",
    phases: [{ title: "Scan", detail: "Collect inputs", model: "default" }],
  }

  phase("Scan");
  return { ok: true };
}"#;

#[test]
fn accepts_literal_workflow_metadata() {
    let parsed = parse_workflow_script(VALID_SCRIPT).expect("parse workflow");
    assert_eq!(parsed.meta.name, "demo_workflow");
    assert_eq!(parsed.meta.description, "A useful workflow");
    assert_eq!(
        parsed.meta.phases,
        Some(vec![WorkflowMetaPhase {
            title: "Scan".to_string(),
            detail: Some("Collect inputs".to_string()),
            model: Some("default".to_string()),
        }])
    );
    assert!(parsed.body.contains("phase(\"Scan\")"));
    assert!(!parsed.body.contains("workflow!"));
}

#[test]
fn accepts_static_template_literals() {
    let parsed = parse_workflow_script(
        "workflow! { meta: { name: `demo`, description: `static` } return true; }",
    )
    .expect("parse workflow");
    assert_eq!(parsed.meta.name, "demo");
    assert_eq!(parsed.meta.description, "static");
}

#[test]
fn requires_workflow_macro_first() {
    let err = parse_workflow_script(
        "let x = 1;\nworkflow! { meta: { name: \"demo\", description: \"desc\" } }",
    )
    .expect_err("must fail");
    assert!(err.contains("workflow script must start"), "{err}");
}

#[test]
fn requires_name_and_description() {
    let name_err =
        parse_workflow_script("workflow! { meta: { name: \"demo\" } }").expect_err("must fail");
    assert!(name_err.contains("meta.description"));
    let description_err = parse_workflow_script("workflow! { meta: { description: \"desc\" } }")
        .expect_err("must fail");
    assert!(description_err.contains("meta.name"));
}

#[test]
fn rejects_non_literal_metadata() {
    let err =
        parse_workflow_script("workflow! { meta: { name: make_name(), description: \"desc\" } }")
            .expect_err("must fail");
    assert!(err.contains("non-literal"));
    let err = parse_workflow_script("workflow! { meta: { name, description: \"desc\" } }")
        .expect_err("must fail");
    assert!(err.contains("Identifier"));
}

#[test]
fn rejects_object_hazards() {
    for (script, expected) in [
        (
            "workflow! { meta: { ...base, name: \"demo\", description: \"desc\" } }",
            "spread not allowed",
        ),
        (
            "workflow! { meta: { [\"name\"]: \"demo\", description: \"desc\" } }",
            "computed keys not allowed",
        ),
        (
            "workflow! { meta: { __proto__: {}, name: \"demo\", description: \"desc\" } }",
            "reserved key name",
        ),
        (
            "workflow! { meta: { get name() { return \"demo\" }, description: \"desc\" } }",
            "methods/accessors not allowed",
        ),
    ] {
        let err = parse_workflow_script(script).expect_err("must fail");
        assert!(err.contains(expected), "{err}");
    }
}

#[test]
fn rejects_array_hazards() {
    for (script, expected) in [
        (
            "workflow! { meta: { name: \"demo\", description: \"desc\", phases: [,,] } }",
            "sparse arrays not allowed",
        ),
        (
            "workflow! { meta: { name: \"demo\", description: \"desc\", phases: [...items] } }",
            "spread not allowed",
        ),
    ] {
        let err = parse_workflow_script(script).expect_err("must fail");
        assert!(err.contains(expected), "{err}");
    }
}

#[test]
fn rejects_template_interpolation_in_metadata() {
    let err =
        parse_workflow_script("workflow! { meta: { name: `demo_${id}`, description: \"desc\" } }")
            .expect_err("must fail");
    assert!(err.contains("template interpolation not allowed"));
}

#[test]
fn rejects_nondeterministic_apis() {
    for expression in [
        "Date.now()",
        "Date['now']()",
        "Date[`now`]()",
        "Date['n' + 'ow']()",
        "Date?.now()",
        "Date.now?.()",
        "Math.random()",
        "Math['random']()",
        "Math[`random`]()",
        "Math['ran' + 'dom']()",
        "Math?.random()",
        "Math.random?.()",
        "new Date()",
        "new (Date)()",
        "`timestamp ${Date.now()}`",
        "SystemTime::now()",
        "Instant::now()",
        "rand()",
        "random()",
        "thread_rng()",
    ] {
        let err = parse_workflow_script(&format!(
            "workflow! {{ meta: {{ name: \"demo\", description: \"desc\" }} return {expression}; }}"
        ))
        .expect_err("must fail");
        assert!(err.contains("must be deterministic"), "{expression}: {err}");
    }
}

#[test]
fn allows_deterministic_date_and_math_apis() {
    for expression in [
        "Date.parse('2020-01-01T00:00:00Z')",
        "Date.UTC(2020, 0, 1)",
        "Math.max(1, 2)",
        "Math.floor(1.5)",
        "({ Date: { now: true }, Math: { random: true } })",
        "({ now: () => 1 }).now()",
        "({ now: || 1 }).now()",
        "({ random: true })",
    ] {
        parse_workflow_script(&format!(
            "workflow! {{ meta: {{ name: \"demo\", description: \"desc\" }} return {expression}; }}"
        ))
        .unwrap_or_else(|err| panic!("{expression}: {err}"));
    }
}

#[test]
fn allows_nondeterministic_names_in_text() {
    let parsed = parse_workflow_script(
        r#"workflow! {
  meta: {
    name: "mentions_demo",
    description: "Catalog Date.now(), Math.random(), and new Date() usage",
    when_to_use: "When prompts mention Date.now()",
    phases: [{ title: "Find Date.now() mentions", detail: "Check Math.random() and new Date() too" }],
  }

// Comments may mention Date.now(), Math.random(), and new Date().
let terms = {
  "Date.now()": "Date.now()",
  "Math.random()": "Math.random()",
  "new Date()": "new Date()"
};
phase("Find Date.now() mentions");
agent("Catalog Date.now(), Math.random(), and new Date() usage");
agent(`Find Date.now(), Math.random(), and new Date() mentions`);
return { ok: true, terms };
}"#,
    )
    .expect("parse workflow");

    assert_eq!(
        parsed.meta.description,
        "Catalog Date.now(), Math.random(), and new Date() usage"
    );
    assert!(parsed.body.contains("Catalog Date.now()"));
}

#[test]
fn accepts_legacy_export_meta_scripts_for_migration() {
    let parsed = parse_workflow_script(
        "export const meta = { name: 'legacy_demo', description: 'desc' }\nphase('Scan')",
    )
    .expect("parse legacy workflow");

    assert_eq!(parsed.meta.name, "legacy_demo");
    assert!(parsed.body.contains("phase('Scan')"));
}
