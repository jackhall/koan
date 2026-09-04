//! Reads through an opaque view: every VAL member surfaces at the view's own per-call types, and
//! the barrier holds in both directions.
//!
//! An opaque view's child scope is born holding *coerced* member values, so these pin one property
//! across every slot shape — an applied constructor (`:(Number AS Wrap)`), a first-order member, a
//! function-typed slot (coerced at each call, in both directions), a list down to its elements, a
//! record down to its fields, a dict down to its value cells, and a union's inhabited member. The
//! transparent (`:!`) and unascribed modes stay concrete, and a member the signature does not name
//! replays unchanged.
//!
//! The same members read by bare name inside a `USING <view> SCOPE` window are pinned in
//! [`crate::builtins::using_scope`]'s own suite, which borrows this very binding table.

use crate::builtins::test_support::{TestRun, lookup_module, type_name};
use crate::machine::model::{KObject, KType, TypeNode};
use crate::machine::{program_storage, run_root_storage};

/// The shared fixture: an identity-wrapper family `Wrapper`, a first-order newtype `Carrier`, and
/// a signature naming both through abstract members — so one module exercises an applied
/// constructor slot, a first-order slot, two function-typed slots (one taking an abstract-typed
/// argument), and a list, record, dict and union slot at once. `hidden` is deliberately absent
/// from the signature: it is the width-subtyping extra whose read must not change.
fn monad_program() -> &'static str {
    "NEWTYPE (Type AS Wrapper)\n\
     NEWTYPE Carrier = Number\n\
     SIG Monad = ((TYPE (Type AS Wrap)) (TYPE Elt) \
     (VAL boxed :(Number AS Wrap)) (VAL zero :Elt) \
     (VAL pure :(FN :{x :Number} -> :(Number AS Wrap))) \
     (VAL unbox :(FN :{w :(Number AS Wrap)} -> Number)) \
     (VAL zs :(LIST OF Elt)) (VAL pair :{first :Elt, tag :Number}) \
     (VAL table :(MAP Str -> Elt)) (VAL choice :(Elt | Str)))\n\
     MODULE id_monad = ((LET Wrap = Wrapper) (LET Elt = Carrier) \
     (LET boxed = (Wrapper (3))) (LET zero = (Carrier 0)) \
     (LET pure = FN (PURE x :Number) -> :(Number AS Wrapper) = (Wrapper (x))) \
     (LET unbox = FN (UNBOX w :(Number AS Wrapper)) -> Number = (7)) \
     (LET zs = [(Carrier 1), (Carrier 2)]) \
     (LET pair = {first = (Carrier 9), tag = 4}) \
     (LET table = {\"a\": (Carrier 6)}) \
     (LET choice = (Carrier 8)) \
     (LET hidden = 42))"
}

/// The view's own per-call binding for one of the signature's abstract members.
fn view_member(test_run: &TestRun<'_>, view: &str, member: &str) -> KType {
    lookup_module(test_run.scope, view, test_run.registries())
        .type_members
        .get(&type_name(member, test_run.registries()))
        .copied()
        .unwrap_or_else(|| panic!("an opaque view mints a `{member}` member"))
}

/// The constructor a `Wrapped` value's identity applies, or the identity itself when it is not an
/// application — what tells an applied-constructor value's family apart from another's.
fn applied_constructor(kt: KType, test_run: &TestRun<'_>) -> KType {
    test_run.types().with_node(kt, |node| match node {
        TypeNode::ConstructorApply { constructor, .. } => *constructor,
        _ => kt,
    })
}

// ---------- Applied-constructor slots ----------

/// **AC1.** A VAL slot typed with an applied abstract constructor reads through the view as an
/// application of the *view's* per-call `Wrap` mint — not the source's `Wrapper`.
#[test]
fn applied_slot_reads_at_the_views_own_constructor() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(monad_program());
    test_run.run("LET view = (id_monad :| Monad)");
    let wrap = view_member(&test_run, "view", "Wrap");

    let boxed = test_run.run_one(test_run.parse_one("view.boxed"));
    let KObject::Wrapped { inner, type_id } = boxed else {
        panic!(
            "expected a Wrapped from an applied slot, got {}",
            boxed.ktype().name(test_run.registries())
        );
    };
    assert!(
        matches!(
            test_run.types().node(*type_id),
            TypeNode::ConstructorApply { .. }
        ),
        "the read must stay an applied constructor, got {}",
        type_id.name(test_run.registries()),
    );
    assert_eq!(
        applied_constructor(*type_id, &test_run),
        wrap,
        "the applied constructor must be the view's own `Wrap` mint",
    );
    assert!(
        matches!(inner.payload(), KObject::Number(n) if *n == 3.0),
        "the representation rides through the coercion unchanged",
    );
}

