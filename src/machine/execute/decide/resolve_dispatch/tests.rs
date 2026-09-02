//! Per-slot admission over union carrier slots: the routing
//! [`slot_admits_strict`] does when a slot type lists more than one carrier spelling, and what
//! [`relaxed_admits`] leans on when it rejects.

use super::*;
use crate::builtins::test_support::{type_token, value_name};
use crate::machine::core::{FrameStorageExt, program_storage, run_root_storage};
use crate::machine::model::labels::BinderSymbol;
use crate::machine::model::{Argument, KLiteral, RunRegistries};

/// The workhorse union: every carrier spelling of a type slot plus the value-name token.
fn carrier_union(registries: &RunRegistries) -> KType {
    registries.types.union_of(&[
        KType::TYPE_NAME_TOKEN,
        KType::SIGILED_TYPE_EXPR,
        KType::RECORD_TYPE,
        KType::IDENTIFIER,
    ])
}

fn slot_of(ktype: KType) -> SignatureElement {
    SignatureElement::Argument(Argument {
        name: BinderSymbol::classify("operand").expect("a fixture parameter is a value token"),
        ktype,
    })
}

/// Strict admission of a single AST part at a single slot, with the given bare-name outcome.
fn admits_with(
    registries: &RunRegistries,
    ktype: KType,
    part: ExpressionPart<'_>,
    outcome: Option<Resolution>,
) -> bool {
    slot_admits_strict(
        &slot_of(ktype),
        &WorkingPart::Ast(part),
        0,
        &[outcome],
        registries,
    )
}

/// A raw `:(…)` and a raw `:{…}` each strict-admit at the workhorse union through their own
/// member — the shape-only early return a bare carrier slot takes, distributed.
#[test]
fn a_raw_type_expression_strict_admits_at_a_union_carrier_slot() {
    let program = program_storage();
    let brand = program.brand();
    let registries = RunRegistries::new();
    let slot = carrier_union(&registries);

    let sigil = ExpressionPart::SigiledTypeExpr(brand.nested_node_from_iter(std::iter::empty()));
    let record = ExpressionPart::RecordType(brand.nested_node_from_iter(std::iter::empty()));
    assert!(admits_with(&registries, slot, sigil, None));
    assert!(admits_with(&registries, slot, record, None));
}

/// A bare `Type` token strict-admits at a union with a `TypeNameToken` member even when the name
/// resolves to nothing: the member captures the token raw, so admission never consults the
/// bare-name outcome. Without the member, the same part rejects on the `Unbound` outcome.
#[test]
fn a_fresh_type_token_strict_admits_through_its_name_member() {
    let registries = RunRegistries::new();
    let token = type_token("Meters");
    let unbound = || Some(Resolution::Unbound(BinderSymbol::Type(token)));

    assert!(admits_with(
        &registries,
        carrier_union(&registries),
        ExpressionPart::Type(token),
        unbound(),
    ));
    let no_name_member = registries
        .types
        .union_of(&[KType::SIGILED_TYPE_EXPR, KType::RECORD_TYPE]);
    assert!(!admits_with(
        &registries,
        no_name_member,
        ExpressionPart::Type(token),
        unbound(),
    ));
}

/// The speculative-eager guard distributes: a union that captures one type-expression kind raw
/// must not speculatively eat the other, or the sibling overload holding the raw capture ties
/// away and the eager fallback wins.
#[test]
fn the_speculative_guard_distributes_over_union_members() {
    let program = program_storage();
    let brand = program.brand();
    let registries = RunRegistries::new();
    let sigil = || ExpressionPart::SigiledTypeExpr(brand.nested_node_from_iter(std::iter::empty()));
    let record = || ExpressionPart::RecordType(brand.nested_node_from_iter(std::iter::empty()));

    let with_sigil = registries
        .types
        .union_of(&[KType::TYPE_NAME_TOKEN, KType::SIGILED_TYPE_EXPR]);
    assert!(!admits_with(&registries, with_sigil, record(), None));
    assert!(admits_with(&registries, with_sigil, sigil(), None));

    let with_record = registries
        .types
        .union_of(&[KType::TYPE_NAME_TOKEN, KType::RECORD_TYPE]);
    assert!(!admits_with(&registries, with_record, sigil(), None));
    assert!(admits_with(&registries, with_record, record(), None));

    // A union claiming neither speculatively admits both, exactly as a bare value slot does.
    let neither = registries
        .types
        .union_of(&[KType::TYPE_NAME_TOKEN, KType::NUMBER]);
    assert!(admits_with(&registries, neither, sigil(), None));
    assert!(admits_with(&registries, neither, record(), None));
}

