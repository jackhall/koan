//! AST laws: what a node's structural cache says about its parts run, and what survives a copy, a
//! resplice and a rendering. The instance pins that fix a runtime lowering or a rendered spelling
//! live beside the impls they pin, in `machine::model::ast`.
//!
//! Every property here builds nodes in fresh program storage per case, so each runs 64 cases.

use proptest::prelude::*;

#[cfg(feature = "pending_rewrite")]
use crate::machine::model::RunRegistries;
#[cfg(feature = "pending_rewrite")]
use crate::machine::model::ast::working::{WorkingExpression, WorkingPart};
#[cfg(feature = "pending_rewrite")]
use crate::machine::model::types::KType;
#[cfg(feature = "pending_rewrite")]
use crate::machine::model::values::KObject;
use crate::memory::{ProgramBrand, program_storage};
#[cfg(feature = "pending_rewrite")]
use crate::parse::ast::shape::operator_probe_for;
#[cfg(feature = "pending_rewrite")]
use crate::parse::classify_dispatch_shape;
#[cfg(feature = "pending_rewrite")]
use crate::parse::forms::form_for;
use crate::parse::labels::{BinderSymbol, KeywordSymbol, LabelInterner, TypeSymbol, ValueSymbol};
use crate::parse::{DispatchShape, ExpressionPart, KExpression, KLiteral, KeyElement, PartClass};
use crate::source::Spanned;

/// The string literals a generated run draws from — `&'static str` coerces into any node's region,
/// so a literal part needs no allocation of its own.
const TEXTS: &[&str] = &["", "hi", "a b"];

/// A part to build, as the generator produces it. Structural equality of two shapes is exactly
/// structural equality of the parts they build, which is what makes it the oracle for that law:
/// a number carries an integer, so two shapes compare exactly when their literals do.
#[derive(Clone, Debug, PartialEq)]
enum PartShape {
    Keyword(String),
    Identifier(String),
    Type(String),
    Number(i32),
    Text(usize),
    Boolean(bool),
    Null,
    Nested(Vec<PartShape>),
    Sigil(Vec<PartShape>),
    RecordType(Vec<PartShape>),
    Quote(Vec<PartShape>),
    List(Vec<PartShape>),
    Dict(Vec<(PartShape, PartShape)>),
    Record(Vec<(String, PartShape)>),
}

fn keyword_spelling() -> impl Strategy<Value = String> {
    prop_oneof!["[A-Z]{2,4}", "[-+*/<>|]{1,2}"]
}

fn value_spelling() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,4}".prop_map(String::from)
}

fn type_spelling() -> impl Strategy<Value = String> {
    "[A-Z][a-z][A-Za-z]{0,3}".prop_map(String::from)
}

/// One part, to a bounded nesting depth.
fn part_shape() -> impl Strategy<Value = PartShape> {
    let leaf = prop_oneof![
        keyword_spelling().prop_map(PartShape::Keyword),
        value_spelling().prop_map(PartShape::Identifier),
        type_spelling().prop_map(PartShape::Type),
        (-9i32..9).prop_map(PartShape::Number),
        (0usize..TEXTS.len()).prop_map(PartShape::Text),
        any::<bool>().prop_map(PartShape::Boolean),
        Just(PartShape::Null),
    ];
    leaf.prop_recursive(3, 16, 3, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 1..3).prop_map(PartShape::Nested),
            prop::collection::vec(inner.clone(), 1..3).prop_map(PartShape::Sigil),
            prop::collection::vec(inner.clone(), 1..3).prop_map(PartShape::RecordType),
            prop::collection::vec(inner.clone(), 1..3).prop_map(PartShape::Quote),
            prop::collection::vec(inner.clone(), 0..3).prop_map(PartShape::List),
            prop::collection::vec((inner.clone(), inner.clone()), 0..2).prop_map(PartShape::Dict),
            prop::collection::vec((value_spelling(), inner), 0..2).prop_map(PartShape::Record),
        ]
    })
}

/// A run drawn from the whole part vocabulary, plus two shapes the vocabulary alone rarely spells:
/// the `Slot (Keyword Slot)+` operator chain and a keyword-led builtin-like run.
fn parts_run() -> impl Strategy<Value = Vec<PartShape>> {
    prop_oneof![
        3 => prop::collection::vec(part_shape(), 0..6),
        1 => chain_run(),
        1 => keyword_led_run(),
    ]
}

