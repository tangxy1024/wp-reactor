use wf_lang::ast::{BinOp, CloseMode, Expr, FieldRef, SystemVar};
use wf_lang::plan::{BranchPlan, EachPlan, StepPlan, YieldField};

use crate::rule::RuleExecutor;
use crate::rule::match_engine::{BindData, CloseOutput, CloseReason, MatchedContext, StepData};

use super::helpers::*;

// ---------------------------------------------------------------------------
// Helper: build a standard one-step match plan + rule plan
// ---------------------------------------------------------------------------

fn default_match_plan() -> wf_lang::plan::MatchPlan {
    simple_plan(
        vec![simple_key("sip")],
        vec![step(vec![branch("fail", count_ge(1.0))])],
    )
}

fn default_matched_context() -> MatchedContext {
    MatchedContext {
        rule_name: "r1".to_string(),
        scope_key: vec![str_val("10.0.0.1")],
        step_data: vec![StepData {
            satisfied_branch_index: 0,
            label: Some("fail".to_string()),
            measure_value: 1.0,
            collected_values: Vec::new(),
            field_values: std::collections::HashMap::new(),
        }],
        bind_data: vec![],
        event_time_nanos: 0,
    }
}

// =========================================================================
// Test 1: execute_match — static score
// =========================================================================

#[test]
fn execute_match_static_score() {
    let plan = simple_rule_plan(
        "r1",
        default_match_plan(),
        Expr::Number(70.0),
        "ip",
        Expr::Field(FieldRef::Qualified("fail".to_string(), "sip".to_string())),
    );
    let exec = RuleExecutor::new(plan);
    let matched = default_matched_context();

    let alert = exec.execute_match(&matched).unwrap();

    assert_eq!(alert.rule_name, "r1");
    assert!((alert.score - 70.0).abs() < f64::EPSILON);
    assert_eq!(alert.entity_type, "ip");
    assert_eq!(alert.entity_id, "10.0.0.1");
    assert_eq!(alert.origin, crate::alert::AlertOrigin::Event);
    assert!(alert.matched_rows.is_empty());
    assert!(alert.fired_at.ends_with('Z'));
}

#[test]
fn execute_each_wfx_id_changes_with_event_content() {
    let mut plan = simple_rule_plan(
        "r1",
        simple_plan(vec![], vec![]),
        Expr::Number(10.0),
        "ip",
        Expr::Field(FieldRef::Qualified("e".to_string(), "sip".to_string())),
    );
    plan.binds[0].alias = "e".to_string();
    plan.each_plan = Some(EachPlan {
        alias: "e".to_string(),
        filter: None,
    });
    let exec = RuleExecutor::new(plan);

    let left = event(vec![("sip", str_val("10.0.0.1"))]);
    let right = event(vec![("sip", str_val("10.0.0.2"))]);

    let left_alert = exec.execute_each(&left, 1_000_000).unwrap().unwrap();
    let right_alert = exec.execute_each(&right, 1_000_000).unwrap().unwrap();

    assert_ne!(left_alert.wfx_id, right_alert.wfx_id);
}

#[test]
fn execute_each_yield_can_reference_score() {
    let mut plan = simple_rule_plan(
        "r1",
        simple_plan(vec![], vec![]),
        Expr::Number(10.0),
        "ip",
        Expr::Field(FieldRef::Qualified("e".to_string(), "sip".to_string())),
    );
    plan.binds[0].alias = "e".to_string();
    plan.each_plan = Some(EachPlan {
        alias: "e".to_string(),
        filter: None,
    });
    plan.yield_plan.fields = vec![YieldField {
        name: "risk_score".to_string(),
        value: Expr::SystemVar(SystemVar::Score),
    }];
    let exec = RuleExecutor::new(plan);

    let alert = exec
        .execute_each(&event(vec![("sip", str_val("10.0.0.1"))]), 1_000_000)
        .unwrap()
        .unwrap();

    assert_eq!(
        alert
            .yield_fields
            .iter()
            .find(|(name, _)| name == "risk_score")
            .map(|(_, value)| value.clone()),
        Some(num(10.0))
    );
}

#[test]
fn execute_each_yield_failure_is_not_silent() {
    let mut plan = simple_rule_plan(
        "r1",
        simple_plan(vec![], vec![]),
        Expr::Number(10.0),
        "ip",
        Expr::Field(FieldRef::Qualified("e".to_string(), "sip".to_string())),
    );
    plan.binds[0].alias = "e".to_string();
    plan.each_plan = Some(EachPlan {
        alias: "e".to_string(),
        filter: None,
    });
    plan.yield_plan.fields = vec![YieldField {
        name: "missing".to_string(),
        value: Expr::Field(FieldRef::Simple("does_not_exist".to_string())),
    }];
    let exec = RuleExecutor::new(plan);

    let err = exec
        .execute_each(&event(vec![("sip", str_val("10.0.0.1"))]), 1_000_000)
        .unwrap_err();

    assert!(err.to_string().contains("on each yield field"));
    assert!(err.to_string().contains("missing"));
}

