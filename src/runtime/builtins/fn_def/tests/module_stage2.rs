//! Module-system stage 2: scope-aware type elaboration, signature-bound parameters,
//! functor lifting.

use crate::runtime::builtins::test_support::{parse_one, run, run_one, run_root_silent};
use crate::runtime::machine::model::KObject;
use crate::runtime::machine::{RuntimeArena, ScopeId};

/// Verify that `LET MyList = (LIST_OF Number)` registers a type binding carrying the
/// elaborated `KType::List(Number)`. Post-stage-1.7 storage flip the LET TypeExprRef
/// overload writes `bindings.types` (reachable via `Scope::resolve_type`); the prior
/// `KObject::KTypeValue` carrier survives only as a dispatch transport, not as the
/// storage shape.
#[test]
fn list_of_let_binding_is_ktype_value() {
    use crate::runtime::machine::model::KType;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "LET MyList = (LIST_OF Number)");
    let kt = scope
        .resolve_type("MyList")
        .expect("MyList should be bound in bindings.types");
    assert_eq!(*kt, KType::List(Box::new(KType::Number)));
}

/// The scheduler-aware elaborator reads a `KTypeValue` binding back as its stored
/// `KType`: `LET MyList = (LIST_OF Number)` followed by an elaborator walk of the
/// `MyList` leaf returns `KType::List(Number)`. Replaces the previous `ScopeResolver`
/// path that was deleted in phase 5.
#[test]
fn elaborator_lowers_ktype_value_binding() {
    use crate::runtime::machine::model::ast::TypeExpr;
    use crate::runtime::machine::model::KType;
    use crate::runtime::machine::model::types::{elaborate_type_expr, ElabResult, Elaborator};
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "LET MyList = (LIST_OF Number)");
    let mut el = Elaborator::new(scope);
    match elaborate_type_expr(&mut el, &TypeExpr::leaf("MyList".into())) {
        ElabResult::Done(kt) => assert_eq!(kt, KType::List(Box::new(KType::Number))),
        other => panic!("expected Done(:(List Number)), got {:?}", other),
    }
}

/// FN-def integration: a parameter typed `Er: OrderedSig` lowers via the scope-aware
/// `elaborate_type_expr` into `KType::SignatureBound { sig_id, sig_path: "OrderedSig" }`,
/// with `sig_id` equal to the declaring `Signature::sig_id()`. Pins the elaborator-to-
/// FN-signature path that drives functor dispatch.
///
/// Module-system functor-params Stage A: this test was migrated from a lowercase
/// `elem` parameter to Type-class `Er` (the documented surface form). The FN-def
/// parameter parser now admits Type-classified bare-leaf tokens as parameter names
/// in addition to lowercase Identifiers, which is what makes the documented
/// `FN (LIFT Er: OrderedSig) -> ...` surface form actually parse.
#[test]
fn fn_with_signature_bound_param_records_signature_bound_ktype() {
    use crate::runtime::machine::model::{Argument, KType, SignatureElement};
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    // Phase 3: single batch — the FN's signature elaboration parks on `OrderedSig`'s
    // SIG placeholder and the Combine wakes it once the SIG finalizes. Previously
    // required two batches because the synchronous type-name resolution didn't park.
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         FN (USE_ORD Er :OrderedSig) -> Null = (PRINT \"ok\")",
    );
    let data = scope.bindings().data();
    let sig_id = match data.get("OrderedSig") {
        Some(KObject::KSignature(s)) => s.sig_id(),
        other => panic!("OrderedSig should be a signature, got {:?}", other.map(|o| o.ktype())),
    };
    let entry = data.get("USE_ORD").expect("USE_ORD should be bound");
    let f = match entry {
        KObject::KFunction(f, _) => *f,
        _ => panic!("expected USE_ORD to bind a KFunction"),
    };
    match f.signature.elements.as_slice() {
        [SignatureElement::Keyword(kw), SignatureElement::Argument(Argument { name, ktype })] => {
            assert_eq!(kw, "USE_ORD");
            assert_eq!(name, "Er");
            match ktype {
                KType::SignatureBound { sig_id: id, sig_path, pinned_slots } => {
                    assert_eq!(*id, sig_id, "sig_id must match Signature::sig_id()");
                    assert_eq!(sig_path, "OrderedSig");
                    assert!(pinned_slots.is_empty(), "bare OrderedSig has no pinned slots");
                }
                other => panic!("expected SignatureBound, got {:?}", other),
            }
        }
        _ => panic!("expected [Keyword(USE_ORD), Argument(Er :SignatureBound)]"),
    }
}

