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
    SignatureElement::Argument(Argument::new(
        BinderSymbol::classify("operand").expect("a fixture parameter is a value token"),
        ktype,
    ))
}

/// Strict admission of a single AST part at a single slot, with the given bare-name outcome, at a
/// slot the pre-admission park did *not* exempt — the ordinary case.
fn admits_with(
    registries: &RunRegistries,
    ktype: KType,
    part: ExpressionPart<'_>,
    outcome: Option<Resolution>,
) -> bool {
    admits_exempt(registries, ktype, part, outcome, false)
}

/// [`admits_with`] with the body-owns-this-name verdict stated — the binder-form operand rail.
/// The verdict itself is `WorkingExpression::body_owns_parked_name`, tested at its own seam; here
/// it is an input, so this pins what admission *does* with it.
fn admits_exempt(
    registries: &RunRegistries,
    ktype: KType,
    part: ExpressionPart<'_>,
    outcome: Option<Resolution>,
    body_owns_parked_name: bool,
) -> bool {
    slot_admits_strict(
        &slot_of(ktype),
        &WorkingPart::Ast(part),
        0,
        body_owns_parked_name,
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

/// A `ProperType` member brings its `:(…)` / `:{…}` shape-only admission into a union, and no
/// more: a bare `Type` token is an ordinary resolving operand at a kind slot, so an unbound one
/// rejects there exactly as it does at the bare `:of_kind(ProperType)` slot. It stays an eager
/// member either way — the token is lowered at bind, not captured raw.
#[test]
fn a_kind_member_carries_only_its_shape_only_admission_into_a_union() {
    let program = program_storage();
    let brand = program.brand();
    let registries = RunRegistries::new();
    let token = type_token("Meters");
    let unbound = || Some(Resolution::Unbound(BinderSymbol::Type(token)));
    let slot = registries
        .types
        .union_of(&[KType::PROPER_TYPE, KType::IDENTIFIER]);

    assert!(!admits_with(
        &registries,
        KType::PROPER_TYPE,
        ExpressionPart::Type(token),
        unbound(),
    ));
    assert!(!admits_with(
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

/// The four clauses of `body_owns_parked_name` over a real binder form. `NEWTYPE <N> = <T>` is
/// the one shape whose operand a body owns, and each clause is what keeps some other shape out:
/// drop the park and a resolved operand wraps; drop the exemption and a non-binder form parks;
/// drop the kind slot and `LET Alias = Cell` parks on its `:Any` slot and waits for the seal.
#[test]
fn only_a_parked_binder_operand_at_a_kind_slot_is_owned_by_the_body() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = crate::builtins::test_support::TestRun::silent(&program, &region);
    let brand = test_run.scope.brand();
    let types = &test_run.registries().types;

    let declarator = WorkingExpression::from_ast(brand, test_run.parse_one("NEWTYPE Bb = Aa"));
    let repr = 3;
    assert!(matches!(
        declarator.parts[repr].value.as_ast(),
        Some(ExpressionPart::Type(_))
    ));
    let part = declarator.parts[repr].value;

    assert!(
        declarator.body_owns_parked_name(repr, &part, KType::PROPER_TYPE, true, types),
        "a parked sibling at NEWTYPE's kind repr slot is the body's to resolve",
    );
    assert!(
        !declarator.body_owns_parked_name(repr, &part, KType::PROPER_TYPE, false, types),
        "an already-resolved name wraps and rides the lane",
    );
    assert!(
        !declarator.body_owns_parked_name(repr, &part, KType::ANY, true, types),
        "an `:Any` slot parks and waits for the seal — `LET Alias = Cell`",
    );

    // A non-binder form has no exempt slot at all, so its operand parks like any other.
    let consumer =
        WorkingExpression::from_ast(brand, test_run.parse_one("MATCH (1) -> Aa WITH (x)"));
    assert!(
        consumer.binder_name_slot().is_none(),
        "MATCH declares no name, so nothing about it is park-exempt",
    );
    assert!(!consumer.body_owns_parked_name(
        3,
        &consumer.parts[3].value,
        KType::PROPER_TYPE,
        true,
        types
    ),);
}

/// What admission does with that verdict: it is the shape pass, so the token is taken on shape and
/// never read out of the `bare_outcomes` cache.
#[test]
fn admission_takes_a_body_owned_token_on_shape() {
    let registries = RunRegistries::new();
    let token = type_token("Sibling");
    assert!(admits_exempt(
        &registries,
        KType::PROPER_TYPE,
        ExpressionPart::Type(token),
        Some(Resolution::Parked(crate::machine::ProducerId::for_test(7))),
        true,
    ));
    // Without it, a parked operand rejects so the relaxed pass can park on the producer, and an
    // unbound one rejects so the dead lean raises against the slot's registered role.
    for outcome in [
        Some(Resolution::Parked(crate::machine::ProducerId::for_test(7))),
        Some(Resolution::Unbound(BinderSymbol::Type(token))),
    ] {
        assert!(!admits_exempt(
            &registries,
            KType::PROPER_TYPE,
            ExpressionPart::Type(token),
            outcome,
            false,
        ));
    }
}
