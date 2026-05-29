//! `SIG_WITH` sharing constraints on functor parameters and return types.

use crate::builtins::test_support::{lookup_fn, parse_one, run, run_root_silent};
use crate::machine::model::KObject;
use crate::machine::{RuntimeArena, ScopeId};

/// Sharing-constraint admissibility: a `SatisfiesSignature` slot with a pinned
/// `type_members["Type"] = Number` rejects modules whose pin disagrees, is absent,
/// or whose `compatible_sigs` set doesn't contain the slot's `sig_id`.
#[test]
fn sharing_constraint_rejects_mismatched_module_type() {
    use crate::machine::model::ast::ExpressionPart;
    use crate::machine::model::KType;
    use crate::machine::model::values::Module;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let child_a = arena.alloc_scope(crate::machine::Scope::child_under_module(
        scope,
        "NumPinned".into(),
    ));
    let m_num: &Module<'_> = arena.alloc_module(Module::new("NumPinned".into(), child_a));
    m_num.type_members.borrow_mut().insert("Type".into(), KType::Number);
    m_num.mark_satisfies(ScopeId::from_raw(0, 42)); // arbitrary sig_id matching the slot below
    let m_num_obj = arena.alloc(KObject::KTypeValue(KType::Module { module: m_num, frame: None }));

    let child_b = arena.alloc_scope(crate::machine::Scope::child_under_module(
        scope,
        "StrPinned".into(),
    ));
    let m_str: &Module<'_> = arena.alloc_module(Module::new("StrPinned".into(), child_b));
    m_str.type_members.borrow_mut().insert("Type".into(), KType::Str);
    m_str.mark_satisfies(ScopeId::from_raw(0, 42));
    let m_str_obj = arena.alloc(KObject::KTypeValue(KType::Module { module: m_str, frame: None }));

    let child_c = arena.alloc_scope(crate::machine::Scope::child_under_module(
        scope,
        "NoTypePin".into(),
    ));
    let m_none: &Module<'_> = arena.alloc_module(Module::new("NoTypePin".into(), child_c));
    m_none.mark_satisfies(ScopeId::from_raw(0, 42));
    let m_none_obj = arena.alloc(KObject::KTypeValue(KType::Module { module: m_none, frame: None }));

    let slot = KType::SatisfiesSignature {
        sig_id: ScopeId::from_raw(0, 42),
        sig_path: "OrderedSig".into(),
        pinned_slots: vec![("Type".into(), KType::Number)],
    };

    assert!(slot.matches_value(m_num_obj));
    assert!(slot.accepts_part(&ExpressionPart::Future(m_num_obj)));
    assert!(!slot.matches_value(m_str_obj));
    assert!(!slot.accepts_part(&ExpressionPart::Future(m_str_obj)));
    assert!(!slot.matches_value(m_none_obj));
    assert!(!slot.accepts_part(&ExpressionPart::Future(m_none_obj)));

    let child_d = arena.alloc_scope(crate::machine::Scope::child_under_module(
        scope,
        "Unascribed".into(),
    ));
    let m_unascribed: &Module<'_> = arena.alloc_module(Module::new("Unascribed".into(), child_d));
    m_unascribed.type_members.borrow_mut().insert("Type".into(), KType::Number);
    // No mark_satisfies: compatible_sigs stays empty, so the sig-membership gate trips
    // before the pin comparison.
    let m_unascribed_obj = arena.alloc(KObject::KTypeValue(KType::Module { module: m_unascribed, frame: None }));
    assert!(!slot.matches_value(m_unascribed_obj));
    assert!(!slot.accepts_part(&ExpressionPart::Future(m_unascribed_obj)));
}

