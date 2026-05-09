use std::collections::HashMap;
use std::rc::Rc;

use crate::dispatch::runtime::{CallArena, KFuture, RuntimeArena};
use crate::dispatch::values::KObject;
use crate::parse::kexpression::{ExpressionPart, KExpression};

/// Lift a KObject value out of the dying frame's arena into the destination arena.
/// Owned variants (Number, KString, Bool, Null) `deep_clone` cleanly because their
/// content is owned. `KObject::KFunction(&f, frame)` is the special case: `&f` may
/// point into the dying frame's arena (an escaping closure). If so, we carry a clone
/// of the dying frame's `Rc<CallArena>` in the lifted value's frame field, so the
/// arena stays alive past the slot's frame drop and the `&f` reference remains valid.
/// If the function lives in a longer-lived arena (run-root or another live frame), no
/// Rc is needed and the lifted value's frame field stays `None`.
///
/// `KObject::KFuture`: the dying-frame Rc is attached only when the KFuture's bundle,
/// parsed parts, or function reference actually borrows from the dying arena. The
/// `kfuture_borrows_dying_arena` walk uses the dying arena's `owns_object` membership
/// query to check each `ExpressionPart::Future(&KObject)` ref and recurses into the
/// bundle's value subtrees; the function reference reuses the existing
/// captured-scope-arena equality check. KFutures whose payload is wholly external (a
/// closure with arena-external bundle args, say) lift with `frame: None`.
///
/// Pre-existing `Some(rc)` on the input value is preserved (the value is already
/// keeping some arena alive; we don't overwrite that with the current dying frame's).
///
/// Composite variants (`List`, `Dict`) recurse to find embedded closures that need an
/// Rc attach, but memoize via `needs_lift`: when no descendant needs lifting, the
/// payload's existing `Rc` is cloned instead of rebuilding the `Vec`/`HashMap`. This
/// makes a value's second-and-later lifts through a return chain O(N) walk + O(1)
/// rebuild for the unchanged composites — Koan's collection-immutability contract is
/// what makes the structural sharing safe.
///
/// Whole-tree fast path: if the dying arena has zero `KFunction`s allocated in it, no
/// descendant `&KFunction` can point into it (per `alloc_function`'s invariant). This
/// is sound *today* because KFutures don't escape as values — the only way a lifted
/// `v` could need anchoring under this condition is via a KFuture descendant, and
/// none exist in current usage. When KFutures begin escaping (planned async), this
/// gate must add a no-unanchored-KFuture-descendant clause; the slow path's KFuture
/// arm is already correct. The check is one O(1) emptiness query on the arena.
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

/// True iff lifting `v` against `dying_frame` would attach an `Rc` to some descendant.
/// Drives both `lift_kobject`'s top-level fast-path skip and the per-composite rebuild
/// decision: when this returns false, the existing `Rc<Vec>`/`Rc<HashMap>` can be cloned
/// instead of allocating a fresh one. Walks composites recursively but bottoms out on
/// the first match (`any`-style).
///
/// `KFuture(_, None)` defers to `kfuture_borrows_dying_arena` — a precise membership query
/// against the dying arena's allocated-object set, mirroring `lift_kobject`'s slow path.
fn needs_lift<'b>(v: &KObject<'b>, dying_frame: &Rc<CallArena>) -> bool {
    match v {
        KObject::KFunction(_, Some(_)) => false,
        KObject::KFunction(f, None) => {
            let dying_runtime: *const RuntimeArena = dying_frame.arena();
            let captured_runtime: *const RuntimeArena = f.captured_scope().arena;
            std::ptr::eq(captured_runtime, dying_runtime)
        }
        KObject::KFuture(_, Some(_)) => false,
        KObject::KFuture(t, None) => kfuture_borrows_dying_arena(t, dying_frame.arena()),
        KObject::List(items) => items.iter().any(|x| needs_lift(x, dying_frame)),
        KObject::Dict(entries) => entries.values().any(|x| needs_lift(x, dying_frame)),
        KObject::Tagged { value, .. } => needs_lift(value, dying_frame),
        _ => false,
    }
}

/// True iff any descendant of an unanchored `KFuture` borrows into `arena`. Walks the
/// `parsed.parts`' `Future(&KObject)` references (matched via `arena.owns_object`),
/// recurses into nested `Expression` / `ListLiteral` / `DictLiteral` parts, and walks the
/// `bundle.args` values (an arg can itself contain a `KExpression` whose parts borrow into
/// the arena, or a Tagged/List/Dict carrying such a thing transitively). The `function`
/// reference's arena is checked via the same captured-scope-equality test
/// `lift_kobject` uses for `KFunction(_, None)` — if `function`'s captured arena equals the
/// dying one, the KFuture's `&KFunction` is borrowing into the dying arena and the lift
/// must anchor.
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

