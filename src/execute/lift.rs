use std::collections::HashMap;
use std::rc::Rc;

use crate::dispatch::{CallArena, KFuture, KObject, RuntimeArena};
use crate::parse::{ExpressionPart, KExpression};

/// Lift a KObject out of `dying_frame`'s arena into the destination arena, attaching
/// an `Rc<CallArena>` to anchor any descendant that borrows into the dying arena.
///
/// Per-arm rules (closure-arena equality, KFuture targeted membership, composite
/// memoization) and the `functions_is_empty` fast-path soundness argument are documented
/// in [memory-model.md § Closure escape](../../design/memory-model.md#closure-escape-per-call-arenas--rc)
/// and [§ Fast path](../../design/memory-model.md#fast-path).
///
/// Caveat the design doc doesn't yet cover: the fast path is sound *today* only because
/// KFutures don't escape as values. Once they do, this gate must add a
/// no-unanchored-KFuture-descendant clause (the slow path's KFuture arm is already correct).
/// Test-only re-export of `lift_kobject` so cross-module Miri tests (e.g.
/// `dispatch::values::module::tests::functor_per_call_module_lifts_correctly`) can
/// exercise the per-arm anchor logic in isolation.
#[cfg(test)]
pub fn lift_kobject_for_test<'b>(v: &KObject<'b>, dying_frame: &Rc<CallArena>) -> KObject<'b> {
    lift_kobject(v, dying_frame)
}

pub(super) fn lift_kobject<'b>(v: &KObject<'b>, dying_frame: &Rc<CallArena>) -> KObject<'b> {
    if dying_frame.arena().functions_is_empty() {
        return v.deep_clone();
    }
    match v {
        KObject::KFunction(f, existing) => {
            let new_frame = if existing.is_some() {
                existing.clone()
            } else {
                let dying_runtime: *const RuntimeArena = dying_frame.arena();
                let captured_runtime: *const RuntimeArena = f.captured_scope().arena;
                if std::ptr::eq(captured_runtime, dying_runtime) {
                    Some(Rc::clone(dying_frame))
                } else {
                    None
                }
            };
            KObject::KFunction(f, new_frame)
        }
        KObject::KFuture(t, existing) => {
            let new_frame = if existing.is_some() {
                existing.clone()
            } else if kfuture_borrows_dying_arena(t, dying_frame.arena()) {
                Some(Rc::clone(dying_frame))
            } else {
                None
            };
            KObject::KFuture(t.deep_clone(), new_frame)
        }
        KObject::KModule(m, existing) => {
            // Mirror of the `KFunction` arm: if the module's child scope was alloc'd in
            // the dying frame's arena (a functor body's freshly-built `MODULE Result =
            // (...)`), anchor on the dying frame's `Rc` so the child scope outlives the
            // returned `&Module`. Pre-anchored values (e.g. lifted twice) keep their
            // existing frame; modules built outside this frame need no anchor.
            let new_frame = if existing.is_some() {
                existing.clone()
            } else {
                let dying_runtime: *const RuntimeArena = dying_frame.arena();
                let module_runtime: *const RuntimeArena = m.child_scope().arena;
                if std::ptr::eq(module_runtime, dying_runtime) {
                    Some(Rc::clone(dying_frame))
                } else {
                    None
                }
            };
            KObject::KModule(m, new_frame)
        }
        KObject::List(items) => {
            if items.iter().any(|x| needs_lift(x, dying_frame)) {
                let lifted: Vec<KObject<'b>> = items
                    .iter()
                    .map(|x| lift_kobject(x, dying_frame))
                    .collect();
                KObject::List(Rc::new(lifted))
            } else {
                KObject::List(Rc::clone(items))
            }
        }
        KObject::Dict(entries) => {
            if entries.values().any(|x| needs_lift(x, dying_frame)) {
                let lifted: HashMap<_, _> = entries
                    .iter()
                    .map(|(k, v)| (k.clone_box(), lift_kobject(v, dying_frame)))
                    .collect();
                KObject::Dict(Rc::new(lifted))
            } else {
                KObject::Dict(Rc::clone(entries))
            }
        }
        KObject::Tagged { tag, value } => {
            if needs_lift(value, dying_frame) {
                KObject::Tagged {
                    tag: tag.clone(),
                    value: Rc::new(lift_kobject(value, dying_frame)),
                }
            } else {
                KObject::Tagged {
                    tag: tag.clone(),
                    value: Rc::clone(value),
                }
            }
        }
        other => other.deep_clone(),
    }
}