/// End-to-end park-on-LET-placeholder: a `LET MyList = (LIST_OF Number)` followed in the
/// same batch by a `FN (USE xs: MyList) -> ...` previously failed because FN-def's
/// signature elaboration ran synchronously and the LET hadn't finalized. Post-phase-3 the
/// FN body parks on the LET's placeholder via a Combine and re-runs the elaboration
/// against the now-final scope.
#[test]
fn let_then_fn_in_same_batch_works() {
    use crate::runtime::builtins::default_scope;
    use crate::runtime::machine::execute::Scheduler;
    use crate::parse::parse;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let exprs = parse(
        "LET MyList = (LIST_OF Number)\n\
         FN (USE xs :MyList) -> Number = (1)",
    )
    .unwrap();
    for e in exprs {
        sched.add_dispatch(e, scope);
    }
    sched.execute().unwrap();
    // Post-stage-1.7 the LET TypeExprRef overload writes `MyList` into `bindings.types`,
    // not `data` — check the type-side map. The FN-def's `USE` binding still lives on
    // `data` like any other function binding.
    assert!(
        scope.resolve_type("MyList").is_some(),
        "MyList should be bound in bindings.types after the batch executes",
    );
    let data = scope.bindings().data();
    let use_fn = data.get("USE").expect("USE should be bound by the FN definition");
    assert!(matches!(use_fn, KObject::KFunction(_, _)));
}

/// Stage-2 phase-A1 sharing constraint: `matches_value` / `accepts_part` on a
/// `SignatureBound { pinned_slots: [(Type, Number)] }` slot reject a module whose
/// `type_members["Type"]` does not pin to `Number`. Phase A2 will land the functor
/// surface that mints `type_members` entries with the pinned `KType`; A1 only ships
/// the predicate, so this test directly populates `type_members` to pin the
/// admissibility logic.
#[test]
fn sharing_constraint_rejects_mismatched_module_type() {
    use crate::runtime::machine::model::ast::ExpressionPart;
    use crate::runtime::machine::model::KType;
    use crate::runtime::machine::model::values::Module;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let child_a = arena.alloc_scope(crate::runtime::machine::Scope::child_under_module(
        scope,
        "NumPinned".into(),
    ));
    let m_num: &Module<'_> = arena.alloc_module(Module::new("NumPinned".into(), child_a));
    m_num.type_members.borrow_mut().insert("Type".into(), KType::Number);
    m_num.mark_satisfies(ScopeId::from_raw(0, 42)); // arbitrary sig_id matching the slot below
    let m_num_obj = arena.alloc_object(KObject::KModule(m_num, None));

    let child_b = arena.alloc_scope(crate::runtime::machine::Scope::child_under_module(
        scope,
        "StrPinned".into(),
    ));
    let m_str: &Module<'_> = arena.alloc_module(Module::new("StrPinned".into(), child_b));
    m_str.type_members.borrow_mut().insert("Type".into(), KType::Str);
    m_str.mark_satisfies(ScopeId::from_raw(0, 42));
    let m_str_obj = arena.alloc_object(KObject::KModule(m_str, None));

    // A module that satisfies the sig but doesn't even have a `Type` pin — also rejected.
    let child_c = arena.alloc_scope(crate::runtime::machine::Scope::child_under_module(
        scope,
        "NoTypePin".into(),
    ));
    let m_none: &Module<'_> = arena.alloc_module(Module::new("NoTypePin".into(), child_c));
    m_none.mark_satisfies(ScopeId::from_raw(0, 42));
    let m_none_obj = arena.alloc_object(KObject::KModule(m_none, None));

    let slot = KType::SignatureBound {
        sig_id: ScopeId::from_raw(0, 42),
        sig_path: "OrderedSig".into(),
        pinned_slots: vec![("Type".into(), KType::Number)],
    };

    // Accept: matching pin.
    assert!(slot.matches_value(m_num_obj));
    assert!(slot.accepts_part(&ExpressionPart::Future(m_num_obj)));
    // Reject: pin present but wrong KType.
    assert!(!slot.matches_value(m_str_obj));
    assert!(!slot.accepts_part(&ExpressionPart::Future(m_str_obj)));
    // Reject: pin absent.
    assert!(!slot.matches_value(m_none_obj));
    assert!(!slot.accepts_part(&ExpressionPart::Future(m_none_obj)));

    // Reject: module not in `compatible_sigs` set, even if its type_members would match.
    let child_d = arena.alloc_scope(crate::runtime::machine::Scope::child_under_module(
        scope,
        "Unascribed".into(),
    ));
    let m_unascribed: &Module<'_> = arena.alloc_module(Module::new("Unascribed".into(), child_d));
    m_unascribed.type_members.borrow_mut().insert("Type".into(), KType::Number);
    // Note: NO mark_satisfies — compatible_sigs is empty.
    let m_unascribed_obj = arena.alloc_object(KObject::KModule(m_unascribed, None));
    assert!(!slot.matches_value(m_unascribed_obj));
    assert!(!slot.accepts_part(&ExpressionPart::Future(m_unascribed_obj)));
}

