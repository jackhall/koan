use super::{Resolution, RuntimeArena, Scope};
use crate::runtime::builtins::test_support::run_root_bare;
use crate::runtime::machine::kfunction::{Body, KFunction, NodeId};
use crate::runtime::model::types::{Argument, ExpressionSignature, KType, SignatureElement, ReturnType};
use crate::runtime::model::values::KObject;

fn unit_signature<'a>() -> ExpressionSignature<'a> {
    ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![SignatureElement::Keyword("FOO".into())],
    }
}

fn body_no_op<'a>(
    _scope: &'a Scope<'a>,
    _sched: &mut dyn crate::runtime::machine::kfunction::SchedulerHandle<'a>,
    _bundle: crate::runtime::machine::kfunction::ArgumentBundle<'a>,
) -> crate::runtime::machine::kfunction::BodyResult<'a> {
    crate::runtime::machine::kfunction::BodyResult::Value(_scope.arena.alloc_object(KObject::Null))
}

/// Snapshot-iteration semantics: a re-entrant `bind_value` queues silently and only
/// becomes visible after `drain_pending`; the held iteration sees the pre-write state.
#[test]
fn add_during_active_data_borrow_queues_and_drains() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let pre = arena.alloc_object(KObject::Number(1.0));
    scope.bind_value("pre".to_string(), pre).unwrap();

    let new_entry = arena.alloc_object(KObject::Number(2.0));
    {
        let snapshot = scope.bindings().data();
        assert!(snapshot.contains_key("pre"));
        scope.bind_value("during".to_string(), new_entry).unwrap();
        assert!(!snapshot.contains_key("during"));
    }
    assert!(scope.bindings().data().get("during").is_none());
    scope.drain_pending();
    let after = scope.bindings().data();
    assert!(matches!(after.get("during"), Some(KObject::Number(n)) if *n == 2.0));
}

/// Companion to the queues-and-drains test above: the `debug_assert!` inside
/// `PendingQueue::drain` must fire when a deferred write surfaces a semantic `Err` on
/// retry. Sequence:
/// 1. Open a `data` borrow → forces step 2 to defer.
/// 2. `bind_value("a", obj1)` where `obj1` wraps `kfn1` → deferred.
/// 3. Drop the borrow.
/// 4. `register_function("b", kfn2, obj2)` where `kfn2` is pointer-distinct from
///    `kfn1` but has the same untyped signature → succeeds, seeds `kfn2` into the
///    bucket.
/// 5. `drain_pending()` retries step 2's deferred write. `try_apply` walks the bucket,
///    finds `kfn2` (pointer-distinct, structurally equal signature) → returns
///    `DuplicateOverload`. The `debug_assert!` fires.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "PendingQueue::drain")]
fn drain_debug_asserts_on_invariant_violation() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let kfn1 = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj1 = arena.alloc_object(KObject::KFunction(kfn1, None));
    let kfn2 = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj2 = arena.alloc_object(KObject::KFunction(kfn2, None));

    // 1. Hold an outer `data` borrow open so the bind in step 2 must defer.
    let snapshot = scope.bindings().data();
    // 2. Defers — borrow contention on `data`.
    scope.bind_value("a".to_string(), obj1).unwrap();
    // 3. Release the outer borrow so step 4's direct write can proceed.
    drop(snapshot);
    // 4. Succeeds and seeds `kfn2` into the functions bucket under the shared
    //    untyped signature.
    scope.register_function("b".to_string(), kfn2, obj2).unwrap();
    // 5. Retries the deferred `bind_value`. Bucket walk finds `kfn2` with a
    //    structurally-equal signature → `DuplicateOverload` → `debug_assert!` fires.
    scope.drain_pending();
}

#[test]
fn bind_value_errors_on_same_scope_rebind() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let v1 = arena.alloc_object(KObject::Number(1.0));
    let v2 = arena.alloc_object(KObject::Number(2.0));
    scope.bind_value("x".to_string(), v1).unwrap();
    let err = scope.bind_value("x".to_string(), v2).unwrap_err();
    match &err.kind {
        crate::runtime::machine::core::KErrorKind::Rebind { name } => assert_eq!(name, "x"),
        _ => panic!("expected Rebind, got {err}"),
    }
}

