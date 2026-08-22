use crate::machine::core::{ProgramBrand, program_storage};
use crate::machine::model::RunRegistries;
use crate::machine::model::ast::{
    DispatchShape, ExpressionPart, KExpression, KLiteral, TypeIdentifier, classify_dispatch_shape,
};
use crate::machine::model::types::KKind;
use crate::machine::model::types::KType;
use crate::machine::model::values::Held;
use crate::source::Spanned;

fn kw(s: &str) -> ExpressionPart<'_> {
    ExpressionPart::Keyword(s)
}
fn ident(s: &str) -> ExpressionPart<'_> {
    ExpressionPart::Identifier(s)
}
fn ty(s: &str) -> ExpressionPart<'_> {
    ExpressionPart::Type(TypeIdentifier::leaf(s))
}
fn num<'a>(n: f64) -> ExpressionPart<'a> {
    ExpressionPart::Literal(KLiteral::Number(n))
}
fn parts_of<'a>(
    items: Vec<ExpressionPart<'a>>,
) -> impl ExactSizeIterator<Item = Spanned<ExpressionPart<'a>>> {
    items.into_iter().map(Spanned::bare)
}
fn expr<'a>(brand: ProgramBrand<'a>, parts: Vec<ExpressionPart<'a>>) -> ExpressionPart<'a> {
    ExpressionPart::expression_from_iter(brand, parts_of(parts))
}
fn list<'a>(brand: ProgramBrand<'a>, items: Vec<ExpressionPart<'a>>) -> ExpressionPart<'a> {
    ExpressionPart::ListLiteral(brand.region().allocator().slice_from_iter(items))
}
fn dict<'a>(
    brand: ProgramBrand<'a>,
    pairs: Vec<(ExpressionPart<'a>, ExpressionPart<'a>)>,
) -> ExpressionPart<'a> {
    ExpressionPart::DictLiteral(brand.region().allocator().slice_from_iter(pairs))
}
fn record<'a>(
    brand: ProgramBrand<'a>,
    fields: Vec<(&'a str, ExpressionPart<'a>)>,
) -> ExpressionPart<'a> {
    ExpressionPart::RecordLiteral(brand.region().allocator().slice_from_iter(fields))
}
fn sigil<'a>(brand: ProgramBrand<'a>, parts: Vec<ExpressionPart<'a>>) -> ExpressionPart<'a> {
    ExpressionPart::SigiledTypeExpr(brand.nested_node_from_iter(parts_of(parts)))
}
/// Freeze a run of parts into a node at `brand` — the door every hand-built AST here goes through.
fn build<'a>(brand: ProgramBrand<'a>, items: Vec<ExpressionPart<'a>>) -> KExpression<'a> {
    KExpression::new_from_iter(brand.region(), parts_of(items))
}

#[test]
fn resolve_for_lowers_builtin_leaf_to_type_arm() {
    let storage = crate::machine::core::run_root_storage();
    let scope = crate::builtins::test_support::run_root_bare(&storage);
    let part = ExpressionPart::Type(TypeIdentifier::leaf("Number"));
    let slot = KType::of_kind(KKind::ProperType);
    // Consume the scope-tied `Held` inside `matches!` so no borrow outlives `storage`.
    assert!(matches!(
        part.resolve_for(&slot, scope),
        Held::Type(t) if t == KType::NUMBER
    ));
}

/// A bare user type name has no builtin lowering, so the bind seam hands it on as the
/// `UnresolvedType` carrier: the surface `TypeIdentifier` survives verbatim and no type handle
/// is ever minted for an unresolved name.
#[test]
fn resolve_for_defers_user_bound_leaf_to_unresolved_carrier() {
    let storage = crate::machine::core::run_root_storage();
    let scope = crate::builtins::test_support::run_root_bare(&storage);
    let registries = RunRegistries::new();
    let part = ExpressionPart::Type(TypeIdentifier::leaf("MyType"));
    let slot = KType::of_kind(KKind::ProperType);
    match part.resolve_for(&slot, scope) {
        Held::UnresolvedType(te) => assert_eq!(te.render(), "MyType"),
        other => panic!(
            "expected the unlowered-name carrier, got {}",
            other.summarize(&registries)
        ),
    }
}