/// True iff some descendant of `v` satisfies `predicate`. The predicate returns
/// `Some(true)` to short-circuit, `Some(false)` to bottom out the current subtree
/// without recursing, or `None` to let the walker recurse into composite payloads.
///
/// Single source of variant coverage for `needs_lift` and `kobject_borrows_arena`;
/// the two consumers differ only in their per-leaf decision. Adding a new composite
/// variant updates this walker once instead of two parallel match trees.
fn any_descendant<'b, F>(v: &KObject<'b>, predicate: &F) -> bool
where
    F: Fn(&KObject<'b>) -> Option<bool>,
{
    if let Some(decision) = predicate(v) {
        return decision;
    }
    match v {
        KObject::List(items) => items.iter().any(|x| any_descendant(x, predicate)),
        KObject::Dict(entries) => entries.values().any(|x| any_descendant(x, predicate)),
        KObject::Tagged { value, .. } => any_descendant(value, predicate),
        KObject::Struct { fields, .. } => fields
            .values()
            .any(|x| any_descendant(x, predicate)),
        KObject::KExpression(e) => e.parts.iter().any(|p| match p {
            ExpressionPart::Future(obj) => any_descendant(obj, predicate),
            ExpressionPart::Expression(inner) => inner.parts.iter().any(|p2| match p2 {
                ExpressionPart::Future(obj) => any_descendant(obj, predicate),
                _ => false,
            }),
            _ => false,
        }),
        // Predicate-returned-None on a non-composite variant is treated as a `false`
        // leaf — the predicate is responsible for classifying every leaf it cares about.
        _ => false,
    }
}

/// True iff lifting `v` against `dying_frame` would attach an `Rc` to some descendant.
/// Drives `lift_kobject`'s fast-path skip and the per-composite rebuild decision.
///
/// Bottoms out on `Struct`/`KExpression`: those variants aren't reachable as values
/// inside a List/Dict/Tagged at lift time in current Koan, so the structural recursion
/// in `any_descendant` is left forward-compatible without changing the observable answer.
fn needs_lift<'b>(v: &KObject<'b>, dying_frame: &Rc<CallArena>) -> bool {
    let dying_runtime: *const RuntimeArena = dying_frame.arena();
    any_descendant(v, &|obj: &KObject<'b>| match obj {
        KObject::KFunction(_, Some(_)) => Some(false),
        KObject::KFunction(f, None) => {
            let captured_runtime: *const RuntimeArena = f.captured_scope().arena;
            Some(std::ptr::eq(captured_runtime, dying_runtime))
        }
        KObject::KFuture(_, Some(_)) => Some(false),
        KObject::KFuture(t, None) => Some(kfuture_borrows_dying_arena(t, dying_frame.arena())),
        KObject::KModule(_, Some(_)) => Some(false),
        KObject::KModule(m, None) => {
            let module_runtime: *const RuntimeArena = m.child_scope().arena;
            Some(std::ptr::eq(module_runtime, dying_runtime))
        }
        KObject::Struct { .. } | KObject::KExpression(_) => Some(false),
        KObject::List(_) | KObject::Dict(_) | KObject::Tagged { .. } => None,
        _ => Some(false),
    })
}

/// True iff any descendant of an unanchored `KFuture` borrows into `arena`: the
/// function reference's captured arena, the parsed expression's `Future(&KObject)`
/// parts, or the bundle args (which may transitively carry a borrowing payload).
fn kfuture_borrows_dying_arena<'b>(t: &KFuture<'b>, arena: &RuntimeArena) -> bool {
    if std::ptr::eq(t.function.captured_scope().arena, arena as *const RuntimeArena) {
        return true;
    }
    if expression_borrows_arena(&t.parsed, arena) {
        return true;
    }
    t.bundle
        .args
        .values()
        .any(|v| kobject_borrows_arena(v, arena))
}

fn expression_borrows_arena<'b>(expr: &KExpression<'b>, arena: &RuntimeArena) -> bool {
    expr.parts.iter().any(|p| part_borrows_arena(p, arena))
}