#[test]
fn bind_value_allows_shadowing_in_child_scope() {
    let arena = RuntimeArena::new();
    let outer = run_root_bare(&arena);
    let v1 = arena.alloc_object(KObject::Number(1.0));
    outer.bind_value("x".to_string(), v1).unwrap();
    let inner = arena.alloc_scope(outer.child_for_call());
    let v2 = arena.alloc_object(KObject::Number(2.0));
    inner.bind_value("x".to_string(), v2).unwrap();
    assert!(matches!(inner.lookup("x"), Some(KObject::Number(n)) if *n == 2.0));
    assert!(matches!(outer.lookup("x"), Some(KObject::Number(n)) if *n == 1.0));
}

#[test]
fn register_function_dedupes_exact_signature() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let f1 = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj1 = arena.alloc_object(KObject::KFunction(f1, None));
    scope.register_function("FOO".to_string(), f1, obj1).unwrap();
    let f2 = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj2 = arena.alloc_object(KObject::KFunction(f2, None));
    let err = scope.register_function("FOO".to_string(), f2, obj2).unwrap_err();
    assert!(
        matches!(&err.kind, crate::runtime::machine::core::KErrorKind::DuplicateOverload { name, .. } if name == "FOO"),
        "expected DuplicateOverload, got {err}",
    );
}

/// Companion to `register_function_dedupes_exact_signature`: routing a structurally
/// identical but pointer-distinct `KFunction` through the LET path
/// (`bind_value(KObject::KFunction(...))`) must also trip `DuplicateOverload`. Pre-
/// façade the LET path only dedup'd by `ptr::eq`, so a fresh-arena-allocated function
/// with matching signature silently doubled the bucket. The unified `try_apply` closes
/// this gap. Uses a different name from the prior FN so the test focuses on bucket
/// dedupe rather than the `Rebind`-on-existing-name path.
#[test]
fn bind_value_with_kfunction_dedupes_exact_signature_with_existing_fn() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let f1 = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj1 = arena.alloc_object(KObject::KFunction(f1, None));
    scope.register_function("FOO".to_string(), f1, obj1).unwrap();
    // Pointer-distinct, structurally identical signature — fresh arena allocation.
    let f2 = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj2 = arena.alloc_object(KObject::KFunction(f2, None));
    let err = scope
        .bind_value("OTHER_NAME".to_string(), obj2)
        .unwrap_err();
    assert!(
        matches!(&err.kind, crate::runtime::machine::core::KErrorKind::DuplicateOverload { name, .. } if name == "OTHER_NAME"),
        "expected DuplicateOverload from LET path, got {err}",
    );
}

/// The `ptr::eq` fast-path still allows intentional aliasing: `LET g = (f)` where the
/// same `&KFunction` is bound under a second name must succeed without
/// `DuplicateOverload`. This pins the rule that the bucket dedupe is silent-success on
/// pointer-equal entries and structural-rejection only on pointer-distinct ones.
#[test]
fn bind_value_with_kfunction_pointer_equal_alias_no_op() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let f = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj1 = arena.alloc_object(KObject::KFunction(f, None));
    let obj2 = arena.alloc_object(KObject::KFunction(f, None));
    scope.bind_value("FIRST".to_string(), obj1).unwrap();
    // Re-binding under a *different* name with the same `&KFunction` pointer — the
    // intentional-alias case. Must succeed.
    scope.bind_value("ALIAS".to_string(), obj2).unwrap();
}

