//! `dispatch` arm of `machine::core` tests.

use crate::machine::BindingIndex;
use super::super::{RuntimeArena, Scope};
use crate::builtins::test_support::run_root_bare;
use crate::machine::model::types::{Argument, ExpressionSignature, KType, SignatureElement, ReturnType};
use crate::machine::ResolveOutcome;
use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral};
use crate::builtins::register_builtin;
use crate::builtins::test_support::{marker, one_slot_sig};
use crate::machine::core::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle};
use crate::builtins::default_scope;
use crate::machine::execute::Scheduler;


fn body_a<'a>(s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker(s, "a")) }
fn body_b<'a>(s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker(s, "b")) }

fn two_slot_sig<'a>(a: KType<'a>, b: KType<'a>) -> ExpressionSignature<'a> {
    ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![
            SignatureElement::Argument(Argument { name: "a".into(), ktype: a }),
            SignatureElement::Keyword("OP".into()),
            SignatureElement::Argument(Argument { name: "b".into(), ktype: b }),
        ],
    }
}

fn body_number_any<'a>(s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker(s, "number_any")) }
fn body_any_number<'a>(s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker(s, "any_number")) }

/// An Identifier in an `Any` slot lands in `wrap_indices`.
#[test]
fn resolve_returns_resolved_with_classified_indices_for_known_overload() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    register_builtin(scope, "ONE", one_slot_sig("v", KType::Any), body_a);
    let expr = KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier("foo".into()))]);
    match scope.resolve_dispatch(&expr) {
        ResolveOutcome::Resolved(r) => {
            assert_eq!(r.slots.wrap_indices, vec![0]);
            assert!(r.slots.ref_name_indices.is_empty());
            assert!(!r.slots.picked_has_binder_name);
        }
        _ => panic!("expected Resolved for known overload"),
    }
}

#[test]
fn resolve_returns_ambiguous_for_tied_overloads() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    register_builtin(scope, "NA", two_slot_sig(KType::Number, KType::Any), body_a);
    register_builtin(scope, "AN", two_slot_sig(KType::Any, KType::Number), body_b);
    let expr = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(5.0))),
        Spanned::bare(ExpressionPart::Keyword("OP".into())),
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(7.0))),
    ]);
    match scope.resolve_dispatch(&expr) {
        ResolveOutcome::Ambiguous(n) => assert_eq!(n, 2),
        _ => panic!("expected Ambiguous(2) for tied overloads"),
    }
}

#[test]
fn resolve_walks_outer_chain_on_unmatched() {
    let arena = RuntimeArena::new();
    let outer = run_root_bare(&arena);
    register_builtin(outer, "ONE", one_slot_sig("v", KType::Any), body_a);
    let inner = arena.alloc_scope(outer.child_for_call());
    let expr = KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier("foo".into()))]);
    assert!(matches!(inner.resolve_dispatch(&expr), ResolveOutcome::Resolved(_)));
}

/// Inner ambiguity must surface even when `outer` has a non-ambiguous overload —
/// resolution does not fall through past a tie.
#[test]
fn resolve_does_not_descend_outer_on_inner_ambiguity() {
    let arena = RuntimeArena::new();
    let outer = run_root_bare(&arena);
    register_builtin(outer, "OUTER", two_slot_sig(KType::Number, KType::Number), body_a);
    let inner = arena.alloc_scope(outer.child_for_call());
    register_builtin(inner, "NA", two_slot_sig(KType::Number, KType::Any), body_a);
    register_builtin(inner, "AN", two_slot_sig(KType::Any, KType::Number), body_b);
    let expr = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(5.0))),
        Spanned::bare(ExpressionPart::Keyword("OP".into())),
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(7.0))),
    ]);
    match inner.resolve_dispatch(&expr) {
        ResolveOutcome::Ambiguous(_) => {}
        _ => panic!("inner ambiguity must surface, not fall through to outer's unique overload"),
    }
}

/// A binder_name-bearing overload populates `placeholder_name` from its extractor.
#[test]
fn resolve_carries_placeholder_name_for_binder_function() {
    use crate::builtins::register_builtin_with_binder;
    fn name_extractor(expr: &KExpression<'_>) -> Option<String> {
        match expr.parts.get(1).map(|p| &p.value) {
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
    register_builtin_with_binder(scope, "LETLIKE", sig, body_a, Some(name_extractor));
    let expr = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Keyword("LETLIKE".into())),
        Spanned::bare(ExpressionPart::Identifier("foo".into())),
        Spanned::bare(ExpressionPart::Keyword("=".into())),
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(1.0))),
    ]);
    match scope.resolve_dispatch(&expr) {
        ResolveOutcome::Resolved(r) => {
            assert_eq!(r.placeholder_name.as_deref(), Some("foo"));
            assert!(r.slots.picked_has_binder_name);
        }
        _ => panic!("expected Resolved with placeholder_name"),
    }
}

/// A `Number` literal against an `<Identifier>`-only overload misses strictly
/// *and* tentatively (a Literal is not a bare name).
#[test]
fn resolve_tentative_falls_back_only_when_strict_empty() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    register_builtin(scope, "ONE_ID", one_slot_sig("v", KType::Identifier), body_a);
    let expr = KExpression::new(vec![Spanned::bare(ExpressionPart::Literal(KLiteral::Number(5.0)))]);
    assert!(matches!(scope.resolve_dispatch(&expr), ResolveOutcome::Unmatched));
}