/// **AC1, the barrier outward.** The value read through the view no longer satisfies the *source*
/// constructor's applied type, so a function declared over it is a dispatch miss.
#[test]
fn applied_slot_read_fails_source_side_dispatch() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(monad_program());
    test_run.run("LET view = (id_monad :| Monad)");
    test_run.run("FN (TAKESRC x :(Number AS Wrapper)) -> Number = (1)");
    let err = test_run.run_one_err(test_run.parse_one("TAKESRC (view.boxed)"));
    assert!(
        matches!(&err.kind, crate::machine::KErrorKind::DispatchFailed { .. }),
        "a view-typed value must not satisfy the source's applied type, got {err}",
    );
}

/// **AC1, the barrier inward.** `:(Number AS view.Wrap)` in type position resolves to an
/// application over the view's mint, so it admits the view's own member and rejects a value built
/// with the source constructor.
#[test]
fn applied_view_type_admits_the_view_and_rejects_the_source() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(monad_program());
    test_run.run("LET view = (id_monad :| Monad)");
    test_run.run("FN (TAKEVIEW x :(Number AS view.Wrap)) -> Number = (2)");

    let admitted = test_run.run_one(test_run.parse_one("TAKEVIEW (view.boxed)"));
    assert!(
        matches!(admitted, KObject::Number(n) if *n == 2.0),
        "the view's own member must satisfy `:(Number AS view.Wrap)`",
    );
    let err = test_run.run_one_err(test_run.parse_one("TAKEVIEW (Wrapper (3))"));
    assert!(
        matches!(&err.kind, crate::machine::KErrorKind::DispatchFailed { .. }),
        "a source-built value must not satisfy the view's applied type, got {err}",
    );
}

/// **AC — type position, all three view modes.** `:(Number AS <module>.Wrap)` elaborates to a
/// `ConstructorApply` over that module's *own* `Wrap` member: the per-call mint for an opaque
/// view, the source constructor for a transparent view and for the unascribed module.
#[test]
fn dotted_constructor_applies_the_named_modules_own_member() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(monad_program());
    test_run.run("LET view = (id_monad :| Monad)");
    test_run.run("LET tview = (id_monad :! Monad)");
    let source = test_run.run_one_type(test_run.parse_one(":Wrapper"));

    for (module, expected) in [
        ("view", view_member(&test_run, "view", "Wrap")),
        ("tview", source),
        ("id_monad", source),
    ] {
        let applied =
            test_run.run_one_type(test_run.parse_one(&format!(":(Number AS {module}.Wrap)")));
        assert!(
            matches!(
                test_run.types().node(applied),
                TypeNode::ConstructorApply { .. }
            ),
            "`:(Number AS {module}.Wrap)` must elaborate to an application, got {}",
            applied.name(test_run.registries()),
        );
        assert_eq!(
            applied_constructor(applied, &test_run),
            expected,
            "`:(Number AS {module}.Wrap)` must apply `{module}`'s own `Wrap`",
        );
    }
}

/// **Generativity.** Two opaque ascriptions of one module mint distinct constructors, so
/// per-view overloads do not collide and each view's member picks its own.
#[test]
fn two_views_of_one_module_are_generative() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(monad_program());
    test_run.run("LET viewa = (id_monad :| Monad)");
    test_run.run("LET viewb = (id_monad :| Monad)");
    assert_ne!(
        view_member(&test_run, "viewa", "Wrap"),
        view_member(&test_run, "viewb", "Wrap"),
        "two ascriptions of one module must mint distinct `Wrap` constructors",
    );
    test_run.run("FN (PICK x :(Number AS viewa.Wrap)) -> Number = (7)");
    test_run.run("FN (PICK x :(Number AS viewb.Wrap)) -> Number = (8)");
    let picked = test_run.run_one(test_run.parse_one("PICK (viewa.boxed)"));
    assert!(
        matches!(picked, KObject::Number(n) if *n == 7.0),
        "viewa's member must pick viewa's overload, got {}",
        picked.summary(test_run.registries()),
    );
}

