//! Nested signatures in a slot type: a SIG value slot naming another signature *inside* a
//! compound type, pinned to the enclosing signature's own abstract member.
//!
//! `VAL subs :(LIST OF (Inner WITH {Item = Elt}))` says "a list of `Inner`s over *my* element
//! type". Satisfying it means substitution, satisfaction and canonicalization all recurse through
//! the nested signature; reading through the view means the nested module is itself born as a
//! coerced view, so its members report the outer view's identities on the same terms every other
//! slot shape does.

use crate::builtins::test_support::{
    TestRun, binds_module, lookup_module, lookup_type, type_name, value_name,
};
use crate::machine::KErrorKind;
use crate::machine::model::{KObject, KType, SigSchema, TypeNode};
use crate::machine::{program_storage, run_root_storage};

/// The running example: an inner interface over one abstract member, an outer interface whose
/// only slot is a list of inners pinned to the outer's own `Elt`, and two modules — one supplying
/// inners over its own `Elt` binding, one over an unrelated newtype.
fn nested_program() -> &'static str {
    "NEWTYPE Carrier = Number\n\
     NEWTYPE Unrelated = Number\n\
     SIG Inner = ((TYPE Item) (VAL one :Item))\n\
     SIG Outer = ((TYPE Elt) (VAL subs :(LIST OF (Inner WITH {Item = Elt}))))\n\
     MODULE carrier_inner = ((LET Item = Carrier) (LET one = (Carrier 1)) (LET extra = 42))\n\
     MODULE unrelated_inner = ((LET Item = Unrelated) (LET one = (Unrelated 1)))\n\
     MODULE matching = ((LET Elt = Carrier) (LET subs = [carrier_inner]))\n\
     MODULE mismatched = ((LET Elt = Carrier) (LET subs = [unrelated_inner]))"
}

/// The view's own per-call binding for one of the ascribed signature's abstract members.
fn view_member(test_run: &TestRun<'_>, view: &str, member: &str) -> KType {
    lookup_module(test_run.scope, view, test_run.registries())
        .type_members
        .get(&type_name(member, test_run.registries()))
        .copied()
        .unwrap_or_else(|| panic!("an opaque view mints a `{member}` member"))
}

/// The single element of a list-valued member read off `expr`.
fn only_element<'a>(test_run: &mut TestRun<'a>, expr: &str) -> &'a KObject<'a> {
    let parsed = test_run.parse_one(expr);
    let list = test_run.run_one(parsed);
    let KObject::List(substrate, _) = list else {
        panic!("expected `{expr}` to read as a list");
    };
    substrate
        .elements()
        .first()
        .and_then(|cell| cell.as_object())
        .expect("the list holds one object element")
}

/// A module value's self-sig schema.
fn self_sig_of(object: &KObject<'_>, test_run: &TestRun<'_>) -> SigSchema {
    let KObject::Module(m) = object else {
        panic!("expected a module value");
    };
    m.self_sig(test_run.types())
}

/// **AC 4.** A module supplying a list of `Inner`s over its own `Elt` binding satisfies the SIG
/// that declares the nested-signature slot.
#[test]
fn a_matching_module_ascribes() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(nested_program());
    test_run.run("LET view = (matching :| Outer)");
    assert!(
        binds_module(test_run.scope, "view"),
        "the ascription must produce a view module",
    );
}

/// **AC 4.** A module whose `subs` elements are `Inner`s over some *other* type is refused: the
/// nested substitution binds the outer `Elt` to `Carrier`, and `Unrelated` does not match it.
#[test]
fn a_module_over_another_type_is_refused() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(nested_program());
    let error = test_run.run_one_err(test_run.parse_one("mismatched :| Outer"));
    assert!(
        matches!(&error.kind, KErrorKind::ShapeError(msg)
            if msg.contains("does not satisfy signature") && msg.contains("`subs`")),
        "expected a ShapeError naming the offending slot `subs`, got {error}",
    );
}

/// **AC 6.** The member filling the nested-signature slot reads through the view at the *view's*
/// types: each element is a module whose `Item` member and whose `one` slot both report the
/// view's per-call `Elt` mint, not the source's `Carrier`.
#[test]
fn a_nested_member_reads_at_the_views_types() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(nested_program());
    test_run.run("LET view = (matching :| Outer)");
    let elt = view_member(&test_run, "view", "Elt");
    let carrier = lookup_type(test_run.scope, "Carrier").expect("`Carrier` is declared");
    assert_ne!(elt, carrier, "an opaque view mints a fresh `Elt`");

    let element = only_element(&mut test_run, "view.subs");
    let KObject::Module(inner) = element else {
        panic!("the nested slot's elements are module values");
    };
    let item = type_name("Item", test_run.registries());
    assert_eq!(
        inner.type_members.get(&item).copied(),
        Some(elt),
        "the nested view's `Item` member must be the outer view's `Elt` mint",
    );
    let one = self_sig_of(element, &test_run)
        .value_slots
        .get(&value_name("one", test_run.registries()))
        .copied();
    assert_eq!(
        one,
        Some(elt),
        "the nested view's `one` slot must read at the outer view's `Elt`",
    );
}