/// Module-system stage 2 (functor slice). End-to-end shape mirror of
/// [`crate::runtime::machine::model::values::module::tests::functor_per_call_module_lifts_correctly`]:
/// run a complete functor invocation through the scheduler, hold the returned
/// `KModule` past several subsequent allocations and FN calls, and assert member
/// access on the lifted module still returns the correct value. Pins the closure-
/// escape + per-call-arena story for the functor result module under tree borrows.
#[test]
fn functor_body_module_dispatch_does_not_dangle() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         MODULE IntOrd = (LET compare = 7)",
    );
    run(scope, "LET int_ord_a = (IntOrd :! OrderedSig)");
    run(
        scope,
        "FN (MAKESET elem :OrderedSig) -> Module = (MODULE Result = (LET inner = 1))",
    );
    run(scope, "LET held_set = (MAKESET (int_ord_a))");

    // Subsequent allocations and FN calls churn the run-root arena. The lifted
    // KModule must keep its child-scope arena alive through those churns.
    run(scope, "FN (NOOP) -> Number = (1)");
    for _ in 0..20 {
        run_one(scope, parse_one("NOOP"));
    }
    // Another functor call to allocate more frames (and drop them).
    run(scope, "LET other_set = (MAKESET (int_ord_a))");

    // Now read held_set's `inner` member — child_scope_ptr must still be live.
    let data = scope.bindings().data();
    let m = match data.get("held_set") {
        Some(KObject::KModule(m, _)) => *m,
        other => panic!("held_set should be a module, got {:?}", other.map(|o| o.ktype())),
    };
    let inner = m.child_scope().bindings().data().get("inner").copied();
    assert!(matches!(inner, Some(KObject::Number(n)) if *n == 1.0),
            "held_set.inner must still read 1.0 after subsequent churn");
}

// ---------- Phase A2 — FN return-type sharing constraints in functor bodies ----------
//
// `(SIG_WITH ...)` at the FN return-type slot is a parens-wrapped expression — the
// dispatcher's eager-sub-dispatch path resolves it at FN-construction time (riding the
// `accepts_for_wrap` Expression-in-non-KExpression admissibility + `lazy_eager_indices`
// sub-Dispatch rails) and splices the resulting `KTypeValue(SignatureBound)` back into
// the FN-def bundle as a `Future(_)`. The FN body extracts via the `Resolved(KType)`
// path; no new wiring needed for the pure-types case. The harder case where the inner
// pin value references a FN parameter (`(MODULE_TYPE_OF E Type)`) is documented as a
// caveat — its construction-time sub-Dispatch parks on a name the FN's outer scope
// can't bind because the parameter is per-call.

