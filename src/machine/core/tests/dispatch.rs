//! `dispatch` arm of `machine::core` tests.

use super::super::{FrameStorage, Scope};
use crate::builtins::test_support::{marker, one_slot_sig, run_root_bare};
use crate::builtins::{register_builtin, register_overload_at};
use crate::machine::core::kfunction::action::{Action, BodyCtx};
use crate::machine::model::ast::{ExpressionPart, KExpression, KLiteral};
use crate::machine::model::types::{
    Argument, ExpressionSignature, KType, ReturnType, SignatureElement,
};
use crate::machine::model::Carried;
use crate::machine::{BindingIndex, LexicalFrame, ResolveOutcome};
use crate::source::Spanned;

fn body_a<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
    Action::Done(Ok(Carried::Object(marker(ctx.scope, "a"))))
}
fn body_b<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
    Action::Done(Ok(Carried::Object(marker(ctx.scope, "b"))))
}

fn two_slot_sig<'a>(a: KType<'a>, b: KType<'a>) -> ExpressionSignature<'a> {
    ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![
            SignatureElement::Argument(Argument {
                name: "a".into(),
                ktype: a,
            }),
            SignatureElement::Keyword("OP".into()),
            SignatureElement::Argument(Argument {
                name: "b".into(),
                ktype: b,
            }),
        ],
    }
}

/// An Identifier in an `Any` slot lands in `wrap_indices`.
#[test]
fn resolve_returns_resolved_with_classified_indices_for_known_overload() {
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    register_builtin(scope, "ONE", one_slot_sig("v", KType::Any), body_a);
    let expr = KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier(
        "foo".into(),
    ))]);
    let chain = LexicalFrame::detached();
    match scope.resolve_dispatch(&expr, Some(&chain), &[]) {
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
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    register_builtin(scope, "NA", two_slot_sig(KType::Number, KType::Any), body_a);
    register_builtin(scope, "AN", two_slot_sig(KType::Any, KType::Number), body_b);
    let expr = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(5.0))),
        Spanned::bare(ExpressionPart::Keyword("OP".into())),
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(7.0))),
    ]);
    let chain = LexicalFrame::detached();
    match scope.resolve_dispatch(&expr, Some(&chain), &[]) {
        ResolveOutcome::Ambiguous(n) => assert_eq!(n, 2),
        _ => panic!("expected Ambiguous(2) for tied overloads"),
    }
}

/// Inner ambiguity must surface even when `outer` has a non-ambiguous overload —
/// resolution does not fall through past a tie.
#[test]
fn resolve_does_not_descend_outer_on_inner_ambiguity() {
    let region = FrameStorage::run_root();
    let outer = run_root_bare(&region);
    // User-position (not BUILTIN) so the builtin root-first short-circuit doesn't fire —
    // this exercises the inner-ambiguity-doesn't-descend walk, not builtin authority.
    register_overload_at(
        outer,
        "OUTER",
        two_slot_sig(KType::Number, KType::Number),
        body_a,
        BindingIndex::value(1),
    );
    let inner = region.brand().alloc_scope(outer.child_for_call());
    register_builtin(inner, "NA", two_slot_sig(KType::Number, KType::Any), body_a);
    register_builtin(inner, "AN", two_slot_sig(KType::Any, KType::Number), body_b);
    let expr = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(5.0))),
        Spanned::bare(ExpressionPart::Keyword("OP".into())),
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(7.0))),
    ]);
    let chain = LexicalFrame::detached();
    match inner.resolve_dispatch(&expr, Some(&chain), &[]) {
        ResolveOutcome::Ambiguous(_) => {}
        _ => panic!("inner ambiguity must surface, not fall through to outer's unique overload"),
    }
}

