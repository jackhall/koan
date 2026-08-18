//! `dispatch` arm of `machine::core` tests.

use super::super::{FrameStorageExt, Scope, program_storage, run_root_storage};
use crate::builtins::test_support::{marker, one_slot_sig, run_root_bare};
use crate::builtins::{register_builtin, register_overload_at};
use crate::machine::core::RegionBrand;
use crate::machine::core::kfunction::action::{Action, BodyCtx};
use crate::machine::model::Carried;
use crate::machine::model::TypeRegistry;
use crate::machine::model::{Argument, KType, ReturnType, SignatureDraft, SignatureElement};
use crate::machine::model::{ExpressionPart, KLiteral, WorkingExpression, WorkingPart};
use crate::machine::{BindingIndex, DispatchOutcome, LexicalFrame};
use crate::source::Spanned;

/// Freeze a run of raw AST parts as the working node a dispatch entry receives — the shape
/// `WorkingExpression::from_ast` produces for a parsed statement, assembled part-by-part here.
fn working<'a>(brand: RegionBrand<'a>, parts: Vec<ExpressionPart<'a>>) -> WorkingExpression<'a> {
    WorkingExpression::new_from_iter(
        brand,
        parts
            .into_iter()
            .map(|part| Spanned::bare(WorkingPart::Ast(part))),
    )
}

fn body_a<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    Action::done_resident(ctx.scope, Carried::Object(marker(ctx.scope, "a")))
}
fn body_b<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    Action::done_resident(ctx.scope, Carried::Object(marker(ctx.scope, "b")))
}

fn two_slot_sig<'a>(a: KType, b: KType) -> SignatureDraft<'a> {
    SignatureDraft {
        return_type: ReturnType::Resolved(KType::ANY),
        elements: vec![
            SignatureElement::Argument(Argument {
                name: "a",
                ktype: a,
            }),
            SignatureElement::Keyword("OP"),
            SignatureElement::Argument(Argument {
                name: "b",
                ktype: b,
            }),
        ],
    }
}

/// An Identifier in an `Any` slot lands in `wrap_indices`.
#[test]
fn resolve_returns_resolved_with_classified_indices_for_known_overload() {
    let types = TypeRegistry::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    register_builtin(
        scope,
        "ONE",
        one_slot_sig("v", KType::ANY),
        body_a,
        &types,
        &mut crate::machine::WriteGate::for_test(),
    );
    let expr = working(region.brand(), vec![ExpressionPart::Identifier("foo")]);
    // ONE was registered at `scope`'s BUILTIN index (0); root the chain there one past it
    // so the registration is visible.
    let chain = LexicalFrame::root(scope.id, 1);
    match scope.resolve_dispatch(&expr, Some(&chain), &[], &types) {
        DispatchOutcome::Resolved(r) => {
            assert_eq!(r.slots.wrap_indices, vec![0]);
        }
        _ => panic!("expected Resolved for known overload"),
    }
}

#[test]
fn resolve_returns_ambiguous_for_tied_overloads() {
    let types = TypeRegistry::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    register_builtin(
        scope,
        "NA",
        two_slot_sig(KType::NUMBER, KType::ANY),
        body_a,
        &types,
        &mut crate::machine::WriteGate::for_test(),
    );
    register_builtin(
        scope,
        "AN",
        two_slot_sig(KType::ANY, KType::NUMBER),
        body_b,
        &types,
        &mut crate::machine::WriteGate::for_test(),
    );
    let expr = working(
        region.brand(),
        vec![
            ExpressionPart::Literal(KLiteral::Number(5.0)),
            ExpressionPart::Keyword("OP"),
            ExpressionPart::Literal(KLiteral::Number(7.0)),
        ],
    );
    // NA and AN were both registered at `scope`'s BUILTIN index (0); root the chain there
    // one past it so both are visible and can tie.
    let chain = LexicalFrame::root(scope.id, 1);
    match scope.resolve_dispatch(&expr, Some(&chain), &[], &types) {
        DispatchOutcome::Ambiguous(n) => assert_eq!(n, 2),
        _ => panic!("expected Ambiguous(2) for tied overloads"),
    }
}