#[test]
fn register_function_allows_overload_with_different_arg_types() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let sig_num = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![
            SignatureElement::Keyword("BAR".into()),
            SignatureElement::Argument(Argument { name: "v".into(), ktype: KType::Number }),
        ],
    };
    let sig_str = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![
            SignatureElement::Keyword("BAR".into()),
            SignatureElement::Argument(Argument { name: "v".into(), ktype: KType::Str }),
        ],
    };
    let f1 = arena.alloc_function(KFunction::new(sig_num, Body::Builtin(body_no_op), scope));
    let f2 = arena.alloc_function(KFunction::new(sig_str, Body::Builtin(body_no_op), scope));
    let obj1 = arena.alloc_object(KObject::KFunction(f1, None));
    let obj2 = arena.alloc_object(KObject::KFunction(f2, None));
    scope.register_function("BAR".to_string(), f1, obj1).unwrap();
    scope.register_function("BAR".to_string(), f2, obj2).unwrap();
}

#[test]
fn register_function_errors_on_function_value_collision() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let v = arena.alloc_object(KObject::Number(1.0));
    scope.bind_value("FOO".to_string(), v).unwrap();
    let f = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
    let obj = arena.alloc_object(KObject::KFunction(f, None));
    let err = scope.register_function("FOO".to_string(), f, obj).unwrap_err();
    assert!(
        matches!(&err.kind, crate::runtime::machine::core::KErrorKind::Rebind { name } if name == "FOO"),
        "expected Rebind on function/value collision, got {err}",
    );
}

#[test]
fn resolve_returns_placeholder_when_only_placeholder_exists() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    scope.install_placeholder("x".to_string(), NodeId(7)).unwrap();
    match scope.resolve("x") {
        Resolution::Placeholder(id) => assert_eq!(id, NodeId(7)),
        _ => panic!("expected Placeholder"),
    }
}

#[test]
fn resolve_stops_at_first_hit_does_not_descend_outer() {
    let arena = RuntimeArena::new();
    let outer = run_root_bare(&arena);
    let v = arena.alloc_object(KObject::Number(1.0));
    outer.bind_value("x".to_string(), v).unwrap();
    let inner = arena.alloc_scope(outer.child_for_call());
    inner.install_placeholder("x".to_string(), NodeId(3)).unwrap();
    match inner.resolve("x") {
        Resolution::Placeholder(id) => assert_eq!(id, NodeId(3)),
        other => panic!(
            "expected Placeholder from inner — outer's Value should not shadow it. Got {}",
            match other {
                Resolution::Value(_) => "Value",
                Resolution::Placeholder(_) => "Placeholder",
                Resolution::Unbound => "Unbound",
            }
        ),
    }
}

#[test]
fn bind_value_clears_own_placeholder() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    scope.install_placeholder("x".to_string(), NodeId(2)).unwrap();
    let v = arena.alloc_object(KObject::Number(42.0));
    scope.bind_value("x".to_string(), v).unwrap();
    assert!(scope.bindings().placeholders().get("x").is_none());
    assert!(matches!(scope.resolve("x"), Resolution::Value(KObject::Number(n)) if *n == 42.0));
}

// -------- resolve_dispatch smoke tests --------

use super::ResolveOutcome;
use crate::ast::{ExpressionPart, KExpression, KLiteral};
use crate::runtime::builtins::register_builtin;
use crate::runtime::builtins::test_support::{marker, one_slot_sig};
use crate::runtime::machine::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle};

fn body_a<'a>(s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker(s, "a")) }
fn body_b<'a>(s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker(s, "b")) }

fn two_slot_sig<'a>(a: KType, b: KType) -> ExpressionSignature<'a> {
    ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![
            SignatureElement::Argument(Argument { name: "a".into(), ktype: a }),
            SignatureElement::Keyword("OP".into()),
            SignatureElement::Argument(Argument { name: "b".into(), ktype: b }),
        ],
    }
}

/// Successful pick on an overload registered in the current scope: the carried
/// `Resolved` exposes the classifier's per-slot indices (here, an Identifier in an
/// `Any` slot lands in `wrap_indices`).
#[test]
fn resolve_returns_resolved_with_classified_indices_for_known_overload() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    register_builtin(scope, "ONE", one_slot_sig("v", KType::Any), body_a);
    let expr = KExpression { parts: vec![ExpressionPart::Identifier("foo".into())] };
    match scope.resolve_dispatch(&expr) {
        ResolveOutcome::Resolved(r) => {
            assert_eq!(r.slots.wrap_indices, vec![0]);
            assert!(r.slots.ref_name_indices.is_empty());
            assert!(!r.slots.picked_has_pre_run);
        }
        _ => panic!("expected Resolved for known overload"),
    }
}