/// A binder_name-bearing overload populates `placeholder_name` from its extractor.
#[test]
fn resolve_carries_placeholder_name_for_binder_function() {
    use crate::builtins::register_builtin_full;
    fn name_extractor(expr: &KExpression<'_>) -> Option<String> {
        match expr.parts.get(1).map(|p| &p.value) {
            Some(ExpressionPart::Identifier(n)) => Some(n.clone()),
            _ => None,
        }
    }
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    let sig = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![
            SignatureElement::Keyword("LETLIKE".into()),
            SignatureElement::Argument(Argument {
                name: "n".into(),
                ktype: KType::Identifier,
            }),
            SignatureElement::Keyword("=".into()),
            SignatureElement::Argument(Argument {
                name: "v".into(),
                ktype: KType::Any,
            }),
        ],
    };
    register_builtin_full(
        scope,
        "LETLIKE",
        sig,
        body_a,
        Some(name_extractor),
        None,
        false,
    );
    let expr = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Keyword("LETLIKE".into())),
        Spanned::bare(ExpressionPart::Identifier("foo".into())),
        Spanned::bare(ExpressionPart::Keyword("=".into())),
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(1.0))),
    ]);
    let chain = LexicalFrame::detached();
    match scope.resolve_dispatch(&expr, Some(&chain), &[]) {
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
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    register_builtin(
        scope,
        "ONE_ID",
        one_slot_sig("v", KType::Identifier),
        body_a,
    );
    let expr = KExpression::new(vec![Spanned::bare(ExpressionPart::Literal(
        KLiteral::Number(5.0),
    ))]);
    let chain = LexicalFrame::detached();
    assert!(matches!(
        scope.resolve_dispatch(&expr, Some(&chain), &[]),
        ResolveOutcome::Unmatched
    ));
}

/// `((deep_call) + 1)` returns `Deferred` rather than `Unmatched`: the typed
/// overload can't match the nested `Expression` strictly or tentatively, but
/// eager evaluation of `(deep_call)` may produce a `Spliced(Number)` that a
/// post-Bind re-dispatch picks. The scheduler routes `Deferred` into its
/// eager-sub loop instead of erroring.
#[test]
fn resolve_returns_deferred_for_nested_expression_in_typed_slot() {
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    register_builtin(
        scope,
        "PLUS",
        two_slot_sig(KType::Number, KType::Number),
        body_a,
    );
    let inner = KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier(
        "deep_call".into(),
    ))]);
    let expr = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Expression(Box::new(inner))),
        Spanned::bare(ExpressionPart::Keyword("OP".into())),
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(1.0))),
    ]);
    let chain = LexicalFrame::detached();
    assert!(matches!(
        scope.resolve_dispatch(&expr, Some(&chain), &[]),
        ResolveOutcome::Deferred
    ));
}

/// `pending_overloads` is keyed by the *full* bucket. An entry for `(MAKESET _)`
/// parks `(MAKESET <bare>)` but must not park `(MAKESET <bare> USING <bare>)` —
/// sharing a lead keyword is not enough to collide.
#[test]
fn pending_overload_parks_only_on_exact_bucket_match() {
    use crate::machine::model::types::{UntypedElement, UntypedKey};
    use crate::machine::NodeId;
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    let bucket_single: UntypedKey = vec![
        UntypedElement::Keyword("MAKESET".into()),
        UntypedElement::Slot,
    ];
    scope
        .install_pending_overload(bucket_single, NodeId(42), BindingIndex::BUILTIN)
        .expect("install_pending_overload");

    let bare = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Keyword("MAKESET".into())),
        Spanned::bare(ExpressionPart::Identifier("fwd".into())),
    ]);
    let chain = LexicalFrame::detached();
    match scope.resolve_dispatch(&bare, Some(&chain), &[]) {
        ResolveOutcome::ParkOnProducers(ps) => assert_eq!(ps, vec![NodeId(42)]),
        other => panic!(
            "expected ParkOnProducers([42]) for matching bucket, got {}",
            std::any::type_name_of_val(&other)
        ),
    }

    let multi = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Keyword("MAKESET".into())),
        Spanned::bare(ExpressionPart::Identifier("fwd".into())),
        Spanned::bare(ExpressionPart::Keyword("USING".into())),
        Spanned::bare(ExpressionPart::Identifier("other".into())),
    ]);
    assert!(
        matches!(
            scope.resolve_dispatch(&multi, Some(&chain), &[]),
            ResolveOutcome::Unmatched
        ),
        "different-bucket call must not park on a lead-keyword sibling",
    );
}