// =========================================================================
// Test 2: execute_match — arithmetic score
// =========================================================================

#[test]
fn execute_match_arithmetic_score() {
    let score_expr = Expr::BinOp {
        op: BinOp::Add,
        left: Box::new(Expr::Number(50.0)),
        right: Box::new(Expr::Number(20.0)),
    };
    let plan = simple_rule_plan(
        "r1",
        default_match_plan(),
        score_expr,
        "ip",
        Expr::Field(FieldRef::Simple("sip".to_string())),
    );
    let exec = RuleExecutor::new(plan);
    let matched = default_matched_context();

    let alert = exec.execute_match(&matched).unwrap();
    assert!((alert.score - 70.0).abs() < f64::EPSILON);
}

#[test]
fn execute_match_yield_can_reference_score() {
    let mut plan = simple_rule_plan(
        "r1",
        default_match_plan(),
        Expr::Number(70.0),
        "ip",
        Expr::Field(FieldRef::Simple("sip".to_string())),
    );
    plan.yield_plan.fields = vec![YieldField {
        name: "risk_score".to_string(),
        value: Expr::SystemVar(SystemVar::Score),
    }];
    let exec = RuleExecutor::new(plan);

    let alert = exec.execute_match(&default_matched_context()).unwrap();

    assert_eq!(
        alert
            .yield_fields
            .iter()
            .find(|(name, _)| name == "risk_score")
            .map(|(_, value)| value.clone()),
        Some(num(70.0))
    );
}

#[test]
fn execute_match_yield_can_use_score_inside_builtin_expr() {
    let mut plan = simple_rule_plan(
        "r1",
        default_match_plan(),
        Expr::Number(70.126),
        "ip",
        Expr::Field(FieldRef::Simple("sip".to_string())),
    );
    plan.yield_plan.fields = vec![
        YieldField {
            name: "rounded".to_string(),
            value: Expr::FuncCall {
                qualifier: None,
                name: "round".to_string(),
                args: vec![Expr::SystemVar(SystemVar::Score), Expr::Number(1.0)],
            },
        },
        YieldField {
            name: "message".to_string(),
            value: Expr::FuncCall {
                qualifier: None,
                name: "concat".to_string(),
                args: vec![
                    Expr::StringLit("risk=".to_string()),
                    Expr::SystemVar(SystemVar::Score),
                ],
            },
        },
    ];
    let exec = RuleExecutor::new(plan);

    let alert = exec.execute_match(&default_matched_context()).unwrap();

    assert_eq!(
        alert
            .yield_fields
            .iter()
            .find(|(name, _)| name == "rounded")
            .map(|(_, value)| value.clone()),
        Some(num(70.1))
    );
    assert_eq!(
        alert
            .yield_fields
            .iter()
            .find(|(name, _)| name == "message")
            .map(|(_, value)| value.clone()),
        Some(str_val("risk=70.126"))
    );
}

#[test]
fn execute_match_yield_failure_is_not_silent() {
    let mut plan = simple_rule_plan(
        "r1",
        default_match_plan(),
        Expr::Number(70.0),
        "ip",
        Expr::Field(FieldRef::Simple("sip".to_string())),
    );
    plan.yield_plan.fields = vec![YieldField {
        name: "missing".to_string(),
        value: Expr::Field(FieldRef::Simple("does_not_exist".to_string())),
    }];
    let exec = RuleExecutor::new(plan);

    let err = exec.execute_match(&default_matched_context()).unwrap_err();

    assert!(err.to_string().contains("match yield field"));
    assert!(err.to_string().contains("missing"));
}

// =========================================================================
// Test 3: execute_match — entity from simple key
// =========================================================================

#[test]
fn execute_match_entity_simple_key() {
    let plan = simple_rule_plan(
        "r1",
        default_match_plan(),
        Expr::Number(50.0),
        "ip",
        Expr::Field(FieldRef::Simple("sip".to_string())),
    );
    let exec = RuleExecutor::new(plan);
    let matched = default_matched_context();

    let alert = exec.execute_match(&matched).unwrap();
    assert_eq!(alert.entity_id, "10.0.0.1");
}

// =========================================================================
// Test 4: execute_match — no keys (global scope)
// =========================================================================

#[test]
fn execute_match_no_keys() {
    let match_plan = simple_plan(vec![], vec![step(vec![branch("fail", count_ge(1.0))])]);
    // Use a literal string as entity since there are no key fields
    let plan = simple_rule_plan(
        "r1",
        match_plan,
        Expr::Number(50.0),
        "global",
        Expr::StringLit("all".to_string()),
    );
    let exec = RuleExecutor::new(plan);
    let matched = MatchedContext {
        rule_name: "r1".to_string(),
        scope_key: vec![],
        step_data: vec![StepData {
            satisfied_branch_index: 0,
            label: None,
            measure_value: 1.0,
            collected_values: Vec::new(),
            field_values: std::collections::HashMap::new(),
        }],
        bind_data: vec![],
        event_time_nanos: 0,
    };

    let alert = exec.execute_match(&matched).unwrap();
    assert_eq!(alert.entity_id, "all");
    assert!(alert.summary.contains("global"));
}

