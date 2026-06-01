use crate::builtins::default_scope;
use crate::machine::core::source::Spanned;
use crate::machine::execute::nodes::{LiftState, NodeOutput, NodeWork};
use crate::machine::execute::Scheduler;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::KObject;
use crate::machine::{NodeId, RuntimeArena};

/// `recent_wakes` is Dispatch-only; non-Dispatch consumers must drain empty.
#[test]
fn recent_wakes_empty_for_non_dispatch_consumer() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let producer = sched.add(
        NodeWork::dispatch(KExpression::new(vec![Spanned::bare(
            ExpressionPart::Identifier("_p".into()),
        )])),
        scope,
    );
    let consumer = sched.add(NodeWork::Lift(LiftState::Pending(producer)), scope);
    let value = arena.alloc(KObject::Number(1.0));
    sched.finalize(producer.index(), NodeOutput::Value(value));
    assert!(sched.store.take_recent_wakes(consumer).is_empty());
}

#[test]
fn recent_wakes_records_producer_for_dispatch_consumer() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let producer = sched.add(
        NodeWork::dispatch(KExpression::new(vec![Spanned::bare(
            ExpressionPart::Identifier("_p".into()),
        )])),
        scope,
    );
    let consumer = sched.add(
        NodeWork::dispatch(KExpression::new(vec![Spanned::bare(
            ExpressionPart::Identifier("_c".into()),
        )])),
        scope,
    );
    sched.deps.add_park_edge(producer, consumer);
    let value = arena.alloc(KObject::Number(1.0));
    sched.finalize(producer.index(), NodeOutput::Value(value));
    let wakes = sched.store.take_recent_wakes(consumer);
    assert_eq!(wakes, vec![producer]);
    assert!(sched.store.take_recent_wakes(consumer).is_empty());
}

#[test]
fn recent_wakes_drain_default_empty() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let id: NodeId = sched.add(
        NodeWork::dispatch(KExpression::new(vec![Spanned::bare(
            ExpressionPart::Identifier("_".into()),
        )])),
        scope,
    );
    assert!(sched.store.take_recent_wakes(id).is_empty());
}