/// An inner-scope pending overload shadows an outer-scope strict Pick: the
/// pending sibling would shadow the outer match once it finalizes, so the inner
/// scope parks rather than letting the outer Pick win on finalize order.
#[test]
fn inner_scope_pending_overload_shadows_outer_strict_pick() {
    use crate::machine::NodeId;
    let region = FrameStorage::run_root();
    let outer = run_root_bare(&region);
    // Outer finalized overload that strictly Picks `(MARK <number>)`.
    let outer_sig = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![
            SignatureElement::Keyword("MARK".into()),
            SignatureElement::Argument(Argument {
                name: "v".into(),
                ktype: KType::Number,
            }),
        ],
    };
    // User-position so the builtin root-first short-circuit doesn't claim it; the inner
    // pending sibling must shadow this outer strict Pick on the ordinary walk.
    register_overload_at(
        outer,
        "outer_mark",
        outer_sig,
        body_a,
        BindingIndex::value(1),
    );

    let inner = region.brand().alloc_scope(outer.child_for_call());
    let expr = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Keyword("MARK".into())),
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(7.0))),
    ]);
    // Inner pending sibling on the same bucket key, body not yet finalized.
    scope_install_pending(inner, &expr, NodeId(55));

    let chain = LexicalFrame::detached();
    match inner.resolve_dispatch(&expr, Some(&chain), &[]) {
        ResolveOutcome::ParkOnProducers(ps) => assert_eq!(
            ps,
            vec![NodeId(55)],
            "inner pending must shadow the outer strict Pick",
        ),
        other => panic!(
            "expected ParkOnProducers([55]), got {}",
            std::any::type_name_of_val(&other),
        ),
    }
}

/// An inner-scope candidate that is strict-Empty but admits once its eager part
/// evaluates (`:Number` slot against a nested `Expression`) shadows an outer
/// strict Pick: the inner scope `Deferred`s rather than letting the outer win.
#[test]
fn inner_scope_eager_lean_shadows_outer_strict_pick() {
    let region = FrameStorage::run_root();
    let outer = run_root_bare(&region);
    // Outer overload that would strictly Pick once the eager sub resolves.
    register_builtin(
        outer,
        "outer_plus",
        two_slot_sig(KType::Number, KType::Number),
        body_a,
    );
    let inner = region.brand().alloc_scope(outer.child_for_call());
    register_builtin(
        inner,
        "inner_plus",
        two_slot_sig(KType::Number, KType::Number),
        body_b,
    );
    let nested = KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier(
        "deep_call".into(),
    ))]);
    let expr = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Expression(Box::new(nested))),
        Spanned::bare(ExpressionPart::Keyword("OP".into())),
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(1.0))),
    ]);
    let chain = LexicalFrame::detached();
    assert!(
        matches!(
            inner.resolve_dispatch(&expr, Some(&chain), &[]),
            ResolveOutcome::Deferred
        ),
        "inner eager-lean must Defer at its scope, not fall through to outer",
    );
}

/// A dead (unbound) bare-name lean at an inner scope must NOT pre-empt an outer
/// `:Identifier` strict Pick: the inner `:Number` overload rejects the bare name
/// (dead lean → continue), and the outer `:Identifier` slot Picks it shape-only.
#[test]
fn dead_bare_name_lean_does_not_preempt_outer_identifier_pick() {
    use crate::machine::NameOutcome;
    let region = FrameStorage::run_root();
    let outer = run_root_bare(&region);
    // Outer `:Identifier` overload that owns the bare name (shape-only admit).
    register_builtin(
        outer,
        "outer_id",
        one_slot_sig("v", KType::Identifier),
        body_a,
    );
    let inner = region.brand().alloc_scope(outer.child_for_call());
    // Inner `:Number` overload: the unbound bare name rejects its shape, so the
    // inner scope's only contribution is a dead lean (must not terminate).
    register_builtin(inner, "inner_num", one_slot_sig("v", KType::Number), body_b);
    let expr = KExpression::new(vec![Spanned::bare(ExpressionPart::Identifier(
        "fwd".into(),
    ))]);
    let bare_outcomes = vec![Some(NameOutcome::Unbound("fwd".into()))];
    let chain = LexicalFrame::detached();
    match inner.resolve_dispatch(&expr, Some(&chain), &bare_outcomes) {
        ResolveOutcome::Resolved(r) => assert!(
            matches!(
                r.function.signature.elements.first(),
                Some(SignatureElement::Argument(arg)) if arg.ktype == KType::Identifier
            ),
            "outer `:Identifier` overload must Pick the bare name shape-only",
        ),
        other => panic!(
            "dead inner lean must not pre-empt the outer `:Identifier` Pick; got {}",
            std::any::type_name_of_val(&other),
        ),
    }
}

