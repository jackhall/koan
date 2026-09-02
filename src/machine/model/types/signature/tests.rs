use super::*;
use crate::builtins::test_support::kw_part;
use crate::builtins::test_support::probe_symbol;
use crate::builtins::test_support::{type_name, type_token};
use crate::machine::core::{RegionBrand, program_storage};
use crate::machine::model::RunRegistries;
use crate::source::Spanned;

// `KType` leaf constants replace the retired enum variants (`KType::NUMBER` etc.); these tests
// build only ground types, so no registry is needed to name a slot type.

/// A signature is minted into a region, so every builder here takes a brand; program storage is the
/// cheapest one a predicate test can stand up.
fn one_slot(brand: RegionBrand<'_>, kt: KType) -> ExpressionSignature<'_> {
    ExpressionSignature::mint(
        brand,
        ReturnType::Resolved(KType::ANY),
        &[SignatureElement::Argument(Argument::new(
            crate::machine::model::BinderSymbol::classify("v").expect("value token"),
            kt,
        ))],
    )
}

fn expr_with_keyword<'a>(
    brand: RegionBrand<'a>,
    kw: &str,
    registries: &RunRegistries,
) -> KExpression<'a> {
    let symbol = crate::machine::model::KeywordSymbol::declared(kw, &registries.labels)
        .expect("a test fixture keyword is keyword-class");
    KExpression::new(
        brand,
        &[Spanned::bare(
            crate::machine::model::ExpressionPart::Keyword(symbol),
        )],
    )
}

#[test]
fn most_specific_picks_number_over_any() {
    let registries = RunRegistries::new();
    let program = program_storage();
    let brand = program.brand().region();
    let any = one_slot(brand, KType::ANY);
    let num = one_slot(brand, KType::NUMBER);
    let cands: Vec<&ExpressionSignature<'_>> = vec![&any, &num];
    assert_eq!(
        ExpressionSignature::most_specific(&cands, &registries),
        Some(1)
    );
}

#[test]
fn most_specific_returns_none_for_empty() {
    let registries = RunRegistries::new();
    let cands: Vec<&ExpressionSignature<'_>> = Vec::new();
    assert_eq!(
        ExpressionSignature::most_specific(&cands, &registries),
        None
    );
}

#[test]
fn most_specific_returns_none_when_tied() {
    let registries = RunRegistries::new();
    // Ambiguity must surface, not a winner.
    let program = program_storage();
    let brand = program.brand().region();
    let a = one_slot(brand, KType::NUMBER);
    let b = one_slot(brand, KType::NUMBER);
    let cands: Vec<&ExpressionSignature<'_>> = vec![&a, &b];
    assert_eq!(
        ExpressionSignature::most_specific(&cands, &registries),
        None
    );
}

#[test]
fn return_type_clone_round_trips_all_arms() {
    let registries = RunRegistries::new();
    let r = ReturnType::Resolved(KType::NUMBER);
    assert_eq!(r.name(&registries), r.clone().name(&registries));
    let d = ReturnType::Deferred(DeferredReturn::Type(type_token("Er")));
    assert_eq!(d.name(&registries), d.clone().name(&registries));
    let program = program_storage();
    let e = ReturnType::Deferred(DeferredReturn::Expression(expr_with_keyword(
        program.brand().region(),
        "FOO",
        &registries,
    )));
    assert_eq!(e.name(&registries), e.clone().name(&registries));
}

#[test]
fn type_name_eq_compares_leaf_names() {
    let leaf_a = type_token("Aa");
    let leaf_a2 = type_token("Aa");
    let leaf_b = type_token("Bb");
    assert_eq!(leaf_a, leaf_a2);
    assert_ne!(leaf_a, leaf_b);
}