/// **AC2 — the transparent mode.** `:(Number AS tview.Wrap)` resolves to the *source*
/// constructor, so a source-built value satisfies it and a transparent read stays concrete.
#[test]
fn transparent_view_applied_type_names_the_source_constructor() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(monad_program());
    test_run.run("LET tview = (id_monad :! Monad)");
    test_run.run("FN (TAKETRANS x :(Number AS tview.Wrap)) -> Number = (5)");
    for expression in ["TAKETRANS (Wrapper (1))", "TAKETRANS (tview.boxed)"] {
        let result = test_run.run_one(test_run.parse_one(expression));
        assert!(
            matches!(result, KObject::Number(n) if *n == 5.0),
            "`{expression}` must satisfy the transparent view's applied type, got {}",
            result.summary(test_run.registries()),
        );
    }
}

/// **AC2 — the unascribed mode.** A plain module's `Wrap` member is the source constructor
/// itself, so `:(Number AS id_monad.Wrap)` admits a source-built value.
#[test]
fn unascribed_module_applied_type_names_the_source_constructor() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(monad_program());
    test_run.run("FN (TAKEPLAIN x :(Number AS id_monad.Wrap)) -> Number = (6)");
    let result = test_run.run_one(test_run.parse_one("TAKEPLAIN (Wrapper (1))"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 6.0),
        "an unascribed module's `Wrap` is the source constructor",
    );
}

// ---------- Function-typed slots ----------

/// **The function boundary, outward.** Calling a function-typed slot through the view returns a
/// value at the *view's* applied type: the wrapper's `CoercedDelegate` runs the source function
/// and rewrites its result on the way out.
#[test]
fn function_slot_result_carries_the_views_type() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(monad_program());
    test_run.run("LET view = (id_monad :| Monad)");
    let wrap = view_member(&test_run, "view", "Wrap");

    let result = test_run.run_one(test_run.parse_one("view.pure {x = 5}"));
    let KObject::Wrapped { inner, type_id } = result else {
        panic!(
            "expected a Wrapped from the function slot, got {}",
            result.ktype().name(test_run.registries())
        );
    };
    assert_eq!(
        applied_constructor(*type_id, &test_run),
        wrap,
        "the call's result must carry the view's own `Wrap` mint",
    );
    assert!(
        matches!(inner.payload(), KObject::Number(n) if *n == 5.0),
        "the underlying function's own computation is unchanged",
    );
}

/// The same result, seen from the dispatch side: it is admitted where the view's applied type is
/// expected and rejected where the source's is.
#[test]
fn function_slot_result_holds_the_barrier_in_both_directions() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(monad_program());
    test_run.run("LET view = (id_monad :| Monad)");
    test_run.run("FN (TAKEVIEW x :(Number AS view.Wrap)) -> Number = (2)");
    test_run.run("FN (TAKESRC x :(Number AS Wrapper)) -> Number = (1)");

    let admitted = test_run.run_one(test_run.parse_one("TAKEVIEW (view.pure {x = 1})"));
    assert!(
        matches!(admitted, KObject::Number(n) if *n == 2.0),
        "the call's result must satisfy the view's applied type",
    );
    let err = test_run.run_one_err(test_run.parse_one("TAKESRC (view.pure {x = 1})"));
    assert!(
        matches!(&err.kind, crate::machine::KErrorKind::DispatchFailed { .. }),
        "and must not satisfy the source's, got {err}",
    );
}

/// **The function boundary, inward.** A slot whose declared *parameter* names an abstract member
/// takes the view's types: a value read through the view is admitted (and reaches the underlying
/// function inhabiting the source's types, or its body's `:(Number AS Wrapper)` parameter would
/// not bind), while a source-built value is a dispatch miss at the wrapper's own signature.
#[test]
fn function_slot_coerces_its_argument_inward() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(monad_program());
    test_run.run("LET view = (id_monad :| Monad)");

    let result = test_run.run_one(test_run.parse_one("view.unbox {w = (view.boxed)}"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 7.0),
        "a view-typed argument must reach the underlying function, got {}",
        result.summary(test_run.registries()),
    );
    let err = test_run.run_one_err(test_run.parse_one("view.unbox {w = (Wrapper (3))}"));
    assert!(
        matches!(&err.kind, crate::machine::KErrorKind::TypeMismatch { .. }),
        "a source-built argument must not cross the barrier inward, got {err}",
    );
}

// ---------- Compound slots ----------