// =========================================================================
// Test 5: execute_match — composite keys
// =========================================================================

#[test]
fn execute_match_composite_keys() {
    let match_plan = simple_plan(
        vec![simple_key("sip"), simple_key("dip")],
        vec![step(vec![branch("fail", count_ge(1.0))])],
    );
    let plan = simple_rule_plan(
        "r1",
        match_plan,
        Expr::Number(60.0),
        "ip_pair",
        Expr::Field(FieldRef::Simple("dip".to_string())),
    );
    let exec = RuleExecutor::new(plan);
    let matched = MatchedContext {
        rule_name: "r1".to_string(),
        scope_key: vec![str_val("10.0.0.1"), str_val("10.0.0.2")],
        step_data: vec![StepData {
            satisfied_branch_index: 0,
            label: None,
            measure_value: 1.0,
            collected_values: Vec::new(),
            field_values: std::collections::HashMap::new(),
        }],
        bind_data: vec![],
        event_time_nanos: 0,
    };

    let alert = exec.execute_match(&matched).unwrap();
    assert_eq!(alert.entity_id, "10.0.0.2");
    // wfx_id should be a 16-hex-char content hash
    assert_eq!(alert.wfx_id.len(), 16);
    assert!(alert.wfx_id.chars().all(|c| c.is_ascii_hexdigit()));
}

// =========================================================================
// Test 6: execute_close — both ok
// =========================================================================

#[test]
fn execute_close_both_ok() {
    let plan = simple_rule_plan(
        "r1",
        default_match_plan(),
        Expr::Number(70.0),
        "ip",
        Expr::Field(FieldRef::Simple("sip".to_string())),
    );
    let exec = RuleExecutor::new(plan);
    let close = CloseOutput {
        rule_name: "r1".to_string(),
        scope_key: vec![str_val("10.0.0.1")],
        close_reason: CloseReason::Timeout,
        event_ok: true,
        close_ok: true,
        close_mode: CloseMode::And,
        event_emitted: false,
        event_step_data: vec![StepData {
            satisfied_branch_index: 0,
            label: Some("fail".to_string()),
            measure_value: 3.0,
            collected_values: Vec::new(),
            field_values: std::collections::HashMap::new(),
        }],
        close_step_data: vec![],
        bind_data: vec![],
        watermark_nanos: 0,
        last_event_nanos: 123,
    };

    let alert = exec.execute_close(&close).unwrap().unwrap();
    assert_eq!(alert.origin.as_str(), "close:timeout");
    assert!((alert.score - 70.0).abs() < f64::EPSILON);
    assert_eq!(alert.entity_id, "10.0.0.1");
    assert_eq!(alert.event_time_nanos, 123);
}

// =========================================================================
// Test 7: execute_close — close_ok = false
// =========================================================================

#[test]
fn execute_close_close_not_ok() {
    let plan = simple_rule_plan(
        "r1",
        default_match_plan(),
        Expr::Number(70.0),
        "ip",
        Expr::Field(FieldRef::Simple("sip".to_string())),
    );
    let exec = RuleExecutor::new(plan);
    let close = CloseOutput {
        rule_name: "r1".to_string(),
        scope_key: vec![str_val("10.0.0.1")],
        close_reason: CloseReason::Flush,
        event_ok: true,
        close_ok: false,
        close_mode: CloseMode::And,
        event_emitted: false,
        event_step_data: vec![],
        close_step_data: vec![],
        bind_data: vec![],
        watermark_nanos: 0,
        last_event_nanos: 0,
    };

    let result = exec.execute_close(&close).unwrap();
    assert!(result.is_none());
}

#[test]
fn execute_close_yield_can_reference_score() {
    let mut plan = simple_rule_plan(
        "r1",
        default_match_plan(),
        Expr::Number(70.0),
        "ip",
        Expr::Field(FieldRef::Simple("sip".to_string())),
    );
    plan.yield_plan.fields = vec![YieldField {
        name: "risk_score".to_string(),
        value: Expr::SystemVar(SystemVar::Score),
    }];
    let exec = RuleExecutor::new(plan);
    let close = CloseOutput {
        rule_name: "r1".to_string(),
        scope_key: vec![str_val("10.0.0.1")],
        close_reason: CloseReason::Timeout,
        event_ok: true,
        close_ok: true,
        close_mode: CloseMode::And,
        event_emitted: false,
        event_step_data: vec![StepData {
            satisfied_branch_index: 0,
            label: Some("fail".to_string()),
            measure_value: 3.0,
            collected_values: Vec::new(),
            field_values: std::collections::HashMap::new(),
        }],
        close_step_data: vec![],
        bind_data: vec![],
        watermark_nanos: 0,
        last_event_nanos: 123,
    };

    let alert = exec.execute_close(&close).unwrap().unwrap();

    assert_eq!(
        alert
            .yield_fields
            .iter()
            .find(|(name, _)| name == "risk_score")
            .map(|(_, value)| value.clone()),
        Some(num(70.0))
    );
}

