//! The registration doors' reach is **composed by the birth that placed the value**, not minted on
//! an asserted claim. One test per production door — the builtin seeds, `FN`, `OP` — observing the
//! description a registered callable carries, plus the operator-group half, whose yoked birth
//! composes a different (member-less) description for the same structural reason.

use crate::builtins::test_support::probe_symbol;
use crate::builtins::test_support::{TestRun, operator_run, run_root_bare};
use crate::machine::core::kfunction::{Body, KFunction};
use crate::machine::core::{BindingIndex, program_storage, run_root_storage};
use crate::machine::model::{
    Argument, KType, ReturnType, SignatureDraft, SignatureElement, UntypedKey,
};
use allocator_api2::alloc::Global;

use super::body_no_op;

/// The untyped bucket key for a signature shape, built the way the registration door derives it —
/// keyword spellings and slots, types irrelevant.
fn key(elements: Vec<SignatureElement>) -> UntypedKey {
    SignatureDraft {
        return_type: ReturnType::Resolved(KType::ANY),
        elements,
    }
    .untyped_key()
}

fn slot(name: &str) -> SignatureElement {
    SignatureElement::Argument(Argument {
        name: crate::machine::model::BinderSymbol::classify(name)
            .expect("a test fixture parameter is a value token"),
        ktype: KType::NUMBER,
    })
}

/// The **builtin seed** door ([`Scope::register_function_direct`]): the callable is born witnessed
/// and the bucket rests the birth's own description, which names the home region as host and as its
/// one member. Nothing here restates the claim — the merge composed it from the seed operand
/// delivered at the captured scope.
#[test]
fn builtin_seed_registers_a_callable_reaching_exactly_its_home_region() {
    let registries = crate::machine::model::RunRegistries::new();
    let region = run_root_storage();
    let scope = run_root_bare(&region);
    let cell = KFunction::alloc_captured(
        scope,
        super::unit_signature(),
        Body::Builtin(body_no_op),
        &registries,
    );
    scope
        .register_function_direct(
            &cell,
            BindingIndex::BUILTIN,
            &registries,
            &mut crate::machine::WriteGate::for_test(),
        )
        .unwrap();

    let foreign = run_root_storage();
    let lookup = scope.bindings().lookup_function_stored(
        &key(vec![SignatureElement::Keyword(probe_symbol("FOO"))]),
        None,
        Global,
    );
    assert_eq!(lookup.overloads.len(), 1);
    let opened = scope.open_function(&lookup.overloads[0]);
    assert!(
        opened.borrows_home(),
        "the birth's composition names the callable's own region as a member",
    );
    assert!(
        opened.reach_covers(region.region()),
        "the composed description covers the home region",
    );
    assert!(
        !opened.reach_covers(foreign.region()),
        "and covers nothing else: exactly the home region",
    );
}

/// The **`FN`** door, driven end-to-end through the interpreter: the declaration's callable carries
/// the same composed description the seed door's does, because both compose from one birth.
#[test]
fn fn_declaration_registers_a_callable_reaching_exactly_its_home_region() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("FN (DOUBLE x :Number) -> Number = (x + x)");

    let foreign = run_root_storage();
    let scope = test_run.scope;
    let lookup = scope.bindings().lookup_function_stored(
        &key(vec![
            SignatureElement::Keyword(probe_symbol("DOUBLE")),
            slot("x"),
        ]),
        None,
        Global,
    );
    assert_eq!(lookup.overloads.len(), 1, "the FN registered its overload");
    let opened = scope.open_function(&lookup.overloads[0]);
    assert!(opened.borrows_home());
    assert!(opened.reach_covers(region.region()));
    assert!(!opened.reach_covers(foreign.region()));
}

/// The **`OP`** door: the operator's body is registered through the same birth, so its bucket entry
/// carries the composed description too.
#[test]
fn op_declaration_registers_a_callable_reaching_exactly_its_home_region() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("OP #(MINUS) OVER Number = (left - right)");

    let foreign = run_root_storage();
    let scope = test_run.scope;
    let lookup = scope.bindings().lookup_function_stored(
        &key(vec![
            slot("left"),
            SignatureElement::Keyword(probe_symbol("MINUS")),
            slot("right"),
        ]),
        None,
        Global,
    );
    assert_eq!(lookup.overloads.len(), 1, "the OP registered its body");
    let opened = scope.open_function(&lookup.overloads[0]);
    assert!(opened.borrows_home());
    assert!(opened.reach_covers(region.region()));
    assert!(!opened.reach_covers(foreign.region()));
}

/// The **group record**'s birth is a yoke, not a merge, so its composed description differs from a
/// callable's: the record is region-pure by the yoke brand's own proof — `OperatorGroup::alloc`
/// re-homes every byte at the brand it is handed — so the description names the declaring region as
/// host with **no members**. Held here as a composed fact, exactly as the callable's is.
#[test]
fn operator_group_birth_composes_a_member_less_description_at_the_declaring_region() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    // A standalone `OP` outside a `GROUP` installs its own size-1 registry entry.
    test_run.run("OP #(MINUS) OVER Number = (left - right)");

    let scope = test_run.scope;
    let sealed = scope
        .bindings()
        .lookup_operator_group(operator_run(&["MINUS"], test_run.registries()), None)
        .expect("a standalone OP declares its own operator group");
    let delivered = scope.lift_resident(sealed);
    let opened = delivered.open_at();
    assert!(
        !opened.has_reach_members(),
        "the yoke composes an empty member set: the record borrows only what it was born from",
    );
    assert!(!opened.borrows_home());
    assert!(
        opened.with_home_region(|home| std::ptr::eq(home, region.region())),
        "and it is hosted in the declaring scope's own region",
    );
}