#[test]
fn expression_signature_matches_rejects_length_and_keyword_part_mismatches() {
    let registries = RunRegistries::new();
    let types = &registries.types;
    let program = program_storage();
    let brand = program.brand().region();
    let sig = ExpressionSignature::mint(
        brand,
        ReturnType::Resolved(KType::ANY),
        &[SignatureElement::Keyword(probe_symbol("FOO"))],
    );
    let empty: KExpression<'_> = KExpression::new(brand, &[]);
    assert!(!sig.matches(&empty, types));

    let mismatched = KExpression::new(
        brand,
        &[Spanned::bare(ExpressionPart::Literal(
            crate::machine::model::ast::KLiteral::Number(1.0),
        ))],
    );
    assert!(!sig.matches(&mismatched, types));

    let matching = KExpression::new(brand, &[Spanned::bare(kw_part("FOO"))]);
    assert!(sig.matches(&matching, types));
}

#[test]
fn return_type_debug_renders_both_arms() {
    let r = ReturnType::Resolved(KType::NUMBER);
    assert!(format!("{:?}", r).contains("Resolved"));
    let d = ReturnType::Deferred(DeferredReturn::Type(type_token("Er")));
    assert!(format!("{:?}", d).contains("Deferred"));
}

#[test]
fn deferred_return_debug_renders_both_arms() {
    let t = DeferredReturn::Type(type_token("Er"));
    assert!(format!("{:?}", t).contains("Type"));
    let program = program_storage();
    let registries = RunRegistries::new();
    let e = DeferredReturn::Expression(expr_with_keyword(
        program.brand().region(),
        "FOO",
        &registries,
    ));
    assert!(format!("{:?}", e).contains("Expression"));
}

#[test]
fn return_type_name_covers_all_arms() {
    let registries = RunRegistries::new();
    let r = ReturnType::Resolved(KType::NUMBER);
    assert_eq!(r.name(&registries), KType::NUMBER.name(&registries));
    let t = ReturnType::Deferred(DeferredReturn::Type(type_name("Er", &registries)));
    assert_eq!(t.name(&registries), "Er");
    let program = program_storage();
    let e = ReturnType::Deferred(DeferredReturn::Expression(expr_with_keyword(
        program.brand().region(),
        "FOO",
        &registries,
    )));
    assert_eq!(e.name(&registries), "FOO");
}

fn sig_with<'a>(
    brand: RegionBrand<'a>,
    ret: ReturnType<'a>,
    slot: KType,
) -> ExpressionSignature<'a> {
    ExpressionSignature::mint(
        brand,
        ret,
        &[SignatureElement::Argument(Argument::new(
            crate::machine::model::BinderSymbol::classify("v").expect("value token"),
            slot,
        ))],
    )
}

/// Return types never distinguish overloads: dispatch selects on argument slots alone, so
/// two same-shape signatures differing only in their return — deferred or resolved — are
/// indistinguishable and collide at definition.
#[test]
fn indistinguishable_ignores_return_type() {
    let program = program_storage();
    let brand = program.brand().region();
    let er = sig_with(
        brand,
        ReturnType::Deferred(DeferredReturn::Type(type_token("Er"))),
        KType::NUMBER,
    );
    let ar = sig_with(
        brand,
        ReturnType::Deferred(DeferredReturn::Type(type_token("Ar"))),
        KType::NUMBER,
    );
    assert!(er.indistinguishable_from(&ar));

    let num = sig_with(brand, ReturnType::Resolved(KType::NUMBER), KType::NUMBER);
    let text = sig_with(brand, ReturnType::Resolved(KType::STR), KType::NUMBER);
    assert!(num.indistinguishable_from(&text));
}

#[test]
fn indistinguishable_splits_on_argument_type_and_keywords() {
    let program = program_storage();
    let brand = program.brand().region();
    let num = sig_with(brand, ReturnType::Resolved(KType::ANY), KType::NUMBER);
    let text = sig_with(brand, ReturnType::Resolved(KType::ANY), KType::STR);
    assert!(!num.indistinguishable_from(&text));

    let labels = crate::machine::model::LabelInterner::new();
    let kw = |token: &'static str| {
        ExpressionSignature::mint(
            brand,
            ReturnType::Resolved(KType::ANY),
            &[SignatureElement::keyword(token, &labels)],
        )
    };
    let empty = ExpressionSignature::mint(brand, ReturnType::Resolved(KType::ANY), &[]);
    assert!(kw("FOO").indistinguishable_from(&kw("FOO")));
    assert!(!kw("FOO").indistinguishable_from(&kw("BAR")));
    assert!(!kw("FOO").indistinguishable_from(&num));
    assert!(!kw("FOO").indistinguishable_from(&empty));
}