#[test]
fn execute_close_score_can_use_count_alias() {
    let plan = simple_rule_plan(
        "r1",
        simple_plan(
            vec![simple_key("sip")],
            vec![step(vec![branch("x", count_ge(1.0))])],
        ),
        Expr::FuncCall {
            qualifier: None,
            name: "count".to_string(),
            args: vec![Expr::Field(FieldRef::Simple("x".to_string()))],
        },
        "ip",
        Expr::Field(FieldRef::Simple("sip".to_string())),
    );
    let exec = RuleExecutor::new(plan);
    let close = CloseOutput {
        rule_name: "r1".to_string(),
        scope_key: vec![str_val("10.0.0.1")],
        close_reason: CloseReason::Timeout,
        event_ok: true,
        close_ok: true,
        close_mode: CloseMode::And,
        event_emitted: false,
        event_step_data: vec![StepData {
            satisfied_branch_index: 0,
            label: None,
            measure_value: 3.0,
            collected_values: Vec::new(),
            field_values: std::collections::HashMap::new(),
        }],
        close_step_data: vec![],
        bind_data: vec![],
        watermark_nanos: 0,
        last_event_nanos: 123,
    };

    let alert = exec.execute_close(&close).unwrap().unwrap();

    assert_eq!(alert.score, 3.0);
}

#[test]
fn execute_close_yield_can_use_count_label_inside_if_and_concat() {
    let mut plan = simple_rule_plan(
        "r1",
        simple_plan(
            vec![simple_key("sip")],
            vec![step(vec![branch_with_label("x", "hi", count_ge(1.0))])],
        ),
        Expr::Number(50.0),
        "ip",
        Expr::Field(FieldRef::Simple("sip".to_string())),
    );
    let count_hi = Expr::FuncCall {
        qualifier: None,
        name: "count".to_string(),
        args: vec![Expr::Field(FieldRef::Simple("hi".to_string()))],
    };
    plan.yield_plan.fields = vec![
        YieldField {
            name: "high_event_count".to_string(),
            value: count_hi.clone(),
        },
        YieldField {
            name: "status".to_string(),
            value: Expr::IfThenElse {
                cond: Box::new(Expr::BinOp {
                    op: BinOp::Ge,
                    left: Box::new(count_hi.clone()),
                    right: Box::new(Expr::Number(2.0)),
                }),
                then_expr: Box::new(Expr::StringLit("high".to_string())),
                else_expr: Box::new(Expr::StringLit("low".to_string())),
            },
        },
        YieldField {
            name: "message".to_string(),
            value: Expr::FuncCall {
                qualifier: None,
                name: "concat".to_string(),
                args: vec![Expr::StringLit("cnt=".to_string()), count_hi],
            },
        },
    ];
    let exec = RuleExecutor::new(plan);
    let close = CloseOutput {
        rule_name: "r1".to_string(),
        scope_key: vec![str_val("10.0.0.1")],
        close_reason: CloseReason::Timeout,
        event_ok: true,
        close_ok: true,
        close_mode: CloseMode::And,
        event_emitted: false,
        event_step_data: vec![StepData {
            satisfied_branch_index: 0,
            label: Some("hi".to_string()),
            measure_value: 2.0,
            collected_values: Vec::new(),
            field_values: std::collections::HashMap::new(),
        }],
        close_step_data: vec![],
        bind_data: vec![],
        watermark_nanos: 0,
        last_event_nanos: 123,
    };

    let alert = exec.execute_close(&close).unwrap().unwrap();

    assert_eq!(
        alert
            .yield_fields
            .iter()
            .find(|(name, _)| name == "high_event_count")
            .map(|(_, value)| value.clone()),
        Some(num(2.0))
    );
    assert_eq!(
        alert
            .yield_fields
            .iter()
            .find(|(name, _)| name == "status")
            .map(|(_, value)| value.clone()),
        Some(str_val("high"))
    );
    assert_eq!(
        alert
            .yield_fields
            .iter()
            .find(|(name, _)| name == "message")
            .map(|(_, value)| value.clone()),
        Some(str_val("cnt=2"))
    );
}

