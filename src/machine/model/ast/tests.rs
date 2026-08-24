use crate::builtins::test_support::{kw_part, probe_symbol, type_name, type_token};
use crate::machine::core::{ProgramBrand, program_storage};
use crate::machine::model::RunRegistries;
use crate::machine::model::ast::{
    DispatchShape, ExpressionPart, KExpression, KLiteral, classify_dispatch_shape,
};
use crate::machine::model::labels::LabelInterner;
use crate::machine::model::types::KKind;
use crate::machine::model::types::KType;
use crate::machine::model::values::Held;
use crate::source::Spanned;

fn kw(s: &str) -> ExpressionPart<'_> {
    kw_part(s)
}
/// [`kw`] for a test that renders the part back: the spelling is recorded in `labels`, so
/// `summarize` resolves it rather than reaching the missing-label placeholder.
fn declared_kw<'a>(s: &str, labels: &LabelInterner) -> ExpressionPart<'a> {
    ExpressionPart::Keyword(
        crate::machine::model::KeywordSymbol::declared(s, labels)
            .expect("a test fixture keyword is keyword-class"),
    )
}
/// The probe key a chain over `glyphs` computes — the run digest, minted the way
/// `operator_probe_for` mints it.
fn operator_probe_of(glyphs: &[&str]) -> crate::machine::model::KeywordSymbol {
    let members: Vec<_> = glyphs.iter().map(|glyph| probe_symbol(glyph)).collect();
    crate::machine::model::KeywordSymbol::of_run(&members)
}
fn ident<'a>(s: &str, labels: &LabelInterner) -> ExpressionPart<'a> {
    ExpressionPart::Identifier(
        crate::machine::model::ValueSymbol::declared(s, labels)
            .expect("a test fixture identifier is a value token"),
    )
}
fn ty(s: &str) -> ExpressionPart<'_> {
    ExpressionPart::Type(type_token(s))
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
    let part = ExpressionPart::Type(type_token("Number"));
    let slot = KType::of_kind(KKind::ProperType);
    // Consume the scope-tied `Held` inside `matches!` so no borrow outlives `storage`.
    assert!(matches!(
        part.resolve_for(&slot, scope),
        Held::Type(t) if t == KType::NUMBER
    ));
}

