use std::hash::BuildHasher;

use super::*;
use crate::machine::core::{program_storage, RegionBrand};
use crate::source::Spanned;

// `KType` leaf constants replace the retired enum variants (`KType::NUMBER` etc.); these tests
// build only ground types, so no registry is needed to name a slot type.

/// A signature is minted into a region, so every builder here takes a brand; program storage is the
/// cheapest one a predicate test can stand up.
fn one_slot(brand: RegionBrand<'_>, kt: KType) -> ExpressionSignature<'_> {
    ExpressionSignature::mint(
        brand,
        SignatureDraft {
            return_type: ReturnType::Resolved(KType::ANY),
            elements: vec![SignatureElement::Argument(Argument {
                name: "v",
                ktype: kt,
            })],
        },
    )
}

fn expr_with_keyword<'a>(brand: RegionBrand<'a>, kw: &'a str) -> KExpression<'a> {
    KExpression::new(brand, vec![Spanned::bare(ExpressionPart::Keyword(kw))])
}

#[test]
fn most_specific_picks_number_over_any() {
    let types = TypeRegistry::new();
    let program = program_storage();
    let brand = program.brand().region();
    let any = one_slot(brand, KType::ANY);
    let num = one_slot(brand, KType::NUMBER);
    let cands: Vec<&ExpressionSignature<'_>> = vec![&any, &num];
    assert_eq!(ExpressionSignature::most_specific(&cands, &types), Some(1));
}

#[test]
fn most_specific_returns_none_for_empty() {
    let types = TypeRegistry::new();
    let cands: Vec<&ExpressionSignature<'_>> = Vec::new();
    assert_eq!(ExpressionSignature::most_specific(&cands, &types), None);
}

#[test]
fn most_specific_returns_none_when_tied() {
    let types = TypeRegistry::new();
    // Ambiguity must surface, not a winner.
    let program = program_storage();
    let brand = program.brand().region();
    let a = one_slot(brand, KType::NUMBER);
    let b = one_slot(brand, KType::NUMBER);
    let cands: Vec<&ExpressionSignature<'_>> = vec![&a, &b];
    assert_eq!(ExpressionSignature::most_specific(&cands, &types), None);
}

#[test]
fn return_type_clone_round_trips_all_arms() {
    let types = TypeRegistry::new();
    let r = ReturnType::Resolved(KType::NUMBER);
    assert_eq!(r.name(&types), r.clone().name(&types));
    let d = ReturnType::Deferred(DeferredReturn::Type(TypeIdentifier::leaf("er")));
    assert_eq!(d.name(&types), d.clone().name(&types));
    let program = program_storage();
    let e = ReturnType::Deferred(DeferredReturn::Expression(expr_with_keyword(
        program.brand().region(),
        "FOO",
    )));
    assert_eq!(e.name(&types), e.clone().name(&types));
}

#[test]
fn type_name_eq_compares_leaf_names() {
    let leaf_a = TypeIdentifier::leaf("A");
    let leaf_a2 = TypeIdentifier::leaf("A");
    let leaf_b = TypeIdentifier::leaf("B");
    assert_eq!(leaf_a, leaf_a2);
    assert_ne!(leaf_a, leaf_b);
}

#[test]
fn expression_signature_matches_rejects_length_and_keyword_part_mismatches() {
    let types = TypeRegistry::new();
    let program = program_storage();
    let brand = program.brand().region();
    let sig = ExpressionSignature::mint(
        brand,
        SignatureDraft {
            return_type: ReturnType::Resolved(KType::ANY),
            elements: vec![SignatureElement::Keyword("FOO")],
        },
    );
    let empty: KExpression<'_> = KExpression::new(brand, vec![]);
    assert!(!sig.matches(&empty, &types));

    let mismatched = KExpression::new(
        brand,
        vec![Spanned::bare(ExpressionPart::Literal(
            crate::machine::model::ast::KLiteral::Number(1.0),
        ))],
    );
    assert!(!sig.matches(&mismatched, &types));

    let matching = KExpression::new(brand, vec![Spanned::bare(ExpressionPart::Keyword("FOO"))]);
    assert!(sig.matches(&matching, &types));
}

#[test]
fn return_type_debug_renders_both_arms() {
    let r = ReturnType::Resolved(KType::NUMBER);
    assert!(format!("{:?}", r).contains("Resolved"));
    let d = ReturnType::Deferred(DeferredReturn::Type(TypeIdentifier::leaf("er")));
    assert!(format!("{:?}", d).contains("Deferred"));
}

#[test]
fn deferred_return_debug_renders_both_arms() {
    let t = DeferredReturn::Type(TypeIdentifier::leaf("er"));
    assert!(format!("{:?}", t).contains("Type"));
    let program = program_storage();
    let e = DeferredReturn::Expression(expr_with_keyword(program.brand().region(), "FOO"));
    assert!(format!("{:?}", e).contains("Expression"));
}

#[test]
fn return_type_name_covers_all_arms() {
    let types = TypeRegistry::new();
    let r = ReturnType::Resolved(KType::NUMBER);
    assert_eq!(r.name(&types), KType::NUMBER.name(&types));
    let t = ReturnType::Deferred(DeferredReturn::Type(TypeIdentifier::leaf("er")));
    assert_eq!(t.name(&types), "er");
    let program = program_storage();
    let e = ReturnType::Deferred(DeferredReturn::Expression(expr_with_keyword(
        program.brand().region(),
        "FOO",
    )));
    assert_eq!(e.name(&types), "FOO");
}

fn sig_with<'a>(
    brand: RegionBrand<'a>,
    ret: ReturnType<'a>,
    slot: KType,
) -> ExpressionSignature<'a> {
    ExpressionSignature::mint(
        brand,
        SignatureDraft {
            return_type: ret,
            elements: vec![SignatureElement::Argument(Argument {
                name: "v",
                ktype: slot,
            })],
        },
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
        ReturnType::Deferred(DeferredReturn::Type(TypeIdentifier::leaf("er"))),
        KType::NUMBER,
    );
    let ar = sig_with(
        brand,
        ReturnType::Deferred(DeferredReturn::Type(TypeIdentifier::leaf("Ar"))),
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

    let kw = |token: &'static str| {
        ExpressionSignature::mint(
            brand,
            SignatureDraft {
                return_type: ReturnType::Resolved(KType::ANY),
                elements: vec![SignatureElement::Keyword(token)],
            },
        )
    };
    let empty = ExpressionSignature::mint(
        brand,
        SignatureDraft {
            return_type: ReturnType::Resolved(KType::ANY),
            elements: vec![],
        },
    );
    assert!(kw("FOO").indistinguishable_from(&kw("FOO")));
    assert!(!kw("FOO").indistinguishable_from(&kw("BAR")));
    assert!(!kw("FOO").indistinguishable_from(&num));
    assert!(!kw("FOO").indistinguishable_from(&empty));
}