/// Tied strict overloads (`<Number> OP <Any>` vs `<Any> OP <Number>` against `5 OP 7`)
/// surface as `Ambiguous(2)` at the scope where the tie occurs.
#[test]
fn resolve_returns_ambiguous_for_tied_overloads() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    register_builtin(scope, "NA", two_slot_sig(KType::Number, KType::Any), body_a);
    register_builtin(scope, "AN", two_slot_sig(KType::Any, KType::Number), body_b);
    let expr = KExpression {
        parts: vec![
            ExpressionPart::Literal(KLiteral::Number(5.0)),
            ExpressionPart::Keyword("OP".into()),
            ExpressionPart::Literal(KLiteral::Number(7.0)),
        ],
    };
    match scope.resolve_dispatch(&expr) {
        ResolveOutcome::Ambiguous(n) => assert_eq!(n, 2),
        _ => panic!("expected Ambiguous(2) for tied overloads"),
    }
}

/// Inner scope has no matching overload; resolution descends to `outer` and picks
/// there.
#[test]
fn resolve_walks_outer_chain_on_unmatched() {
    let arena = RuntimeArena::new();
    let outer = run_root_bare(&arena);
    register_builtin(outer, "ONE", one_slot_sig("v", KType::Any), body_a);
    let inner = arena.alloc_scope(outer.child_for_call());
    let expr = KExpression { parts: vec![ExpressionPart::Identifier("foo".into())] };
    assert!(matches!(inner.resolve_dispatch(&expr), ResolveOutcome::Resolved(_)));
}

/// Inner ambiguity does NOT fall through to `outer`: the outer scope has a
/// non-ambiguous overload, but resolution still reports Ambiguous from the inner tie.
#[test]
fn resolve_does_not_descend_outer_on_inner_ambiguity() {
    let arena = RuntimeArena::new();
    let outer = run_root_bare(&arena);
    register_builtin(outer, "OUTER", two_slot_sig(KType::Number, KType::Number), body_a);
    let inner = arena.alloc_scope(outer.child_for_call());
    register_builtin(inner, "NA", two_slot_sig(KType::Number, KType::Any), body_a);
    register_builtin(inner, "AN", two_slot_sig(KType::Any, KType::Number), body_b);
    let expr = KExpression {
        parts: vec![
            ExpressionPart::Literal(KLiteral::Number(5.0)),
            ExpressionPart::Keyword("OP".into()),
            ExpressionPart::Literal(KLiteral::Number(7.0)),
        ],
    };
    match inner.resolve_dispatch(&expr) {
        ResolveOutcome::Ambiguous(_) => {}
        _ => panic!("inner ambiguity must surface, not fall through to outer's unique overload"),
    }
}

/// A pre_run-bearing overload (here a synthetic LET-like binder) populates
/// `placeholder_name` from its extractor.
#[test]
fn resolve_carries_placeholder_name_for_pre_run_function() {
    use crate::runtime::builtins::register_builtin_with_pre_run;
    fn name_extractor(expr: &KExpression<'_>) -> Option<String> {
        // Mirror LET's shape: expression's 2nd part is the binder Identifier.
        match expr.parts.get(1) {
            Some(ExpressionPart::Identifier(n)) => Some(n.clone()),
            _ => None,
        }
    }
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let sig = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![
            SignatureElement::Keyword("LETLIKE".into()),
            SignatureElement::Argument(Argument { name: "n".into(), ktype: KType::Identifier }),
            SignatureElement::Keyword("=".into()),
            SignatureElement::Argument(Argument { name: "v".into(), ktype: KType::Any }),
        ],
    };
    register_builtin_with_pre_run(scope, "LETLIKE", sig, body_a, Some(name_extractor));
    let expr = KExpression {
        parts: vec![
            ExpressionPart::Keyword("LETLIKE".into()),
            ExpressionPart::Identifier("foo".into()),
            ExpressionPart::Keyword("=".into()),
            ExpressionPart::Literal(KLiteral::Number(1.0)),
        ],
    };
    match scope.resolve_dispatch(&expr) {
        ResolveOutcome::Resolved(r) => {
            assert_eq!(r.placeholder_name.as_deref(), Some("foo"));
            assert!(r.slots.picked_has_pre_run);
        }
        _ => panic!("expected Resolved with placeholder_name"),
    }
}