#[test]
fn execute_close_yield_can_use_avg_on_field() {
    let mut plan = simple_rule_plan(
        "r1",
        simple_plan(
            vec![simple_key("sip")],
            vec![StepPlan {
                branches: vec![BranchPlan {
                    label: None,
                    source: "x".to_string(),
                    field: None,
                    guard: None,
                    agg: count_ge(1.0),
                }],
            }],
        ),
        Expr::Number(50.0),
        "ip",
        Expr::Field(FieldRef::Simple("sip".to_string())),
    );
    let avg_risk = Expr::FuncCall {
        qualifier: None,
        name: "avg".to_string(),
        args: vec![Expr::Field(FieldRef::Qualified(
            "x".to_string(),
            "risk_score".to_string(),
        ))],
    };
    plan.yield_plan.fields = vec![
        YieldField {
            name: "avg_risk_score".to_string(),
            value: avg_risk.clone(),
        },
        YieldField {
            name: "message".to_string(),
            value: Expr::FuncCall {
                qualifier: None,
                name: "concat".to_string(),
                args: vec![Expr::StringLit("avg=".to_string()), avg_risk],
            },
        },
    ];
    let exec = RuleExecutor::new(plan);
    let close = CloseOutput {
        rule_name: "r1".to_string(),
        scope_key: vec![str_val("10.0.0.1")],
        close_reason: CloseReason::Timeout,
        event_ok: true,
        close_ok: true,
        close_mode: CloseMode::And,
        event_emitted: false,
        event_step_data: vec![StepData {
            satisfied_branch_index: 0,
            label: None,
            measure_value: 2.0,
            collected_values: Vec::new(),
            field_values: std::collections::HashMap::from([(
                "risk_score".to_string(),
                vec![num(20.0), num(40.0)],
            )]),
        }],
        close_step_data: vec![],
        bind_data: vec![],
        watermark_nanos: 0,
        last_event_nanos: 123,
    };

    let alert = exec.execute_close(&close).unwrap().unwrap();

    assert_eq!(
        alert
            .yield_fields
            .iter()
            .find(|(name, _)| name == "avg_risk_score")
            .map(|(_, value)| value.clone()),
        Some(num(30.0))
    );
    assert_eq!(
        alert
            .yield_fields
            .iter()
            .find(|(name, _)| name == "message")
            .map(|(_, value)| value.clone()),
        Some(str_val("avg=30"))
    );
}

#[test]
fn execute_close_yield_can_use_bind_alias_aggregates() {
    let mut plan = simple_rule_plan(
        "r1",
        simple_plan(
            vec![simple_key("sip")],
            vec![step(vec![branch("x", count_ge(1.0))])],
        ),
        Expr::Number(50.0),
        "ip",
        Expr::Field(FieldRef::Simple("sip".to_string())),
    );
    plan.yield_plan.fields = vec![
        YieldField {
            name: "source_avg".to_string(),
            value: Expr::FuncCall {
                qualifier: None,
                name: "avg".to_string(),
                args: vec![Expr::Field(FieldRef::Qualified(
                    "x".to_string(),
                    "risk_score".to_string(),
                ))],
            },
        },
        YieldField {
            name: "high_event_count".to_string(),
            value: Expr::FuncCall {
                qualifier: None,
                name: "count".to_string(),
                args: vec![Expr::Field(FieldRef::Simple("hi".to_string()))],
            },
        },
        YieldField {
            name: "elevated_avg".to_string(),
            value: Expr::FuncCall {
                qualifier: None,
                name: "avg".to_string(),
                args: vec![Expr::Field(FieldRef::Qualified(
                    "elevated".to_string(),
                    "risk_score".to_string(),
                ))],
            },
        },
        YieldField {
            name: "first_high_action".to_string(),
            value: Expr::FuncCall {
                qualifier: None,
                name: "first".to_string(),
                args: vec![Expr::Field(FieldRef::Qualified(
                    "hi".to_string(),
                    "action".to_string(),
                ))],
            },
        },
    ];
    let exec = RuleExecutor::new(plan);
    let close = CloseOutput {
        rule_name: "r1".to_string(),
        scope_key: vec![str_val("10.0.0.1")],
        close_reason: CloseReason::Timeout,
        event_ok: true,
        close_ok: true,
        close_mode: CloseMode::And,
        event_emitted: false,
        event_step_data: vec![StepData {
            satisfied_branch_index: 0,
            label: None,
            measure_value: 2.0,
            collected_values: Vec::new(),
            field_values: std::collections::HashMap::from([(
                "risk_score".to_string(),
                vec![num(90.0), num(70.0)],
            )]),
        }],
        close_step_data: vec![],
        bind_data: vec![
            BindData {
                alias: "x".to_string(),
                count: 2,
                field_values: std::collections::HashMap::from([(
                    "risk_score".to_string(),
                    vec![num(90.0), num(70.0)],
                )]),
            },
            BindData {
                alias: "hi".to_string(),
                count: 1,
                field_values: std::collections::HashMap::from([(
                    "action".to_string(),
                    vec![str_val("block")],
                )]),
            },
            BindData {
                alias: "elevated".to_string(),
                count: 2,
                field_values: std::collections::HashMap::from([(
                    "risk_score".to_string(),
                    vec![num(90.0), num(70.0)],
                )]),
            },
        ],
        watermark_nanos: 0,
        last_event_nanos: 123,
    };

    let alert = exec.execute_close(&close).unwrap().unwrap();

    assert_eq!(
        alert
            .yield_fields
            .iter()
            .find(|(name, _)| name == "high_event_count")
            .map(|(_, value)| value.clone()),
        Some(num(1.0))
    );
    assert_eq!(
        alert
            .yield_fields
            .iter()
            .find(|(name, _)| name == "elevated_avg")
            .map(|(_, value)| value.clone()),
        Some(num(80.0))
    );
    assert_eq!(
        alert
            .yield_fields
            .iter()
            .find(|(name, _)| name == "first_high_action")
            .map(|(_, value)| value.clone()),
        Some(str_val("block"))
    );
}

