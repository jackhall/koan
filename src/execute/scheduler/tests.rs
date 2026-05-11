use crate::dispatch::{default_scope, KObject, NodeId, RuntimeArena};
use crate::parse::{ExpressionPart, KExpression, KLiteral};

use super::super::nodes::{DepEdge, NodeOutput, NodeWork};
use super::Scheduler;

fn let_expr<'a>(name: &str, value: f64) -> KExpression<'a> {
    KExpression {
        parts: vec![
            ExpressionPart::Keyword("LET".into()),
            ExpressionPart::Identifier(name.into()),
            ExpressionPart::Keyword("=".into()),
            ExpressionPart::Literal(KLiteral::Number(value)),
        ],
    }
}

#[test]
fn dispatches_independent_expressions_in_order() {
    let arena = RuntimeArena::new();
    let root = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let id1 = sched.add_dispatch(let_expr("x", 1.0), root);
    let id2 = sched.add_dispatch(let_expr("y", 2.0), root);

    sched.execute().unwrap();

    assert!(matches!(sched.read(id1), KObject::Number(n) if *n == 1.0));
    assert!(matches!(sched.read(id2), KObject::Number(n) if *n == 2.0));
    let data = root.data.borrow();
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
    sched.add_dispatch(let_expr("a", 10.0), root);

    let lookup_a = KExpression {
        parts: vec![
            ExpressionPart::Keyword("LET".into()),
            ExpressionPart::Identifier("b".into()),
            ExpressionPart::Keyword("=".into()),
            ExpressionPart::Expression(Box::new(KExpression {
                parts: vec![ExpressionPart::Identifier("a".into())],
            })),
        ],
    };
    sched.add_dispatch(lookup_a, root);

    sched.execute().unwrap();
    let data = root.data.borrow();
    assert!(matches!(data.get("b"), Some(KObject::Number(n)) if *n == 10.0));
}

#[test]
fn free_reclaims_owned_subtree() {
    // Synthetic state:
    //   slot 0: parent Bind with subs [1]
    //   slot 1: Lift-shim dispatch owning bind 2
    //   slot 2: nested Bind with subs [3], result Value
    //   slot 3: leaf Dispatch with Value
    // After `free(1)`: slots 1, 2, 3 reclaimed; slot 0 untouched.
    let arena = RuntimeArena::new();
    let root = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let value: &KObject = arena.alloc_object(KObject::Number(42.0));
    // Allocate four slots by adding placeholder Dispatches.
    let mk_dispatch = || NodeWork::Dispatch(KExpression { parts: Vec::new() });
    let s0 = sched.add(mk_dispatch(), root).index();
    let s1 = sched.add(mk_dispatch(), root).index();
    let s2 = sched.add(mk_dispatch(), root).index();
    let s3 = sched.add(mk_dispatch(), root).index();
    // Simulate post-run state and wire the ownership graph by hand.
    for i in [s0, s1, s2, s3] {
        sched.nodes[i] = None;
    }
    sched.results[s1] = Some(NodeOutput::Value(value));
    sched.results[s2] = Some(NodeOutput::Value(value));
    sched.results[s3] = Some(NodeOutput::Value(value));
    sched.dep_edges[s0] = vec![DepEdge::Owned(NodeId(s1))];
    sched.dep_edges[s1] = vec![DepEdge::Owned(NodeId(s2))];
    sched.dep_edges[s2] = vec![DepEdge::Owned(NodeId(s3))];

    sched.free(s1);

    // s1, s2, s3 reclaimed; s0 untouched.
    assert!(sched.results[s1].is_none(), "s1 result cleared");
    assert!(sched.results[s2].is_none(), "s2 result cleared");
    assert!(sched.results[s3].is_none(), "s3 result cleared");
    assert!(sched.dep_edges[s1].is_empty(), "s1 deps drained");
    assert!(sched.dep_edges[s2].is_empty(), "s2 deps drained");
    assert_eq!(sched.dep_edges[s0].len(), 1, "s0 edges untouched");
    assert!(
        matches!(sched.dep_edges[s0][0], DepEdge::Owned(id) if id.index() == s1),
        "s0 still owns s1",
    );
    let mut freed: Vec<usize> = sched.free_list.to_vec();
    freed.sort();
    assert_eq!(freed, vec![s1, s2, s3]);

    let reused = sched.add(mk_dispatch(), root).index();
    assert!(sched.free_list.len() == 2, "one slot popped from free_list");
    assert!([s1, s2, s3].contains(&reused), "reused index came from free_list");
}