/// A bucket holding a finalized overload that strictly Picks AND an in-flight
/// pending sibling parks until the sibling finalizes — pending park takes
/// precedence even over a same-scope finalized strict Pick (Decision 5). Once
/// the pending entry is removed at finalize, the bucket resolves.
#[test]
fn finalized_pick_with_pending_sibling_parks_until_finalize() {
    use crate::machine::core::kfunction::{Body, KFunction};
    use crate::machine::model::values::KObject;
    use crate::machine::NodeId;
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    // Finalized `(PICK <number>)` user overload that strictly Picks. Registered at a
    // user index (not BUILTIN) so the same-bucket sibling below is a legitimate
    // user-vs-user overload — a builtin bucket admits no user siblings.
    let pick_num = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![
            SignatureElement::Keyword("PICK".into()),
            SignatureElement::Argument(Argument {
                name: "v".into(),
                ktype: KType::Number,
            }),
        ],
    };
    let pick_num_fn =
        region
            .brand()
            .alloc_function(KFunction::new(pick_num, Body::Builtin(body_a), scope));
    let pick_num_obj = region
        .brand()
        .alloc_object(KObject::KFunction(pick_num_fn));
    scope
        .register_function(
            "pick_num".to_string(),
            pick_num_fn,
            pick_num_obj,
            BindingIndex::value(1),
        )
        .expect("register pick_num overload");
    let expr = KExpression::new(vec![
        Spanned::bare(ExpressionPart::Keyword("PICK".into())),
        Spanned::bare(ExpressionPart::Literal(KLiteral::Number(7.0))),
    ]);
    // In-flight pending sibling on the same bucket key, finalizing at index 3.
    scope
        .install_pending_overload(expr.untyped_key(), NodeId(77), BindingIndex::value(3))
        .expect("install_pending_overload");

    let chain = LexicalFrame::detached();
    match scope.resolve_dispatch(&expr, Some(&chain), &[]) {
        ResolveOutcome::ParkOnProducers(ps) => assert_eq!(
            ps,
            vec![NodeId(77)],
            "finalized Pick must park on the in-flight pending sibling",
        ),
        other => panic!(
            "expected ParkOnProducers([77]) while pending sibling is in flight; got {}",
            std::any::type_name_of_val(&other),
        ),
    }

    // Finalize the pending sibling: registering a same-bucket overload at the
    // pending's index removes its `pending_overloads` entry (mirrors the real
    // finalize-clear path, which retains-by-`BindingIndex`).
    let pick_str = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![
            SignatureElement::Keyword("PICK".into()),
            SignatureElement::Argument(Argument {
                name: "v".into(),
                ktype: KType::Str,
            }),
        ],
    };
    let sibling = region.brand().alloc_function(KFunction::new(
        pick_str,
        Body::Builtin(super::body_no_op),
        scope,
    ));
    let sibling_obj = region.brand().alloc_object(KObject::KFunction(sibling));
    scope
        .register_function(
            "pick_str".to_string(),
            sibling,
            sibling_obj,
            BindingIndex::value(3),
        )
        .expect("register sibling overload");

    match scope.resolve_dispatch(&expr, Some(&chain), &[]) {
        ResolveOutcome::Resolved(_) => {}
        other => panic!(
            "bucket must resolve once the pending sibling finalizes; got {}",
            std::any::type_name_of_val(&other),
        ),
    }
}

/// Install a pending overload keyed by `expr`'s bucket key onto `scope`.
fn scope_install_pending<'a>(
    scope: &'a Scope<'a>,
    expr: &KExpression<'a>,
    producer: crate::machine::NodeId,
) {
    scope
        .install_pending_overload(expr.untyped_key(), producer, BindingIndex::BUILTIN)
        .expect("install_pending_overload");
}

/// Two sibling binders that share a bucket key each install their own
/// `pending_overloads[bucket]` entry — coalescing or rejecting the second would
/// drop a distinct wake source. A consumer parks on the earliest-index visible
/// entry.
#[test]
fn sibling_pending_overloads_park_on_earliest_visible_entry() {
    use crate::machine::model::types::{UntypedElement, UntypedKey};
    use crate::machine::NodeId;
    let region = FrameStorage::run_root();
    let scope = run_root_bare(&region);
    let bucket: UntypedKey = vec![UntypedElement::Keyword("PICK".into()), UntypedElement::Slot];
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
    let chain = LexicalFrame::detached();
    match scope.resolve_dispatch(&expr, Some(&chain), &[]) {
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