/// Walk `expr`'s parts recursively, checking each `Future(&KObject)` against `arena.owns_object`
/// and recursing into nested `Expression` / `ListLiteral` / `DictLiteral` shapes. `Keyword`,
/// `Identifier`, `Type`, `Literal` carry only owned data, so they short-circuit `false`.
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

/// Recurse into a `KObject` looking for embedded `KExpression`s (whose `Future` parts may
/// borrow), nested KFutures, Tagged/Struct/List/Dict payloads, and KFunction captures whose
/// arena matches `arena`. Mirrors the structural shape of `needs_lift` but answers the
/// arena-membership question used by Stream 2's targeted KFuture anchor.
fn kobject_borrows_arena<'b>(v: &KObject<'b>, arena: &RuntimeArena) -> bool {
    match v {
        KObject::KExpression(e) => expression_borrows_arena(e, arena),
        KObject::KFuture(t, _) => kfuture_borrows_dying_arena(t, arena),
        KObject::KFunction(f, _) => std::ptr::eq(
            f.captured_scope().arena,
            arena as *const RuntimeArena,
        ),
        KObject::List(items) => items.iter().any(|x| kobject_borrows_arena(x, arena)),
        KObject::Dict(entries) => entries.values().any(|x| kobject_borrows_arena(x, arena)),
        KObject::Tagged { value, .. } => kobject_borrows_arena(value, arena),
        KObject::Struct { fields, .. } => fields.values().any(|x| kobject_borrows_arena(x, arena)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::builtins::default_scope;
    use crate::dispatch::runtime::CallArena;
    use crate::dispatch::values::KObject;
    use crate::parse::expression_tree::parse;

    /// A KFuture lifted against an arena it has no descendant borrow into should NOT
    /// pick up the dying frame's Rc — `frame: None` means no over-keep. This is the
    /// payoff of the targeted `kfuture_borrows_dying_arena` walk vs the previous
    /// always-anchor behavior. Forces the slow path by installing a dummy KFunction in
    /// the dying arena (otherwise `functions_is_empty()` short-circuits to deep_clone).
    #[test]
    fn unanchored_kfuture_no_arena_borrow_does_not_anchor() {
        use crate::dispatch::kfunction::{Body, KFunction};
        use crate::dispatch::types::{ExpressionSignature, KType, SignatureElement};

        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        // Dying frame is a fresh CallArena under the same scope chain; its arena is a
        // distinct allocation. We want `kfuture_borrows_dying_arena` to answer:
        // PRINT's captured scope is in `arena`, not in `dying.arena()`, and the parsed
        // expression's parts are owned data ("hi" literal) — no borrow into `dying`.
        let dying = CallArena::new(scope, None);
        let kf = KFunction::new(
            ExpressionSignature {
                return_type: KType::Null,
                elements: vec![SignatureElement::Keyword("__SLOW__".into())],
            },
            Body::Builtin(|s, _, _| crate::dispatch::kfunction::BodyResult::Value(
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

    /// Symmetric case: when the KFuture's parsed parts contain a `Future(&KObject)` that
    /// IS allocated in the dying arena, the lift must still anchor (frame=Some). To
    /// exercise the slow path the dying arena needs at least one KFunction allocated in
    /// it (otherwise `lift_kobject`'s `functions_is_empty` fast path returns
    /// `deep_clone`); we install a copy of PRINT under a new keyword to satisfy that.
    #[test]
    fn unanchored_kfuture_with_arena_borrow_does_anchor() {
        use crate::dispatch::kfunction::{Body, KFunction};
        use crate::dispatch::types::{ExpressionSignature, KType, SignatureElement};

        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let dying = CallArena::new(scope, None);

        // Force the dying arena out of the fast-path: allocate a no-op KFunction in it.
        // Captured scope lives in `dying.arena()`, satisfying `alloc_function`'s invariant.
        let kf = KFunction::new(
            ExpressionSignature {
                return_type: KType::Null,
                elements: vec![SignatureElement::Keyword("__SLOW__".into())],
            },
            Body::Builtin(|s, _, _| crate::dispatch::kfunction::BodyResult::Value(
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
        // Drop the lifted result (its frame Rc) and `kf_obj` (which holds `inside` via
        // `Future`) before `dying` drops, to keep arena teardown order well-defined.
        drop(lifted);
        drop(kf_obj);
    }
}