/// `Slot (Keyword Slot)+` — one more operand than operator.
fn chain_run() -> impl Strategy<Value = Vec<PartShape>> {
    prop::collection::vec(keyword_spelling(), 1..4).prop_flat_map(|operators| {
        prop::collection::vec(part_shape(), operators.len() + 1).prop_map(move |operands| {
            let mut run = Vec::new();
            for (index, operand) in operands.into_iter().enumerate() {
                if index > 0 {
                    run.push(PartShape::Keyword(operators[index - 1].clone()));
                }
                run.push(operand);
            }
            run
        })
    })
}

/// A keyword-led run: a lead keyword, then a mix of slots and keywords.
fn keyword_led_run() -> impl Strategy<Value = Vec<PartShape>> {
    (
        keyword_spelling(),
        prop::collection::vec(part_shape(), 1..5),
    )
        .prop_map(|(lead, tail)| {
            let mut run = vec![PartShape::Keyword(lead)];
            run.extend(tail);
            run
        })
}

/// Build the part `shape` names into `brand`'s program storage, recording every spelling in
/// `labels` so a rendering resolves it.
fn build_part<'a>(
    brand: ProgramBrand<'a>,
    shape: &PartShape,
    labels: &LabelInterner,
) -> ExpressionPart<'a> {
    let allocator = brand.region().allocator();
    match shape {
        PartShape::Keyword(text) => ExpressionPart::Keyword(
            KeywordSymbol::declared(text, labels).expect("keyword-class by construction"),
        ),
        PartShape::Identifier(text) => ExpressionPart::Identifier(
            ValueSymbol::declared(text, labels).expect("a value token by construction"),
        ),
        PartShape::Type(text) => ExpressionPart::Type(
            TypeSymbol::declared(text, labels).expect("a Type token by construction"),
        ),
        PartShape::Number(n) => ExpressionPart::Literal(KLiteral::Number(f64::from(*n))),
        PartShape::Text(index) => ExpressionPart::Literal(KLiteral::String(TEXTS[*index])),
        PartShape::Boolean(b) => ExpressionPart::Literal(KLiteral::Boolean(*b)),
        PartShape::Null => ExpressionPart::Literal(KLiteral::Null),
        PartShape::Nested(items) => {
            ExpressionPart::Expression(brand.nested_node_from_iter(build_run(brand, items, labels)))
        }
        PartShape::Sigil(items) => ExpressionPart::SigiledTypeExpr(
            brand.nested_node_from_iter(build_run(brand, items, labels)),
        ),
        PartShape::RecordType(items) => {
            ExpressionPart::RecordType(brand.nested_node_from_iter(build_run(brand, items, labels)))
        }
        PartShape::Quote(items) => ExpressionPart::QuotedExpression(
            brand.nested_node_from_iter(build_run(brand, items, labels)),
        ),
        PartShape::List(items) => ExpressionPart::ListLiteral(
            allocator.slice_from_iter(items.iter().map(|item| build_part(brand, item, labels))),
        ),
        PartShape::Dict(pairs) => ExpressionPart::DictLiteral(allocator.slice_from_iter(
            pairs.iter().map(|(key, value)| {
                (
                    build_part(brand, key, labels),
                    build_part(brand, value, labels),
                )
            }),
        )),
        PartShape::Record(fields) => ExpressionPart::RecordLiteral(allocator.slice_from_iter(
            fields.iter().map(|(name, value)| {
                (
                    BinderSymbol::declared(name, labels).expect("a value token by construction"),
                    build_part(brand, value, labels),
                )
            }),
        )),
    }
}

/// The spanless parts run `shapes` names.
fn build_run<'a>(
    brand: ProgramBrand<'a>,
    shapes: &[PartShape],
    labels: &LabelInterner,
) -> Vec<Spanned<ExpressionPart<'a>>> {
    shapes
        .iter()
        .map(|shape| Spanned::bare(build_part(brand, shape, labels)))
        .collect()
}

/// The node `shapes` names, frozen through the construction door.
fn build<'a>(
    brand: ProgramBrand<'a>,
    shapes: &[PartShape],
    labels: &LabelInterner,
) -> KExpression<'a> {
    KExpression::new_from_iter(brand.region(), build_run(brand, shapes, labels))
}

