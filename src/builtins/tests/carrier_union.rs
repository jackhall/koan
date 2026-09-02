//! The registration door's union-carrier-slot check: [`arg`](crate::builtins::arg) is where every
//! builtin slot type is declared, so it is where an ill-formed union carrier slot fails. The rule
//! itself — which unions are constrained and why — is
//! [`carrier_union_error`](crate::machine::model::carrier_union_error), unit-tested beside its own
//! definition; these pin that the door actually consults it.

use crate::builtins::test_support::{TestRun, lookup_binding, marker};
use crate::builtins::{arg, kw, register_builtin, sig};
use crate::machine::core::kfunction::action::{Action, BodyCtx};
use crate::machine::model::{BinderSymbol, Carried, KObject, KType, RunRegistries};
use crate::machine::{program_storage, run_root_storage};
use crate::static_name;

static SLOT: crate::machine::model::StaticName<crate::machine::model::ValueSymbol> =
    static_name!(crate::machine::model::ValueSymbol, "repr");

/// The workhorse union every carrier spelling of a type slot registers as: disjoint members, no
/// `KExpression`, so the door takes it.
#[test]
fn the_door_accepts_a_disjoint_carrier_union() {
    let registries = RunRegistries::new();
    let slot = registries.types.union_of(&[
        KType::TYPE_NAME_TOKEN,
        KType::SIGILED_TYPE_EXPR,
        KType::RECORD_TYPE,
        KType::IDENTIFIER,
    ]);
    arg(&registries, &SLOT, slot);
}

/// A `(…)` group is the eager sub-expression shape, so a `KExpression` member would leave the
/// seal-time raw-kind derivation and the group's staging ambiguous.
#[test]
#[should_panic(expected = "may not have a `KExpression` member")]
fn the_door_rejects_a_kexpression_member() {
    let registries = RunRegistries::new();
    let slot = registries
        .types
        .union_of(&[KType::SIGILED_TYPE_EXPR, KType::KEXPRESSION]);
    arg(&registries, &SLOT, slot);
}

/// `Union` identity is order-blind, so two members claiming the same raw part shape would make
/// admission and capture depend on how the union happened to be written.
#[test]
#[should_panic(expected = "overlapping members")]
fn the_door_rejects_members_claiming_one_shape() {
    let registries = RunRegistries::new();
    let slot = registries
        .types
        .union_of(&[KType::TYPE_NAME_TOKEN, KType::NAME_TOKEN]);
    arg(&registries, &SLOT, slot);
}

/// A pure value union carries no raw-capture semantics, so it is an ordinary eager slot the door
/// leaves alone — including one a user can spell with a `:KExpression` member.
#[test]
fn the_door_leaves_a_value_union_alone() {
    let registries = RunRegistries::new();
    for slot in [
        registries.types.union_of(&[KType::NUMBER, KType::STR]),
        registries
            .types
            .union_of(&[KType::KEXPRESSION, KType::NUMBER]),
    ] {
        arg(&registries, &SLOT, slot);
    }
}

/// The body of the fixture form: it reports which name class its captured cell carries, so the
/// end-to-end assertion reads the *capture* rather than a value the argument evaluated to.
fn echo_captured_class<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    let label = match ctx.args.name(&SLOT) {
        Some(BinderSymbol::Type(_)) => "type-token",
        Some(BinderSymbol::Value(_)) => "value-token",
        None => "not-captured",
    };
    Action::done_resident(ctx.scope, Carried::Object(marker(ctx.scope, label)))
}

/// End to end: one overload whose slot is `union_of(TypeNameToken, Identifier)` admits both bare
/// name shapes and captures each raw, through its own member — the two registrations this
/// replaces would have needed one overload each.
#[test]
fn one_union_overload_captures_both_name_shapes_raw() {
    let program = program_storage();
    let region = run_root_storage();
    let mut run = TestRun::silent(&program, &region);
    let registries = run.registry_handle();
    let scope = run.scope;
    let slot = registries.union_of(&[KType::TYPE_NAME_TOKEN, KType::IDENTIFIER]);
    register_builtin(
        scope,
        sig(
            KType::STR,
            vec![
                kw(registries.registries(), "MYCAP"),
                arg(registries.registries(), &SLOT, slot),
            ],
        ),
        echo_captured_class,
        registries.registries(),
        &mut crate::machine::WriteGate::for_test(),
    );
    // A fresh `Type` token and a bare value name, neither of which is bound to anything.
    run.run("LET fromType = (MYCAP Meters)\nLET fromValue = (MYCAP width)");
    assert!(
        matches!(lookup_binding(scope, "fromType"), Some(KObject::KString(s)) if **s == *"type-token"),
    );
    assert!(
        matches!(lookup_binding(scope, "fromValue"), Some(KObject::KString(s)) if **s == *"value-token"),
    );
}