#[test]
fn execute_match_yield_can_use_bind_alias_aggregates() {
    let mut plan = simple_rule_plan(
        "r1",
        simple_plan(
            vec![simple_key("sip")],
            vec![step(vec![branch("x", count_ge(2.0))])],
        ),
        Expr::Number(50.0),
        "ip",
        Expr::Field(FieldRef::Simple("sip".to_string())),
    );
    plan.yield_plan.fields = vec![
        YieldField {
            name: "source_avg".to_string(),
            value: Expr::FuncCall {
                qualifier: None,
                name: "avg".to_string(),
                args: vec![Expr::Field(FieldRef::Qualified(
                    "x".to_string(),
                    "risk_score".to_string(),
                ))],
            },
        },
        YieldField {
            name: "high_event_count".to_string(),
            value: Expr::FuncCall {
                qualifier: None,
                name: "count".to_string(),
                args: vec![Expr::Field(FieldRef::Simple("hi".to_string()))],
            },
        },
        YieldField {
            name: "elevated_avg".to_string(),
            value: Expr::FuncCall {
                qualifier: None,
                name: "avg".to_string(),
                args: vec![Expr::Field(FieldRef::Qualified(
                    "elevated".to_string(),
                    "risk_score".to_string(),
                ))],
            },
        },
        YieldField {
            name: "last_high_action".to_string(),
            value: Expr::FuncCall {
                qualifier: None,
                name: "last".to_string(),
                args: vec![Expr::Field(FieldRef::Qualified(
                    "hi".to_string(),
                    "action".to_string(),
                ))],
            },
        },
    ];
    let exec = RuleExecutor::new(plan);
    let matched = MatchedContext {
        rule_name: "r1".to_string(),
        scope_key: vec![str_val("10.0.0.1")],
        step_data: vec![StepData {
            satisfied_branch_index: 0,
            label: None,
            measure_value: 2.0,
            collected_values: Vec::new(),
            field_values: std::collections::HashMap::new(),
        }],
        bind_data: vec![
            BindData {
                alias: "x".to_string(),
                count: 2,
                field_values: std::collections::HashMap::from([(
                    "risk_score".to_string(),
                    vec![num(90.0), num(70.0)],
                )]),
            },
            BindData {
                alias: "hi".to_string(),
                count: 1,
                field_values: std::collections::HashMap::from([(
                    "action".to_string(),
                    vec![str_val("block")],
                )]),
            },
            BindData {
                alias: "elevated".to_string(),
                count: 2,
                field_values: std::collections::HashMap::from([(
                    "risk_score".to_string(),
                    vec![num(90.0), num(70.0)],
                )]),
            },
        ],
        event_time_nanos: 0,
    };

    let alert = exec.execute_match(&matched).unwrap();

    assert_eq!(
        alert
            .yield_fields
            .iter()
            .find(|(name, _)| name == "source_avg")
            .map(|(_, value)| value.clone()),
        Some(num(80.0))
    );
    assert_eq!(
        alert
            .yield_fields
            .iter()
            .find(|(name, _)| name == "high_event_count")
            .map(|(_, value)| value.clone()),
        Some(num(1.0))
    );
    assert_eq!(
        alert
            .yield_fields
            .iter()
            .find(|(name, _)| name == "elevated_avg")
            .map(|(_, value)| value.clone()),
        Some(num(80.0))
    );
    assert_eq!(
        alert
            .yield_fields
            .iter()
            .find(|(name, _)| name == "last_high_action")
            .map(|(_, value)| value.clone()),
        Some(str_val("block"))
    );
}