/// The unlowered carrier still classifies as a proper type for slot matching, so an unresolved
/// name keeps riding the type channel exactly where the lowered arm did.
#[test]
fn unresolved_carrier_classifies_as_a_proper_type() {
    let storage = crate::machine::core::run_root_storage();
    let scope = crate::builtins::test_support::run_root_bare(&storage);
    let registries = RunRegistries::new();
    let types = &registries.types;
    let part = ExpressionPart::Type(TypeIdentifier::leaf("MyType"));
    let slot = KType::of_kind(KKind::ProperType);
    let held = part.resolve_for(&slot, scope);
    assert_eq!(held.ktype(types), KType::of_kind(KKind::ProperType));
    assert!(held.as_type().is_none(), "it carries no type handle");
    assert!(held.as_object().is_none(), "and it is not a value");
}

#[test]
fn summarize_atomic_variants() {
    assert_eq!(kw("LET").summarize(), "LET");
    assert_eq!(ident("x").summarize(), "x");
    assert_eq!(
        ExpressionPart::Type(TypeIdentifier::leaf("Number")).summarize(),
        "Number",
    );
}

#[test]
fn summarize_literal_variants() {
    assert_eq!(
        ExpressionPart::Literal(KLiteral::Number(1.5)).summarize(),
        "1.5"
    );
    assert_eq!(
        ExpressionPart::Literal(KLiteral::String("hi")).summarize(),
        "hi"
    );
    assert_eq!(
        ExpressionPart::Literal(KLiteral::Boolean(true)).summarize(),
        "true"
    );
    assert_eq!(ExpressionPart::Literal(KLiteral::Null).summarize(), "null");
}

#[test]
fn summarize_list_and_dict_literals() {
    let program = program_storage();
    let brand = program.brand();
    let items = list(brand, vec![num(1.0), num(2.0)]);
    assert_eq!(items.summarize(), "[1 2]");

    let pairs = dict(
        brand,
        vec![(ExpressionPart::Literal(KLiteral::String("k")), num(7.0))],
    );
    assert_eq!(pairs.summarize(), "{k: 7}");
}

#[test]
fn summarize_nested_expression_part_threads_through() {
    let program = program_storage();
    let brand = program.brand();
    let inner = expr(brand, vec![kw("ADD"), ident("a"), ident("b")]);
    assert_eq!(inner.summarize(), "ADD a b");
}

#[test]
fn kexpression_summarize_joins_parts_with_spaces() {
    let program = program_storage();
    let brand = program.brand();
    let e = build(brand, vec![kw("LET"), ident("x"), ident("=")]);
    assert_eq!(e.summarize(), "LET x =");
}

#[test]
fn structural_equal_and_ktype_for_kexpression() {
    let program = program_storage();
    let brand = program.brand();
    let registries = RunRegistries::new();
    use crate::machine::model::values::KObject;
    let a =
        KObject::KExpression(brand.new_expression_from_iter(parts_of(vec![kw("LET"), ident("x")])));
    let b =
        KObject::KExpression(brand.new_expression_from_iter(parts_of(vec![kw("LET"), ident("x")])));
    let c =
        KObject::KExpression(brand.new_expression_from_iter(parts_of(vec![kw("LET"), ident("y")])));
    assert_eq!(a.value_equal(&b, &registries), Ok(true));
    assert_eq!(a.value_equal(&c, &registries), Ok(false));
    assert_eq!(a.ktype(), KType::KEXPRESSION);
}

#[test]
fn binder_name_from_type_part_extracts_or_none() {
    let program = program_storage();
    let brand = program.brand();
    let with_type = build(brand, vec![kw("STRUCT"), ty("Point")]);
    assert_eq!(with_type.binder_name_from_type_part(), Some("Point"));

    let with_ident = build(brand, vec![kw("STRUCT"), ident("Point")]);
    assert_eq!(with_ident.binder_name_from_type_part(), None);

    let too_short = build(brand, vec![kw("STRUCT")]);
    assert_eq!(too_short.binder_name_from_type_part(), None);
}

#[test]
fn borrow_inner_expressions_success_and_mismatch() {
    let program = program_storage();
    let brand = program.brand();
    let all_exprs = build(
        brand,
        vec![expr(brand, vec![ident("a")]), expr(brand, vec![ident("b")])],
    );
    let borrowed = all_exprs
        .borrow_inner_expressions()
        .expect("all parts are expressions");
    assert_eq!(borrowed.len(), 2);
    assert_eq!(borrowed[0].summarize(), "a");
    assert_eq!(borrowed[1].summarize(), "b");

    let mixed = build(brand, vec![expr(brand, vec![ident("a")]), ident("b")]);
    assert!(mixed.borrow_inner_expressions().is_none());
}

