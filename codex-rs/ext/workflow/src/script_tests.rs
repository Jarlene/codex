use pretty_assertions::assert_eq;

use super::*;

#[test]
fn parses_dynamic_phase_and_agent_statements() {
    let statements = parse_workflow_body(
        r#"
phase("Scan");
let scan = agent("scan", { label: "scan" });
return { scan };
"#,
    )
    .expect("parse script");

    assert_eq!(statements.len(), 3);
    assert!(matches!(statements[0], Stmt::Expr(_)));
    assert!(matches!(statements[1], Stmt::Let { .. }));
    assert!(matches!(statements[2], Stmt::Return(_)));
}

#[test]
fn parses_conditionals_and_for_in_loops() {
    let statements = parse_workflow_body(
        r#"
if args.needsReview {
  phase("Review");
}
for area in args.areas {
  phase("Inspect " + area);
  agent("inspect " + area, { label: "inspect " + area });
}
return { ok: true };
"#,
    )
    .expect("parse script");

    assert_eq!(statements.len(), 3);
    assert!(matches!(statements[0], Stmt::If { .. }));
    assert!(matches!(statements[1], Stmt::ForOf { .. }));
    assert!(matches!(statements[2], Stmt::Return(_)));
}

#[test]
fn parses_rust_style_closures() {
    let statements = parse_workflow_body(
        r#"
let scans = parallel(args.areas.map(|area| || agent("scan " + area, { label: "scan " + area })));
return { scans };
"#,
    )
    .expect("parse script");

    assert_eq!(statements.len(), 2);
    assert!(matches!(statements[0], Stmt::Let { .. }));
    assert!(matches!(statements[1], Stmt::Return(_)));
}