#[test]
fn free_skips_live_slot_and_is_idempotent() {
    let arena = RuntimeArena::new();
    let root = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let mk_dispatch = || NodeWork::Dispatch(KExpression { parts: Vec::new() });
    let s = sched.add(mk_dispatch(), root).index();
    // Live slot: free should be a no-op.
    sched.free(s);
    assert!(sched.nodes[s].is_some());
    assert!(sched.free_list.is_empty());

    sched.nodes[s] = None;
    let value: &KObject = arena.alloc_object(KObject::Number(1.0));
    sched.results[s] = Some(NodeOutput::Value(value));
    sched.free(s);
    assert_eq!(sched.free_list, vec![s]);
    sched.free(s);
    assert_eq!(sched.free_list, vec![s], "no duplicate free");
}

#[test]
fn free_does_not_recurse_through_notify_edges() {
    // Regression canary for the conflation bug fixed by `DepEdge`. Synthetic state:
    //   s_owner:   parent with dep_edges = [Owned(s_owned), Notify(s_sibling)]
    //   s_owned:   terminalized, owned by s_owner
    //   s_sibling: terminalized, parked-on by s_owner (must survive free of owner)
    // After `free(s_owner)`: only s_owner and s_owned land on `free_list`. The
    // sibling's `results` and `dep_edges` are untouched — the prior single-list
    // implementation would have reclaimed it as a transitive owned dep.
    let arena = RuntimeArena::new();
    let root = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let value: &KObject = arena.alloc_object(KObject::Number(7.0));
    let mk_dispatch = || NodeWork::Dispatch(KExpression { parts: Vec::new() });
    let s_owner = sched.add(mk_dispatch(), root).index();
    let s_owned = sched.add(mk_dispatch(), root).index();
    let s_sibling = sched.add(mk_dispatch(), root).index();
    for i in [s_owner, s_owned, s_sibling] {
        sched.nodes[i] = None;
    }
    sched.results[s_owner] = Some(NodeOutput::Value(value));
    sched.results[s_owned] = Some(NodeOutput::Value(value));
    sched.results[s_sibling] = Some(NodeOutput::Value(value));
    // Give the sibling a non-empty edge list so the bug-shape would observably
    // walk into it: a self-loop would never be installed in the real scheduler,
    // but it lets us assert the walk stopped at the Notify edge by checking the
    // list is still intact after free.
    sched.dep_edges[s_owner] = vec![
        DepEdge::Owned(NodeId(s_owned)),
        DepEdge::Notify(NodeId(s_sibling)),
    ];
    sched.dep_edges[s_owned] = Vec::new();
    sched.dep_edges[s_sibling] = vec![DepEdge::Owned(NodeId(s_sibling))];

    sched.free(s_owner);

    let mut freed = sched.free_list.clone();
    freed.sort();
    let mut expected = vec![s_owner, s_owned];
    expected.sort();
    assert_eq!(freed, expected, "free must not recurse through Notify edges");
    assert!(
        sched.results[s_sibling].is_some(),
        "sibling's result must survive free of a slot that only parked on it",
    );
    assert_eq!(
        sched.dep_edges[s_sibling].len(),
        1,
        "sibling's dep_edges must survive (the free walk stopped at the Notify edge)",
    );
}