#[test]
fn return_type_matches_value_deferred_always_true_resolved_delegates() {
    let registries = RunRegistries::new();
    use crate::machine::model::values::KObject;
    let obj = KObject::Number(42.0);
    // Deferred always matches — per-call check runs elsewhere.
    let d = ReturnType::Deferred(DeferredReturn::Type(type_token("Er")));
    assert!(d.matches_value(&obj, &registries));
    assert!(!d.is_resolved());
    let r_num = ReturnType::Resolved(KType::NUMBER);
    assert!(r_num.matches_value(&obj, &registries));
    assert!(r_num.is_resolved());
    let r_bool = ReturnType::Resolved(KType::BOOL);
    assert!(!r_bool.matches_value(&obj, &registries));
}

/// [`DispatchToken`] equality is the stored form of [`ExpressionSignature::indistinguishable_from`]:
/// the write path keys its bucket dedupe on precomputed tokens, so the two must agree on every
/// pair — same shape and same slot types, differing slot types, differing keywords, and differing
/// arity alike.
#[test]
fn dispatch_token_equality_matches_indistinguishable_from() {
    fn keyworded<'a>(
        brand: RegionBrand<'a>,
        keyword: &str,
        slots: &[KType],
    ) -> ExpressionSignature<'a> {
        let mut elements = vec![SignatureElement::Keyword(probe_symbol(keyword))];
        elements.extend(slots.iter().map(|kt| {
            SignatureElement::Argument(Argument::new(
                crate::machine::model::BinderSymbol::classify("v").expect("value token"),
                *kt,
            ))
        }));
        ExpressionSignature::mint(brand, ReturnType::Resolved(KType::ANY), &elements)
    }

    let program = program_storage();
    let brand = program.brand().region();
    let signatures = [
        one_slot(brand, KType::ANY),
        one_slot(brand, KType::NUMBER),
        // Same shape and slot type as the previous, different slot *name* — the predicate is
        // independent of `Argument::name`, so both must call these indistinguishable.
        ExpressionSignature::mint(
            brand,
            ReturnType::Resolved(KType::BOOL),
            &[SignatureElement::Argument(Argument::new(
                crate::machine::model::BinderSymbol::classify("other").expect("value token"),
                KType::NUMBER,
            ))],
        ),
        keyworded(brand, "TAKE", &[KType::NUMBER]),
        keyworded(brand, "TAKE", &[KType::ANY]),
        keyworded(brand, "DROP", &[KType::NUMBER]),
        keyworded(brand, "TAKE", &[KType::NUMBER, KType::NUMBER]),
        ExpressionSignature::mint(brand, ReturnType::Resolved(KType::ANY), &[]),
    ];
    for (i, a) in signatures.iter().enumerate() {
        for (j, b) in signatures.iter().enumerate() {
            assert_eq!(
                a.indistinguishable_from(b),
                a.dispatch_token() == b.dispatch_token(),
                "signatures {i} and {j} disagree between the live predicate and the stored token",
            );
        }
    }
}

/// The probe round-trip through a real table: a map keyed on runs bumped into a region answers an
/// owned probe through the standard `Borrow` blanket, and a same-shape-but-different-content key
/// misses rather than aliasing.
#[test]
fn an_owned_key_probes_a_bumped_run_keyed_table() {
    let program = program_storage();
    let brand = program.brand().region();
    let mut table: hashbrown::HashMap<&[KeyElement], u32> = hashbrown::HashMap::new();

    let take: UntypedKey = vec![
        crate::builtins::test_support::key_keyword("TAKE"),
        KeyElement::Slot,
    ];
    let drop_key: UntypedKey = vec![
        crate::builtins::test_support::key_keyword("DROP"),
        KeyElement::Slot,
    ];
    table.insert(brand.allocator().slice(&take), 7);

    assert_eq!(table.get(take.as_slice()), Some(&7));
    assert_eq!(table.get(drop_key.as_slice()), None);
    // A run bumped into another region probes the same entry: equality is content — a tag and a
    // `u128` per element — not an address.
    let elsewhere = program_storage();
    let restored: &[KeyElement] = elsewhere.brand().region().allocator().slice(&take);
    assert_eq!(table.get(restored), Some(&7));
    assert_eq!(restored.to_vec(), take);
}

