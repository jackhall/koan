//! Primitive ascription behaviors: transparent passthrough, missing-member errors, opaque type-minting.

use crate::builtins::test_support::{binds_module, lookup_module, parse_one, TestRun};
use crate::machine::model::{KObject, KType};
use crate::machine::run_root_storage;
use crate::machine::KErrorKind;
use crate::parse::parse;

#[test]
fn transparent_ascription_returns_module() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run(
        "MODULE int_ord = (LET compare = 0)\n\
         SIG Ordered = (VAL compare :Number)\n\
         LET int_ord_view = (int_ord :! Ordered)",
    );
    // A view is a module value: `LET` binds it on the value channel (`bindings.data`).
    assert!(binds_module(scope, "int_ord_view"));
}

#[test]
fn ascription_missing_member_errors() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run(
        "MODULE empty = (LET unrelated = 0)\n\
         SIG Ordered = (VAL compare :Number)",
    );
    let err = test_run.run_one_err(parse_one("empty :| Ordered"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("Ordered") && msg.contains("`compare`")),
        "expected ShapeError naming Ordered and the missing member, got {err}",
    );
}

#[test]
fn opaque_ascription_mints_distinct_module_type_per_application() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let src = "MODULE int_ord = ((LET Carrier = Number) (LET compare = 0))\n\
         SIG Ordered = ((TYPE Carrier) (VAL compare :Number))\n\
         LET first_abstract = (int_ord :| Ordered)\n\
         LET second_abstract = (int_ord :| Ordered)";
    let exprs = parse(src).expect("parse should succeed");
    let mut ids = Vec::new();
    for expr in exprs {
        ids.push(test_run.runtime.dispatch_in_scope(expr, scope));
    }
    test_run
        .runtime
        .execute()
        .expect("scheduler should succeed");
    for (i, id) in ids.iter().enumerate() {
        if let Err(e) = test_run.runtime.result_error(*id) {
            panic!("expr {} errored: {}", i, e);
        }
    }
    let a = lookup_module(scope, "first_abstract", &test_run.types);
    let b = lookup_module(scope, "second_abstract", &test_run.types);
    let a_t = a.type_members.borrow().get("Carrier").cloned();
    let b_t = b.type_members.borrow().get("Carrier").cloned();
    // An opaque-ascription abstract-type member mints as
    // `KType::AbstractType { name, nonce: Some(<view module's scope id>), .. }`.
    assert!(matches!(
        &a_t,
        Some(KType::AbstractType { name, .. }) if name == "Carrier"
    ));
    assert!(matches!(
        &b_t,
        Some(KType::AbstractType { name, .. }) if name == "Carrier"
    ));
    assert_ne!(
        a_t, b_t,
        "two opaque ascriptions must mint distinct module abstract types"
    );
}

#[test]
fn transparent_ascription_does_not_mint_module_types() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run(
        "MODULE int_ord = (LET compare = 0)\n\
         SIG Ordered = (VAL compare :Number)\n\
         LET view_mod = (int_ord :! Ordered)",
    );
    let v = lookup_module(scope, "view_mod", &test_run.types);
    assert!(v.type_members.borrow().is_empty());
}

/// End-to-end example from [design/typing/modules.md](../../../../design/typing/modules.md).
#[test]
fn roadmap_example_int_ord_with_ordered_sig() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run(
        "MODULE int_ord = ((LET Carrier = Number) (LET compare = 7))\n\
         SIG Ordered = ((TYPE Carrier) (VAL compare :Number))\n\
         LET int_ord_abstract = (int_ord :| Ordered)",
    );

    let abstract_mod = lookup_module(scope, "int_ord_abstract", &test_run.types);
    let minted = abstract_mod
        .type_members
        .borrow()
        .get("Carrier")
        .cloned()
        .expect("opaque ascription should mint a Carrier member");
    match &minted {
        KType::AbstractType { name, .. } => assert_eq!(name, "Carrier"),
        other => panic!("minted abstract type must be AbstractType, got {:?}", other),
    }
    assert_ne!(
        minted,
        KType::Number,
        "opaque int_ord_abstract.Carrier must not equal Number"
    );
    let compare = abstract_mod
        .child_scope()
        .bindings()
        .data()
        .get("compare")
        .map(|(o, _, _)| *o);
    assert!(matches!(compare, Some(KObject::Number(n)) if *n == 7.0));
}

