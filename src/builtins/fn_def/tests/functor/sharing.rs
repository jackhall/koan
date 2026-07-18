//! `WITH` sharing constraints on functor parameters and return types.

use crate::builtins::test_support::{
    lookup_fn, lookup_module, parse_one, run, run_root_silent, spliced_part,
};
use crate::machine::model::Carried;
use crate::machine::model::SigSource;
use crate::machine::model::TypeRegistry;
use crate::machine::{run_root_storage, FrameStorageExt};

/// Pinned-slot admissibility: a `Signature { .. }` slot pinned to `{Elem = Number}` admits a
/// module iff its self-sig satisfies the signature *and* every pin names a manifest member
/// fixed equal. The signature's decl scope is empty, so every module bare-satisfies it and pin
/// agreement alone decides: `Elem = Number` admitted, `Elem = Str` rejected (pin disagrees),
/// no `Elem` rejected (pin absent). Admission is structural, so ascription is never required.
#[test]
fn sharing_constraint_rejects_mismatched_module_type() {
    let types = TypeRegistry::new();
    use crate::machine::model::KType;
    use crate::machine::model::ModuleSignature;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    // An empty signature: every module bare-satisfies it, so the pins alone gate. Declared
    // directly rather than through `SIG`, which has no empty-body surface form.
    let sig_scope = region
        .brand()
        .alloc_scope(crate::machine::Scope::child_under_sig(
            scope,
            "Ordered".into(),
        ));
    let sig = region
        .brand()
        .alloc_signature(ModuleSignature::new("Ordered".into(), sig_scope));

    // `no_elem_pin` binds no `Elem` member, so the `{Elem = Number}` pin finds nothing to agree
    // with. `num_bare` is a second `Elem = Number` module, ascribed to nothing: admission is
    // structural, so it is behaviorally identical to `num_pinned`.
    run(
        scope,
        "MODULE num_pinned = ((LET Elem = Number) (LET compare = 0))\n\
         MODULE str_pinned = ((LET Elem = Str) (LET compare = 0))\n\
         MODULE no_elem_pin = (LET compare = 0)\n\
         MODULE num_bare = ((LET Elem = Number) (LET compare = 0))",
    );

    let slot = KType::signature(
        SigSource::Declared(sig),
        vec![("Elem".into(), KType::Number)],
    );

    // A module binds value-side, so both the overload probe and the built argument cell carry it
    // on the Object channel — its satisfaction of a `Signature` slot goes through
    // `accepts_carried`'s `Carried::Object(KObject::Module)` arm.
    let module_part = |name: &str| {
        spliced_part(Carried::Object(scope.lookup(name).unwrap_or_else(|| {
            panic!("{name} must bind a module value-side");
        })))
    };
    assert!(slot.accepts_part(&module_part("num_pinned"), &types));
    assert!(!slot.accepts_part(&module_part("str_pinned"), &types));
    assert!(!slot.accepts_part(&module_part("no_elem_pin"), &types));
    assert!(slot.accepts_part(&module_part("num_bare"), &types));
}

/// Pure-type pinned slots (no parameter references) resolve synchronously at
/// FN-construction and land on the FN's stored return type as a
/// `Signature { .. }` with both pins captured.
#[test]
fn functor_with_two_pinned_slots_round_trips() {
    use crate::machine::model::KType;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG OrderedSet = ((TYPE Elt) (TYPE Ord) (VAL tag :Number))\n\
         SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = (LET compare = 7)\n\
         LET int_ord_view = (int_ord :! Ordered)",
    );
    run(
        scope,
        "FN (TWOPIN p :Ordered) -> :(OrderedSet WITH {Elt = Number, Ord = Number}) = \
         (MODULE generated = ((LET Elt = Number) (LET Ord = Number) (LET tag = 0)))",
    );
    let f = lookup_fn(scope, "TWOPIN");
    use crate::machine::model::ReturnType;
    match &f.signature.return_type {
        ReturnType::Resolved(KType::Signature {
            sig, pinned_slots, ..
        }) => {
            assert_eq!(sig.path(), "OrderedSet");
            assert_eq!(pinned_slots.len(), 2);
            assert_eq!(pinned_slots[0].0, "Elt");
            assert_eq!(pinned_slots[0].1, KType::Number);
            assert_eq!(pinned_slots[1].0, "Ord");
            assert_eq!(pinned_slots[1].1, KType::Number);
        }
        other => panic!(
            "expected Resolved(Signature) on TWOPIN's return type, got {:?}",
            other,
        ),
    }
}