#[test]
fn execute_close_yield_can_use_fmt_with_count() {
    let mut plan = simple_rule_plan(
        "r1",
        simple_plan(
            vec![simple_key("sip")],
            vec![step(vec![branch("fail", count_ge(1.0))])],
        ),
        Expr::Number(50.0),
        "ip",
        Expr::Field(FieldRef::Simple("sip".to_string())),
    );
    plan.yield_plan.fields = vec![YieldField {
        name: "message".to_string(),
        value: Expr::FuncCall {
            qualifier: None,
            name: "fmt".to_string(),
            args: vec![
                Expr::StringLit("{} failed {} times".to_string()),
                Expr::Field(FieldRef::Qualified("fail".to_string(), "sip".to_string())),
                Expr::FuncCall {
                    qualifier: None,
                    name: "count".to_string(),
                    args: vec![Expr::Field(FieldRef::Simple("fail".to_string()))],
                },
            ],
        },
    }];
    let exec = RuleExecutor::new(plan);
    let close = CloseOutput {
        rule_name: "r1".to_string(),
        scope_key: vec![str_val("10.0.0.1")],
        close_reason: CloseReason::Timeout,
        event_ok: true,
        close_ok: true,
        close_mode: CloseMode::And,
        event_emitted: false,
        event_step_data: vec![StepData {
            satisfied_branch_index: 0,
            label: None,
            measure_value: 3.0,
            collected_values: Vec::new(),
            field_values: std::collections::HashMap::from([(
                "sip".to_string(),
                vec![
                    str_val("10.0.0.1"),
                    str_val("10.0.0.1"),
                    str_val("10.0.0.1"),
                ],
            )]),
        }],
        close_step_data: vec![],
        bind_data: vec![],
        watermark_nanos: 0,
        last_event_nanos: 123,
    };

    let alert = exec.execute_close(&close).unwrap().unwrap();

    assert_eq!(
        alert
            .yield_fields
            .iter()
            .find(|(name, _)| name == "message")
            .map(|(_, value)| value.clone()),
        Some(str_val("10.0.0.1 failed 3 times"))
    );
}

// =========================================================================
// Test 8: execute_close — event_ok = false
// =========================================================================

#[test]
fn execute_close_event_not_ok() {
    let plan = simple_rule_plan(
        "r1",
        default_match_plan(),
        Expr::Number(70.0),
        "ip",
        Expr::Field(FieldRef::Simple("sip".to_string())),
    );
    let exec = RuleExecutor::new(plan);
    let close = CloseOutput {
        rule_name: "r1".to_string(),
        scope_key: vec![str_val("10.0.0.1")],
        close_reason: CloseReason::Eos,
        event_ok: false,
        close_ok: true,
        close_mode: CloseMode::And,
        event_emitted: false,
        event_step_data: vec![],
        close_step_data: vec![],
        bind_data: vec![],
        watermark_nanos: 0,
        last_event_nanos: 0,
    };

    let result = exec.execute_close(&close).unwrap();
    assert!(result.is_none());
}

// =========================================================================
// Test 9: score clamped to [0, 100]
// =========================================================================

#[test]
fn score_clamped_to_range() {
    let plan_high = simple_rule_plan(
        "r1",
        default_match_plan(),
        Expr::Number(150.0),
        "ip",
        Expr::Field(FieldRef::Simple("sip".to_string())),
    );
    let exec_high = RuleExecutor::new(plan_high);
    let matched = default_matched_context();

    let alert = exec_high.execute_match(&matched).unwrap();
    assert!((alert.score - 100.0).abs() < f64::EPSILON);

    let plan_low = simple_rule_plan(
        "r1",
        default_match_plan(),
        Expr::Number(-10.0),
        "ip",
        Expr::Field(FieldRef::Simple("sip".to_string())),
    );
    let exec_low = RuleExecutor::new(plan_low);

    let alert = exec_low.execute_match(&matched).unwrap();
    assert!(alert.score.abs() < f64::EPSILON); // 0.0
}

// =========================================================================
// Test 10: entity eval failure — nonexistent field
// =========================================================================

#[test]
fn entity_eval_failure() {
    let plan = simple_rule_plan(
        "r1",
        default_match_plan(),
        Expr::Number(70.0),
        "ip",
        // Reference a field that doesn't exist in scope_key
        Expr::Field(FieldRef::Simple("nonexistent".to_string())),
    );
    let exec = RuleExecutor::new(plan);
    let matched = default_matched_context();

    let result = exec.execute_match(&matched);
    assert!(result.is_err());
}

// =========================================================================
// Test 11: wfx_id deterministic
// =========================================================================

#[test]
fn wfx_id_deterministic() {
    let plan = simple_rule_plan(
        "r1",
        default_match_plan(),
        Expr::Number(50.0),
        "ip",
        Expr::Field(FieldRef::Simple("sip".to_string())),
    );
    let exec = RuleExecutor::new(plan);
    let matched = default_matched_context();

    let alert1 = exec.execute_match(&matched).unwrap();
    let alert2 = exec.execute_match(&matched).unwrap();

    // Same inputs produce the same content hash
    assert_eq!(alert1.wfx_id, alert2.wfx_id);
    // 16 hex characters
    assert_eq!(alert1.wfx_id.len(), 16);
    assert!(alert1.wfx_id.chars().all(|c| c.is_ascii_hexdigit()));
}

// =========================================================================
// Test 12: summary format
// =========================================================================