/// The coerced list inhabits the type it is stamped with: its element type is the substituted
/// nested signature, and each element genuinely satisfies it. A list whose elements did not
/// coerce would carry a type none of them matches.
#[test]
fn the_coerced_list_inhabits_its_own_element_type() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(nested_program());
    test_run.run("LET view = (matching :| Outer)");

    let parsed = test_run.parse_one("view.subs");
    let subs = test_run.run_one(parsed);
    let element_type = test_run.types().with_node(subs.ktype(), |node| match node {
        TypeNode::List { element } => *element,
        _ => panic!("`subs` reads as a list"),
    });
    assert!(
        test_run.types().with_node(element_type, |node| matches!(
            node,
            TypeNode::Signature { .. }
        )),
        "the element type is the substituted nested signature",
    );
    assert!(
        element_type.matches_value(
            only_element(&mut test_run, "view.subs"),
            test_run.registries()
        ),
        "each coerced element must inhabit the element type the list is stamped with",
    );
}

/// **AC 4.** The nested boundary narrows on the same terms the outer one does: a member the nested
/// signature does not name is absent from the nested view, even though the source element binds it
/// (which is what let the element satisfy the nested signature in the first place).
#[test]
fn the_nested_view_drops_a_member_its_signature_omits() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(nested_program());
    test_run.run("LET view = (matching :| Outer)");

    let element = only_element(&mut test_run, "view.subs");
    let extra = self_sig_of(element, &test_run)
        .value_slots
        .get(&value_name("extra", test_run.registries()))
        .copied();
    assert_eq!(
        extra, None,
        "a member the nested signature does not name is pruned from the nested view",
    );
}

/// **AC 2.** A nested signature declaring its own member of the enclosing signature's name
/// shadows it: `Shadowing`'s `Elt` is its *own* binder, so the outer `Elt` is inexpressible in
/// that position and the slot substitutes to nothing. A module binding the nested `Elt` to any
/// type at all therefore satisfies the outer signature.
#[test]
fn a_nested_binder_shadows_the_enclosing_one() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "NEWTYPE Carrier = Number\n\
         NEWTYPE Unrelated = Number\n\
         SIG Shadowing = ((TYPE Elt) (VAL one :Elt))\n\
         SIG Host = ((TYPE Elt) (VAL subs :(LIST OF Shadowing)))\n\
         MODULE shadowed_inner = ((LET Elt = Unrelated) (LET one = (Unrelated 1)))\n\
         MODULE host = ((LET Elt = Carrier) (LET subs = [shadowed_inner]))",
    );
    test_run.run("LET view = (host :| Host)");
    assert!(
        binds_module(test_run.scope, "view"),
        "a nested binder shadows the enclosing one, so the inner `Elt` is free to be any type",
    );
    // The nested member's own `Elt` stays at its source binding: shadowing means the outer plan
    // never reaches it.
    let unrelated = lookup_type(test_run.scope, "Unrelated").expect("`Unrelated` is declared");
    let element = only_element(&mut test_run, "view.subs");
    let KObject::Module(inner) = element else {
        panic!("the nested slot's elements are module values");
    };
    assert_eq!(
        inner
            .type_members
            .get(&type_name("Elt", test_run.registries()))
            .copied(),
        Some(unrelated),
    );
}

/// Transparent ascription stays concrete through the nesting, as it does at every other slot
/// shape: the nested member's `Item` still reads the source's `Carrier`.
#[test]
fn transparent_ascription_stays_concrete_through_the_nesting() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(nested_program());
    test_run.run("LET plain = (matching :! Outer)");
    let carrier = lookup_type(test_run.scope, "Carrier").expect("`Carrier` is declared");

    let element = only_element(&mut test_run, "plain.subs");
    let KObject::Module(inner) = element else {
        panic!("the nested slot's elements are module values");
    };
    assert_eq!(
        inner
            .type_members
            .get(&type_name("Item", test_run.registries()))
            .copied(),
        Some(carrier),
        "a transparent view hides nothing, so the nested member keeps the source's type",
    );
}

/// A function-typed slot *inside* the nested signature takes the eta-wrapper the `KFunction` arm
/// builds, carrying the narrowed plan: the wrapper's own signature is written in the outer view's
/// types, and a call through it round-trips.
#[test]
fn a_function_slot_inside_the_nested_signature_is_wrapped() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "NEWTYPE Carrier = Number\n\
         SIG Applier = ((TYPE Item) (VAL apply :(FN :{x :Item} -> Item)))\n\
         SIG Host = ((TYPE Elt) (VAL subs :(LIST OF (Applier WITH {Item = Elt}))))\n\
         MODULE carrier_applier = ((LET Item = Carrier) \
         (LET apply = FN (APPLY x :Carrier) -> Carrier = (x)))\n\
         MODULE host = ((LET Elt = Carrier) (LET subs = [carrier_applier]))",
    );
    test_run.run("LET view = (host :| Host)");
    let elt = view_member(&test_run, "view", "Elt");

    let element = only_element(&mut test_run, "view.subs");
    let apply = self_sig_of(element, &test_run)
        .value_slots
        .get(&value_name("apply", test_run.registries()))
        .copied()
        .expect("the nested view carries its `apply` slot");
    let (params, ret) = test_run.types().with_node(apply, |node| match node {
        TypeNode::KFunction { params, ret } => (params.clone(), *ret),
        _ => panic!("`apply` reads as a function type"),
    });
    assert_eq!(ret, elt, "the wrapper's return is the view's own `Elt`");
    assert!(
        params.values().all(|p| *p == elt),
        "the wrapper's parameter is the view's own `Elt`",
    );
}

