//! `WITH` sharing constraints on functor parameters and return types.

use crate::builtins::test_support::{lookup_fn, parse_one, run, run_root_silent, spliced_part};
use crate::machine::core::{run_root_storage, FrameStorageExt};
use crate::machine::model::Carried;

/// Pinned-slot admissibility: a `Signature { .. }` slot pinned to `{Type = Number}` admits a
/// module iff its self-sig satisfies the signature *and* every pin names a manifest member
/// fixed equal. The signature's decl scope is empty, so every module bare-satisfies it and pin
/// agreement alone decides: `Type = Number` admitted, `Type = Str` rejected (pin disagrees),
/// no `Type` rejected (pin absent). Admission is structural, so ascription is never required.
#[test]
fn sharing_constraint_rejects_mismatched_module_type() {
    use crate::machine::model::values::{Module, ModuleSignature};
    use crate::machine::model::KType;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    // An empty signature: every module bare-satisfies it, so the pins alone gate.
    let sig_scope = region
        .brand()
        .alloc_scope(crate::machine::Scope::child_under_sig(
            scope,
            "OrderedSig".into(),
        ));
    let sig = region
        .brand()
        .alloc_signature(ModuleSignature::new("OrderedSig".into(), sig_scope));

    let child_a = region
        .brand()
        .alloc_scope(crate::machine::Scope::child_under_module(
            scope,
            "NumPinned".into(),
        ));
    let m_num: &Module<'_> = region
        .brand()
        .alloc_module(Module::new("NumPinned".into(), child_a));
    m_num
        .type_members
        .borrow_mut()
        .insert("Type".into(), KType::Number);
    let m_num_obj = region
        .brand()
        .alloc_ktype_checked(KType::Module { module: m_num })
        .expect("m_num was just allocated into region's own region");

    let child_b = region
        .brand()
        .alloc_scope(crate::machine::Scope::child_under_module(
            scope,
            "StrPinned".into(),
        ));
    let m_str: &Module<'_> = region
        .brand()
        .alloc_module(Module::new("StrPinned".into(), child_b));
    m_str
        .type_members
        .borrow_mut()
        .insert("Type".into(), KType::Str);
    let m_str_obj = region
        .brand()
        .alloc_ktype_checked(KType::Module { module: m_str })
        .expect("m_str was just allocated into region's own region");

    let child_c = region
        .brand()
        .alloc_scope(crate::machine::Scope::child_under_module(
            scope,
            "NoTypePin".into(),
        ));
    let m_none: &Module<'_> = region
        .brand()
        .alloc_module(Module::new("NoTypePin".into(), child_c));
    let m_none_obj = region
        .brand()
        .alloc_ktype_checked(KType::Module { module: m_none })
        .expect("m_none was just allocated into region's own region");

    let slot = KType::Signature {
        sig,
        pinned_slots: vec![("Type".into(), KType::Number)],
    };

    // A module rides the type channel, so its satisfaction of a `Signature` slot is the
    // `accepts_part(Carried::Type(Module))` path; `matches_value` is value-only and rejects
    // modules outright.
    assert!(slot.accepts_part(&spliced_part(Carried::Type(m_num_obj))));
    assert!(!slot.accepts_part(&spliced_part(Carried::Type(m_str_obj))));
    assert!(!slot.accepts_part(&spliced_part(Carried::Type(m_none_obj))));

    // A second `Type = Number` module never ascribed to anything: admission is structural, so
    // it is behaviorally identical to `m_num` — pin agreement alone decides, no ascription.
    let child_d = region
        .brand()
        .alloc_scope(crate::machine::Scope::child_under_module(
            scope,
            "NumBare".into(),
        ));
    let m_num_bare: &Module<'_> = region
        .brand()
        .alloc_module(Module::new("NumBare".into(), child_d));
    m_num_bare
        .type_members
        .borrow_mut()
        .insert("Type".into(), KType::Number);
    let m_num_bare_obj = region
        .brand()
        .alloc_ktype_checked(KType::Module { module: m_num_bare })
        .expect("m_num_bare was just allocated into region's own region");
    assert!(slot.accepts_part(&spliced_part(Carried::Type(m_num_bare_obj))));
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
        "SIG Set = ((TYPE Elt) (TYPE Ord) (VAL tag :Number))\n\
         SIG OrderedSig = (VAL compare :Number)\n\
         MODULE IntOrd = (LET compare = 7)\n\
         LET IntOrdView = (IntOrd :! OrderedSig)",
    );
    run(
        scope,
        "FN (TWOPIN p :OrderedSig) -> :(Set WITH {Elt = Number, Ord = Number}) = \
         (MODULE Generated = ((LET Elt = Number) (LET Ord = Number) (LET tag = 0)))",
    );
    let f = lookup_fn(scope, "TWOPIN");
    use crate::machine::model::ReturnType;
    match &f.signature.return_type {
        ReturnType::Resolved(KType::Signature { sig, pinned_slots }) => {
            assert_eq!(sig.path, "Set");
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

/// FN-construction-time capture: a declared `(SetSig WITH {Elt = Number})`
/// return type lands on the FN's stored signature with `Elt` pinned to `Number`.
#[test]
fn functor_return_with_sharing_constraint_pins_output_type() {
    use crate::machine::model::KType;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         SIG SetSig = ((TYPE Elt) (VAL insert :Number))\n\
         MODULE IntOrd = (LET compare = 7)\n\
         LET IntOrdView = (IntOrd :! OrderedSig)",
    );
    run(
        scope,
        "FN (MAKESETN p :OrderedSig) -> :(SetSig WITH {Elt = Number}) = \
         (MODULE Generated = ((LET Elt = Number) (LET insert = 0)))",
    );
    let f = lookup_fn(scope, "MAKESETN");
    use crate::machine::model::ReturnType;
    match &f.signature.return_type {
        ReturnType::Resolved(KType::Signature { sig, pinned_slots }) => {
            assert_eq!(sig.path, "SetSig");
            assert_eq!(pinned_slots, &vec![("Elt".to_string(), KType::Number)]);
        }
        other => panic!(
            "expected Resolved(Signature) on MAKESETN's return type, got {:?}",
            other,
        ),
    }
}

/// Return-type admissibility rejects a body whose module fails the `Signature { .. }`
/// constraint check — here the body module bare-satisfies `SetSig` but its `Elt = Str`
/// disagrees with the `{Elt = Number}` pin, so `satisfies_pins` rejects it. The positive
/// counterpart is `functor_return_with_matching_sharing_constraint_passes`.
#[test]
fn functor_return_with_mismatched_sharing_constraint_errors() {
    use crate::machine::execute::KoanRuntime;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         SIG SetSig = ((TYPE Elt) (VAL insert :Number))\n\
         MODULE IntOrd = (LET compare = 7)\n\
         LET IntOrdView = (IntOrd :! OrderedSig)",
    );
    run(
        scope,
        "FN (MAKEBAD p :OrderedSig) -> :(SetSig WITH {Elt = Number}) = \
         (MODULE Generated = ((LET Elt = Str) (LET insert = 0)))",
    );
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(parse_one("MAKEBAD IntOrdView"), scope);
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
/// `SetSig` and its `Elt` manifest member agrees with the `{Elt = Number}` pin — no ascription
/// required. Counterpart to `functor_return_with_mismatched_sharing_constraint_errors`.
#[test]
fn functor_return_with_matching_sharing_constraint_passes() {
    use crate::machine::execute::KoanRuntime;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         SIG SetSig = ((TYPE Elt) (VAL insert :Number))\n\
         MODULE IntOrd = (LET compare = 7)\n\
         LET IntOrdView = (IntOrd :! OrderedSig)",
    );
    run(
        scope,
        "FN (MAKEGOOD p :OrderedSig) -> :(SetSig WITH {Elt = Number}) = \
         (MODULE Generated = ((LET Elt = Number) (LET insert = 0)))",
    );
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(parse_one("MAKEGOOD IntOrdView"), scope);
    runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let res = runtime.read_result_with(id, |v| format!("{:?}", v.ktype()));
    assert!(
        res.is_ok(),
        "MAKEGOOD must pass return-type check — the unascribed body module structurally \
         satisfies `SetSig WITH {{Elt = Number}}`, got Err({:?})",
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
    use crate::builtins::test_support::run_root_silent;
    use crate::machine::model::KType;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE NumMod = ((LET Elem = Number) (LET compare = 0))\n\
         MODULE StrMod = ((LET Elem = Str) (LET compare = 0))\n\
         SIG OrderedSig = ((TYPE Elem) (VAL compare :Number))\n\
         LET NumView = (NumMod :! OrderedSig)\n\
         LET StrView = (StrMod :! OrderedSig)",
    );
    let sig = match scope.resolve_type("OrderedSig") {
        Some(KType::Signature { sig, .. }) => *sig,
        _ => panic!("OrderedSig must bind a Signature KType"),
    };
    let slot = KType::Signature {
        sig,
        pinned_slots: vec![("Elem".to_string(), KType::Number)],
    };
    let num_view = scope.resolve_type("NumView").expect("NumView bound");
    let str_view = scope.resolve_type("StrView").expect("StrView bound");
    assert!(
        slot.accepts_part(&spliced_part(Carried::Type(num_view))),
        "transparent view over `Elem = Number` must agree with the `{{Elem = Number}}` pin",
    );
    assert!(
        !slot.accepts_part(&spliced_part(Carried::Type(str_view))),
        "transparent view over `Elem = Str` must not agree with the `{{Elem = Number}}` pin",
    );
}