/// The bucket key a parts run spells, recomputed from the parts rather than read off the cache.
#[cfg(feature = "pending_rewrite")]
fn recomputed_key(parts: &[Spanned<ExpressionPart<'_>>]) -> Vec<KeyElement> {
    parts
        .iter()
        .map(|part| match part.value {
            ExpressionPart::Keyword(symbol) => KeyElement::Keyword(symbol),
            _ => KeyElement::Slot,
        })
        .collect()
}

/// The dispatch shape a run with key `key` and head class `head` must take, written from the rule
/// the design states rather than read off the classifier: keywords decide first, and only a
/// keyword-free run branches on its head.
fn expected_shape(key: &[KeyElement], head: Option<PartClass>) -> DispatchShape {
    if key
        .iter()
        .any(|element| matches!(element, KeyElement::Keyword(_)))
    {
        return if is_chain_key(key) {
            DispatchShape::OperatorChain
        } else {
            DispatchShape::Keyworded
        };
    }
    let Some(head) = head else {
        return DispatchShape::NonCallableHead;
    };
    let single = key.len() == 1;
    match (head, single) {
        (PartClass::Identifier | PartClass::StagedSlot, true) => DispatchShape::BareIdentifier,
        (PartClass::Type, true) => DispatchShape::BareTypeLeaf,
        (PartClass::SigiledTypeExpr, true) => DispatchShape::SigiledTypeExpr,
        (PartClass::RecordType, true) => DispatchShape::RecordType,
        (_, true) => DispatchShape::LiteralPassThrough,
        (PartClass::Type, false) => DispatchShape::TypeCall,
        (PartClass::Identifier | PartClass::StagedSlot, false) => DispatchShape::FunctionValueCall,
        (PartClass::Expression, false) => DispatchShape::HeadDeferred,
        (PartClass::SigiledTypeExpr, false) => DispatchShape::TypeHeadDeferred,
        (_, false) => DispatchShape::NonCallableHead,
    }
}

/// `Slot (Keyword Slot)+` with two or more keyword positions, stated as the pattern rather than as
/// a parity arithmetic: every even index a slot, every odd index a keyword, odd length ≥ 5.
fn is_chain_key(key: &[KeyElement]) -> bool {
    let keywords = key
        .iter()
        .filter(|element| matches!(element, KeyElement::Keyword(_)))
        .count();
    keywords >= 2
        && key.len() == keywords * 2 + 1
        && key
            .iter()
            .enumerate()
            .all(|(index, element)| matches!(element, KeyElement::Keyword(_)) == (index % 2 == 1))
}

/// Another eager part of a different variant, for the interchangeability clause: every eager part
/// contributes `Slot`, so which one fills a non-head position changes nothing structural.
fn other_eager_shape(shape: &PartShape) -> PartShape {
    match shape {
        PartShape::Number(_) => PartShape::List(vec![PartShape::Null]),
        _ => PartShape::Number(7),
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// The dispatch shape is a function of the stored key and the head part's class, and of nothing
    /// else: it matches the rule stated independently here, `Keyworded` appears only when the run
    /// spells a keyword, the chain shape appears exactly on the alternating slot-led key, and
    /// swapping one eager part for an eager part of another variant at a non-head position leaves
    /// key and shape alone — which is what lets a splice write a resolved sub-result into a slot
    /// without reclassifying.
    #[test]
    fn the_shape_is_a_function_of_the_key_and_the_head_class(shapes in parts_run()) {
        let program = program_storage();
        let brand = program.brand();
        let labels = LabelInterner::new();
        let expression = build(brand, &shapes, &labels);

        let key = expression.stored_key();
        let head = expression.parts.first().map(|part| part.value.class());
        prop_assert_eq!(expression.shape(), expected_shape(key, head));

        let has_keyword = key.iter().any(|e| matches!(e, KeyElement::Keyword(_)));
        prop_assert_eq!(expression.shape() == DispatchShape::Keyworded, has_keyword && !is_chain_key(key));
        prop_assert_eq!(expression.shape() == DispatchShape::OperatorChain, is_chain_key(key));

        for index in 1..shapes.len() {
            if matches!(shapes[index], PartShape::Keyword(_)) {
                continue;
            }
            let mut swapped = shapes.clone();
            swapped[index] = other_eager_shape(&shapes[index]);
            let other = build(brand, &swapped, &labels);
            prop_assert_eq!(other.stored_key(), expression.stored_key());
            prop_assert_eq!(other.shape(), expression.shape());
            prop_assert_eq!(other.operator_probe(), expression.operator_probe());
        }
    }

    /// The cache is what a fresh recompute says, it rides a copy whole, and it rides a resplice
    /// whole — the key as the very run construction bumped, not merely an equal one, so a chain
    /// splicing once per reduction step bumps no duplicate. The type-context stamp rides with it,
    /// for the same reason: a splice substitutes slots and does not change how the node was reached.
    #[cfg(feature = "pending_rewrite")]
    #[test]
    fn the_cache_agrees_with_a_recompute_and_rides_a_copy_and_a_resplice(shapes in parts_run()) {
        let program = program_storage();
        let brand = program.brand();
        let region = brand.region();
        let labels = LabelInterner::new();
        let expression = build(brand, &shapes, &labels);

        let head = expression.parts.first().map(|part| part.value.class());
        prop_assert_eq!(expression.stored_key().to_vec(), recomputed_key(expression.parts));
        prop_assert_eq!(
            expression.shape(),
            classify_dispatch_shape(expression.stored_key(), head),
        );
        prop_assert_eq!(
            expression.operator_probe(),
            operator_probe_for(expression.stored_key(), expression.shape()),
        );
        prop_assert_eq!(
            expression.cache().form().map(|form| form.id),
            form_for(expression.stored_key().iter().copied()).map(|form| form.id),
        );

        let copy = expression;
        prop_assert!(std::ptr::eq(copy.stored_key(), expression.stored_key()));
        prop_assert_eq!(copy.shape(), expression.shape());
        prop_assert_eq!(copy.operator_probe(), expression.operator_probe());
        prop_assert_eq!(copy.binder_name_slot(), expression.binder_name_slot());

        // The splice shape: every eager slot gives way to a staging hole, every keyword stands.
        let working = WorkingExpression::from_ast(region, expression).in_type_context();
        let respliced = working.respliced(
            region,
            working.parts.iter().map(|part| Spanned {
                value: match part.value {
                    WorkingPart::Ast(ExpressionPart::Keyword(_)) => part.value,
                    _ => WorkingPart::StagedSlot,
                },
                span: part.span,
            }),
        );
        prop_assert!(std::ptr::eq(working.stored_key(), respliced.stored_key()));
        prop_assert_eq!(working.operator_probe(), respliced.operator_probe());
        prop_assert_eq!(
            working.cache().form().map(|form| form.id),
            respliced.cache().form().map(|form| form.id),
        );
        prop_assert!(respliced.under_type_sigil());
    }

    /// A node's bucket key and the untyped key of a signature spelling the same pattern agree —
    /// the invariant a registration and a call meet under, with keywords in position and every
    /// argument a slot.
    #[cfg(feature = "pending_rewrite")]
    #[test]
    fn a_node_key_equals_the_signature_key_of_the_same_pattern(shapes in parts_run()) {
        use crate::machine::model::types::{Argument, ReturnType, SignatureDraft, SignatureElement};

        let program = program_storage();
        let brand = program.brand();
        let labels = LabelInterner::new();
        let expression = build(brand, &shapes, &labels);

        let draft = SignatureDraft {
            return_type: ReturnType::Resolved(KType::ANY),
            elements: shapes
                .iter()
                .map(|shape| match shape {
                    PartShape::Keyword(text) => SignatureElement::Keyword(
                        KeywordSymbol::of(text).expect("keyword-class by construction"),
                    ),
                    _ => SignatureElement::Argument(Argument::new(
                        BinderSymbol::classify("slot").expect("a value token"),
                        KType::ANY,
                    )),
                })
                .collect(),
        };
        prop_assert_eq!(expression.stored_key().to_vec(), draft.untyped_key());
    }

    /// A node's rendering is the space-join of its parts' own renderings, so a diagnostic naming a
    /// whole expression and one naming a single part agree about every token.
    #[test]
    fn a_summary_is_the_space_join_of_its_part_summaries(shapes in parts_run()) {
        let program = program_storage();
        let brand = program.brand();
        let labels = LabelInterner::new();
        let expression = build(brand, &shapes, &labels);

        let joined = expression
            .parts
            .iter()
            .map(|part| part.value.summarize(&labels))
            .collect::<Vec<_>>()
            .join(" ");
        prop_assert_eq!(expression.summarize(&labels), joined);
    }

    /// Quoted code compares as syntax: two nodes are structurally equal exactly when they spell the
    /// same part sequence, with literals compared by their written form and container literals
    /// compared in order.
    #[cfg(feature = "pending_rewrite")]
    #[test]
    fn structural_equality_is_the_same_part_sequence(
        left in parts_run(),
        right in parts_run(),
    ) {
        let program = program_storage();
        let brand = program.brand();
        let registries = RunRegistries::new();
        let labels = &registries.labels;

        let make = |shapes: &[PartShape]| {
            KObject::KExpression(brand.new_expression_from_iter(build_run(brand, shapes, labels)))
        };
        let a = make(&left);
        let b = make(&left);
        let c = make(&right);

        prop_assert_eq!(a.value_equal(&b, &registries), Ok(true));
        prop_assert_eq!(a.value_equal(&c, &registries), Ok(left == right));
        prop_assert_eq!(a.ktype(), KType::KEXPRESSION);
    }
}
