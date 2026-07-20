use crate::machine::model::ast::{
    classify_dispatch_shape, DispatchShape, ExpressionPart, KExpression, KLiteral, TypeIdentifier,
};
use crate::machine::model::types::KKind;
use crate::machine::model::types::KType;
use crate::machine::model::values::Held;
use crate::machine::model::TypeRegistry;
use crate::source::Spanned;

fn kw(s: &str) -> ExpressionPart<'static> {
    ExpressionPart::Keyword(s.into())
}
fn ident(s: &str) -> ExpressionPart<'static> {
    ExpressionPart::Identifier(s.into())
}
fn ty(s: &str) -> ExpressionPart<'static> {
    ExpressionPart::Type(TypeIdentifier::leaf(s.into()))
}
fn expr(parts: Vec<ExpressionPart<'static>>) -> ExpressionPart<'static> {
    ExpressionPart::expression(parts)
}
fn parts_of(items: Vec<ExpressionPart<'static>>) -> Vec<Spanned<ExpressionPart<'static>>> {
    items.into_iter().map(Spanned::bare).collect()
}

#[test]
fn resolve_for_lowers_builtin_leaf_to_type_arm() {
    let types = TypeRegistry::new();
    let storage = crate::machine::core::run_root_storage();
    let scope = crate::builtins::test_support::run_root_bare(&storage);
    let part = ExpressionPart::Type(TypeIdentifier::leaf("Number".into()));
    let slot = KType::OfKind(KKind::ProperType);
    // Consume the scope-tied `Held` inside `matches!` so no borrow outlives `storage`.
    assert!(matches!(
        part.resolve_for(&slot, scope, &types),
        Held::Type(KType::Number)
    ));
}

/// A bare user type name has no builtin lowering, so the bind seam hands it on as the
/// `UnresolvedType` carrier: the surface `TypeIdentifier` survives verbatim and no type handle
/// is ever minted for an unresolved name.
#[test]
fn resolve_for_defers_user_bound_leaf_to_unresolved_carrier() {
    let storage = crate::machine::core::run_root_storage();
    let scope = crate::builtins::test_support::run_root_bare(&storage);
    let types = TypeRegistry::new();
    let part = ExpressionPart::Type(TypeIdentifier::leaf("MyType".into()));
    let slot = KType::OfKind(KKind::ProperType);
    match part.resolve_for(&slot, scope, &types) {
        Held::UnresolvedType(te) => assert_eq!(te.render(), "MyType"),
        other => panic!(
            "expected the unlowered-name carrier, got {}",
            other.summarize(&types)
        ),
    }
}

/// The unlowered carrier still classifies as a proper type for slot matching, so an unresolved
/// name keeps riding the type channel exactly where the lowered arm did.
#[test]
fn unresolved_carrier_classifies_as_a_proper_type() {
    let storage = crate::machine::core::run_root_storage();
    let scope = crate::builtins::test_support::run_root_bare(&storage);
    let types = TypeRegistry::new();
    let part = ExpressionPart::Type(TypeIdentifier::leaf("MyType".into()));
    let slot = KType::OfKind(KKind::ProperType);
    let held = part.resolve_for(&slot, scope, &types);
    assert!(matches!(
        held.ktype(&types),
        KType::OfKind(KKind::ProperType)
    ));
    assert!(held.as_type().is_none(), "it carries no type handle");
    assert!(held.as_object().is_none(), "and it is not a value");
}

#[test]
fn summarize_atomic_variants() {
    assert_eq!(kw("LET").summarize(), "LET");
    assert_eq!(ident("x").summarize(), "x");
    assert_eq!(
        ExpressionPart::Type(TypeIdentifier::leaf("Number".into())).summarize(),
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
        ExpressionPart::Literal(KLiteral::String("hi".into())).summarize(),
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
    let list = ExpressionPart::ListLiteral(vec![
        ExpressionPart::Literal(KLiteral::Number(1.0)),
        ExpressionPart::Literal(KLiteral::Number(2.0)),
    ]);
    assert_eq!(list.summarize(), "[1 2]");

    let dict = ExpressionPart::DictLiteral(vec![(
        ExpressionPart::Literal(KLiteral::String("k".into())),
        ExpressionPart::Literal(KLiteral::Number(7.0)),
    )]);
    assert_eq!(dict.summarize(), "{k: 7}");
}

#[test]
fn summarize_nested_expression_part_threads_through() {
    let inner = expr(vec![kw("ADD"), ident("a"), ident("b")]);
    assert_eq!(inner.summarize(), "ADD a b");
}

#[test]
fn kexpression_summarize_joins_parts_with_spaces() {
    let e = KExpression::new(parts_of(vec![kw("LET"), ident("x"), ident("=")]));
    assert_eq!(e.summarize(), "LET x =");
}

#[test]
fn structural_equal_and_ktype_for_kexpression() {
    let types = TypeRegistry::new();
    use crate::machine::model::values::KObject;
    let a = KObject::KExpression(KExpression::new(parts_of(vec![kw("LET"), ident("x")])));
    let b = KObject::KExpression(KExpression::new(parts_of(vec![kw("LET"), ident("x")])));
    let c = KObject::KExpression(KExpression::new(parts_of(vec![kw("LET"), ident("y")])));
    assert_eq!(a.value_equal(&b, &types), Ok(true));
    assert_eq!(a.value_equal(&c, &types), Ok(false));
    assert!(matches!(a.ktype(), KType::KExpression));
}

#[test]
fn binder_name_from_type_part_extracts_or_none() {
    let with_type = KExpression::new(parts_of(vec![
        kw("STRUCT"),
        ExpressionPart::Type(TypeIdentifier::leaf("Point".into())),
    ]));
    assert_eq!(with_type.binder_name_from_type_part(), Some("Point".into()));

    let with_ident = KExpression::new(parts_of(vec![kw("STRUCT"), ident("Point")]));
    assert_eq!(with_ident.binder_name_from_type_part(), None);

    let too_short = KExpression::new(parts_of(vec![kw("STRUCT")]));
    assert_eq!(too_short.binder_name_from_type_part(), None);
}

#[test]
fn borrow_inner_expressions_success_and_mismatch() {
    let all_exprs = KExpression::new(parts_of(vec![
        expr(vec![ident("a")]),
        expr(vec![ident("b")]),
    ]));
    let borrowed = all_exprs
        .borrow_inner_expressions()
        .expect("all parts are expressions");
    assert_eq!(borrowed.len(), 2);
    assert_eq!(borrowed[0].summarize(), "a");
    assert_eq!(borrowed[1].summarize(), "b");

    let mixed = KExpression::new(parts_of(vec![expr(vec![ident("a")]), ident("b")]));
    assert!(mixed.borrow_inner_expressions().is_none());
}

#[test]
fn try_take_inner_expressions_split_empty_returns_err() {
    let e: KExpression<'static> = KExpression::new(vec![]);
    let err = e
        .try_take_inner_expressions_split()
        .expect_err("empty must Err");
    assert!(err.parts.is_empty());
}

#[test]
fn try_take_inner_expressions_split_first_non_expression_returns_err() {
    let e = KExpression::new(parts_of(vec![ident("a"), expr(vec![ident("b")])]));
    let err = e
        .try_take_inner_expressions_split()
        .expect_err("non-expr head must Err");
    assert_eq!(err.summarize(), "a b");
}

#[test]
fn try_take_inner_expressions_split_middle_non_expression_returns_err() {
    let e = KExpression::new(parts_of(vec![
        expr(vec![ident("a")]),
        ident("b"),
        expr(vec![ident("c")]),
    ]));
    let err = e
        .try_take_inner_expressions_split()
        .expect_err("non-expr middle must Err");
    assert_eq!(err.summarize(), "a b c");
}

#[test]
fn try_take_inner_expressions_split_all_expressions_returns_ok() {
    let e = KExpression::new(parts_of(vec![
        expr(vec![ident("a")]),
        expr(vec![ident("b")]),
        expr(vec![ident("c")]),
    ]));
    let (preceding, last) = e
        .try_take_inner_expressions_split()
        .expect("all-expr is Ok");
    assert_eq!(preceding.len(), 2);
    assert_eq!(preceding[0].summarize(), "a");
    assert_eq!(preceding[1].summarize(), "b");
    assert_eq!(last.summarize(), "c");
}

// ---------- Structural cache: shape, untyped_key, operator_probe ----------

#[test]
fn operator_chain_three_operand_classifies_and_probes() {
    // `a + b + c` — Slot Keyword Slot Keyword Slot, ≥2 keyword positions.
    let e = KExpression::new(parts_of(vec![
        ident("a"),
        kw("+"),
        ident("b"),
        kw("+"),
        ident("c"),
    ]));
    assert_eq!(e.shape(), DispatchShape::OperatorChain);
    assert_eq!(e.operator_probe(), Some("+"));
}

#[test]
fn operator_chain_mixed_operators_probe_is_sorted_unique() {
    // `a + b * c` — two distinct operators; probe is sorted-joined uniques.
    let e = KExpression::new(parts_of(vec![
        ident("a"),
        kw("+"),
        ident("b"),
        kw("*"),
        ident("c"),
    ]));
    assert_eq!(e.shape(), DispatchShape::OperatorChain);
    assert_eq!(e.operator_probe(), Some("* +"));
}

#[test]
fn union_pipe_chain_over_types_is_operator_chain() {
    // `A | B | C` — type operands, two `|` positions.
    let e = KExpression::new(parts_of(vec![ty("A"), kw("|"), ty("B"), kw("|"), ty("C")]));
    assert_eq!(e.shape(), DispatchShape::OperatorChain);
    assert_eq!(e.operator_probe(), Some("|"));
}

#[test]
fn single_operator_is_keyworded_not_a_chain() {
    // `a + b` — one keyword position; ordinary binary dispatch, no chain.
    let e = KExpression::new(parts_of(vec![ident("a"), kw("+"), ident("b")]));
    assert_eq!(e.shape(), DispatchShape::Keyworded);
    assert_eq!(e.operator_probe(), None);
}

#[test]
fn keyword_led_shape_is_not_a_chain() {
    // `LET x = a + b` is keyword-led (first part a keyword), so not the
    // slot-led chain shape even though it carries operator-like tokens.
    let e = KExpression::new(parts_of(vec![
        kw("LET"),
        ident("x"),
        kw("="),
        ident("a"),
        kw("+"),
    ]));
    assert_eq!(e.shape(), DispatchShape::Keyworded);
    assert_eq!(e.operator_probe(), None);
}

#[test]
fn function_value_call_shape_unchanged() {
    // `f x y` — lowercase identifier head, no keywords.
    let e = KExpression::new(parts_of(vec![ident("f"), ident("x"), ident("y")]));
    assert_eq!(e.shape(), DispatchShape::FunctionValueCall);
    assert_eq!(e.operator_probe(), None);
}

#[test]
fn cached_fields_equal_on_demand_recompute() {
    let e = KExpression::new(parts_of(vec![
        ident("a"),
        kw("+"),
        ident("b"),
        kw("-"),
        ident("c"),
    ]));
    // Cache must match a fresh structural recompute.
    assert_eq!(e.shape(), classify_dispatch_shape(&e));
    let recomputed_key: crate::machine::model::types::UntypedKey = e
        .parts
        .iter()
        .map(|p| match &p.value {
            ExpressionPart::Keyword(s) => {
                crate::machine::model::types::UntypedElement::Keyword(s.clone())
            }
            _ => crate::machine::model::types::UntypedElement::Slot,
        })
        .collect();
    assert_eq!(e.untyped_key(), recomputed_key);
}

#[test]
fn cache_survives_clone() {
    let e = KExpression::new(parts_of(vec![
        ident("a"),
        kw("|"),
        ident("b"),
        kw("|"),
        ident("c"),
    ]));
    let c = e.clone();
    assert_eq!(c.shape(), DispatchShape::OperatorChain);
    assert_eq!(c.operator_probe(), Some("|"));
    assert_eq!(c.untyped_key(), e.untyped_key());
}

#[test]
fn key_and_shape_invariant_across_eager_slot_variants() {
    // The dispatch-time splice replaces an eager `Slot` part with `Spliced` (also a
    // `Slot`), so shape / key / probe are invariant under it. Every eager-part
    // variant contributes `Slot`, so the classification of an `a + <slot> + c` chain
    // must be identical regardless of which eager variant fills the middle slot.
    let with_expr = KExpression::new(parts_of(vec![
        ident("a"),
        kw("+"),
        expr(vec![ident("b")]),
        kw("+"),
        ident("c"),
    ]));
    let with_list = KExpression::new(parts_of(vec![
        ident("a"),
        kw("+"),
        ExpressionPart::ListLiteral(vec![ident("b")]),
        kw("+"),
        ident("c"),
    ]));
    let with_dict = KExpression::new(parts_of(vec![
        ident("a"),
        kw("+"),
        ExpressionPart::DictLiteral(vec![(ident("k"), ident("v"))]),
        kw("+"),
        ident("c"),
    ]));
    assert_eq!(with_expr.shape(), DispatchShape::OperatorChain);
    assert_eq!(with_expr.shape(), with_list.shape());
    assert_eq!(with_expr.shape(), with_dict.shape());
    assert_eq!(with_expr.untyped_key(), with_list.untyped_key());
    assert_eq!(with_expr.untyped_key(), with_dict.untyped_key());
    assert_eq!(with_expr.operator_probe(), with_list.operator_probe());
}

#[test]
fn cached_key_agrees_with_expression_signature_untyped_key() {
    use crate::machine::model::types::{
        Argument, ExpressionSignature, ReturnType, SignatureElement,
    };
    // `a + b + c` against a `Slot + Slot + Slot` signature: the two
    // `untyped_key`s MUST agree (the invariant at signature.rs:23).
    let e = KExpression::new(parts_of(vec![
        ident("a"),
        kw("+"),
        ident("b"),
        kw("+"),
        ident("c"),
    ]));
    let sig = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![
            SignatureElement::Argument(Argument {
                name: "x".into(),
                ktype: KType::Any,
            }),
            SignatureElement::Keyword("+".into()),
            SignatureElement::Argument(Argument {
                name: "y".into(),
                ktype: KType::Any,
            }),
            SignatureElement::Keyword("+".into()),
            SignatureElement::Argument(Argument {
                name: "z".into(),
                ktype: KType::Any,
            }),
        ],
    };
    assert_eq!(e.untyped_key(), sig.untyped_key());
}

fn num(n: f64) -> ExpressionPart<'static> {
    ExpressionPart::Literal(KLiteral::Number(n))
}
fn record(fields: Vec<(&str, ExpressionPart<'static>)>) -> ExpressionPart<'static> {
    ExpressionPart::RecordLiteral(
        fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}
fn sigil(parts: Vec<ExpressionPart<'static>>) -> ExpressionPart<'static> {
    ExpressionPart::SigiledTypeExpr(Box::new(KExpression::new(parts_of(parts))))
}

/// `(f x) 1` — nested-`Expression` head followed by a non-keyword part.
/// Classifier routes to `HeadDeferred` so the head is evaluated first.
#[test]
fn head_deferred_for_nested_expression_head() {
    let e = KExpression::new(parts_of(vec![expr(vec![ident("f"), ident("x")]), num(1.0)]));
    assert_eq!(e.shape(), DispatchShape::HeadDeferred);
}

/// `:(MyCarrier) {x = 1}` — `:(...)` sigil head followed by a non-keyword part.
/// Routes to `TypeHeadDeferred` (the type-shaped head lane).
#[test]
fn type_head_deferred_for_sigiled_head() {
    let e = KExpression::new(parts_of(vec![
        sigil(vec![ty("MyCarrier")]),
        record(vec![("x", num(1.0))]),
    ]));
    assert_eq!(e.shape(), DispatchShape::TypeHeadDeferred);
}

/// `((inner))` — a single-part nested `Expression` is the literal-pass-through
/// surface, not a head-deferred call (no body to apply the head to).
#[test]
fn single_part_nested_expression_stays_literal_pass_through() {
    let e = KExpression::new(parts_of(vec![expr(vec![ident("inner")])]));
    assert_eq!(e.shape(), DispatchShape::LiteralPassThrough);
}

/// `Point {x = 1}` — leaf-`Type` head + body. Routes to `TypeCall`.
#[test]
fn type_leaf_head_multipart_is_type_call() {
    let e = KExpression::new(parts_of(vec![ty("Point"), record(vec![("x", num(1.0))])]));
    assert_eq!(e.shape(), DispatchShape::TypeCall);
}

/// `f {x = 1}` — lowercase-`Identifier` head + body. Routes to
/// `FunctionValueCall`.
#[test]
fn identifier_head_multipart_is_function_value_call() {
    let e = KExpression::new(parts_of(vec![ident("f"), record(vec![("x", num(1.0))])]));
    assert_eq!(e.shape(), DispatchShape::FunctionValueCall);
}

/// `99 1` — a literal head in a multi-part expression is a non-callable head.
/// Heads must resolve to something callable; this is the error shape.
#[test]
fn non_callable_literal_head_is_error_shape() {
    let e = KExpression::new(parts_of(vec![num(99.0), num(1.0)]));
    assert_eq!(e.shape(), DispatchShape::NonCallableHead);

    // `[1 2 3] x` — list head is equally non-callable.
    let with_list = KExpression::new(parts_of(vec![
        ExpressionPart::ListLiteral(vec![num(1.0), num(2.0), num(3.0)]),
        ident("x"),
    ]));
    assert_eq!(with_list.shape(), DispatchShape::NonCallableHead);
}

/// A keyword-free multi-part expression never classifies as `Keyworded` — that
/// shape is now produced only by the keyword sweep. Covers every callable-head
/// and non-callable-head surface.
#[test]
fn keyworded_only_on_real_keyword() {
    let cases: Vec<KExpression<'static>> = vec![
        KExpression::new(parts_of(vec![ty("Point"), record(vec![("x", num(1.0))])])),
        KExpression::new(parts_of(vec![ident("f"), record(vec![("x", num(1.0))])])),
        KExpression::new(parts_of(vec![expr(vec![ident("g")]), num(1.0)])),
        KExpression::new(parts_of(vec![sigil(vec![ty("F")]), num(1.0)])),
        KExpression::new(parts_of(vec![num(99.0), num(1.0)])),
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
    // Exact format isn't load-bearing; just assert non-empty / tagged output.
    let parts: Vec<ExpressionPart<'static>> = vec![
        kw("LET"),
        ident("x"),
        ExpressionPart::Type(TypeIdentifier::leaf("Number".into())),
        ExpressionPart::Literal(KLiteral::Number(1.0)),
        ExpressionPart::ListLiteral(vec![ident("a")]),
        ExpressionPart::DictLiteral(vec![(ident("k"), ident("v"))]),
        expr(vec![ident("z")]),
    ];
    for p in &parts {
        let s = format!("{:?}", p);
        assert!(!s.is_empty());
    }
    let e = KExpression::new(parts.into_iter().map(Spanned::bare).collect());
    assert!(format!("{:?}", e).starts_with("KExpression"));
}