/// Depth two: a signature nested inside a signature nested inside a list. The recursion composes
/// because the nested coercion re-enters the same walk through the replay's coerced members.
#[test]
fn the_recursion_composes_at_depth_two() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "NEWTYPE Carrier = Number\n\
         SIG Leaf = ((TYPE Item) (VAL one :Item))\n\
         SIG Middle = ((TYPE Cell) (VAL leaves :(LIST OF (Leaf WITH {Item = Cell}))))\n\
         SIG Top = ((TYPE Elt) (VAL middles :(LIST OF (Middle WITH {Cell = Elt}))))\n\
         MODULE leaf = ((LET Item = Carrier) (LET one = (Carrier 1)))\n\
         MODULE middle = ((LET Cell = Carrier) (LET leaves = [leaf]))\n\
         MODULE top = ((LET Elt = Carrier) (LET middles = [middle]))",
    );
    test_run.run("LET view = (top :| Top)");
    let elt = view_member(&test_run, "view", "Elt");

    let middle = only_element(&mut test_run, "view.middles");
    let KObject::Module(middle_view) = middle else {
        panic!("the outer slot's elements are module values");
    };
    assert_eq!(
        middle_view
            .type_members
            .get(&type_name("Cell", test_run.registries()))
            .copied(),
        Some(elt),
        "the depth-one view binds `Cell` to the top view's `Elt`",
    );

    // The depth-one view's own `leaves` member, read straight off the scope it was born holding.
    let leaves = middle_view
        .child_scope()
        .lookup("leaves")
        .expect("the depth-one view carries its `leaves` member");
    let KObject::List(substrate, _) = leaves else {
        panic!("`leaves` reads as a list");
    };
    let leaf = substrate
        .elements()
        .first()
        .and_then(|cell| cell.as_object())
        .expect("the list holds one object element");
    let KObject::Module(leaf_view) = leaf else {
        panic!("the inner slot's elements are module values");
    };
    assert_eq!(
        leaf_view
            .type_members
            .get(&type_name("Item", test_run.registries()))
            .copied(),
        Some(elt),
        "the depth-two view binds `Item` to the same top-level `Elt` mint",
    );
}

/// An outer view minted **inside a per-call frame** and returned from it, then read down through
/// its nested member after that frame is gone.
///
/// The nested view is the one coerced shape whose product is a *whole scope and module* — born in
/// the **source** module's region from inside the coercion fold's closure, with a binding table,
/// a type table, a re-homed path and a self-sig all bumped there, which the outer view's region
/// only points at. That is a fourth borrow leaf on top of the re-tag, the rebuilt list and the
/// function wrapper, and `Scope::coerce_delivered`'s pin is what keeps it: the source region rides
/// in the view region's union bundle, and the returned view carries the whole closure out of its
/// minting frame.
///
/// A Miri audit-slate test: a release claim that dropped the source region would free the very
/// storage these reads walk — the nested scope's tables among it — which only tree borrows
/// observes, since a normal build reads the freed bytes back intact.
#[test]
fn a_returned_view_keeps_its_nested_member_alive() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(nested_program());
    test_run.run("FN (MAKEVIEW er :Outer) -> Module = (er :| Outer)");
    test_run.run("LET made = (MAKEVIEW matching)");
    let elt = view_member(&test_run, "made", "Elt");

    // The nested view's own tables, read after the frame that minted the outer view is gone.
    let element = only_element(&mut test_run, "made.subs");
    let KObject::Module(inner) = element else {
        panic!("the nested slot's elements are module values");
    };
    assert_eq!(
        inner
            .type_members
            .get(&type_name("Item", test_run.registries()))
            .copied(),
        Some(elt),
    );
    // The path is re-homed into the nested view's own region; reading it walks that storage.
    assert!(!inner.path.is_empty());
    // The member the nested replay coerced, and the width extra it replayed verbatim — both read
    // out of the nested scope's binding table.
    let one = inner
        .child_scope()
        .lookup("one")
        .expect("the nested view carries its `one` member");
    assert_eq!(one.ktype(), elt);
    let extra = inner
        .child_scope()
        .lookup("extra")
        .expect("the nested view replays the source's width");
    assert!(matches!(extra, KObject::Number(n) if *n == 42.0));
}
