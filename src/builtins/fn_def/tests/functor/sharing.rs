//! `WITH` sharing constraints on functor parameters and return types.

use crate::builtins::test_support::{lookup_fn, parse_one, run, run_root_silent};
use crate::machine::core::FrameStorage;
use crate::machine::model::Carried;

/// Sharing-constraint admissibility: a `Signature { .. }` slot with a pinned
/// `type_members["Type"] = Number` rejects modules whose pin disagrees, is absent,
/// or whose `compatible_sigs` set doesn't contain the slot's `sig.sig_id()`.
#[test]
fn sharing_constraint_rejects_mismatched_module_type() {
    use crate::machine::model::ast::ExpressionPart;
    use crate::machine::model::values::{Module, ModuleSignature};
    use crate::machine::model::KType;
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    // Real signature so the slot's `sig.sig_id()` is the one modules `mark_satisfies`.
    let sig_scope = region
        .brand()
        .alloc_scope(crate::machine::Scope::child_under_sig(
            scope,
            "OrderedSig".into(),
        ));
    let sig = region
        .brand()
        .alloc_signature(ModuleSignature::new("OrderedSig".into(), sig_scope));
    let sig_id = sig.sig_id();

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
    m_num.mark_satisfies(sig_id);
    let m_num_obj = region.brand().alloc_ktype(KType::Module { module: m_num });

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
    m_str.mark_satisfies(sig_id);
    let m_str_obj = region.brand().alloc_ktype(KType::Module { module: m_str });

    let child_c = region
        .brand()
        .alloc_scope(crate::machine::Scope::child_under_module(
            scope,
            "NoTypePin".into(),
        ));
    let m_none: &Module<'_> = region
        .brand()
        .alloc_module(Module::new("NoTypePin".into(), child_c));
    m_none.mark_satisfies(sig_id);
    let m_none_obj = region.brand().alloc_ktype(KType::Module { module: m_none });

    let slot = KType::Signature {
        sig,
        pinned_slots: vec![("Type".into(), KType::Number)],
    };

    // A module rides the type channel, so its satisfaction of a `Signature` slot is the
    // `accepts_part(Carried::Type(Module))` path; `matches_value` is value-only and rejects
    // modules outright.
    assert!(slot.accepts_part(&ExpressionPart::Spliced(Carried::Type(m_num_obj))));
    assert!(!slot.accepts_part(&ExpressionPart::Spliced(Carried::Type(m_str_obj))));
    assert!(!slot.accepts_part(&ExpressionPart::Spliced(Carried::Type(m_none_obj))));

    let child_d = region
        .brand()
        .alloc_scope(crate::machine::Scope::child_under_module(
            scope,
            "Unascribed".into(),
        ));
    let m_unascribed: &Module<'_> = region
        .brand()
        .alloc_module(Module::new("Unascribed".into(), child_d));
    m_unascribed
        .type_members
        .borrow_mut()
        .insert("Type".into(), KType::Number);
    // No mark_satisfies: compatible_sigs stays empty, so the sig-membership gate trips
    // before the pin comparison.
    let m_unascribed_obj = region.brand().alloc_ktype(KType::Module {
        module: m_unascribed,
    });
    assert!(!slot.accepts_part(&ExpressionPart::Spliced(Carried::Type(m_unascribed_obj))));
}

/// Pure-type pinned slots (no parameter references) resolve synchronously at
/// FN-construction and land on the FN's stored return type as a
/// `Signature { .. }` with both pins captured.
#[test]
fn functor_with_two_pinned_slots_round_trips() {
    use crate::machine::model::KType;
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Set = ((LET Elt = Number) (LET Ord = Number) (VAL tag :Number))\n\
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
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         SIG SetSig = ((LET Elt = Number) (VAL insert :Number))\n\
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

/// Return-type admissibility rejects a body whose module fails the
/// `Signature { .. }` constraint check — here via an unascribed body module (empty
/// `compatible_sigs`), which trips the sig-membership gate before pin comparison.
/// The pin comparison itself is covered by `sharing_constraint_rejects_mismatched_module_type`.
#[test]
fn functor_return_with_mismatched_sharing_constraint_errors() {
    use crate::machine::execute::KoanRuntime;
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         SIG SetSig = ((LET Elt = Number) (VAL insert :Number))\n\
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
        "MAKEBAD must fail return-type check (mismatched pin or unascribed module), \
         got Ok({:?})",
        res.ok(),
    );
}