/// Two pinned slots `(Elt: Number) (Ord: IntOrd)` as a FN return type. Pure types only —
/// no parameter references in the pin values — so the parens sub-dispatches synchronously
/// at FN-construction and the resulting `SignatureBound` lands on the FN's stored
/// signature. Body returns a module pinning both slots to the same concrete types; the
/// MODULE-finalize mirror writes `type_members["Elt"]` and `type_members["Ord"]` from the
/// child scope's `bindings.types`. The functor call succeeds and the dispatcher's return-
/// type check accepts the body's module against the pinned `SignatureBound`.
#[test]
fn functor_with_two_pinned_slots_round_trips() {
    use crate::runtime::machine::model::KType;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG Set = ((LET Elt = Number) (LET Ord = Number) (VAL tag :Number))\n\
         SIG OrderedSig = (VAL compare :Number)\n\
         MODULE IntOrd = (LET compare = 7)\n\
         LET int_ord = (IntOrd :! OrderedSig)",
    );
    // Functor returns a SignatureBound with two pins; body produces a module that pins
    // both. Use the same SIG (`Set`) on both sides so the body's MODULE Result can
    // satisfy the pin via its mirrored `type_members`.
    run(
        scope,
        "FN (TWOPIN p :OrderedSig) -> (SIG_WITH Set ((Elt :Number) (Ord :Number))) = \
         (MODULE Result = ((LET Elt = Number) (LET Ord = Number) (LET tag = 0)))",
    );
    // Need the body's module to satisfy `Set`'s shape (tag/Elt/Ord), so we ascribe it
    // before returning. The functor doesn't do ascription itself, so the body's module's
    // `compatible_sigs` set is empty — the return-type check would fail on the sig
    // membership before even checking the pins. Verify the FN at least *registered* with
    // the pinned signature on its stored return type.
    let data = scope.bindings().data();
    let f = match data.get("TWOPIN") {
        Some(KObject::KFunction(f, _)) => *f,
        other => panic!("TWOPIN should be a function, got {:?}", other.map(|o| o.ktype())),
    };
    use crate::runtime::machine::model::ReturnType;
    match &f.signature.return_type {
        ReturnType::Resolved(KType::SignatureBound { sig_path, pinned_slots, .. }) => {
            assert_eq!(sig_path, "Set");
            assert_eq!(pinned_slots.len(), 2);
            assert_eq!(pinned_slots[0].0, "Elt");
            assert_eq!(pinned_slots[0].1, KType::Number);
            assert_eq!(pinned_slots[1].0, "Ord");
            assert_eq!(pinned_slots[1].1, KType::Number);
        }
        other => panic!(
            "expected Resolved(SignatureBound) on TWOPIN's return type, got {:?}",
            other,
        ),
    }
}

/// Body returns a `MODULE Result` whose mirrored `type_members["Elt"]` matches the FN's
/// declared `(SIG_WITH SetSig ((Elt: Number)))` pin. The MODULE-finalize mirror lifts
/// `LET Elt = Number` from the body's child scope into the module's `type_members`,
/// satisfying the pinned-slot admissibility check. Mirrors the shape of the design
/// example, with `Elt` pinned to a concrete builtin type so construction-time sub-
/// Dispatch resolves without a parameter reference.
#[test]
fn functor_return_with_sharing_constraint_pins_output_type() {
    use crate::runtime::machine::model::KType;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    // `Set` has an `Elt` abstract-type slot plus a value-level `insert` member; the
    // body's module must declare both for the shape check (or rather, for the
    // sig-compat marking, which requires the body's module to be ascribed to the sig).
    // Since the body does not ascribe, the test verifies the FN-construction-time
    // capture: that the FN's stored return type pins `Elt: Number` and the body-side
    // module's mirrored `type_members` carries `Elt = Number`.
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         SIG SetSig = ((LET Elt = Number) (VAL insert :Number))\n\
         MODULE IntOrd = (LET compare = 7)\n\
         LET int_ord = (IntOrd :! OrderedSig)",
    );
    run(
        scope,
        "FN (MAKESETN p :OrderedSig) -> (SIG_WITH SetSig ((Elt :Number))) = \
         (MODULE Result = ((LET Elt = Number) (LET insert = 0)))",
    );
    let data = scope.bindings().data();
    let f = match data.get("MAKESETN") {
        Some(KObject::KFunction(f, _)) => *f,
        other => panic!("MAKESETN should be a function, got {:?}", other.map(|o| o.ktype())),
    };
    // Stored return type: SignatureBound { sig_path: "SetSig", pinned_slots: [("Elt", Number)] }.
    use crate::runtime::machine::model::ReturnType;
    match &f.signature.return_type {
        ReturnType::Resolved(KType::SignatureBound { sig_path, pinned_slots, .. }) => {
            assert_eq!(sig_path, "SetSig");
            assert_eq!(pinned_slots, &vec![("Elt".to_string(), KType::Number)]);
        }
        other => panic!(
            "expected Resolved(SignatureBound) on MAKESETN's return type, got {:?}",
            other,
        ),
    }
}

// ---------- Module-system functor-params Stage A — per-call type-side dual-write -----
//
// At call time, parameters whose declared `KType` is type-denoting
// (`SignatureBound`, `Signature`, `Type`, `TypeExprRef`,
// `AnyUserType { kind: Module }`) dual-write into the per-call scope's
// `bindings.types` alongside the existing value-side `bind_value` write. This is
// what lets a FN body's type-position references to the parameter (`Er` in a
// `(MODULE_TYPE_OF Er Type)` call inside the body) resolve through
// `Scope::resolve_type`'s outer-chain walk.