#[test]
fn freed_slot_does_not_appear_in_other_notify_lists() {
    // Reclamation invariant: after `free(idx)`, `idx` must not appear in any other
    // slot's `notify_list`. Holds by construction — by the time `idx` is freed, its
    // pending_deps reached zero, which means every producer has already drained.
    // Canary against a future change that would free a slot before its producer
    // drained, leaving a stale edge to misfire onto a reused slot.
    let arena = RuntimeArena::new();
    let root = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();

    // Run a small program with sub-Dispatch fan-out to populate notify edges.
    let exprs = crate::parse::parse(
        "LET x = 1\n\
         LET y = 2\n\
         LET z = (LET a = 3)",
    )
    .expect("parse should succeed");
    for e in exprs {
        sched.add_dispatch(e, root);
    }
    sched.execute().expect("program should run");

    let freed: std::collections::HashSet<usize> =
        sched.free_list.iter().copied().collect();
    for (producer_idx, consumers) in sched.notify_list.iter().enumerate() {
        for &consumer in consumers {
            assert!(
                !freed.contains(&consumer),
                "stale notify edge: producer slot {producer_idx} still lists \
                 freed consumer slot {consumer} in its notify_list",
            );
        }
    }
}

#[test]
fn combine_waits_on_deps_then_runs_finish() {
    // Direct exercise of `Combine`: two trivial dep slots that resolve to numbers,
    // a finish closure that concatenates their string renderings into a KString.
    // Pins the contract that Combine waits on every dep before invoking finish and
    // that finish-returned BodyResult::Value lands in the slot's result.
    use crate::dispatch::{BodyResult, CombineFinish};
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let dep_a = sched.add_dispatch(let_expr("ca", 7.0), scope);
    let dep_b = sched.add_dispatch(let_expr("cb", 11.0), scope);
    let finish: CombineFinish = Box::new(|scope, _sched, results| {
        let a = match results[0] {
            KObject::Number(n) => *n,
            _ => return BodyResult::Err(crate::dispatch::KError::new(
                crate::dispatch::KErrorKind::ShapeError("a not number".into()),
            )),
        };
        let b = match results[1] {
            KObject::Number(n) => *n,
            _ => return BodyResult::Err(crate::dispatch::KError::new(
                crate::dispatch::KErrorKind::ShapeError("b not number".into()),
            )),
        };
        let allocated = scope.arena.alloc_object(KObject::KString(format!("{a}+{b}")));
        BodyResult::Value(allocated)
    });
    let combine_id = sched.add_combine(vec![dep_a, dep_b], scope, finish);
    sched.execute().unwrap();
    assert!(matches!(sched.read(combine_id), KObject::KString(s) if s == "7+11"));
}

#[test]
fn combine_short_circuits_on_dep_error() {
    // Synthetic state: a Combine whose two deps already hold terminal results — one
    // Value, one Err. Pins the contract that finish does not run when any dep
    // errored, and that the propagated error carries a "<combine>" frame matching
    // run_bind's "<bind>" convention.
    use crate::dispatch::{BodyResult, CombineFinish, KError, KErrorKind};
    use std::cell::Cell;
    use std::rc::Rc;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();

    // Allocate two placeholder Dispatch slots, drain the queue so add() doesn't
    // re-enqueue them at execute time, then overwrite their results directly
    // (mirrors the synthetic-state pattern used by `free_reclaims_owned_subtree`).
    let mk_dispatch = || NodeWork::Dispatch(KExpression { parts: Vec::new() });
    let dep_ok = sched.add(mk_dispatch(), scope);
    let dep_err = sched.add(mk_dispatch(), scope);
    sched.nodes[dep_ok.index()] = None;
    sched.nodes[dep_err.index()] = None;
    sched.queue.clear();
    sched.ready_set.clear();
    let value = arena.alloc_object(KObject::Number(99.0));
    sched.results[dep_ok.index()] = Some(NodeOutput::Value(value));
    sched.results[dep_err.index()] = Some(NodeOutput::Err(
        KError::new(KErrorKind::ShapeError("dep_err synthetic".into())),
    ));

    let invoked: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let invoked_clone = Rc::clone(&invoked);
    let finish: CombineFinish = Box::new(move |_scope, _sched, _results| {
        invoked_clone.set(true);
        BodyResult::Value(value)
    });
    let combine_id = sched.add_combine(vec![dep_ok, dep_err], scope, finish);
    sched.execute().unwrap();

    assert!(!invoked.get(), "finish must not run when a dep errored");
    let result = sched.read_result(combine_id);
    let err = match result {
        Err(e) => e.clone(),
        Ok(_) => panic!("combine should have errored"),
    };
    assert!(
        err.frames.iter().any(|f| f.function == "<combine>"),
        "propagated error should carry a <combine> frame, got {err}",
    );
}