/// **A list slot.** The list reads at the view's element type, and an element pulled off it
/// carries the view's identity rather than the source's `Carrier`.
#[test]
fn list_slot_reads_at_the_views_element_type() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(monad_program());
    test_run.run("LET view = (id_monad :| Monad)");
    let elt = view_member(&test_run, "view", "Elt");

    let zs = test_run.run_one(test_run.parse_one("view.zs"));
    assert_eq!(
        zs.ktype(),
        test_run.types().list(elt),
        "the list slot must read as a list of the view's own `Elt`",
    );
    let KObject::List(substrate, _) = zs else {
        panic!("expected a List from the list slot");
    };
    let first = substrate
        .elements()
        .first()
        .and_then(|cell| cell.as_object())
        .expect("the list slot holds object elements");
    assert_eq!(
        first.ktype(),
        elt,
        "each element must carry the view's `Elt`, not the source's `Carrier`",
    );
}

/// **A record slot.** A field whose declared type names an abstract member reads at the view's
/// identity; a field the slot types concretely is untouched.
#[test]
fn record_slot_coerces_its_abstract_typed_field() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(monad_program());
    test_run.run("LET view = (id_monad :| Monad)");
    let elt = view_member(&test_run, "view", "Elt");

    let first = test_run.run_one(test_run.parse_one("view.pair.first"));
    assert_eq!(
        first.ktype(),
        elt,
        "the abstract-typed field must read at the view's `Elt`",
    );
    let tag = test_run.run_one(test_run.parse_one("view.pair.tag"));
    assert_eq!(
        tag.ktype(),
        KType::NUMBER,
        "a concretely-typed field crosses no barrier",
    );
}

/// **A dict slot.** The value cells coerce and the dict's own type re-stamps. Keys do not: a
/// `KKey` is a concrete scalar with no type identity to carry, so a declared key type naming an
/// abstract member would re-stamp the dict's type only — the documented limitation.
#[test]
fn dict_slot_coerces_its_value_cells() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(monad_program());
    test_run.run("LET view = (id_monad :| Monad)");
    let elt = view_member(&test_run, "view", "Elt");

    let table = test_run.run_one(test_run.parse_one("view.table"));
    assert_eq!(
        table.ktype(),
        test_run.types().dict(KType::STR, elt),
        "the dict slot must read as a dict of the view's own `Elt`",
    );
    let KObject::Dict(substrate, _) = table else {
        panic!("expected a Dict from the dict slot");
    };
    let cell = substrate
        .entries()
        .next()
        .and_then(|(_, cell)| cell.as_object())
        .expect("the dict slot holds one object value cell");
    assert_eq!(
        cell.ktype(),
        elt,
        "each value cell must carry the view's `Elt`, not the source's `Carrier`",
    );
}

/// **A union slot.** A value inhabits exactly one declared member, so the coercion is that
/// member's: the `Elt` arm re-tags and the `Str` arm would pass through untouched.
#[test]
fn union_slot_coerces_the_inhabited_member() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(monad_program());
    test_run.run("LET view = (id_monad :| Monad)");

    let choice = test_run.run_one(test_run.parse_one("view.choice"));
    assert_eq!(
        choice.ktype(),
        view_member(&test_run, "view", "Elt"),
        "the inhabited union member must read at the view's `Elt`",
    );
}

// ---------- Deferred functor returns ----------

/// **AC3.** A functor parameter's deferred return `-> :(Number AS er.Wrap)` elaborates per call
/// against the argument module's own `Wrap`: an unascribed argument returns a source-applied
/// value, an opaque-view argument the view's — and the latter fails source-side dispatch.
#[test]
fn deferred_return_elaborates_per_argument_module() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(monad_program());
    test_run.run("LET view = (id_monad :| Monad)");
    test_run.run("FN (REBOX er :Monad) -> :(Number AS er.Wrap) = (er.boxed)");
    test_run.run("FN (TAKESRC x :(Number AS Wrapper)) -> Number = (1)");

    let plain = test_run.run_one(test_run.parse_one("REBOX (id_monad)"));
    assert_eq!(
        applied_constructor(plain.ktype(), &test_run),
        test_run.run_one_type(test_run.parse_one(":Wrapper")),
        "an unascribed argument's deferred return is the source constructor's application",
    );

    let through_view = test_run.run_one(test_run.parse_one("REBOX (view)"));
    assert_eq!(
        applied_constructor(through_view.ktype(), &test_run),
        view_member(&test_run, "view", "Wrap"),
        "an opaque-view argument's deferred return is that view's own application",
    );
    let err = test_run.run_one_err(test_run.parse_one("TAKESRC (REBOX (view))"));
    assert!(
        matches!(&err.kind, crate::machine::KErrorKind::DispatchFailed { .. }),
        "the returned value must not satisfy the source's applied type, got {err}",
    );
}