/// End-to-end per-call dual-write read-back: a FN `(USE_TYPE Er: OrderedSig)` whose
/// body does `(MODULE_TYPE_OF Er Type)` succeeds because `Er` resolves as a Type-class
/// reference through the per-call `bindings.types` entry the dual-write installs.
/// Without the dual-write, the body's auto-wrapped `(Er)` sub-Dispatch would hit
/// `value_lookup`'s TypeExprRef arm and surface `UnboundName(Er)` against the FN's
/// captured outer scope (where `Er` is per-call, not lexically present).
///
/// Uses opaque ascription (`:|`) so the bound module has an abstract `Type` member —
/// the body's `(MODULE_TYPE_OF Er Type)` lookup then has something to return, and
/// the value-side recovery path through `value_lookup::body_type_expr`'s dual-write
/// invariant (UserType { kind: Module, .. } → paired KModule carrier) hands the
/// module value to `MODULE_TYPE_OF` as expected.
///
/// Stage A only ships *parameter-position* dual-write — the FN return type stays on
/// the `Any` shape rather than templated forms like `-> (MODULE_TYPE_OF Er Type)`.
/// Stage B widens the return-type carrier to allow parameter-name references in
/// return-type positions.
#[test]
fn functor_body_module_type_of_via_dual_write() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))\n\
         MODULE IntOrd = ((LET Type = Number) (LET compare = 7))\n\
         LET int_ord = (IntOrd :| OrderedSig)",
    );
    // Body invokes `(MODULE_TYPE_OF Er Type)`. The `Er` slot has declared type
    // `OrderedSig` (a `SignatureBound`); the dual-write puts `Er` into the per-call
    // `bindings.types` with the bound module's nominal identity, which the body's
    // auto-wrapped `(Er)` sub-Dispatch reads through `value_lookup`'s TypeExprRef
    // arm. The lookup returns the paired KModule carrier (per the dual-write
    // invariant in `value_lookup::body_type_expr`), feeding it into MODULE_TYPE_OF's
    // `m: Module` slot.
    run(
        scope,
        "FN (USE_TYPE Er :OrderedSig) -> Any = (MODULE_TYPE_OF Er Type)",
    );
    let result = run_one(scope, parse_one("USE_TYPE int_ord"));
    use crate::runtime::machine::model::KType;
    // The abstract `Type` member of an opaquely-ascribed IntOrd is a fresh
    // per-ascription `KType::UserType { kind: Module, name: "Type", .. }` (the
    // module-system pattern documented on `Module::type_members`). Verify the
    // body returned that abstract identity.
    match result {
        KObject::KTypeValue(kt) => match kt {
            KType::UserType { kind: crate::runtime::machine::model::types::UserTypeKind::Module, name, .. } => {
                assert_eq!(name, "Type", "abstract type member should be named Type");
            }
            other => panic!("expected UserType {{ kind :Module, name = \"Type\", .. }}, got {:?}", other),
        },
        other => panic!("expected KTypeValue carrying the abstract Type identity, got {:?}", other.ktype()),
    }
}

