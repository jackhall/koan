//! Basic dispatch ordering and inter-expression lookup.

use crate::builtins::default_scope;
use crate::machine::model::KObject;
use crate::machine::{RuntimeArena, SchedulerHandle};
use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use super::super::Scheduler;

use super::let_expr;

#[test]
fn dispatches_independent_expressions_in_order() {
    let arena = RuntimeArena::new();
    let root = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let ids = sched.enter_block(root.id, vec![let_expr("x", 1.0), let_expr("y", 2.0)], root);
    let id1 = ids[0];
    let id2 = ids[1];

    sched.execute().unwrap();

    assert!(matches!(sched.read(id1), KObject::Number(n) if *n == 1.0));
    assert!(matches!(sched.read(id2), KObject::Number(n) if *n == 2.0));
    let data = root.bindings().data();
    assert!(data.contains_key("x"));
    assert!(data.contains_key("y"));
}

#[test]
fn later_expression_sees_earlier_binding_via_lookup() {
    // The second top-level expression spawns a sub-Dispatch for `(x)`; the earlier
    // LET runs first because its NodeId is smaller. Guards in-order processing.
    let arena = RuntimeArena::new();
    let root = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();

    let lookup_a = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Keyword("LET".into())),
        Spanned::bare(ExpressionPart::Identifier("b".into())),
        Spanned::bare(ExpressionPart::Keyword("=".into())),
        Spanned::bare(ExpressionPart::Expression(Box::new(KExpression::new(vec![
            Spanned::bare(ExpressionPart::Identifier("a".into())),
        ])))),
    ]);
    sched.enter_block(root.id, vec![let_expr("a", 10.0), lookup_a], root);

    sched.execute().unwrap();
    let data = root.bindings().data();
    assert!(matches!(data.get("b"), Some(KObject::Number(n)) if *n == 10.0));
}