/// A bare carrier slot keeps its exact-constant behaviour: the distribution is a widening, not a
/// change of verdict for the spellings that already worked.
#[test]
fn bare_carrier_slots_keep_their_verdicts() {
    let program = program_storage();
    let brand = program.brand();
    let registries = RunRegistries::new();
    let sigil = || ExpressionPart::SigiledTypeExpr(brand.nested_node_from_iter(std::iter::empty()));
    let record = || ExpressionPart::RecordType(brand.nested_node_from_iter(std::iter::empty()));

    assert!(admits_with(
        &registries,
        KType::SIGILED_TYPE_EXPR,
        sigil(),
        None
    ));
    assert!(!admits_with(
        &registries,
        KType::SIGILED_TYPE_EXPR,
        record(),
        None
    ));
    assert!(!admits_with(&registries, KType::RECORD_TYPE, sigil(), None));
    assert!(!admits_with(&registries, KType::KEXPRESSION, sigil(), None));
    assert!(admits_with(&registries, KType::PROPER_TYPE, sigil(), None));
}

/// A part no member claims is no member's business: a literal falls through to the ordinary
/// value walk, which the union already distributes over.
#[test]
fn an_unclaimed_part_falls_through_to_the_value_walk() {
    let registries = RunRegistries::new();
    let number_union = registries
        .types
        .union_of(&[KType::TYPE_NAME_TOKEN, KType::NUMBER]);
    assert!(admits_with(
        &registries,
        number_union,
        ExpressionPart::Literal(KLiteral::Number(1.0)),
        None,
    ));
    // The workhorse union lists no value member, so the same literal finds nothing to admit it.
    assert!(!admits_with(
        &registries,
        carrier_union(&registries),
        ExpressionPart::Literal(KLiteral::Number(1.0)),
        None,
    ));
}

/// Relaxed admission leans on the parked producer at a union slot with no member claiming the
/// part: the slot strict-rejects, and the relaxed pass reports the wait rather than a hard miss.
#[test]
fn relaxed_admission_leans_on_a_parked_name_at_a_union_slot() {
    let registries = RunRegistries::new();
    let region = run_root_storage();
    let brand = region.brand();
    let scratch = brand.allocator();

    let part = ExpressionPart::Identifier(value_name("width", &registries));
    let expr = WorkingExpression::from_ast(
        brand,
        crate::machine::model::KExpression::new(brand, &[crate::source::Spanned::bare(part)]),
    );
    // No `Identifier` member, so the bare name must resolve — and its producer has yet to land.
    let slot = registries
        .types
        .union_of(&[KType::TYPE_NAME_TOKEN, KType::NUMBER]);
    let sig = ExpressionSignature::mint(
        brand,
        crate::machine::model::ReturnType::Resolved(KType::ANY),
        &[slot_of(slot)],
    );
    let producer = crate::machine::ProducerId::for_test(7);
    let leans = relaxed_admits(
        &sig,
        &expr,
        &[Some(Resolution::Parked(producer))],
        &registries,
        scratch,
    )
    .expect("a parked slot is relaxed-satisfiable");
    assert!(matches!(&*leans, [Lean::Parked(p)] if *p == producer));

    // With an `Identifier` member the slot captures the token raw, so strict admits outright and
    // the relaxed pass leans on nothing.
    let owning = carrier_union(&registries);
    let sig = ExpressionSignature::mint(
        brand,
        crate::machine::model::ReturnType::Resolved(KType::ANY),
        &[slot_of(owning)],
    );
    let leans = relaxed_admits(
        &sig,
        &expr,
        &[Some(Resolution::Parked(producer))],
        &registries,
        scratch,
    )
    .expect("a strictly-admitting slot leans on nothing");
    assert!(leans.is_empty());
}

/// A `ProperType` member brings its own shape-only admission into a union: a fresh `Type` token
/// strict-admits at `union_of(ProperType, Identifier)` exactly as it does at the bare
/// `:of_kind(ProperType)` slot, so the member's lowering is reachable rather than lost to the
/// unbound name. It stays an eager member — the token is lowered at bind, not captured raw.
#[test]
fn a_kind_member_keeps_its_shape_only_admission_in_a_union() {
    let program = program_storage();
    let brand = program.brand();
    let registries = RunRegistries::new();
    let token = type_token("Meters");
    let unbound = || Some(Resolution::Unbound(BinderSymbol::Type(token)));
    let slot = registries
        .types
        .union_of(&[KType::PROPER_TYPE, KType::IDENTIFIER]);

    assert!(admits_with(
        &registries,
        KType::PROPER_TYPE,
        ExpressionPart::Type(token),
        unbound(),
    ));
    assert!(admits_with(
        &registries,
        slot,
        ExpressionPart::Type(token),
        unbound(),
    ));
    // The `:(…)` / `:{…}` shapes the bare kind slot admits come along with it.
    for shape in [
        ExpressionPart::SigiledTypeExpr(brand.nested_node_from_iter(std::iter::empty())),
        ExpressionPart::RecordType(brand.nested_node_from_iter(std::iter::empty())),
    ] {
        assert!(admits_with(&registries, slot, shape, None));
    }
    // No raw-capture semantics ride on it: the routing lookup still answers nothing.
    assert_eq!(
        slot.raw_capture_member(&ExpressionPart::Type(token), &registries.types),
        None,
    );
}