/// Inner ambiguity must surface even when `outer` has a non-ambiguous overload —
/// resolution does not fall through past a tie.
#[test]
fn resolve_does_not_descend_outer_on_inner_ambiguity() {
    let types = TypeRegistry::new();
    let region = run_root_storage();
    let outer = run_root_bare(&region);
    // User-position (not BUILTIN) so the builtin root-first short-circuit doesn't fire —
    // this exercises the inner-ambiguity-doesn't-descend walk, not builtin authority.
    register_overload_at(
        outer,
        "OUTER",
        two_slot_sig(KType::NUMBER, KType::NUMBER),
        body_a,
        BindingIndex::value(1),
        &TypeRegistry::new(),
        &mut crate::machine::WriteGate::for_test(),
    );
    let inner = outer.alloc_child_under();
    register_builtin(
        inner,
        "NA",
        two_slot_sig(KType::NUMBER, KType::ANY),
        body_a,
        &types,
        &mut crate::machine::WriteGate::for_test(),
    );
    register_builtin(
        inner,
        "AN",
        two_slot_sig(KType::ANY, KType::NUMBER),
        body_b,
        &types,
        &mut crate::machine::WriteGate::for_test(),
    );
    let expr = working(
        region.brand(),
        vec![
            ExpressionPart::Literal(KLiteral::Number(5.0)),
            ExpressionPart::Keyword("OP"),
            ExpressionPart::Literal(KLiteral::Number(7.0)),
        ],
    );
    // NA/AN were registered at `inner`'s BUILTIN index (0); root the chain on `inner` one
    // past it. `outer` is never named on this chain, so `OUTER` (index 1) stays visible
    // through the unmentioned-scope "fully visible" rule.
    let chain = LexicalFrame::root(inner.id, 1);
    match inner.resolve_dispatch(&expr, Some(&chain), &[], &types) {
        DispatchOutcome::Ambiguous(_) => {}
        _ => panic!("inner ambiguity must surface, not fall through to outer's unique overload"),
    }
}

/// A `Number` literal against an `<Identifier>`-only overload misses strictly
/// *and* tentatively (a Literal is not a bare name).
#[test]
fn resolve_tentative_falls_back_only_when_strict_empty() {
    let types = TypeRegistry::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    register_builtin(
        scope,
        "ONE_ID",
        one_slot_sig("v", KType::IDENTIFIER),
        body_a,
        &types,
        &mut crate::machine::WriteGate::for_test(),
    );
    let expr = working(
        region.brand(),
        vec![ExpressionPart::Literal(KLiteral::Number(5.0))],
    );
    // ONE_ID was registered at `scope`'s BUILTIN index (0); root the chain there one past
    // it so the registration is visible.
    let chain = LexicalFrame::root(scope.id, 1);
    assert!(matches!(
        scope.resolve_dispatch(&expr, Some(&chain), &[], &types),
        DispatchOutcome::Unmatched
    ));
}

/// `((deep_call) + 1)` returns `Deferred` rather than `Unmatched`: the typed
/// overload can't match the nested `Expression` strictly or tentatively, but
/// eager evaluation of `(deep_call)` may produce a `Spliced(Number)` that a
/// post-Bind re-dispatch picks. The scheduler routes `Deferred` into its
/// eager-sub loop instead of erroring.
#[test]
fn resolve_returns_deferred_for_nested_expression_in_typed_slot() {
    let types = TypeRegistry::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    register_builtin(
        scope,
        "PLUS",
        two_slot_sig(KType::NUMBER, KType::NUMBER),
        body_a,
        &types,
        &mut crate::machine::WriteGate::for_test(),
    );
    let brand = region.brand();
    let program = program_storage();
    let inner = ExpressionPart::expression(
        program.brand(),
        vec![Spanned::bare(ExpressionPart::Identifier("deep_call"))],
    );
    let expr = working(
        brand,
        vec![
            inner,
            ExpressionPart::Keyword("OP"),
            ExpressionPart::Literal(KLiteral::Number(1.0)),
        ],
    );
    // PLUS was registered at `scope`'s BUILTIN index (0); root the chain there one past
    // it so the registration is visible.
    let chain = LexicalFrame::root(scope.id, 1);
    assert!(matches!(
        scope.resolve_dispatch(&expr, Some(&chain), &[], &types),
        DispatchOutcome::Deferred
    ));
}