#[test]
fn try_split_inner_expressions_empty_returns_err() {
    let program = program_storage();
    let brand = program.brand();
    let e = build(brand, vec![]);
    let err = e.try_split_inner_expressions().expect_err("empty must Err");
    assert!(err.parts.is_empty());
}

#[test]
fn try_split_inner_expressions_first_non_expression_returns_err() {
    let program = program_storage();
    let brand = program.brand();
    let e = build(brand, vec![ident("a"), expr(brand, vec![ident("b")])]);
    let err = e
        .try_split_inner_expressions()
        .expect_err("non-expr head must Err");
    assert_eq!(err.summarize(), "a b");
}

#[test]
fn try_split_inner_expressions_middle_non_expression_returns_err() {
    let program = program_storage();
    let brand = program.brand();
    let e = build(
        brand,
        vec![
            expr(brand, vec![ident("a")]),
            ident("b"),
            expr(brand, vec![ident("c")]),
        ],
    );
    let err = e
        .try_split_inner_expressions()
        .expect_err("non-expr middle must Err");
    assert_eq!(err.summarize(), "a b c");
}

#[test]
fn try_split_inner_expressions_all_expressions_returns_ok() {
    let program = program_storage();
    let brand = program.brand();
    let e = build(
        brand,
        vec![
            expr(brand, vec![ident("a")]),
            expr(brand, vec![ident("b")]),
            expr(brand, vec![ident("c")]),
        ],
    );
    let (preceding, last) = e.try_split_inner_expressions().expect("all-expr is Ok");
    assert_eq!(preceding.len(), 2);
    assert_eq!(preceding[0].summarize(), "a");
    assert_eq!(preceding[1].summarize(), "b");
    assert_eq!(last.summarize(), "c");
}

// ---------- Structural cache: shape, untyped_key, operator_probe ----------

#[test]
fn operator_chain_three_operand_classifies_and_probes() {
    let program = program_storage();
    let brand = program.brand();
    // `a + b + c` — Slot Keyword Slot Keyword Slot, ≥2 keyword positions.
    let e = build(
        brand,
        vec![ident("a"), kw("+"), ident("b"), kw("+"), ident("c")],
    );
    assert_eq!(e.shape(), DispatchShape::OperatorChain);
    assert_eq!(e.operator_probe(), Some("+"));
}

#[test]
fn operator_chain_mixed_operators_probe_is_sorted_unique() {
    let program = program_storage();
    let brand = program.brand();
    // `a + b * c` — two distinct operators; probe is sorted-joined uniques.
    let e = build(
        brand,
        vec![ident("a"), kw("+"), ident("b"), kw("*"), ident("c")],
    );
    assert_eq!(e.shape(), DispatchShape::OperatorChain);
    assert_eq!(e.operator_probe(), Some("* +"));
}

#[test]
fn union_pipe_chain_over_types_is_operator_chain() {
    let program = program_storage();
    let brand = program.brand();
    // `A | B | C` — type operands, two `|` positions.
    let e = build(brand, vec![ty("A"), kw("|"), ty("B"), kw("|"), ty("C")]);
    assert_eq!(e.shape(), DispatchShape::OperatorChain);
    assert_eq!(e.operator_probe(), Some("|"));
}

#[test]
fn single_operator_is_keyworded_not_a_chain() {
    let program = program_storage();
    let brand = program.brand();
    // `a + b` — one keyword position; ordinary binary dispatch, no chain.
    let e = build(brand, vec![ident("a"), kw("+"), ident("b")]);
    assert_eq!(e.shape(), DispatchShape::Keyworded);
    assert_eq!(e.operator_probe(), None);
}

#[test]
fn keyword_led_shape_is_not_a_chain() {
    let program = program_storage();
    let brand = program.brand();
    // `LET x = a + b` is keyword-led (first part a keyword), so not the
    // slot-led chain shape even though it carries operator-like tokens.
    let e = build(
        brand,
        vec![kw("LET"), ident("x"), kw("="), ident("a"), kw("+")],
    );
    assert_eq!(e.shape(), DispatchShape::Keyworded);
    assert_eq!(e.operator_probe(), None);
}

#[test]
fn function_value_call_shape_unchanged() {
    let program = program_storage();
    let brand = program.brand();
    // `f x y` — lowercase identifier head, no keywords.
    let e = build(brand, vec![ident("f"), ident("x"), ident("y")]);
    assert_eq!(e.shape(), DispatchShape::FunctionValueCall);
    assert_eq!(e.operator_probe(), None);
}