/// A bare user type name has no builtin lowering, so the bind seam hands it on as the
/// `UnresolvedType` carrier: the token's symbol survives verbatim and no type handle is ever
/// minted for an unresolved name.
#[test]
fn resolve_for_defers_user_bound_leaf_to_unresolved_carrier() {
    let storage = crate::machine::core::run_root_storage();
    let scope = crate::builtins::test_support::run_root_bare(&storage);
    let registries = RunRegistries::new();
    let part = ExpressionPart::Type(type_name("MyType", &registries));
    let slot = KType::of_kind(KKind::ProperType);
    match part.resolve_for(&slot, scope) {
        Held::UnresolvedType(te) => {
            assert_eq!(registries.labels.render(te.symbol()), "MyType")
        }
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
    let part = ExpressionPart::Type(type_token("MyType"));
    let slot = KType::of_kind(KKind::ProperType);
    let held = part.resolve_for(&slot, scope);
    assert_eq!(held.ktype(types), KType::of_kind(KKind::ProperType));
    assert!(held.as_type().is_none(), "it carries no type handle");
    assert!(held.as_object().is_none(), "and it is not a value");
}

#[test]
fn summarize_atomic_variants() {
    let registries = RunRegistries::new();
    assert_eq!(
        declared_kw("LET", &registries.labels).summarize(&registries.labels),
        "LET"
    );
    assert_eq!(
        ident("x", &registries.labels).summarize(&registries.labels),
        "x"
    );
    // A type token renders through the interner it was declared into.
    assert_eq!(
        ExpressionPart::Type(type_name("Number", &registries)).summarize(&registries.labels),
        "Number",
    );
}

#[test]
fn summarize_literal_variants() {
    assert_eq!(
        ExpressionPart::Literal(KLiteral::Number(1.5)).summarize(&LabelInterner::new()),
        "1.5"
    );
    assert_eq!(
        ExpressionPart::Literal(KLiteral::String("hi")).summarize(&LabelInterner::new()),
        "hi"
    );
    assert_eq!(
        ExpressionPart::Literal(KLiteral::Boolean(true)).summarize(&LabelInterner::new()),
        "true"
    );
    assert_eq!(
        ExpressionPart::Literal(KLiteral::Null).summarize(&LabelInterner::new()),
        "null"
    );
}

#[test]
fn summarize_list_and_dict_literals() {
    let program = program_storage();
    let brand = program.brand();
    let items = list(brand, vec![num(1.0), num(2.0)]);
    assert_eq!(items.summarize(&LabelInterner::new()), "[1 2]");

    let pairs = dict(
        brand,
        vec![(ExpressionPart::Literal(KLiteral::String("k")), num(7.0))],
    );
    assert_eq!(pairs.summarize(&LabelInterner::new()), "{k: 7}");
}

#[test]
fn summarize_nested_expression_part_threads_through() {
    let labels = LabelInterner::new();
    let program = program_storage();
    let brand = program.brand();
    let inner = expr(
        brand,
        vec![
            declared_kw("ADD", &labels),
            ident("a", &labels),
            ident("b", &labels),
        ],
    );
    assert_eq!(inner.summarize(&labels), "ADD a b");
}

#[test]
fn kexpression_summarize_joins_parts_with_spaces() {
    let labels = LabelInterner::new();
    let program = program_storage();
    let brand = program.brand();
    let e = build(
        brand,
        vec![
            declared_kw("LET", &labels),
            ident("x", &labels),
            declared_kw("=", &labels),
        ],
    );
    assert_eq!(e.summarize(&labels), "LET x =");
}

#[test]
fn structural_equal_and_ktype_for_kexpression() {
    let program = program_storage();
    let brand = program.brand();
    let registries = RunRegistries::new();
    use crate::machine::model::values::KObject;
    let a = KObject::KExpression(
        brand.new_expression_from_iter(parts_of(vec![kw("LET"), ident("x", &registries.labels)])),
    );
    let b = KObject::KExpression(
        brand.new_expression_from_iter(parts_of(vec![kw("LET"), ident("x", &registries.labels)])),
    );
    let c = KObject::KExpression(
        brand.new_expression_from_iter(parts_of(vec![kw("LET"), ident("y", &registries.labels)])),
    );
    assert_eq!(a.value_equal(&b, &registries), Ok(true));
    assert_eq!(a.value_equal(&c, &registries), Ok(false));
    assert_eq!(a.ktype(), KType::KEXPRESSION);
}

#[test]
fn binder_name_from_type_part_extracts_or_none() {
    let labels = LabelInterner::new();
    let program = program_storage();
    let brand = program.brand();
    let with_type = build(brand, vec![kw("STRUCT"), ty("Point")]);
    assert_eq!(
        with_type.binder_name_from_type_part(),
        Some(type_token("Point"))
    );

    let with_ident = build(brand, vec![kw("STRUCT"), ident("point", &labels)]);
    assert_eq!(with_ident.binder_name_from_type_part(), None);

    let too_short = build(brand, vec![kw("STRUCT")]);
    assert_eq!(too_short.binder_name_from_type_part(), None);
}

#[test]
fn borrow_inner_expressions_success_and_mismatch() {
    let labels = LabelInterner::new();
    let program = program_storage();
    let brand = program.brand();
    let all_exprs = build(
        brand,
        vec![
            expr(brand, vec![ident("a", &labels)]),
            expr(brand, vec![ident("b", &labels)]),
        ],
    );
    let borrowed = all_exprs
        .borrow_inner_expressions()
        .expect("all parts are expressions");
    assert_eq!(borrowed.len(), 2);
    assert_eq!(borrowed[0].summarize(&labels), "a");
    assert_eq!(borrowed[1].summarize(&labels), "b");

    let mixed = build(
        brand,
        vec![expr(brand, vec![ident("a", &labels)]), ident("b", &labels)],
    );
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
    let labels = LabelInterner::new();
    let program = program_storage();
    let brand = program.brand();
    let e = build(
        brand,
        vec![ident("a", &labels), expr(brand, vec![ident("b", &labels)])],
    );
    let err = e
        .try_split_inner_expressions()
        .expect_err("non-expr head must Err");
    assert_eq!(err.summarize(&labels), "a b");
}

#[test]
fn try_split_inner_expressions_middle_non_expression_returns_err() {
    let labels = LabelInterner::new();
    let program = program_storage();
    let brand = program.brand();
    let e = build(
        brand,
        vec![
            expr(brand, vec![ident("a", &labels)]),
            ident("b", &labels),
            expr(brand, vec![ident("c", &labels)]),
        ],
    );
    let err = e
        .try_split_inner_expressions()
        .expect_err("non-expr middle must Err");
    assert_eq!(err.summarize(&labels), "a b c");
}

#[test]
fn try_split_inner_expressions_all_expressions_returns_ok() {
    let labels = LabelInterner::new();
    let program = program_storage();
    let brand = program.brand();
    let e = build(
        brand,
        vec![
            expr(brand, vec![ident("a", &labels)]),
            expr(brand, vec![ident("b", &labels)]),
            expr(brand, vec![ident("c", &labels)]),
        ],
    );
    let (preceding, last) = e.try_split_inner_expressions().expect("all-expr is Ok");
    assert_eq!(preceding.len(), 2);
    assert_eq!(preceding[0].summarize(&labels), "a");
    assert_eq!(preceding[1].summarize(&labels), "b");
    assert_eq!(last.summarize(&labels), "c");
}

// ---------- Structural cache: shape, untyped_key, operator_probe ----------

#[test]
fn operator_chain_three_operand_classifies_and_probes() {
    let labels = LabelInterner::new();
    let program = program_storage();
    let brand = program.brand();
    // `a + b + c` — Slot Keyword Slot Keyword Slot, ≥2 keyword positions.
    let e = build(
        brand,
        vec![
            ident("a", &labels),
            kw("+"),
            ident("b", &labels),
            kw("+"),
            ident("c", &labels),
        ],
    );
    assert_eq!(e.shape(), DispatchShape::OperatorChain);
    assert_eq!(e.operator_probe(), Some(operator_probe_of(&["+"])));
}

#[test]
fn operator_chain_mixed_operators_probe_is_sorted_unique() {
    let labels = LabelInterner::new();
    let program = program_storage();
    let brand = program.brand();
    // `a + b * c` — two distinct operators; the probe is the digest of the two-member run.
    let e = build(
        brand,
        vec![
            ident("a", &labels),
            kw("+"),
            ident("b", &labels),
            kw("*"),
            ident("c", &labels),
        ],
    );
    assert_eq!(e.shape(), DispatchShape::OperatorChain);
    assert_eq!(e.operator_probe(), Some(operator_probe_of(&["*", "+"])));
}

#[test]
fn union_pipe_chain_over_types_is_operator_chain() {
    let program = program_storage();
    let brand = program.brand();
    // `Aa | Bb | Cc` — type operands, two `|` positions.
    let e = build(brand, vec![ty("Aa"), kw("|"), ty("Bb"), kw("|"), ty("Cc")]);
    assert_eq!(e.shape(), DispatchShape::OperatorChain);
    assert_eq!(e.operator_probe(), Some(operator_probe_of(&["|"])));
}

#[test]
fn single_operator_is_keyworded_not_a_chain() {
    let labels = LabelInterner::new();
    let program = program_storage();
    let brand = program.brand();
    // `a + b` — one keyword position; ordinary binary dispatch, no chain.
    let e = build(
        brand,
        vec![ident("a", &labels), kw("+"), ident("b", &labels)],
    );
    assert_eq!(e.shape(), DispatchShape::Keyworded);
    assert_eq!(e.operator_probe(), None);
}

#[test]
fn keyword_led_shape_is_not_a_chain() {
    let labels = LabelInterner::new();
    let program = program_storage();
    let brand = program.brand();
    // `LET x = a + b` is keyword-led (first part a keyword), so not the
    // slot-led chain shape even though it carries operator-like tokens.
    let e = build(
        brand,
        vec![
            kw("LET"),
            ident("x", &labels),
            kw("="),
            ident("a", &labels),
            kw("+"),
        ],
    );
    assert_eq!(e.shape(), DispatchShape::Keyworded);
    assert_eq!(e.operator_probe(), None);
}

#[test]
fn function_value_call_shape_unchanged() {
    let labels = LabelInterner::new();
    let program = program_storage();
    let brand = program.brand();
    // `f x y` — lowercase identifier head, no keywords.
    let e = build(
        brand,
        vec![
            ident("f", &labels),
            ident("x", &labels),
            ident("y", &labels),
        ],
    );
    assert_eq!(e.shape(), DispatchShape::FunctionValueCall);
    assert_eq!(e.operator_probe(), None);
}

#[test]
fn cached_fields_equal_on_demand_recompute() {
    let labels = LabelInterner::new();
    let program = program_storage();
    let brand = program.brand();
    let e = build(
        brand,
        vec![
            ident("a", &labels),
            kw("+"),
            ident("b", &labels),
            kw("-"),
            ident("c", &labels),
        ],
    );
    // Cache must match a fresh structural recompute.
    assert_eq!(e.shape(), classify_dispatch_shape(e.parts));
    let recomputed_key: crate::machine::model::types::UntypedKey = e
        .parts
        .iter()
        .map(|p| match &p.value {
            ExpressionPart::Keyword(symbol) => {
                crate::machine::model::types::KeyElement::Keyword(*symbol)
            }
            _ => crate::machine::model::types::KeyElement::Slot,
        })
        .collect();
    assert_eq!(e.untyped_key(), recomputed_key);
}

/// A node is `Copy`, so handing one on copies the whole structural cache with it — no rebuild and
/// no re-derivation at the destination.
#[test]
fn cache_rides_a_copy() {
    let labels = LabelInterner::new();
    let program = program_storage();
    let brand = program.brand();
    let e = build(
        brand,
        vec![
            ident("a", &labels),
            kw("|"),
            ident("b", &labels),
            kw("|"),
            ident("c", &labels),
        ],
    );
    let c = e;
    assert_eq!(c.shape(), DispatchShape::OperatorChain);
    assert_eq!(c.operator_probe(), Some(operator_probe_of(&["|"])));
    assert_eq!(c.untyped_key(), e.untyped_key());
    assert_eq!(c.stored_key(), e.stored_key());
}

#[test]
fn key_and_shape_invariant_across_eager_slot_variants() {
    let labels = LabelInterner::new();
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
            ident("a", &labels),
            kw("+"),
            expr(brand, vec![ident("b", &labels)]),
            kw("+"),
            ident("c", &labels),
        ],
    );
    let with_list = build(
        brand,
        vec![
            ident("a", &labels),
            kw("+"),
            list(brand, vec![ident("b", &labels)]),
            kw("+"),
            ident("c", &labels),
        ],
    );
    let with_dict = build(
        brand,
        vec![
            ident("a", &labels),
            kw("+"),
            dict(brand, vec![(ident("k", &labels), ident("v", &labels))]),
            kw("+"),
            ident("c", &labels),
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
    let labels = LabelInterner::new();
    use crate::machine::model::types::{Argument, ReturnType, SignatureDraft, SignatureElement};
    let program = program_storage();
    let brand = program.brand();
    // `a + b + c` against a `Slot + Slot + Slot` signature: the two
    // `untyped_key`s MUST agree (the invariant at signature.rs:23).
    let e = build(
        brand,
        vec![
            ident("a", &labels),
            kw("+"),
            ident("b", &labels),
            kw("+"),
            ident("c", &labels),
        ],
    );
    let sig = SignatureDraft {
        return_type: ReturnType::Resolved(KType::ANY),
        elements: vec![
            SignatureElement::Argument(Argument {
                name: crate::machine::model::BinderSymbol::classify("x")
                    .expect("a test fixture parameter is a value token"),
                ktype: KType::ANY,
            }),
            SignatureElement::Keyword(probe_symbol("+")),
            SignatureElement::Argument(Argument {
                name: crate::machine::model::BinderSymbol::classify("y")
                    .expect("a test fixture parameter is a value token"),
                ktype: KType::ANY,
            }),
            SignatureElement::Keyword(probe_symbol("+")),
            SignatureElement::Argument(Argument {
                name: crate::machine::model::BinderSymbol::classify("z")
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
    let labels = LabelInterner::new();
    let program = program_storage();
    let brand = program.brand();
    let e = build(
        brand,
        vec![
            expr(brand, vec![ident("f", &labels), ident("x", &labels)]),
            num(1.0),
        ],
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
    let labels = LabelInterner::new();
    let program = program_storage();
    let brand = program.brand();
    let e = build(brand, vec![expr(brand, vec![ident("inner", &labels)])]);
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
    let labels = LabelInterner::new();
    let program = program_storage();
    let brand = program.brand();
    let e = build(
        brand,
        vec![ident("f", &labels), record(brand, vec![("x", num(1.0))])],
    );
    assert_eq!(e.shape(), DispatchShape::FunctionValueCall);
}

/// `99 1` — a literal head in a multi-part expression is a non-callable head.
/// Heads must resolve to something callable; this is the error shape.
#[test]
fn non_callable_literal_head_is_error_shape() {
    let labels = LabelInterner::new();
    let program = program_storage();
    let brand = program.brand();
    let e = build(brand, vec![num(99.0), num(1.0)]);
    assert_eq!(e.shape(), DispatchShape::NonCallableHead);

    // `[1 2 3] x` — list head is equally non-callable.
    let with_list = build(
        brand,
        vec![
            list(brand, vec![num(1.0), num(2.0), num(3.0)]),
            ident("x", &labels),
        ],
    );
    assert_eq!(with_list.shape(), DispatchShape::NonCallableHead);
}

/// A keyword-free multi-part expression never classifies as `Keyworded` — that
/// shape is produced only by the keyword sweep. Covers every callable-head
/// and non-callable-head surface.
#[test]
fn keyworded_only_on_real_keyword() {
    let labels = LabelInterner::new();
    let program = program_storage();
    let brand = program.brand();
    let cases: Vec<KExpression<'_>> = vec![
        build(
            brand,
            vec![ty("Point"), record(brand, vec![("x", num(1.0))])],
        ),
        build(
            brand,
            vec![ident("f", &labels), record(brand, vec![("x", num(1.0))])],
        ),
        build(
            brand,
            vec![expr(brand, vec![ident("g", &labels)]), num(1.0)],
        ),
        build(brand, vec![sigil(brand, vec![ty("Ff")]), num(1.0)]),
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
    let labels = LabelInterner::new();
    let program = program_storage();
    let brand = program.brand();
    // Exact format isn't load-bearing; just assert non-empty / tagged output.
    let parts: Vec<ExpressionPart<'_>> = vec![
        kw("LET"),
        ident("x", &labels),
        ty("Number"),
        num(1.0),
        list(brand, vec![ident("a", &labels)]),
        dict(brand, vec![(ident("k", &labels), ident("v", &labels))]),
        expr(brand, vec![ident("z", &labels)]),
    ];
    for p in &parts {
        let s = format!("{:?}", p);
        assert!(!s.is_empty());
    }
    let e = build(brand, parts);
    assert!(format!("{:?}", e).starts_with("KExpression"));
}
