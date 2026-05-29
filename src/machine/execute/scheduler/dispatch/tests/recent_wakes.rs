use crate::builtins::default_scope;
use crate::machine::core::source::Spanned;
use crate::machine::execute::Scheduler;
use crate::machine::execute::nodes::{LiftState, NodeOutput, NodeWork};
use crate::machine::model::KObject;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::{NodeId, RuntimeArena};

/// The `recent_wakes` side-channel is `Dispatch`-only. A non-`Dispatch`
/// consumer (here a `Lift(Pending(producer))` slot) parked on a
/// `Dispatch` producer must drain to an empty Vec — `push_recent_wake`
/// filters non-Dispatch work via the same peek-discriminator pattern
/// as `stamp_lift_ready`. The Lift's stamp-then-enqueue path stays
/// intact (asserted indirectly through full-suite parity).
#[test]
fn recent_wakes_empty_for_non_dispatch_consumer() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let producer = sched.add_dispatch(
        KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier("_p".into()))]),
        scope,
    );
    let consumer = sched.add(NodeWork::Lift(LiftState::Pending(producer)), scope);
    let value = arena.alloc(KObject::Number(1.0));
    sched.finalize(producer.index(), NodeOutput::Value(value));
    assert!(sched.store.take_recent_wakes(consumer).is_empty());
}

/// A `Dispatch` consumer parked on a `Dispatch` producer records the
/// producer's `NodeId` in `recent_wakes` when the producer finalizes.
/// The drained list is the side channel the per-variant resume entries
/// key off when waking from a track-install.
#[test]
fn recent_wakes_records_producer_for_dispatch_consumer() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let producer = sched.add_dispatch(
        KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier("_p".into()))]),
        scope,
    );
    let consumer = sched.add_dispatch(
        KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier("_c".into()))]),
        scope,
    );
    sched.deps.add_park_edge(producer, consumer);
    let value = arena.alloc(KObject::Number(1.0));
    sched.finalize(producer.index(), NodeOutput::Value(value));
    let wakes = sched.store.take_recent_wakes(consumer);
    assert_eq!(wakes, vec![producer]);
    assert!(sched.store.take_recent_wakes(consumer).is_empty());
}

/// Sanity: a `NodeId` constructed via `add_dispatch` indexes the
/// freshly-grown `recent_wakes` slot — drain on a never-woken
/// Dispatch returns empty.
#[test]
fn recent_wakes_drain_default_empty() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let id: NodeId = sched.add_dispatch(
        KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier("_".into()))]),
        scope,
    );
    assert!(sched.store.take_recent_wakes(id).is_empty());
}