/// An opaque view minted **inside a per-call frame** and returned from it, then read through every
/// coerced shape after that frame is gone.
///
/// Coercion is where a member value stops being the source's own seal: a re-tagged member shares
/// the source's payload substrate, a rebuilt list's cells hold borrows into it, and a function
/// slot's wrapper is a callable born in the *source* module's region that the view's region only
/// points at. All three are borrow leaves the coercion door's retention claim has to keep —
/// `Scope::coerce_delivered` claims the pin, so the source region rides in the view region's union
/// bundle, and the returned view carries that whole closure out of its minting frame.
///
/// A Miri audit-slate test: a release claim that dropped either region would free the very storage
/// these reads walk, which only tree borrows observes — a normal build reads the freed bytes back
/// intact. The reads run after the minting frame is gone and probe each shape by content.
#[test]
fn a_returned_opaque_view_keeps_every_coerced_member_alive() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(monad_program());
    test_run.run("FN (MAKEVIEW er :Monad) -> Module = (er :| Monad)");
    test_run.run("LET made = (MAKEVIEW id_monad)");
    let wrap = view_member(&test_run, "made", "Wrap");
    let elt = view_member(&test_run, "made", "Elt");

    // The re-tagged member: its identity is the dead frame's mint, its payload the source's own
    // substrate.
    let boxed = test_run.run_one(test_run.parse_one("made.boxed"));
    assert_eq!(applied_constructor(boxed.ktype(), &test_run), wrap);
    assert!(matches!(
        boxed,
        KObject::Wrapped { inner, .. } if matches!(inner.payload(), KObject::Number(n) if *n == 3.0)
    ));

    // The rebuilt list: cells sectioned in the view's region over the source's element payloads.
    let zs = test_run.run_one(test_run.parse_one("made.zs"));
    assert_eq!(zs.ktype(), test_run.types().list(elt));
    let KObject::List(substrate, _) = zs else {
        panic!("expected a List");
    };
    for cell in substrate.elements() {
        let element = cell.as_object().expect("object elements");
        assert_eq!(element.ktype(), elt);
    }

    // The coercion wrapper: a callable born in the source module's region, called through the view
    // after the frame that minted the view is gone.
    let called = test_run.run_one(test_run.parse_one("made.pure {x = 5}"));
    assert_eq!(applied_constructor(called.ktype(), &test_run), wrap);
    assert!(matches!(
        called,
        KObject::Wrapped { inner, .. } if matches!(inner.payload(), KObject::Number(n) if *n == 5.0)
    ));
}

// ---------- Regressions ----------

/// **AC 4.** A member the signature does not name is *absent* from the view. Width subtyping is a
/// property of the matching relation — `id_monad` satisfies `Monad` while binding more than it
/// declares — never of the view the match produces.
#[test]
fn a_member_the_signature_omits_is_absent_from_the_view() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(monad_program());
    test_run.run("LET view = (id_monad :| Monad)");
    let error = test_run.run_one_err(test_run.parse_one("view.hidden"));
    assert!(
        error.to_string().contains("no member `hidden`"),
        "an undeclared member must not be reachable through the view, got {error}",
    );
    // The source is untouched: pruning shapes the view, not the module it was taken of.
    let hidden = test_run.run_one(test_run.parse_one("id_monad.hidden"));
    assert!(
        matches!(hidden, KObject::Number(n) if *n == 42.0),
        "the source module still binds the member the view drops",
    );
}

/// A transparent view binds every abstract member to the source's own concrete type, so the two
/// substitutions of each slot agree and nothing is coerced: reads stay concrete on every shape.
#[test]
fn transparent_view_reads_stay_concrete() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(monad_program());
    test_run.run("LET tview = (id_monad :! Monad)");
    test_run.run("FN (TAKESRC x :(Number AS Wrapper)) -> Number = (1)");

    for expression in ["TAKESRC (tview.boxed)", "TAKESRC (tview.pure {x = 2})"] {
        let result = test_run.run_one(test_run.parse_one(expression));
        assert!(
            matches!(result, KObject::Number(n) if *n == 1.0),
            "`{expression}` must stay at the source's applied type, got {}",
            result.summary(test_run.registries()),
        );
    }
}