fn part_borrows_arena<'b>(part: &ExpressionPart<'b>, arena: &RuntimeArena) -> bool {
    match part {
        ExpressionPart::Future(obj) => arena.owns_object(*obj as *const KObject<'b>),
        ExpressionPart::Expression(e) => expression_borrows_arena(e, arena),
        ExpressionPart::ListLiteral(items) => items.iter().any(|p| part_borrows_arena(p, arena)),
        ExpressionPart::DictLiteral(pairs) => pairs.iter().any(|(k, v)| {
            part_borrows_arena(k, arena) || part_borrows_arena(v, arena)
        }),
        _ => false,
    }
}

/// True iff any descendant of `v` borrows into `arena`. KExpression and KFuture
/// settle as predicate leaves (their recursion is not `KObject`-shaped — parts,
/// bundle args, function ref) so the walker doesn't double-traverse via the
/// KExpression arm.
fn kobject_borrows_arena<'b>(v: &KObject<'b>, arena: &RuntimeArena) -> bool {
    any_descendant(v, &|obj: &KObject<'b>| match obj {
        KObject::KExpression(e) => Some(expression_borrows_arena(e, arena)),
        KObject::KFuture(t, _) => Some(kfuture_borrows_dying_arena(t, arena)),
        KObject::KFunction(f, _) => Some(std::ptr::eq(
            f.captured_scope().arena,
            arena as *const RuntimeArena,
        )),
        KObject::KModule(m, _) => Some(std::ptr::eq(
            m.child_scope().arena,
            arena as *const RuntimeArena,
        )),
        KObject::List(_)
        | KObject::Dict(_)
        | KObject::Tagged { .. }
        | KObject::Struct { .. } => None,
        _ => Some(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::{default_scope, CallArena, KObject};
    use crate::parse::parse;

    /// A KFuture with no descendant borrow into the dying arena must lift to
    /// `frame: None` — anchoring would over-keep the arena. The dummy KFunction
    /// below defeats `functions_is_empty()`'s fast path so the slow path runs.
    #[test]
    fn unanchored_kfuture_no_arena_borrow_does_not_anchor() {
        use crate::dispatch::{Body, ExpressionSignature, KFunction, KType, SignatureElement};

        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let dying = CallArena::new(scope, None);
        let kf = KFunction::new(
            ExpressionSignature {
                return_type: KType::Null,
                elements: vec![SignatureElement::Keyword("__SLOW__".into())],
            },
            Body::Builtin(|s, _, _| crate::dispatch::BodyResult::Value(
                s.arena.alloc_object(KObject::Null)
            )),
            dying.scope(),
        );
        let _ = dying.arena().alloc_function(kf);

        let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
        let parsed = exprs.remove(0);
        let future = scope.dispatch(parsed).expect("dispatch should succeed");
        let kf_obj = KObject::KFuture(future, None);

        let strong_before = Rc::strong_count(&dying);

        let lifted = lift_kobject(&kf_obj, &dying);

        match lifted {
            KObject::KFuture(_, frame) => assert!(
                frame.is_none(),
                "KFuture without descendant borrows into dying arena must lift to frame=None",
            ),
            other => panic!("expected lifted KFuture, got {:?}", other.ktype()),
        }
        assert_eq!(
            Rc::strong_count(&dying),
            strong_before,
            "lifting a non-borrowing KFuture must not bump the dying frame's Rc",
        );
    }

    /// Symmetric case: a KFuture whose parsed parts contain a `Future(&KObject)`
    /// allocated in the dying arena must lift with `frame: Some(rc)`.
    #[test]
    fn unanchored_kfuture_with_arena_borrow_does_anchor() {
        use crate::dispatch::{Body, ExpressionSignature, KFunction, KType, SignatureElement};

        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let dying = CallArena::new(scope, None);

        // Defeat `functions_is_empty()` fast path so the slow path runs. Captured
        // scope lives in `dying.arena()` to satisfy `alloc_function`'s invariant.
        let kf = KFunction::new(
            ExpressionSignature {
                return_type: KType::Null,
                elements: vec![SignatureElement::Keyword("__SLOW__".into())],
            },
            Body::Builtin(|s, _, _| crate::dispatch::BodyResult::Value(
                s.arena.alloc_object(KObject::Null)
            )),
            dying.scope(),
        );
        let _ = dying.arena().alloc_function(kf);

        let mut exprs = parse("PRINT \"hi\"").expect("parse should succeed");
        let parsed = exprs.remove(0);
        let mut future = scope.dispatch(parsed).expect("dispatch should succeed");
        let inside: &KObject = dying.arena().alloc_object(KObject::Number(7.0));
        future.parsed.parts.push(ExpressionPart::Future(inside));
        let kf_obj = KObject::KFuture(future, None);

        let strong_before = Rc::strong_count(&dying);
        let lifted = lift_kobject(&kf_obj, &dying);
        match &lifted {
            KObject::KFuture(_, frame) => assert!(
                frame.is_some(),
                "KFuture borrowing into dying arena must lift with frame=Some(rc)",
            ),
            other => panic!("expected lifted KFuture, got {:?}", other.ktype()),
        }
        assert_eq!(
            Rc::strong_count(&dying),
            strong_before + 1,
            "lifting a borrowing KFuture must clone the dying frame's Rc once",
        );
        // Drop borrowers before `dying` so arena teardown order is well-defined.
        drop(lifted);
        drop(kf_obj);
    }
}