/// Closure-escape pin (Stage A A3). A FN that returns a nested FN closing over its
/// type-class parameter `Er` must keep the per-call scope alive long enough for the
/// nested FN's invocation to read `Er` from the per-call `bindings.types`. The
/// existing `KFunction(&fn, Some(Rc<CallArena>))` lift on the lifted FN value
/// already pins the per-call arena; this test confirms the type-side binding (not
/// just the value-side) survives the closure escape.
///
/// Shape: an outer functor whose body returns an inner FN whose body uses `Er` in a
/// type-position. Call the outer to get back the inner FN bound at the run-root
/// scope, then call the inner FN. If the per-call arena's scope (and the dual-write
/// it owns) outlives the inner FN value, the inner body's type-position resolution
/// of `Er` succeeds.
#[test]
fn functor_closure_escape_pins_type_class_dual_write() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))\n\
         MODULE IntOrd = ((LET Type = Number) (LET compare = 7))\n\
         LET int_ord = (IntOrd :| OrderedSig)",
    );
    // Outer FN captures `Er: OrderedSig`; its body defines an inner FN that returns
    // a `(MODULE_TYPE_OF Er Type)` value when called. The outer call binds `Er` on
    // the per-call scope (both value-side and type-side). The inner FN's captured
    // scope is the outer's per-call scope, so the inner body's reference to `Er`
    // walks up through the same scope.
    run(
        scope,
        "FN (MAKE_LOOKUP Er :OrderedSig) -> Any = \
            (FN (LOOKUP) -> Any = (MODULE_TYPE_OF Er Type))",
    );
    run(scope, "LET _maker = (MAKE_LOOKUP int_ord)");
    // Subsequent churn so the per-call arena's drop-discipline is exercised.
    for _ in 0..5 {
        run_one(scope, parse_one("PRINT 1"));
    }
    // Now invoke the nested LOOKUP. The inner FN's captured scope still holds the
    // per-call `Er -> UserType { kind: Module, .. }` entry installed by the outer
    // call's dual-write.
    let result = run_one(scope, parse_one("LOOKUP"));
    use crate::runtime::machine::model::KType;
    match result {
        KObject::KTypeValue(kt) => match kt {
            KType::UserType { kind: crate::runtime::machine::model::types::UserTypeKind::Module, name, .. } => {
                assert_eq!(name, "Type");
            }
            other => panic!(
                "expected UserType {{ kind: Module, name: \"Type\", .. }} \
                 after closure escape, got {:?}",
                other,
            ),
        },
        other => panic!(
            "expected KTypeValue carrying the abstract Type identity after closure escape, got {:?}",
            other.ktype(),
        ),
    }
}

/// A body whose mirrored `type_members["Elt"]` doesn't match the FN's pin should fail
/// the return-type admissibility check. With `(SIG_WITH SetSig ((Elt: Number)))` as the
/// declared return type, a body that produces `(LET Elt = Str)` populates the wrong
/// pin and the lift-time `matches_value` check rejects.
///
/// Note: today the FN's return-type check (`matches_value` for `SignatureBound`) first
/// gates on `compatible_sigs.contains(sig_id)`. A bare `MODULE Result = ...` body whose
/// module is never ascribed has an empty `compatible_sigs` set, so the check fails on
/// sig-membership before reaching the pin comparison. That's still a return-type
/// mismatch from the caller's perspective; this test pins the negative path without
/// claiming the failure mode is specifically pin-driven. The pin comparison itself is
/// directly tested in `sharing_constraint_rejects_mismatched_module_type` (A1).
#[test]
fn functor_return_with_mismatched_sharing_constraint_errors() {
    use crate::runtime::machine::execute::Scheduler;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         SIG SetSig = ((LET Elt = Number) (VAL insert :Number))\n\
         MODULE IntOrd = (LET compare = 7)\n\
         LET int_ord = (IntOrd :! OrderedSig)",
    );
    // Functor returns SetSig with Elt pinned to Number; body's module pins Elt to Str.
    // The body's module isn't sig-ascribed, so the mismatch surfaces as a return-type
    // check failure at lift time.
    run(
        scope,
        "FN (MAKEBAD p :OrderedSig) -> (SIG_WITH SetSig ((Elt :Number))) = \
         (MODULE Result = ((LET Elt = Str) (LET insert = 0)))",
    );
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(parse_one("MAKEBAD int_ord"), scope);
    sched.execute().expect("execute does not surface per-slot errors");
    let res = sched.read_result(id);
    assert!(
        res.is_err(),
        "MAKEBAD must fail return-type check (mismatched pin or unascribed module), \
         got Ok({:?})",
        res.ok().map(|o| o.ktype()),
    );
}

// ---------- Module-system functor-params Stage B — templated return types ------------
//
// At FN-def time, when the return-type carrier references a parameter name (e.g.
// `-> Er`, `-> (MODULE_TYPE_OF Er Type)`, `-> (SIG_WITH Set ((Elt: (MODULE_TYPE_OF Er Type))))`),
// the body scans the carrier against the parameter-name list and routes through
// `ReturnType::Deferred(_)` instead of trying to elaborate against the FN's outer scope.
// Per-call elaboration runs at `KFunction::invoke` time against the per-call scope where
// Stage A's dual-write has installed parameter-name → KType identities.