/// A pending overload sits in the *full* bucket it resolves into. A slot in `(MAKESET _)`
/// parks `(MAKESET <bare>)` but must not park `(MAKESET <bare> USING <bare>)` —
/// sharing a lead keyword is not enough to collide.
#[test]
fn pending_overload_parks_only_on_exact_bucket_match() {
    let types = TypeRegistry::new();
    use crate::machine::ProducerId;
    use crate::machine::model::{UntypedElement, UntypedKey};
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let bucket_single: UntypedKey = vec![
        UntypedElement::Keyword("MAKESET".into()),
        UntypedElement::Slot,
    ];
    scope
        .install_pending_overload(
            bucket_single,
            ProducerId::for_test(42),
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("install_pending_overload");

    let bare = working(
        region.brand(),
        vec![
            ExpressionPart::Keyword("MAKESET"),
            ExpressionPart::Identifier("fwd"),
        ],
    );
    // The pending overload was installed at `scope`'s BUILTIN index (0); root the chain
    // there one past it so it is visible.
    let chain = LexicalFrame::root(scope.id, 1);
    match scope.resolve_dispatch(&bare, Some(&chain), &[], &types) {
        DispatchOutcome::ParkOnProducers(ps) => assert_eq!(ps, vec![ProducerId::for_test(42)]),
        other => panic!(
            "expected ParkOnProducers([42]) for matching bucket, got {}",
            std::any::type_name_of_val(&other)
        ),
    }

    let multi = working(
        region.brand(),
        vec![
            ExpressionPart::Keyword("MAKESET"),
            ExpressionPart::Identifier("fwd"),
            ExpressionPart::Keyword("USING"),
            ExpressionPart::Identifier("other"),
        ],
    );
    assert!(
        matches!(
            scope.resolve_dispatch(&multi, Some(&chain), &[], &types),
            DispatchOutcome::Unmatched
        ),
        "different-bucket call must not park on a lead-keyword sibling",
    );
}

/// An inner-scope pending overload shadows an outer-scope strict Pick: the
/// pending sibling would shadow the outer match once it finalizes, so the inner
/// scope parks rather than letting the outer Pick win on finalize order.
#[test]
fn inner_scope_pending_overload_shadows_outer_strict_pick() {
    let types = TypeRegistry::new();
    use crate::machine::ProducerId;
    let region = run_root_storage();
    let outer = run_root_bare(&region);
    // Outer finalized overload that strictly Picks `(MARK <number>)`.
    let outer_sig = SignatureDraft {
        return_type: ReturnType::Resolved(KType::ANY),
        elements: vec![
            SignatureElement::Keyword("MARK"),
            SignatureElement::Argument(Argument {
                name: "v",
                ktype: KType::NUMBER,
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
        &TypeRegistry::new(),
        &mut crate::machine::WriteGate::for_test(),
    );

    let inner = outer.alloc_child_under();
    let expr = working(
        region.brand(),
        vec![
            ExpressionPart::Keyword("MARK"),
            ExpressionPart::Literal(KLiteral::Number(7.0)),
        ],
    );
    // Inner pending sibling on the same bucket key, body not yet finalized.
    scope_install_pending(inner, &expr, ProducerId::for_test(55));

    // The pending sibling was installed at `inner`'s BUILTIN index (0); root the chain on
    // `inner` one past it. `outer` is never named on this chain, so its strict Pick at
    // index 1 stays visible through the unmentioned-scope "fully visible" rule.
    let chain = LexicalFrame::root(inner.id, 1);
    match inner.resolve_dispatch(&expr, Some(&chain), &[], &types) {
        DispatchOutcome::ParkOnProducers(ps) => assert_eq!(
            ps,
            vec![ProducerId::for_test(55)],
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
    let types = TypeRegistry::new();
    let region = run_root_storage();
    let outer = run_root_bare(&region);
    // Outer overload that would strictly Pick once the eager sub resolves.
    register_builtin(
        outer,
        "outer_plus",
        two_slot_sig(KType::NUMBER, KType::NUMBER),
        body_a,
        &types,
        &mut crate::machine::WriteGate::for_test(),
    );
    let inner = outer.alloc_child_under();
    register_builtin(
        inner,
        "inner_plus",
        two_slot_sig(KType::NUMBER, KType::NUMBER),
        body_b,
        &types,
        &mut crate::machine::WriteGate::for_test(),
    );
    let brand = region.brand();
    let program = program_storage();
    let nested = ExpressionPart::expression(
        program.brand(),
        vec![Spanned::bare(ExpressionPart::Identifier("deep_call"))],
    );
    let expr = working(
        brand,
        vec![
            nested,
            ExpressionPart::Keyword("OP"),
            ExpressionPart::Literal(KLiteral::Number(1.0)),
        ],
    );
    // inner_plus was registered at `inner`'s BUILTIN index (0); root the chain on `inner`
    // one past it. `outer` is never named on this chain, so `outer_plus` stays visible
    // through the unmentioned-scope "fully visible" rule.
    let chain = LexicalFrame::root(inner.id, 1);
    assert!(
        matches!(
            inner.resolve_dispatch(&expr, Some(&chain), &[], &types),
            DispatchOutcome::Deferred
        ),
        "inner eager-lean must Defer at its scope, not fall through to outer",
    );
}

/// A dead (unbound) bare-name lean at an inner scope must NOT pre-empt an outer
/// `:Identifier` strict Pick: the inner `:Number` overload rejects the bare name
/// (dead lean → continue), and the outer `:Identifier` slot Picks it shape-only.
#[test]
fn dead_bare_name_lean_does_not_preempt_outer_identifier_pick() {
    let types = TypeRegistry::new();
    use crate::machine::execute::Resolution;
    let region = run_root_storage();
    let outer = run_root_bare(&region);
    // Outer `:Identifier` overload that owns the bare name (shape-only admit).
    register_builtin(
        outer,
        "outer_id",
        one_slot_sig("v", KType::IDENTIFIER),
        body_a,
        &types,
        &mut crate::machine::WriteGate::for_test(),
    );
    let inner = outer.alloc_child_under();
    // Inner `:Number` overload: the unbound bare name rejects its shape, so the
    // inner scope's only contribution is a dead lean (must not terminate).
    register_builtin(
        inner,
        "inner_num",
        one_slot_sig("v", KType::NUMBER),
        body_b,
        &types,
        &mut crate::machine::WriteGate::for_test(),
    );
    let expr = working(region.brand(), vec![ExpressionPart::Identifier("fwd")]);
    let bare_outcomes = vec![Some(Resolution::Unbound("fwd".into()))];
    // inner_num was registered at `inner`'s BUILTIN index (0); root the chain on `inner`
    // one past it. `outer` is never named on this chain, so `outer_id` stays visible
    // through the unmentioned-scope "fully visible" rule.
    let chain = LexicalFrame::root(inner.id, 1);
    match inner.resolve_dispatch(&expr, Some(&chain), &bare_outcomes, &types) {
        DispatchOutcome::Resolved(r) => assert!(
            matches!(
                r.function.value().signature.elements().first(),
                Some(SignatureElement::Argument(arg)) if arg.ktype == KType::IDENTIFIER
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
    let types = TypeRegistry::new();
    use crate::machine::ProducerId;
    use crate::machine::core::kfunction::{Body, KFunction};
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    // Finalized `(PICK <number>)` user overload that strictly Picks. Registered at a
    // user index (not BUILTIN) so the same-bucket sibling below is a legitimate
    // user-vs-user overload — a builtin bucket admits no user siblings.
    let pick_num = SignatureDraft {
        return_type: ReturnType::Resolved(KType::ANY),
        elements: vec![
            SignatureElement::Keyword("PICK"),
            SignatureElement::Argument(Argument {
                name: "v",
                ktype: KType::NUMBER,
            }),
        ],
    };
    let pick_num_fn = KFunction::alloc_captured(scope, pick_num, Body::Builtin(body_a), &types);
    scope
        .register_function_direct(
            "pick_num".to_string(),
            &pick_num_fn,
            BindingIndex::value(1),
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("register pick_num overload");
    let expr = working(
        region.brand(),
        vec![
            ExpressionPart::Keyword("PICK"),
            ExpressionPart::Literal(KLiteral::Number(7.0)),
        ],
    );
    // In-flight pending sibling on the same bucket key, finalizing at index 3.
    scope
        .install_pending_overload(
            expr.untyped_key(),
            ProducerId::for_test(77),
            BindingIndex::value(3),
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("install_pending_overload");

    // pick_num sits at index 1, the pending sibling finalizes at index 3, and the
    // finalizing overload below lands at index 3 too; root the chain on `scope` one past
    // the highest of those so every entry stays visible across both resolves below.
    let chain = LexicalFrame::root(scope.id, 4);
    match scope.resolve_dispatch(&expr, Some(&chain), &[], &types) {
        DispatchOutcome::ParkOnProducers(ps) => assert_eq!(
            ps,
            vec![ProducerId::for_test(77)],
            "finalized Pick must park on the in-flight pending sibling",
        ),
        other => panic!(
            "expected ParkOnProducers([77]) while pending sibling is in flight; got {}",
            std::any::type_name_of_val(&other),
        ),
    }

    // Finalize the pending sibling: registering a same-bucket overload at the
    // pending's index overwrites its pending slot in place (the real finalize path,
    // which matches by `BindingIndex`).
    let pick_str = SignatureDraft {
        return_type: ReturnType::Resolved(KType::ANY),
        elements: vec![
            SignatureElement::Keyword("PICK"),
            SignatureElement::Argument(Argument {
                name: "v",
                ktype: KType::STR,
            }),
        ],
    };
    let sibling =
        KFunction::alloc_captured(scope, pick_str, Body::Builtin(super::body_no_op), &types);
    scope
        .register_function_direct(
            "pick_str".to_string(),
            &sibling,
            BindingIndex::value(3),
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("register sibling overload");

    match scope.resolve_dispatch(&expr, Some(&chain), &[], &types) {
        DispatchOutcome::Resolved(_) => {}
        other => panic!(
            "bucket must resolve once the pending sibling finalizes; got {}",
            std::any::type_name_of_val(&other),
        ),
    }
}

/// Install a pending overload keyed by `expr`'s bucket key onto `scope`.
fn scope_install_pending<'a>(
    scope: &'a Scope<'a>,
    expr: &WorkingExpression<'a>,
    claim: crate::machine::ProducerId,
) {
    scope
        .install_pending_overload(
            expr.untyped_key(),
            claim,
            BindingIndex::BUILTIN,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("install_pending_overload");
}

/// Two sibling binders that share a bucket key each claim their own pending slot in
/// `functions[bucket]` — coalescing or rejecting the second would
/// drop a distinct wake source. A consumer parks on the earliest-index visible
/// one.
#[test]
fn sibling_pending_overloads_park_on_earliest_visible_entry() {
    let types = TypeRegistry::new();
    use crate::machine::ProducerId;
    use crate::machine::model::{UntypedElement, UntypedKey};
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let bucket: UntypedKey = vec![UntypedElement::Keyword("PICK".into()), UntypedElement::Slot];
    scope
        .install_pending_overload(
            bucket.clone(),
            ProducerId::for_test(101),
            BindingIndex::value(3),
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("first install");
    scope
        .install_pending_overload(
            bucket.clone(),
            ProducerId::for_test(102),
            BindingIndex::value(4),
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("second install must not collide");
    let entries = scope.bindings().pending_overload_entries(&bucket);
    assert_eq!(
        entries.len(),
        2,
        "both sibling installs must coexist as distinct slots; got {:?}",
        entries,
    );

    let expr = working(
        region.brand(),
        vec![
            ExpressionPart::Keyword("PICK"),
            ExpressionPart::Identifier("fwd"),
        ],
    );
    // The two sibling pending overloads finalize at indices 3 and 4; root the chain on
    // `scope` one past the higher so both stay visible.
    let chain = LexicalFrame::root(scope.id, 5);
    match scope.resolve_dispatch(&expr, Some(&chain), &[], &types) {
        DispatchOutcome::ParkOnProducers(ps) => {
            assert_eq!(
                ps,
                vec![ProducerId::for_test(101)],
                "consumer must park on earliest-index visible pending entry",
            );
        }
        other => panic!(
            "expected ParkOnProducers([101]), got variant {}",
            std::any::type_name_of_val(&other),
        ),
    }
}

/// A still-finalizing bare name parks dispatch resolution before any pick — even sharing the
/// expression with an eager part, the shape whose speculative pick used to reach the splice walk
/// and drop the staged sub on the park. The park carries the name's producer, nothing is staged,
/// and the wake re-resolves against the landed value.
#[test]
fn parked_bare_name_parks_before_any_pick() {
    let types = TypeRegistry::new();
    use crate::machine::ProducerId;
    use crate::machine::execute::Resolution;
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    register_builtin(
        scope,
        "OP",
        two_slot_sig(KType::ANY, KType::ANY),
        body_a,
        &types,
        &mut crate::machine::WriteGate::for_test(),
    );
    let program = program_storage();
    let inner = ExpressionPart::expression(
        program.brand(),
        vec![
            Spanned::bare(ExpressionPart::Literal(KLiteral::Number(1.0))),
            Spanned::bare(ExpressionPart::Keyword("OP")),
            Spanned::bare(ExpressionPart::Literal(KLiteral::Number(2.0))),
        ],
    );
    let expr = working(
        region.brand(),
        vec![
            ExpressionPart::Identifier("z"),
            ExpressionPart::Keyword("OP"),
            inner,
        ],
    );
    let producer = ProducerId::for_test(7);
    let bare_outcomes = vec![Some(Resolution::Parked(producer)), None, None];
    let chain = LexicalFrame::root(scope.id, 1);
    match scope.resolve_dispatch(&expr, Some(&chain), &bare_outcomes, &types) {
        DispatchOutcome::ParkOnProducers(ps) => assert_eq!(ps, vec![producer]),
        other => panic!(
            "a parked bare name must park before any pick; got variant {}",
            std::any::type_name_of_val(&other),
        ),
    }
}

/// The park pre-scan's binder exemptions, keyed off the expression's cached declared-name
/// position: the declaration slot must not wait on a same-named outer binder, and a binder form's
/// `Type`-token operands belong to the binder body's own type machinery — while a binder's
/// ordinary value slot still parks on a still-finalizing reference.
#[test]
fn binder_declaration_slots_are_exempt_from_the_park_pre_scan() {
    use crate::machine::ProducerId;
    use crate::machine::execute::Resolution;
    use crate::machine::model::{KExpression, TypeIdentifier};
    let types = TypeRegistry::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let brand = region.brand();
    let chain = LexicalFrame::root(scope.id, 1);
    let parked = || Some(Resolution::Parked(ProducerId::for_test(9)));
    let let_form = |name, value| {
        WorkingExpression::from_ast(
            brand,
            KExpression::new(
                brand,
                vec![
                    Spanned::bare(ExpressionPart::Keyword("LET")),
                    Spanned::bare(name),
                    Spanned::bare(ExpressionPart::Keyword("=")),
                    Spanned::bare(value),
                ],
            ),
        )
    };

    // Declaration slot: an inner `LET x = 1` shadowing a still-finalizing outer `x` must not wait.
    let decl = let_form(
        ExpressionPart::Identifier("x"),
        ExpressionPart::Literal(KLiteral::Number(1.0)),
    );
    let outcomes = vec![None, parked(), None, None];
    assert!(
        !matches!(
            scope.resolve_dispatch(&decl, Some(&chain), &outcomes, &types),
            DispatchOutcome::ParkOnProducers(_)
        ),
        "the declaration slot owns its name; it must not park on an outer claim",
    );

    // Type-token operand of a binder form: the binder body's type machinery owns the wait.
    let alias = let_form(
        ExpressionPart::Type(TypeIdentifier::leaf("Alias")),
        ExpressionPart::Type(TypeIdentifier::leaf("OtherT")),
    );
    let outcomes = vec![None, parked(), None, parked()];
    assert!(
        !matches!(
            scope.resolve_dispatch(&alias, Some(&chain), &outcomes, &types),
            DispatchOutcome::ParkOnProducers(_)
        ),
        "a binder form's Type-token operands resolve through the binder body, not the pre-scan",
    );

    // A binder's ordinary value slot is a reference: it waits like any other.
    let reference = let_form(
        ExpressionPart::Identifier("x"),
        ExpressionPart::Identifier("y"),
    );
    let producer = ProducerId::for_test(11);
    let outcomes = vec![None, None, None, Some(Resolution::Parked(producer))];
    match scope.resolve_dispatch(&reference, Some(&chain), &outcomes, &types) {
        DispatchOutcome::ParkOnProducers(ps) => assert_eq!(ps, vec![producer]),
        other => panic!(
            "a binder's value-slot reference must park; got variant {}",
            std::any::type_name_of_val(&other),
        ),
    }
}