/// The tentative pass only fires when strict picked nothing at the same scope.
/// Register only a `<Identifier>` overload; calling with a `Number` literal must miss
/// strictly *and* tentatively (Literal is not a bare name), giving Unmatched at
/// run-root.
#[test]
fn resolve_tentative_falls_back_only_when_strict_empty() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    register_builtin(scope, "ONE_ID", one_slot_sig("v", KType::Identifier), body_a);
    let expr = KExpression { parts: vec![ExpressionPart::Literal(KLiteral::Number(5.0))] };
    assert!(matches!(scope.resolve_dispatch(&expr), ResolveOutcome::Unmatched));
}

/// A nested-Expression shape `((deep_call) + 1)` returns `Deferred`: the typed `+`
/// overload doesn't strictly match (Expression in Number slot) and doesn't tentatively
/// match either (Expression isn't a bare name), but eager evaluation of `(deep_call)`
/// may produce a `Future(Number)` that the post-Bind re-dispatch picks. Distinct from
/// `Unmatched` — the scheduler falls through to its eager-sub loop on `Deferred`.
#[test]
fn resolve_returns_deferred_for_nested_expression_in_typed_slot() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    register_builtin(scope, "PLUS", two_slot_sig(KType::Number, KType::Number), body_a);
    let inner = KExpression {
        parts: vec![ExpressionPart::Identifier("deep_call".into())],
    };
    let expr = KExpression {
        parts: vec![
            ExpressionPart::Expression(Box::new(inner)),
            ExpressionPart::Keyword("OP".into()),
            ExpressionPart::Literal(KLiteral::Number(1.0)),
        ],
    };
    assert!(matches!(scope.resolve_dispatch(&expr), ResolveOutcome::Deferred));
}

// -------- unit-level dispatch tests on `resolve_dispatch` --------
//
// Cover overload-resolution behaviors at the `resolve_dispatch` boundary. The
// end-to-end variants that drive `Scheduler::execute` live with the rest of the
// scheduler integration tests at `execute::scheduler::tests`.

use crate::runtime::builtins::default_scope;

fn body_number_any<'a>(s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker(s, "number_any")) }
fn body_any_number<'a>(s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker(s, "any_number")) }

/// Parent owns the LET builtin; child has no functions of its own. `resolve_dispatch`
/// against the child must climb to the parent.
#[test]
fn resolve_walks_outer_chain_to_find_builtin() {
    let arena = RuntimeArena::new();
    let outer = default_scope(&arena, Box::new(std::io::sink()));
    let inner = arena.alloc_scope(outer.child_for_call());

    let expr = KExpression {
        parts: vec![
            ExpressionPart::Keyword("LET".into()),
            ExpressionPart::Identifier("x".into()),
            ExpressionPart::Keyword("=".into()),
            ExpressionPart::Literal(KLiteral::Number(1.0)),
        ],
    };

    assert!(
        matches!(inner.resolve_dispatch(&expr), ResolveOutcome::Resolved(_)),
        "child scope should inherit LET from outer",
    );
}

/// No overload anywhere on the chain, and no nested eager parts → `Unmatched`.
#[test]
fn resolve_dispatch_with_no_outer_and_no_match_is_unmatched() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let expr = KExpression {
        parts: vec![ExpressionPart::Identifier("nope".into())],
    };
    assert!(matches!(scope.resolve_dispatch(&expr), ResolveOutcome::Unmatched));
}