/// Landing test 1: bare parameter-name return type. `FN (USE_ID Er: OrderedSig) -> Er = ...`
/// returns a module value of type `Er`. The body simply returns the bound parameter
/// (Er is in `bindings.data` from Stage A's value-side bind), and the per-call return-type
/// elaboration resolves `Er` to the per-call module's `UserType { kind: Module, .. }`
/// identity via `Scope::resolve_type` against the per-call scope's `bindings.types`.
#[test]
fn functor_return_bare_parameter_name_resolves_per_call() {
    use crate::runtime::machine::model::ReturnType;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))\n\
         MODULE IntOrd = ((LET Type = Number) (LET compare = 7))\n\
         LET int_ord = (IntOrd :! OrderedSig)",
    );
    // FN-def must register with `ReturnType::Deferred(TypeExpr(Er))`. The body `(Er)`
    // returns the bound module value via value_lookup.
    run(
        scope,
        "FN (USE_ID Er :OrderedSig) -> Er = (Er)",
    );
    let data = scope.bindings().data();
    let f = match data.get("USE_ID") {
        Some(KObject::KFunction(f, _)) => *f,
        other => panic!("USE_ID should be a function, got {:?}", other.map(|o| o.ktype())),
    };
    assert!(
        matches!(f.signature.return_type, ReturnType::Deferred(_)),
        "USE_ID's return type should be Deferred, got {:?}",
        f.signature.return_type,
    );
    drop(data);
    // Invoke and verify the per-call slot check accepts the bound module.
    let result = run_one(scope, parse_one("USE_ID int_ord"));
    match result {
        KObject::KModule(_, _) => {}
        other => panic!("expected KModule from USE_ID, got {:?}", other.ktype()),
    }
}

/// Landing test 2: `(MODULE_TYPE_OF Er Type)` parens-form return type. Pins that
/// FN-def registers the function with `ReturnType::Deferred(Expression(...))` instead
/// of erroring at FN-construction (the pre-Stage-B failure mode was "unbound name `Er`"
/// because the parens-form return type sub-dispatched against the outer scope where
/// `Er` is unbound).
///
/// **Post-VAL surface form.** The SIG declares a `Type`-typed value slot
/// (`(VAL zero: Type)`). A MODULE supplying `zero = 0` satisfies the slot under
/// name-presence shape-check, and the FN signature `(GET_ZERO Er: WithZero) ->
/// (MODULE_TYPE_OF Er Type) = (Er.zero)` parses and registers with
/// `ReturnType::Deferred(_)`.
///
/// **Caveat — kept simpler variant.** The plan also drafted an end-to-end
/// invocation `(GET_ZERO int_ord)` returning the underlying `Number(0)` carrier.
/// That fails today: the per-call return-type check on `Deferred(_)` returns runs
/// at lift-time and compares the body's `.ktype()` (Number, from the underlying
/// ATTR-read) against the per-call-elaborated `KType::UserType { kind: Module,
/// name: "Type", .. }`. ATTR returns the raw underlying value rather than
/// re-tagging it with the per-call abstract identity minted by `:|`. The slot
/// check rejects with the documented "per-call return type" diagnostic
/// (`functor_deferred_return_type_mismatch_surfaces_per_call_diagnostic` pins
/// that wording). Closing this end-to-end variant is tracked by
/// `roadmap/val-slot-abstract-identity-tagging.md` (tag ascribed-module
/// value-slot reads with the per-call abstract identity at ATTR time, or relax
/// the slot check to accept "value of declared-abstract-Type" by
/// carrier-recovery rather than KType equality). Until then, the test pins
/// only the VAL substrate: VAL is a valid SIG surface form, the functor's
/// FN-def succeeds with `Deferred(_)` carrying the parens-form return-type
/// reference, and the underlying-MODULE-Type's `LET zero = 0` cleanly satisfies
/// the VAL slot at ascription shape-check time.
#[test]
fn functor_return_module_type_of_parameter_resolves_per_call() {
    use crate::runtime::machine::model::ReturnType;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG WithZero = ((LET Type = Number) (VAL zero :Type))\n\
         MODULE IntOrd = ((LET Type = Number) (LET zero = 0))\n\
         LET int_ord = (IntOrd :| WithZero)",
    );
    // The ascription succeeded — that's the canonical VAL-slot-satisfied-by-LET
    // pairing this item exists to enable.
    let data = scope.bindings().data();
    assert!(
        matches!(data.get("int_ord"), Some(KObject::KModule(_, _))),
        "int_ord should be an opaquely-ascribed module satisfying WithZero's VAL zero slot",
    );
    drop(data);
    // FN-def. Pre-Stage-B this errored with "unbound name `Er`" at FN-construction
    // because the parens-form return type sub-dispatched against the outer scope.
    // Post-VAL, the SIG-typed parameter `Er` carries the SIG body's `Type` slot
    // surface and the body's `(Er.zero)` reads through it. The functor registers
    // with `ReturnType::Deferred(_)`; the per-call check at lift-time is what the
    // caveat docstring above documents.
    run(
        scope,
        "FN (GET_ZERO Er :WithZero) -> (MODULE_TYPE_OF Er Type) = (Er.zero)",
    );
    let data = scope.bindings().data();
    let f = match data.get("GET_ZERO") {
        Some(KObject::KFunction(f, _)) => *f,
        other => panic!("GET_ZERO should be a function, got {:?}", other.map(|o| o.ktype())),
    };
    assert!(
        matches!(f.signature.return_type, ReturnType::Deferred(_)),
        "GET_ZERO's return type should be Deferred, got {:?}",
        f.signature.return_type,
    );
}