/// An opaque view agrees with a pin naming its own per-call abstract identity: the view's
/// self-sig fixes `Carrier` manifest to the abstract type it minted, so a slot pinned to that
/// same identity accepts it.
#[test]
fn opaque_view_pin_agreement_names_its_abstract_identity() {
    use crate::builtins::test_support::run_root_silent;
    use crate::machine::model::KType;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE IntOrd = ((LET Carrier = Number) (LET compare = 0))\n\
         SIG OrderedSig = ((TYPE Carrier) (VAL compare :Number))\n\
         LET View = (IntOrd :| OrderedSig)",
    );
    let sig = match scope.resolve_type("OrderedSig") {
        Some(KType::Signature { sig, .. }) => *sig,
        _ => panic!("OrderedSig must bind a Signature KType"),
    };
    let view = match scope.resolve_type("View") {
        Some(KType::Module { module }) => module,
        _ => panic!("View must bind a Module KType"),
    };
    let carrier_abstract = view
        .type_members
        .borrow()
        .get("Carrier")
        .cloned()
        .expect("opaque view mints an abstract `Carrier`");
    let slot = KType::Signature {
        sig,
        pinned_slots: vec![("Carrier".to_string(), carrier_abstract)],
    };
    let view_kt = scope.resolve_type("View").expect("View bound");
    assert!(
        slot.accepts_part(&spliced_part(Carried::Type(view_kt))),
        "opaque view must agree with a pin naming its own per-call abstract `Carrier`",
    );
}