/// `<Number> OP <Any>` vs `<Any> OP <Number>` against `5 OP 7` are incomparable: each is
/// more specific in one slot and less in the other. `resolve_dispatch` reports
/// `Ambiguous`; the integration path surfaces the same error via Scheduler::execute.
#[test]
fn dispatch_errors_on_ambiguous_overlap() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    register_builtin(scope, "number_any", two_slot_sig(KType::Number, KType::Any), body_number_any);
    register_builtin(scope, "any_number", two_slot_sig(KType::Any, KType::Number), body_any_number);

    let expr = KExpression {
        parts: vec![
            ExpressionPart::Literal(KLiteral::Number(5.0)),
            ExpressionPart::Keyword("OP".into()),
            ExpressionPart::Literal(KLiteral::Number(7.0)),
        ],
    };
    assert!(
        matches!(scope.resolve_dispatch(&expr), ResolveOutcome::Ambiguous(_)),
        "equally-specific overloads should produce an Ambiguous outcome",
    );
}

/// Ambiguous shape (two equally-specific overloads matching) surfaces as
/// `ResolveOutcome::Ambiguous` — the wrap pass mustn't speculatively transform an
/// ambiguous expression. Semantics sharpen vs. today's `shape_pick → None`: that arm
/// collapsed ambiguity and no-match into one variant; the new surface separates them.
#[test]
fn resolve_returns_ambiguous_for_overlap_that_shape_pick_returned_none_for() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    register_builtin(scope, "OP_NA", two_slot_sig(KType::Number, KType::Any), body_number_any);
    register_builtin(scope, "OP_AN", two_slot_sig(KType::Any, KType::Number), body_any_number);
    let expr = KExpression {
        parts: vec![
            ExpressionPart::Literal(KLiteral::Number(5.0)),
            ExpressionPart::Keyword("OP".into()),
            ExpressionPart::Literal(KLiteral::Number(7.0)),
        ],
    };
    assert!(
        matches!(scope.resolve_dispatch(&expr), ResolveOutcome::Ambiguous(_)),
        "ambiguous overlap → Ambiguous",
    );
}

// -------- `register_type` rewire + `resolve_type` tests --------
//
// Pin the three load-bearing properties of the rewire:
// - storage flip: `register_type` writes `types`, not `data`;
// - `resolve_type` outer-chain walk;
// - inner-scope shadowing of outer type bindings.
//
// Stage 1.5 deleted the transient `Scope::resolve` fallback that previously
// synthesized a `KObject::KTypeValue` from the same `types` map at `lookup`
// time — the corresponding test for that fallback (deleted with it) is no
// longer part of this slate.

#[test]
fn register_type_inserts_into_types_map_not_data() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    scope.register_type("Foo".into(), KType::Number);
    assert!(scope.bindings().types().get("Foo").is_some());
    assert!(
        scope.bindings().data().get("Foo").is_none(),
        "post-1.4: type binding must not appear in data map",
    );
}

#[test]
fn resolve_type_walks_outer_chain_and_returns_none_past_root() {
    let arena = RuntimeArena::new();
    let root = run_root_bare(&arena);
    root.register_type("Foo".into(), KType::Number);
    let child = arena.alloc_scope(Scope::child_under(root));
    assert!(matches!(child.resolve_type("Foo"), Some(KType::Number)));
    assert!(
        child.resolve_type("Nope").is_none(),
        "unbound name past run-root yields None, not panic",
    );
}

#[test]
fn resolve_type_inner_scope_shadows_outer() {
    let arena = RuntimeArena::new();
    let root = run_root_bare(&arena);
    root.register_type("Foo".into(), KType::Number);
    let child = arena.alloc_scope(Scope::child_under(root));
    child.register_type("Foo".into(), KType::Str);
    assert!(matches!(child.resolve_type("Foo"), Some(KType::Str)));
    assert!(matches!(root.resolve_type("Foo"), Some(KType::Number)));
}