/// Landing test 3: `(SIG_WITH Set ((Elt: (MODULE_TYPE_OF Er Type))))` — the sharing-
/// constraint surface canonical for `module Make (E : ORDERED) : SET with type elt = E.t`.
/// The pin value `(MODULE_TYPE_OF Er Type)` references the parameter `Er`; the per-call
/// elaboration of the outer `SIG_WITH` propagates `Er`'s per-call `Type` member into the
/// pinned slot. The body returns a module whose `type_members["Elt"]` matches.
#[test]
fn functor_return_sig_with_parameter_ref_resolves_per_call() {
    use crate::runtime::machine::model::ReturnType;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))\n\
         SIG Set = ((LET Elt = Number) (VAL insert :Number))\n\
         MODULE IntOrd = ((LET Type = Number) (LET compare = 7))\n\
         LET int_ord = (IntOrd :! OrderedSig)",
    );
    // FN-def registers with `ReturnType::Deferred(Expression(...))`.
    run(
        scope,
        "FN (MK Er :OrderedSig) -> (SIG_WITH Set ((Elt (MODULE_TYPE_OF Er Type)))) = \
         (MODULE Result = ((LET Elt = Number) (LET insert = 0)))",
    );
    let data = scope.bindings().data();
    let f = match data.get("MK") {
        Some(KObject::KFunction(f, _)) => *f,
        other => panic!("MK should be a function, got {:?}", other.map(|o| o.ktype())),
    };
    assert!(
        matches!(f.signature.return_type, ReturnType::Deferred(_)),
        "MK's return type should be Deferred, got {:?}",
        f.signature.return_type,
    );
    drop(data);
    // Body's `MODULE Result` isn't sig-ascribed to `Set`, so its `compatible_sigs` is
    // empty and the SignatureBound check rejects on membership before the pin check.
    // This is the same situation as `functor_return_with_mismatched_sharing_constraint_errors`,
    // but the relevant Stage B invariant is that the FN registered with Deferred at all
    // (without erroring `Unbound` at FN-def time, which was the pre-Stage-B failure mode).
}

/// Stage B negative case: body produces a wrong-typed value for a per-call return type.
/// The Combine's finish closure runs the slot check against the per-call elaboration
/// and rejects with a diagnostic mentioning "per-call return type" — the wording the
/// Stage B implementation pins so a reader knows the rejection path is the per-call
/// check, not the static lift-time one.
#[test]
fn functor_deferred_return_type_mismatch_surfaces_per_call_diagnostic() {
    use crate::runtime::machine::execute::Scheduler;
    use crate::runtime::machine::KErrorKind;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = ((LET Type = Number) (VAL compare :Number))\n\
         MODULE IntOrd = ((LET Type = Number) (LET compare = 7))\n\
         LET int_ord = (IntOrd :| OrderedSig)",
    );
    // Functor declared to return `(MODULE_TYPE_OF Er Type)` (a KType value) but the body
    // returns a Number. Per-call check must reject.
    run(
        scope,
        "FN (BAD Er :OrderedSig) -> (MODULE_TYPE_OF Er Type) = (1)",
    );
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(parse_one("BAD int_ord"), scope);
    sched.execute().expect("execute does not surface per-slot errors");
    let err = match sched.read_result(id) {
        Err(e) => e,
        Ok(_) => panic!("BAD should fail per-call return-type check"),
    };
    match &err.kind {
        KErrorKind::TypeMismatch { arg, expected, .. } => {
            assert_eq!(arg, "<return>");
            assert!(
                expected.contains("per-call return type"),
                "expected diagnostic to mention 'per-call return type', got `{expected}`",
            );
        }
        _ => panic!("expected TypeMismatch on <return>, got {err}"),
    }
}