/// A dispatch token bumped into a region decides the duplicate-overload predicate exactly as the
/// owned token does — the write path compares against the bumped run and allocates nothing to do it.
#[test]
fn a_bumped_dispatch_token_matches_what_its_owned_form_does() {
    let program = program_storage();
    let brand = program.brand().region();
    fn keyworded<'a>(
        brand: RegionBrand<'a>,
        keyword: &str,
        slots: &[KType],
    ) -> ExpressionSignature<'a> {
        let mut elements = vec![SignatureElement::Keyword(probe_symbol(keyword))];
        elements.extend(slots.iter().map(|kt| {
            SignatureElement::Argument(Argument::new(
                crate::machine::model::BinderSymbol::classify("v").expect("value token"),
                *kt,
            ))
        }));
        ExpressionSignature::mint(brand, ReturnType::Resolved(KType::ANY), &elements)
    }

    let tokens: Vec<DispatchToken> = [
        keyworded(brand, "TAKE", &[KType::NUMBER]),
        keyworded(brand, "TAKE", &[KType::ANY]),
        keyworded(brand, "DROP", &[KType::NUMBER]),
        keyworded(brand, "TAKE", &[KType::NUMBER, KType::NUMBER]),
    ]
    .iter()
    .map(ExpressionSignature::dispatch_token)
    .collect();

    for (i, a) in tokens.iter().enumerate() {
        for (j, b) in tokens.iter().enumerate() {
            assert_eq!(
                a.elements() == b.store_in(brand),
                a == b,
                "tokens {i} and {j} disagree between owned equality and the stored predicate",
            );
        }
    }
}

/// The `DuplicateOverload` text a bucket entry's stored token renders to: keywords resolved
/// through the interner, slot types under the `:`-sigil convention — and a compound slot, whose
/// surface already opens a sigil, is not given a second one.
#[test]
fn a_dispatch_token_renders_its_keywords_and_slot_types() {
    fn keyworded<'a>(
        brand: RegionBrand<'a>,
        keyword: &str,
        slots: &[KType],
        registries: &RunRegistries,
    ) -> ExpressionSignature<'a> {
        let symbol = crate::machine::model::KeywordSymbol::declared(keyword, &registries.labels)
            .expect("a test fixture keyword is keyword-class");
        let mut elements = vec![SignatureElement::Keyword(symbol)];
        elements.extend(slots.iter().map(|kt| {
            SignatureElement::Argument(Argument::new(
                crate::machine::model::BinderSymbol::classify("v").expect("value token"),
                *kt,
            ))
        }));
        ExpressionSignature::mint(brand, ReturnType::Resolved(KType::ANY), &elements)
    }

    let registries = RunRegistries::new();
    let program = program_storage();
    let brand = program.brand().region();

    let leaf = keyworded(brand, "TAKE", &[KType::NUMBER], &registries);
    assert_eq!(
        summarize_dispatch(leaf.dispatch_token().elements(), &registries),
        "fn(TAKE :Number)",
    );

    let list = registries.types.list(KType::NUMBER);
    let compound = keyworded(brand, "TAKE", &[list], &registries);
    assert_eq!(
        summarize_dispatch(compound.dispatch_token().elements(), &registries),
        "fn(TAKE :(LIST OF Number))",
    );

    let two = keyworded(brand, "TAKE", &[KType::NUMBER, KType::ANY], &registries);
    assert_eq!(
        summarize_dispatch(two.dispatch_token().elements(), &registries),
        "fn(TAKE :Number :Any)",
    );
}