#[test]
fn cached_fields_equal_on_demand_recompute() {
    let program = program_storage();
    let brand = program.brand();
    let e = build(
        brand,
        vec![ident("a"), kw("+"), ident("b"), kw("-"), ident("c")],
    );
    // Cache must match a fresh structural recompute.
    assert_eq!(e.shape(), classify_dispatch_shape(e.parts));
    let recomputed_key: crate::machine::model::types::UntypedKey = e
        .parts
        .iter()
        .map(|p| match &p.value {
            ExpressionPart::Keyword(s) => {
                crate::machine::model::types::UntypedElement::Keyword((*s).to_string())
            }
            _ => crate::machine::model::types::UntypedElement::Slot,
        })
        .collect();
    assert_eq!(e.untyped_key(), recomputed_key);
}

/// A node is `Copy`, so handing one on copies the whole structural cache with it — no rebuild and
/// no re-derivation at the destination.
#[test]
fn cache_rides_a_copy() {
    let program = program_storage();
    let brand = program.brand();
    let e = build(
        brand,
        vec![ident("a"), kw("|"), ident("b"), kw("|"), ident("c")],
    );
    let c = e;
    assert_eq!(c.shape(), DispatchShape::OperatorChain);
    assert_eq!(c.operator_probe(), Some("|"));
    assert_eq!(c.untyped_key(), e.untyped_key());
    assert_eq!(c.stored_key(), e.stored_key());
}

#[test]
fn key_and_shape_invariant_across_eager_slot_variants() {
    let program = program_storage();
    let brand = program.brand();
    // Every eager-part variant contributes `Slot`, so the classification of an
    // `a + <slot> + c` chain is identical regardless of which eager variant fills
    // the middle slot — which is what lets the scheduler carry the cache over to
    // its working node and splice a resolved sub-result into that slot without
    // reclassifying.
    let with_expr = build(
        brand,
        vec![
            ident("a"),
            kw("+"),
            expr(brand, vec![ident("b")]),
            kw("+"),
            ident("c"),
        ],
    );
    let with_list = build(
        brand,
        vec![
            ident("a"),
            kw("+"),
            list(brand, vec![ident("b")]),
            kw("+"),
            ident("c"),
        ],
    );
    let with_dict = build(
        brand,
        vec![
            ident("a"),
            kw("+"),
            dict(brand, vec![(ident("k"), ident("v"))]),
            kw("+"),
            ident("c"),
        ],
    );
    assert_eq!(with_expr.shape(), DispatchShape::OperatorChain);
    assert_eq!(with_expr.shape(), with_list.shape());
    assert_eq!(with_expr.shape(), with_dict.shape());
    assert_eq!(with_expr.untyped_key(), with_list.untyped_key());
    assert_eq!(with_expr.untyped_key(), with_dict.untyped_key());
    assert_eq!(with_expr.operator_probe(), with_list.operator_probe());
}

#[test]
fn cached_key_agrees_with_expression_signature_untyped_key() {
    use crate::machine::model::types::{Argument, ReturnType, SignatureDraft, SignatureElement};
    let program = program_storage();
    let brand = program.brand();
    // `a + b + c` against a `Slot + Slot + Slot` signature: the two
    // `untyped_key`s MUST agree (the invariant at signature.rs:23).
    let e = build(
        brand,
        vec![ident("a"), kw("+"), ident("b"), kw("+"), ident("c")],
    );
    let sig = SignatureDraft {
        return_type: ReturnType::Resolved(KType::ANY),
        elements: vec![
            SignatureElement::Argument(Argument {
                name: crate::machine::model::BinderSymbol::of("x")
                    .expect("a test fixture parameter is a value token"),
                ktype: KType::ANY,
            }),
            SignatureElement::Keyword("+"),
            SignatureElement::Argument(Argument {
                name: crate::machine::model::BinderSymbol::of("y")
                    .expect("a test fixture parameter is a value token"),
                ktype: KType::ANY,
            }),
            SignatureElement::Keyword("+"),
            SignatureElement::Argument(Argument {
                name: crate::machine::model::BinderSymbol::of("z")
                    .expect("a test fixture parameter is a value token"),
                ktype: KType::ANY,
            }),
        ],
    };
    assert_eq!(e.untyped_key(), sig.untyped_key());
}