#[test]
fn return_type_matches_value_deferred_always_true_resolved_delegates() {
    let types = TypeRegistry::new();
    use crate::machine::model::values::KObject;
    let obj = KObject::Number(42.0);
    // Deferred always matches — per-call check runs elsewhere.
    let d = ReturnType::Deferred(DeferredReturn::Type(TypeIdentifier::leaf("er")));
    assert!(d.matches_value(&obj, &types));
    assert!(!d.is_resolved());
    let r_num = ReturnType::Resolved(KType::NUMBER);
    assert!(r_num.matches_value(&obj, &types));
    assert!(r_num.is_resolved());
    let r_bool = ReturnType::Resolved(KType::BOOL);
    assert!(!r_bool.matches_value(&obj, &types));
}

/// [`DispatchToken`] equality is the stored form of [`ExpressionSignature::indistinguishable_from`]:
/// the write path keys its bucket dedupe on precomputed tokens, so the two must agree on every
/// pair — same shape and same slot types, differing slot types, differing keywords, and differing
/// arity alike.
#[test]
fn dispatch_token_equality_matches_indistinguishable_from() {
    fn keyworded<'a>(
        brand: RegionBrand<'a>,
        keyword: &'a str,
        slots: &[KType],
    ) -> ExpressionSignature<'a> {
        let mut elements = vec![SignatureElement::Keyword(keyword)];
        elements.extend(slots.iter().map(|kt| {
            SignatureElement::Argument(Argument {
                name: "v",
                ktype: *kt,
            })
        }));
        ExpressionSignature::mint(
            brand,
            SignatureDraft {
                return_type: ReturnType::Resolved(KType::ANY),
                elements,
            },
        )
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
            SignatureDraft {
                return_type: ReturnType::Resolved(KType::BOOL),
                elements: vec![SignatureElement::Argument(Argument {
                    name: "other",
                    ktype: KType::NUMBER,
                })],
            },
        ),
        keyworded(brand, "TAKE", &[KType::NUMBER]),
        keyworded(brand, "TAKE", &[KType::ANY]),
        keyworded(brand, "DROP", &[KType::NUMBER]),
        keyworded(brand, "TAKE", &[KType::NUMBER, KType::NUMBER]),
        ExpressionSignature::mint(
            brand,
            SignatureDraft {
                return_type: ReturnType::Resolved(KType::ANY),
                elements: vec![],
            },
        ),
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