#[test]
fn defer_to_lifts_slot_terminal_off_combine_id() {
    // Round-trip for `BodyResult::DeferTo(id)`: a builtin body returns
    // `DeferTo(combine_id)`, the slot rewrites to `Lift { from: combine_id }`, the
    // Combine resolves to a value, and the builtin's slot ends up with the same
    // terminal as the Combine. Pins the binder-body wrap-up shape MODULE / SIG use.
    use crate::dispatch::{
        default_scope, register_builtin, ArgumentBundle, BodyResult, CombineFinish,
        ExpressionSignature, KType, Scope, SignatureElement,
    };
    use crate::parse::ExpressionPart;

    // Builtin "DEFERTEST": no args; schedules a Combine over zero deps whose finish
    // returns a known KString, then returns `BodyResult::DeferTo(combine_id)`.
    fn body<'a>(
        scope: &'a Scope<'a>,
        sched: &mut dyn crate::dispatch::SchedulerHandle<'a>,
        _bundle: ArgumentBundle<'a>,
    ) -> BodyResult<'a> {
        let finish: CombineFinish<'a> = Box::new(|scope, _sched, _results| {
            let v = scope.arena.alloc_object(KObject::KString("from-combine".into()));
            BodyResult::Value(v)
        });
        let combine_id = sched.add_combine(Vec::new(), scope, finish);
        BodyResult::DeferTo(combine_id)
    }

    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    register_builtin(
        scope,
        "DEFERTEST",
        ExpressionSignature {
            return_type: KType::Str,
            elements: vec![SignatureElement::Keyword("DEFERTEST".into())],
        },
        body,
    );

    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(
        KExpression { parts: vec![ExpressionPart::Keyword("DEFERTEST".into())] },
        scope,
    );
    sched.execute().unwrap();
    assert!(
        matches!(sched.read(id), KObject::KString(s) if s == "from-combine"),
        "DEFERTEST slot's terminal should match the Combine's terminal",
    );
}

#[test]
fn tail_call_reuses_node_slot_in_place() {
    // MATCH returns `BodyResult::Tail`; the scheduler rewrites MATCH's slot to a
    // Dispatch of the matched branch body in place rather than spawning a fresh slot.
    let arena = RuntimeArena::new();
    let root = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let exprs = crate::parse::parse(
        "MATCH true WITH (true -> (\"hi\") false -> (\"no\"))",
    )
    .expect("parse should succeed");
    assert_eq!(exprs.len(), 1);
    let id = sched.add_dispatch(exprs.into_iter().next().unwrap(), root);

    sched.execute().unwrap();

    assert!(matches!(sched.read(id), KObject::KString(s) if s == "hi"));
    assert_eq!(
        sched.len(),
        1,
        "tail-call slot reuse: the MATCH's original slot should have been rewritten \
         to evaluate the matched branch's body, not allocate a new slot",
    );
}