/// `(f x) 1` — nested-`Expression` head followed by a non-keyword part.
/// Classifier routes to `HeadDeferred` so the head is evaluated first.
#[test]
fn head_deferred_for_nested_expression_head() {
    let program = program_storage();
    let brand = program.brand();
    let e = build(
        brand,
        vec![expr(brand, vec![ident("f"), ident("x")]), num(1.0)],
    );
    assert_eq!(e.shape(), DispatchShape::HeadDeferred);
}

/// `:(MyCarrier) {x = 1}` — `:(...)` sigil head followed by a non-keyword part.
/// Routes to `TypeHeadDeferred` (the type-shaped head lane).
#[test]
fn type_head_deferred_for_sigiled_head() {
    let program = program_storage();
    let brand = program.brand();
    let e = build(
        brand,
        vec![
            sigil(brand, vec![ty("MyCarrier")]),
            record(brand, vec![("x", num(1.0))]),
        ],
    );
    assert_eq!(e.shape(), DispatchShape::TypeHeadDeferred);
}

/// `((inner))` — a single-part nested `Expression` is the literal-pass-through
/// surface, not a head-deferred call (no body to apply the head to).
#[test]
fn single_part_nested_expression_stays_literal_pass_through() {
    let program = program_storage();
    let brand = program.brand();
    let e = build(brand, vec![expr(brand, vec![ident("inner")])]);
    assert_eq!(e.shape(), DispatchShape::LiteralPassThrough);
}

/// `Point {x = 1}` — leaf-`Type` head + body. Routes to `TypeCall`.
#[test]
fn type_leaf_head_multipart_is_type_call() {
    let program = program_storage();
    let brand = program.brand();
    let e = build(
        brand,
        vec![ty("Point"), record(brand, vec![("x", num(1.0))])],
    );
    assert_eq!(e.shape(), DispatchShape::TypeCall);
}

/// `f {x = 1}` — lowercase-`Identifier` head + body. Routes to
/// `FunctionValueCall`.
#[test]
fn identifier_head_multipart_is_function_value_call() {
    let program = program_storage();
    let brand = program.brand();
    let e = build(
        brand,
        vec![ident("f"), record(brand, vec![("x", num(1.0))])],
    );
    assert_eq!(e.shape(), DispatchShape::FunctionValueCall);
}

/// `99 1` — a literal head in a multi-part expression is a non-callable head.
/// Heads must resolve to something callable; this is the error shape.
#[test]
fn non_callable_literal_head_is_error_shape() {
    let program = program_storage();
    let brand = program.brand();
    let e = build(brand, vec![num(99.0), num(1.0)]);
    assert_eq!(e.shape(), DispatchShape::NonCallableHead);

    // `[1 2 3] x` — list head is equally non-callable.
    let with_list = build(
        brand,
        vec![list(brand, vec![num(1.0), num(2.0), num(3.0)]), ident("x")],
    );
    assert_eq!(with_list.shape(), DispatchShape::NonCallableHead);
}

/// A keyword-free multi-part expression never classifies as `Keyworded` — that
/// shape is produced only by the keyword sweep. Covers every callable-head
/// and non-callable-head surface.
#[test]
fn keyworded_only_on_real_keyword() {
    let program = program_storage();
    let brand = program.brand();
    let cases: Vec<KExpression<'_>> = vec![
        build(
            brand,
            vec![ty("Point"), record(brand, vec![("x", num(1.0))])],
        ),
        build(
            brand,
            vec![ident("f"), record(brand, vec![("x", num(1.0))])],
        ),
        build(brand, vec![expr(brand, vec![ident("g")]), num(1.0)]),
        build(brand, vec![sigil(brand, vec![ty("F")]), num(1.0)]),
        build(brand, vec![num(99.0), num(1.0)]),
    ];
    for e in &cases {
        assert_ne!(
            e.shape(),
            DispatchShape::Keyworded,
            "keyword-free expression must never classify as Keyworded",
        );
    }
}

#[test]
fn debug_for_expression_part_and_kexpression() {
    let program = program_storage();
    let brand = program.brand();
    // Exact format isn't load-bearing; just assert non-empty / tagged output.
    let parts: Vec<ExpressionPart<'_>> = vec![
        kw("LET"),
        ident("x"),
        ty("Number"),
        num(1.0),
        list(brand, vec![ident("a")]),
        dict(brand, vec![(ident("k"), ident("v"))]),
        expr(brand, vec![ident("z")]),
    ];
    for p in &parts {
        let s = format!("{:?}", p);
        assert!(!s.is_empty());
    }
    let e = build(brand, parts);
    assert!(format!("{:?}", e).starts_with("KExpression"));
}