/// FN-construction-time capture: a declared `(Set WITH {Elt = Number})`
/// return type lands on the FN's stored signature with `Elt` pinned to `Number`.
#[test]
fn functor_return_with_sharing_constraint_pins_output_type() {
    use crate::machine::model::KType;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Ordered = (VAL compare :Number)\n\
         SIG Set = ((TYPE Elt) (VAL insert :Number))\n\
         MODULE int_ord = (LET compare = 7)\n\
         LET int_ord_view = (int_ord :! Ordered)",
    );
    run(
        scope,
        "FN (MAKESETN p :Ordered) -> :(Set WITH {Elt = Number}) = \
         (MODULE generated = ((LET Elt = Number) (LET insert = 0)))",
    );
    let f = lookup_fn(scope, "MAKESETN");
    use crate::machine::model::ReturnType;
    match &f.signature.return_type {
        ReturnType::Resolved(KType::Signature {
            sig, pinned_slots, ..
        }) => {
            assert_eq!(sig.path(), "Set");
            assert_eq!(pinned_slots, &vec![("Elt".to_string(), KType::Number)]);
        }
        other => panic!(
            "expected Resolved(Signature) on MAKESETN's return type, got {:?}",
            other,
        ),
    }
}

/// Return-type admissibility rejects a body whose module fails the `Signature { .. }`
/// constraint check — here the body module bare-satisfies `Set` but its `Elt = Str`
/// disagrees with the `{Elt = Number}` pin, so `satisfies_pins` rejects it. The positive
/// counterpart is `functor_return_with_matching_sharing_constraint_passes`.
#[test]
fn functor_return_with_mismatched_sharing_constraint_errors() {
    use crate::machine::KoanRuntime;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Ordered = (VAL compare :Number)\n\
         SIG Set = ((TYPE Elt) (VAL insert :Number))\n\
         MODULE int_ord = (LET compare = 7)\n\
         LET int_ord_view = (int_ord :! Ordered)",
    );
    run(
        scope,
        "FN (MAKEBAD p :Ordered) -> :(Set WITH {Elt = Number}) = \
         (MODULE generated = ((LET Elt = Str) (LET insert = 0)))",
    );
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(parse_one("MAKEBAD int_ord_view"), scope);
    runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let res = runtime.read_result_with(id, |v| format!("{:?}", v.ktype()));
    assert!(
        res.is_err(),
        "MAKEBAD must fail return-type check (mismatched pin), got Ok({:?})",
        res.ok(),
    );
}

/// Return-type admissibility passes an unascribed body module that structurally satisfies the
/// pinned return signature: the body binds `Elt = Number` and `insert`, so it bare-satisfies
/// `Set` and its `Elt` manifest member agrees with the `{Elt = Number}` pin — no ascription
/// required. Counterpart to `functor_return_with_mismatched_sharing_constraint_errors`.
#[test]
fn functor_return_with_matching_sharing_constraint_passes() {
    use crate::machine::KoanRuntime;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Ordered = (VAL compare :Number)\n\
         SIG Set = ((TYPE Elt) (VAL insert :Number))\n\
         MODULE int_ord = (LET compare = 7)\n\
         LET int_ord_view = (int_ord :! Ordered)",
    );
    run(
        scope,
        "FN (MAKEGOOD p :Ordered) -> :(Set WITH {Elt = Number}) = \
         (MODULE generated = ((LET Elt = Number) (LET insert = 0)))",
    );
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(parse_one("MAKEGOOD int_ord_view"), scope);
    runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let res = runtime.read_result_with(id, |v| format!("{:?}", v.ktype()));
    assert!(
        res.is_ok(),
        "MAKEGOOD must pass return-type check — the unascribed body module structurally \
         satisfies `Set WITH {{Elt = Number}}`, got Err({:?})",
        res.err(),
    );
}