/// Pure-type pinned slots (no parameter references) resolve synchronously at
/// FN-construction and land on the FN's stored return type as a
/// `SatisfiesSignature` with both pins captured.
#[test]
fn functor_with_two_pinned_slots_round_trips() {
    use crate::machine::model::KType;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG Set = ((LET Elt = Number) (LET Ord = Number) (VAL tag :Number))\n\
         SIG OrderedSig = (VAL compare :Number)\n\
         MODULE IntOrd = (LET compare = 7)\n\
         LET IntOrdView = (IntOrd :! OrderedSig)",
    );
    run(
        scope,
        "FN (TWOPIN p :OrderedSig) -> (SIG_WITH Set ((Elt :Number) (Ord :Number))) = \
         (MODULE Result = ((LET Elt = Number) (LET Ord = Number) (LET tag = 0)))",
    );
    let f = lookup_fn(scope, "TWOPIN");
    use crate::machine::model::ReturnType;
    match &f.signature.return_type {
        ReturnType::Resolved(KType::SatisfiesSignature { sig_path, pinned_slots, .. }) => {
            assert_eq!(sig_path, "Set");
            assert_eq!(pinned_slots.len(), 2);
            assert_eq!(pinned_slots[0].0, "Elt");
            assert_eq!(pinned_slots[0].1, KType::Number);
            assert_eq!(pinned_slots[1].0, "Ord");
            assert_eq!(pinned_slots[1].1, KType::Number);
        }
        other => panic!(
            "expected Resolved(SatisfiesSignature) on TWOPIN's return type, got {:?}",
            other,
        ),
    }
}

/// FN-construction-time capture: a declared `(SIG_WITH SetSig ((Elt :Number)))`
/// return type lands on the FN's stored signature with `Elt` pinned to `Number`.
#[test]
fn functor_return_with_sharing_constraint_pins_output_type() {
    use crate::machine::model::KType;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         SIG SetSig = ((LET Elt = Number) (VAL insert :Number))\n\
         MODULE IntOrd = (LET compare = 7)\n\
         LET IntOrdView = (IntOrd :! OrderedSig)",
    );
    run(
        scope,
        "FN (MAKESETN p :OrderedSig) -> (SIG_WITH SetSig ((Elt :Number))) = \
         (MODULE Result = ((LET Elt = Number) (LET insert = 0)))",
    );
    let f = lookup_fn(scope, "MAKESETN");
    use crate::machine::model::ReturnType;
    match &f.signature.return_type {
        ReturnType::Resolved(KType::SatisfiesSignature { sig_path, pinned_slots, .. }) => {
            assert_eq!(sig_path, "SetSig");
            assert_eq!(pinned_slots, &vec![("Elt".to_string(), KType::Number)]);
        }
        other => panic!(
            "expected Resolved(SatisfiesSignature) on MAKESETN's return type, got {:?}",
            other,
        ),
    }
}

/// Return-type admissibility rejects a body whose module fails the
/// `SatisfiesSignature` check — here via an unascribed body module (empty
/// `compatible_sigs`), which trips the sig-membership gate before pin comparison.
/// The pin comparison itself is covered by `sharing_constraint_rejects_mismatched_module_type`.
#[test]
fn functor_return_with_mismatched_sharing_constraint_errors() {
    use crate::machine::execute::Scheduler;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         SIG SetSig = ((LET Elt = Number) (VAL insert :Number))\n\
         MODULE IntOrd = (LET compare = 7)\n\
         LET IntOrdView = (IntOrd :! OrderedSig)",
    );
    run(
        scope,
        "FN (MAKEBAD p :OrderedSig) -> (SIG_WITH SetSig ((Elt :Number))) = \
         (MODULE Result = ((LET Elt = Str) (LET insert = 0)))",
    );
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(parse_one("MAKEBAD IntOrdView"), scope);
    sched.execute().expect("execute does not surface per-slot errors");
    let res = sched.read_result(id);
    assert!(
        res.is_err(),
        "MAKEBAD must fail return-type check (mismatched pin or unascribed module), \
         got Ok({:?})",
        res.ok().map(|o| o.ktype()),
    );
}