#[test]
fn summary_format() {
    let plan = simple_rule_plan(
        "brute_force",
        default_match_plan(),
        Expr::Number(70.0),
        "ip",
        Expr::Field(FieldRef::Simple("sip".to_string())),
    );
    let exec = RuleExecutor::new(plan);
    let matched = MatchedContext {
        rule_name: "brute_force".to_string(),
        scope_key: vec![str_val("10.0.0.1")],
        step_data: vec![StepData {
            satisfied_branch_index: 0,
            label: Some("fail".to_string()),
            measure_value: 5.0,
            collected_values: Vec::new(),
            field_values: std::collections::HashMap::new(),
        }],
        bind_data: vec![],
        event_time_nanos: 0,
    };

    let alert = exec.execute_match(&matched).unwrap();
    assert!(alert.summary.contains("brute_force"));
    assert!(alert.summary.contains("sip=10.0.0.1"));
    assert!(alert.summary.contains("fail=5.0"));
}

// =========================================================================
// Test 13: numeric key preserves type in eval context
// =========================================================================

#[test]
fn numeric_key_preserves_type_in_eval_context() {
    // Use dport=443 as a numeric key, score = dport / 100.
    // If dport is correctly preserved as Value::Number, score = 443/100 = 4.43.
    // If dport were stringified ("443") then the division would fail.
    let match_plan = simple_plan(
        vec![simple_key("dport")],
        vec![step(vec![branch("conn", count_ge(1.0))])],
    );
    let score_expr = Expr::BinOp {
        op: BinOp::Div,
        left: Box::new(Expr::Field(FieldRef::Simple("dport".to_string()))),
        right: Box::new(Expr::Number(100.0)),
    };
    let plan = simple_rule_plan(
        "r_numeric_key",
        match_plan,
        score_expr,
        "port",
        Expr::Field(FieldRef::Simple("dport".to_string())),
    );
    let exec = RuleExecutor::new(plan);
    let matched = MatchedContext {
        rule_name: "r_numeric_key".to_string(),
        scope_key: vec![num(443.0)],
        step_data: vec![StepData {
            satisfied_branch_index: 0,
            label: None,
            measure_value: 1.0,
            collected_values: Vec::new(),
            field_values: std::collections::HashMap::new(),
        }],
        bind_data: vec![],
        event_time_nanos: 0,
    };

    let alert = exec.execute_match(&matched).unwrap();
    // score = 443.0 / 100.0 = 4.43, clamped to [0, 100]
    assert!((alert.score - 4.43).abs() < f64::EPSILON);
    assert_eq!(alert.entity_id, "443");
}

// =========================================================================
// Test 14: label does not overwrite key in eval context
// =========================================================================

#[test]
fn label_cannot_overwrite_key_in_eval_context() {
    // Key "sip" = "10.0.0.1" (string), label also named "sip" with measure 99.0.
    // entity(ip, sip) should resolve to "10.0.0.1" (the key), not "99" (the label).
    let match_plan = simple_plan(
        vec![simple_key("sip")],
        vec![step(vec![branch_with_label("fail", "sip", count_ge(1.0))])],
    );
    let plan = simple_rule_plan(
        "r1",
        match_plan,
        Expr::Number(50.0),
        "ip",
        Expr::Field(FieldRef::Simple("sip".to_string())),
    );
    let exec = RuleExecutor::new(plan);
    let matched = MatchedContext {
        rule_name: "r1".to_string(),
        scope_key: vec![str_val("10.0.0.1")],
        step_data: vec![StepData {
            satisfied_branch_index: 0,
            label: Some("sip".to_string()),
            measure_value: 99.0,
            collected_values: Vec::new(),
            field_values: std::collections::HashMap::new(),
        }],
        bind_data: vec![],
        event_time_nanos: 0,
    };

    let alert = exec.execute_match(&matched).unwrap();
    // Key must win: entity_id should be "10.0.0.1", not "99"
    assert_eq!(alert.entity_id, "10.0.0.1");
}

// =========================================================================
// Test 15: wfx_id is valid 16-hex format with no separators
// =========================================================================

#[test]
fn wfx_id_hex_format() {
    let match_plan = simple_plan(
        vec![simple_key("tag")],
        vec![step(vec![branch("src", count_ge(1.0))])],
    );
    let plan = simple_rule_plan(
        "r1",
        match_plan,
        Expr::Number(50.0),
        "tag",
        Expr::Field(FieldRef::Simple("tag".to_string())),
    );
    let exec = RuleExecutor::new(plan);
    let matched = MatchedContext {
        rule_name: "r1".to_string(),
        scope_key: vec![str_val("a,b|c")],
        step_data: vec![StepData {
            satisfied_branch_index: 0,
            label: None,
            measure_value: 1.0,
            collected_values: Vec::new(),
            field_values: std::collections::HashMap::new(),
        }],
        bind_data: vec![],
        event_time_nanos: 0,
    };

    let alert = exec.execute_match(&matched).unwrap();
    // wfx_id is exactly 16 hex characters, no separators
    assert_eq!(alert.wfx_id.len(), 16);
    assert!(
        alert.wfx_id.chars().all(|c| c.is_ascii_hexdigit()),
        "wfx_id should be all hex digits, got: {}",
        alert.wfx_id
    );
    // No structural separators
    assert!(!alert.wfx_id.contains('|'));
    assert!(!alert.wfx_id.contains('#'));
}