/// A manifest SIG member (`LET Tag = Number`) reads *concretely* through an opaque
/// (`:|`) view: unlike an abstract `TYPE` member, opaque ascription mirrors its fixed
/// `KType` into the view's `type_members` verbatim rather than minting a per-call
/// abstract identity, so `view.Tag` resolves to `Number`.
#[test]
fn opaque_view_reads_manifest_type_member_concretely() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run(
        "MODULE implementation = ((LET Tag = Number) (LET item = 5))\n\
         SIG Tagged = ((LET Tag = Number) (VAL item :Number))\n\
         LET view = (implementation :| Tagged)",
    );
    let view = lookup_module(scope, "view", &test_run.types);
    let tag = view.type_members.borrow().get("Tag").cloned();
    assert_eq!(
        tag,
        Some(KType::Number),
        "manifest `LET Tag = Number` must mirror concretely into the opaque view, got {tag:?}",
    );
}

/// A VAL slot whose declared type is a *manifest* member (`VAL x :Tag` after
/// `LET Tag = Number`) resolves concrete: its declared type is `Number`, not a
/// `Sig`-rooted `AbstractType`, so opaque ascription records no `slot_type_tags`
/// entry for it and `view.x` reads the underlying `Number` unwrapped.
#[test]
fn opaque_view_manifest_typed_val_slot_reads_concrete() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run(
        "MODULE implementation = ((LET Tag = Number) (LET x = 3))\n\
         SIG Tagged = ((LET Tag = Number) (VAL x :Tag))\n\
         LET view = (implementation :| Tagged)",
    );
    let view = lookup_module(scope, "view", &test_run.types);
    assert!(
        view.slot_type_tags.borrow().get("x").is_none(),
        "a manifest-typed VAL slot must not be re-tagged in slot_type_tags",
    );
    let result = test_run.run_one(parse_one("view.x"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 3.0),
        "view.x on a manifest-typed slot reads the underlying Number(3), got {:?}",
        result.ktype(),
    );
}

/// A module lacking a `TYPE`-declared abstract member fails the opaque (`:|`) satisfaction
/// check with the "missing type member" error.
#[test]
fn opaque_missing_abstract_member_rejected() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run(
        "MODULE implementation = (LET item = 0)\n\
         SIG Container = ((TYPE Elt) (VAL item :Number))",
    );
    let err = test_run.run_one_err(parse_one("implementation :| Container"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("Container") && msg.contains("missing type member `Elt`")),
        "expected the missing-type-member error, got {err}",
    );
}

/// The same absent abstract member is rejected through transparent (`:!`) ascription too.
#[test]
fn transparent_missing_abstract_member_rejected() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run(
        "MODULE implementation = (LET item = 0)\n\
         SIG Container = ((TYPE Elt) (VAL item :Number))",
    );
    let err = test_run.run_one_err(parse_one("implementation :! Container"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("Container") && msg.contains("missing type member `Elt`")),
        "expected the missing-type-member error, got {err}",
    );
}

/// A manifest member the module supplies at the wrong type (`LET Tag = Str` against a
/// signature fixing `LET Tag = Number`) is rejected with the "fixes it to" error.
#[test]
fn manifest_type_member_mismatch_rejected() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    test_run.run(
        "MODULE implementation = ((LET Tag = Str) (LET item = 0))\n\
         SIG Tagged = ((LET Tag = Number) (VAL item :Number))",
    );
    let err = test_run.run_one_err(parse_one("implementation :| Tagged"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("Tagged")
                && msg.contains("type member `Tag`")
                && msg.contains("fixes it to")),
        "expected the manifest fixes-it-to error, got {err}",
    );
}

/// A manifest member the module supplies at the matching type (`LET Tag = Number` on both
/// sides) satisfies the signature.
#[test]
fn manifest_type_member_match_accepted() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run(
        "MODULE implementation = ((LET Tag = Number) (LET item = 0))\n\
         SIG Tagged = ((LET Tag = Number) (VAL item :Number))\n\
         LET view = (implementation :| Tagged)",
    );
    assert!(
        binds_module(scope, "view"),
        "a matching manifest member must satisfy the signature",
    );
}

/// An abstract member is presence-only: a module supplying `LET Elt = Str` for an abstract
/// `TYPE Elt` satisfies the signature regardless of the concrete type it chooses.
#[test]
fn abstract_member_bound_to_any_type_accepted() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run(
        "MODULE implementation = ((LET Elt = Str) (LET item = 0))\n\
         SIG Container = ((TYPE Elt) (VAL item :Number))\n\
         LET view = (implementation :| Container)",
    );
    assert!(
        binds_module(scope, "view"),
        "an abstract member supplied at any concrete type must satisfy the signature",
    );
}
