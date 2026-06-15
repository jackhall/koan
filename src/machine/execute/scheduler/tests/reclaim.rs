//! `free` / node-reclamation invariants.

use super::super::dep_graph::DepEdge;
use crate::builtins::default_scope;
use crate::machine::execute::KoanRuntime;
use crate::machine::model::ast::KExpression;
use crate::machine::model::{Carried, KObject};
use crate::machine::RuntimeArena;

#[test]
fn free_reclaims_owned_subtree() {
    // s0 ─Owned→ s1 ─Owned→ s2 ─Owned→ s3; free(s1) reclaims s1..s3, leaves s0.
    let arena = RuntimeArena::new();
    let root = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = KoanRuntime::new();
    let value: &KObject = arena.alloc_object(KObject::Number(42.0));
    let mk_dispatch = || crate::machine::execute::dispatch::decide(KExpression::new(Vec::new()));
    let s0 = sched.add(mk_dispatch(), root);
    let s1 = sched.add(mk_dispatch(), root);
    let s2 = sched.add(mk_dispatch(), root);
    let s3 = sched.add(mk_dispatch(), root);
    let store = sched.scheduler_mut();
    for id in [s0, s1, s2, s3] {
        store.store.clear_node(id);
    }
    store.store.set_result(s1, Ok(Carried::Object(value)));
    store.store.set_result(s2, Ok(Carried::Object(value)));
    store.store.set_result(s3, Ok(Carried::Object(value)));
    store
        .deps
        .set_dep_edges(s0.index(), vec![DepEdge::Owned(s1)]);
    store
        .deps
        .set_dep_edges(s1.index(), vec![DepEdge::Owned(s2)]);
    store
        .deps
        .set_dep_edges(s2.index(), vec![DepEdge::Owned(s3)]);

    sched.free(s1.index());

    assert!(
        sched.scheduler().store.result_is_none(s1),
        "s1 result cleared"
    );
    assert!(
        sched.scheduler().store.result_is_none(s2),
        "s2 result cleared"
    );
    assert!(
        sched.scheduler().store.result_is_none(s3),
        "s3 result cleared"
    );
    assert!(
        sched.scheduler().deps.dep_edges_at(s1.index()).is_empty(),
        "s1 deps drained"
    );
    assert!(
        sched.scheduler().deps.dep_edges_at(s2.index()).is_empty(),
        "s2 deps drained"
    );
    let s0_edges = sched.scheduler().deps.dep_edges_at(s0.index());
    assert_eq!(s0_edges.len(), 1, "s0 edges untouched");
    assert!(
        matches!(s0_edges[0], DepEdge::Owned(id) if id == s1),
        "s0 still owns s1",
    );
    let mut freed = sched.scheduler().store.free_list_snapshot();
    freed.sort();
    assert_eq!(freed, vec![s1, s2, s3]);

    let reused = sched.add(mk_dispatch(), root);
    assert!(
        sched.scheduler().store.free_list_len() == 2,
        "one slot popped from free_list"
    );
    assert!(
        [s1, s2, s3].contains(&reused),
        "reused index came from free_list"
    );
}

#[test]
fn free_skips_live_slot_and_is_idempotent() {
    let arena = RuntimeArena::new();
    let root = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = KoanRuntime::new();
    let mk_dispatch = || crate::machine::execute::dispatch::decide(KExpression::new(Vec::new()));
    let s = sched.add(mk_dispatch(), root);
    // Live slot: free must be a no-op.
    sched.free(s.index());
    assert!(sched.scheduler().store.is_live(s));
    assert_eq!(sched.scheduler().store.free_list_len(), 0);

    sched.scheduler_mut().store.clear_node(s);
    let value: &KObject = arena.alloc_object(KObject::Number(1.0));
    sched
        .scheduler_mut()
        .store
        .set_result(s, Ok(Carried::Object(value)));
    sched.free(s.index());
    assert_eq!(sched.scheduler().store.free_list_snapshot(), vec![s]);
    sched.free(s.index());
    assert_eq!(
        sched.scheduler().store.free_list_snapshot(),
        vec![s],
        "no duplicate free"
    );
}

#[test]
fn free_does_not_recurse_through_notify_edges() {
    // Regression canary for the Owned/Notify conflation fixed by `DepEdge`:
    // free(owner) must reclaim only Owned descendants, not parked-on siblings.
    let arena = RuntimeArena::new();
    let root = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = KoanRuntime::new();
    let value: &KObject = arena.alloc_object(KObject::Number(7.0));
    let mk_dispatch = || crate::machine::execute::dispatch::decide(KExpression::new(Vec::new()));
    let s_owner = sched.add(mk_dispatch(), root);
    let s_owned = sched.add(mk_dispatch(), root);
    let s_sibling = sched.add(mk_dispatch(), root);
    let store = sched.scheduler_mut();
    for id in [s_owner, s_owned, s_sibling] {
        store.store.clear_node(id);
    }
    store.store.set_result(s_owner, Ok(Carried::Object(value)));
    store.store.set_result(s_owned, Ok(Carried::Object(value)));
    store
        .store
        .set_result(s_sibling, Ok(Carried::Object(value)));
    // Sibling self-loop is synthetic: a real scheduler never installs one, but it
    // gives the bug-shape something to walk into so we can assert the walk stopped.
    store.deps.set_dep_edges(
        s_owner.index(),
        vec![DepEdge::Owned(s_owned), DepEdge::Notify(s_sibling)],
    );
    store.deps.set_dep_edges(s_owned.index(), Vec::new());
    store
        .deps
        .set_dep_edges(s_sibling.index(), vec![DepEdge::Owned(s_sibling)]);

    sched.free(s_owner.index());

    let mut freed = sched.scheduler().store.free_list_snapshot();
    freed.sort();
    let mut expected = vec![s_owner, s_owned];
    expected.sort();
    assert_eq!(
        freed, expected,
        "free must not recurse through Notify edges"
    );
    assert!(
        sched.scheduler().store.result_is_some(s_sibling),
        "sibling's result must survive free of a slot that only parked on it",
    );
    assert_eq!(
        sched.scheduler().deps.dep_edges_at(s_sibling.index()).len(),
        1,
        "sibling's dep_edges must survive (the free walk stopped at the Notify edge)",
    );
}

#[test]
fn freed_slot_does_not_appear_in_other_notify_lists() {
    // Reclamation invariant: after `free(idx)`, no other slot's `notify_list` may
    // reference `idx`. Canary against a future change that frees a slot before its
    // producer drains, leaving a stale edge to misfire onto a reused slot.
    let arena = RuntimeArena::new();
    let root = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = KoanRuntime::new();

    let exprs = crate::parse::parse(
        "LET x = 1\n\
         LET y = 2\n\
         LET z = (LET a = 3)",
    )
    .expect("parse should succeed");
    for e in exprs {
        sched.dispatch_in_scope(e, root);
    }
    sched.execute().expect("program should run");

    let freed: std::collections::HashSet<usize> = sched
        .scheduler()
        .store
        .free_list_snapshot()
        .into_iter()
        .map(|id| id.index())
        .collect();
    for (producer_idx, consumers) in sched.scheduler().deps.notify_list_iter() {
        for &consumer in consumers {
            assert!(
                !freed.contains(&consumer),
                "stale notify edge = producer slot {producer_idx} still lists \
                 freed consumer slot {consumer} in its notify_list",
            );
        }
    }
}