/// `((deep_call) + 1)` returns `Deferred` rather than `Unmatched`: the typed
/// overload can't match the nested `Expression` strictly or tentatively, but
/// eager evaluation of `(deep_call)` may produce a `Future(Number)` that a
/// post-Bind re-dispatch picks. The scheduler routes `Deferred` into its
/// eager-sub loop instead of erroring.
#[test]
fn resolve_returns_deferred_for_nested_expression_in_typed_slot() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    register_builtin(scope, "PLUS", two_slot_sig(KType::Number, KType::Number), body_a);
    let inner = KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier("deep_call".into()))]);
    let expr = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Expression(Box::new(inner))),
        Spanned::bare(ExpressionPart::Keyword("OP".into())),
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(1.0))),
    ]);
    assert!(matches!(scope.resolve_dispatch(&expr), ResolveOutcome::Deferred));
}

/// Parent owns the LET builtin; child has no functions of its own —
/// `resolve_dispatch` against the child must climb to the parent.
#[test]
fn resolve_walks_outer_chain_to_find_builtin() {
    let arena = RuntimeArena::new();
    let outer = default_scope(&arena, Box::new(std::io::sink()));
    let inner = arena.alloc_scope(outer.child_for_call());

    let expr = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Keyword("LET".into())),
        Spanned::bare(ExpressionPart::Identifier("x".into())),
        Spanned::bare(ExpressionPart::Keyword("=".into())),
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(1.0))),
    ]);

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
    let expr = KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier("nope".into()))]);
    assert!(matches!(scope.resolve_dispatch(&expr), ResolveOutcome::Unmatched));
}

/// `pending_overloads` is keyed by the *full* bucket. An entry for `(MAKESET _)`
/// parks `(MAKESET <bare>)` but must not park `(MAKESET <bare> USING <bare>)` —
/// sharing a lead keyword is not enough to collide.
#[test]
fn pending_overload_parks_only_on_exact_bucket_match() {
    use crate::machine::model::types::{UntypedElement, UntypedKey};
    use crate::machine::NodeId;
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let bucket_single: UntypedKey =
        vec![UntypedElement::Keyword("MAKESET".into()), UntypedElement::Slot];
    scope
        .install_pending_overload(bucket_single, NodeId(42), BindingIndex::BUILTIN)
        .expect("install_pending_overload");

    let bare = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Keyword("MAKESET".into())),
        Spanned::bare(ExpressionPart::Identifier("fwd".into())),
    ]);
    match scope.resolve_dispatch(&bare) {
        ResolveOutcome::ParkOnProducers(ps) => assert_eq!(ps, vec![NodeId(42)]),
        other => panic!("expected ParkOnProducers([42]) for matching bucket, got {}",
            std::any::type_name_of_val(&other)),
    }

    let multi = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Keyword("MAKESET".into())),
        Spanned::bare(ExpressionPart::Identifier("fwd".into())),
        Spanned::bare(ExpressionPart::Keyword("USING".into())),
        Spanned::bare(ExpressionPart::Identifier("other".into())),
    ]);
    assert!(
        matches!(scope.resolve_dispatch(&multi), ResolveOutcome::Unmatched),
        "different-bucket call must not park on a lead-keyword sibling",
    );
}

/// Two sibling binders that share a bucket key each install their own
/// `pending_overloads[bucket]` entry — coalescing or rejecting the second would
/// drop a distinct wake source. A consumer parks on the earliest-index visible
/// entry.
#[test]
fn sibling_pending_overloads_park_on_earliest_visible_entry() {
    use crate::machine::model::types::{UntypedElement, UntypedKey};
    use crate::machine::NodeId;
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let bucket: UntypedKey =
        vec![UntypedElement::Keyword("PICK".into()), UntypedElement::Slot];
    scope
        .install_pending_overload(bucket.clone(), NodeId(101), BindingIndex::value(3))
        .expect("first install");
    scope
        .install_pending_overload(bucket.clone(), NodeId(102), BindingIndex::value(4))
        .expect("second install must not collide");
    let entries = scope.bindings().pending_overloads().get(&bucket).cloned();
    let entries = entries.expect("bucket should be populated");
    assert_eq!(
        entries.len(),
        2,
        "both sibling installs must coexist as distinct entries; got {:?}",
        entries,
    );

    let expr = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Keyword("PICK".into())),
        Spanned::bare(ExpressionPart::Identifier("fwd".into())),
    ]);
    match scope.resolve_dispatch(&expr) {
        ResolveOutcome::ParkOnProducers(ps) => {
            assert_eq!(
                ps,
                vec![NodeId(101)],
                "consumer must park on earliest-index visible pending entry",
            );
        }
        other => panic!(
            "expected ParkOnProducers([101]), got variant {}",
            std::any::type_name_of_val(&other),
        ),
    }
}

/// Pairs the unit `Ambiguous` outcome with the end-to-end `AmbiguousDispatch`
/// error from `Scheduler::execute`.
#[test]
fn dispatch_errors_on_ambiguous_overlap() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    register_builtin(scope, "number_any", two_slot_sig(KType::Number, KType::Any), body_number_any);
    register_builtin(scope, "any_number", two_slot_sig(KType::Any, KType::Number), body_any_number);

    let expr = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(5.0))),
        Spanned::bare(ExpressionPart::Keyword("OP".into())),
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(7.0))),
    ]);
    assert!(
        matches!(scope.resolve_dispatch(&expr), ResolveOutcome::Ambiguous(_)),
        "equally-specific overloads should produce an Ambiguous outcome",
    );

    let mut sched = Scheduler::new();
    sched.add_dispatch(expr, scope);
    let err = sched.execute().expect_err("ambiguous dispatch should error end-to-end");
    assert!(
        matches!(err.kind, crate::machine::core::KErrorKind::AmbiguousDispatch { .. }),
        "expected AmbiguousDispatch from Scheduler::execute, got {err}",
    );
}