/// The `:!` fix: a transparent view carries an empty `type_members`, so under the old
/// `type_members`-equality pin loop it silently failed every `WITH`-pinned slot. Pin agreement
/// now rides the self-sig, whose manifest members read the source's concrete types — so a
/// transparent view of a module binding `Elem = Number` agrees with `{Elem = Number}` and a
/// view binding `Elem = Str` does not.
#[test]
fn transparent_view_pin_agreement_reads_source_types() {
    let types = TypeRegistry::new();
    use crate::builtins::test_support::run_root_silent;
    use crate::machine::model::KType;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE num_mod = ((LET Elem = Number) (LET compare = 0))\n\
         MODULE str_mod = ((LET Elem = Str) (LET compare = 0))\n\
         SIG Ordered = ((TYPE Elem) (VAL compare :Number))\n\
         LET num_view = (num_mod :! Ordered)\n\
         LET str_view = (str_mod :! Ordered)",
    );
    let sig = match scope.resolve_type("Ordered") {
        Some(KType::Signature {
            sig: SigSource::Declared(sig),
            ..
        }) => *sig,
        _ => panic!("Ordered must bind a Signature KType"),
    };
    let slot = KType::signature(
        SigSource::Declared(sig),
        vec![("Elem".to_string(), KType::Number)],
    );
    // A view binds value-side, so its argument cell carries the module on the Object channel.
    let num_view = scope.lookup("num_view").expect("num_view bound");
    let str_view = scope.lookup("str_view").expect("str_view bound");
    assert!(
        slot.accepts_part(&spliced_part(Carried::Object(num_view)), &types),
        "transparent view over `Elem = Number` must agree with the `{{Elem = Number}}` pin",
    );
    assert!(
        !slot.accepts_part(&spliced_part(Carried::Object(str_view)), &types),
        "transparent view over `Elem = Str` must not agree with the `{{Elem = Number}}` pin",
    );
}

/// An opaque view agrees with a pin naming its own per-call abstract identity: the view's
/// self-sig fixes `Carrier` manifest to the abstract type it minted, so a slot pinned to that
/// same identity accepts it.
#[test]
fn opaque_view_pin_agreement_names_its_abstract_identity() {
    let types = TypeRegistry::new();
    use crate::builtins::test_support::run_root_silent;
    use crate::machine::model::KType;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE int_ord = ((LET Carrier = Number) (LET compare = 0))\n\
         SIG Ordered = ((TYPE Carrier) (VAL compare :Number))\n\
         LET view = (int_ord :| Ordered)",
    );
    let sig = match scope.resolve_type("Ordered") {
        Some(KType::Signature {
            sig: SigSource::Declared(sig),
            ..
        }) => *sig,
        _ => panic!("Ordered must bind a Signature KType"),
    };
    let view = lookup_module(scope, "view");
    let carrier_abstract = view
        .type_members
        .borrow()
        .get("Carrier")
        .cloned()
        .expect("opaque view mints an abstract `Carrier`");
    let slot = KType::signature(
        SigSource::Declared(sig),
        vec![("Carrier".to_string(), carrier_abstract)],
    );
    // A view binds value-side, so its argument cell carries the module on the Object channel.
    let view_obj = scope.lookup("view").expect("view bound");
    assert!(
        slot.accepts_part(&spliced_part(Carried::Object(view_obj)), &types),
        "opaque view must agree with a pin naming its own per-call abstract `Carrier`",
    );
}