/// **The tripwire for the two key forms.** The `functions` table is keyed on stored runs and probed
/// with owned ones, so if the two hashing schemes ever drift a probe lands in the wrong bucket and
/// every lookup silently misses — no type error, no panic, just a language that forgets its
/// functions. Assert the hashes agree directly, under the very hasher the tables use.
#[test]
fn owned_and_stored_key_forms_hash_identically() {
    let program = program_storage();
    let brand = program.brand().region();
    let build = hashbrown::DefaultHashBuilder::default();

    for owned in [
        vec![],
        vec![UntypedElement::Slot],
        vec![UntypedElement::Keyword("TAKE".to_string())],
        vec![
            UntypedElement::Keyword("MAKESET".to_string()),
            UntypedElement::Slot,
            UntypedElement::Keyword("USING".to_string()),
            UntypedElement::Slot,
        ],
        // Prefix-of-another and empty-keyword cases, where a missing length prefix or a missing
        // arm tag would collide.
        vec![
            UntypedElement::Keyword("MAKESET".to_string()),
            UntypedElement::Slot,
        ],
        vec![UntypedElement::Keyword(String::new())],
    ] {
        let stored = store_untyped_key(brand, &owned);
        assert_eq!(
            build.hash_one(UntypedKeyProbe(&owned)),
            build.hash_one(stored),
            "key {owned:?} hashes differently in its owned and stored forms",
        );
    }
}

/// The probe round-trip through a real table: a stored-run-keyed map answers an owned probe, and a
/// same-shape-but-different-content key misses rather than aliasing.
#[test]
fn an_owned_key_probes_a_stored_run_keyed_table() {
    let program = program_storage();
    let brand = program.brand().region();
    let mut table: hashbrown::HashMap<&[StoredElement<'_>], u32> = hashbrown::HashMap::new();

    let take: UntypedKey = vec![
        UntypedElement::Keyword("TAKE".to_string()),
        UntypedElement::Slot,
    ];
    let drop_key: UntypedKey = vec![
        UntypedElement::Keyword("DROP".to_string()),
        UntypedElement::Slot,
    ];
    table.insert(store_untyped_key(brand, &take), 7);

    assert_eq!(table.get(&UntypedKeyProbe(&take)), Some(&7));
    assert_eq!(table.get(&UntypedKeyProbe(&drop_key)), None);
    // A run stored in another region probes the same entry: the blanket `Equivalent` compares
    // element-wise, so key identity is content, not address.
    let elsewhere = program_storage();
    let restored = restore_stored_key(elsewhere.brand().region(), store_untyped_key(brand, &take));
    assert_eq!(table.get(&restored), Some(&7));
    assert_eq!(owned_untyped_key(restored), take);
}

/// A stored dispatch token decides the duplicate-overload predicate exactly as the owned token
/// does — the write path compares against the stored run and allocates nothing to do it.
#[test]
fn a_stored_dispatch_token_matches_what_its_owned_form_does() {
    let program = program_storage();
    let brand = program.brand().region();
    fn keyworded<'a>(
        brand: RegionBrand<'a>,
        keyword: &'a str,
        slots: &[KType],
    ) -> ExpressionSignature<'a> {
        let mut elements = vec![SignatureElement::Keyword(keyword)];
        elements.extend(slots.iter().map(|kt| {
            SignatureElement::Argument(Argument {
                name: "v",
                ktype: *kt,
            })
        }));
        ExpressionSignature::mint(
            brand,
            SignatureDraft {
                return_type: ReturnType::Resolved(KType::ANY),
                elements,
            },
        )
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
                a.matches_stored(b.store_in(brand)),
                a == b,
                "tokens {i} and {j} disagree between owned equality and the stored predicate",
            );
        }
    }
}
