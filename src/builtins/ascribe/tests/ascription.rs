//! Primitive ascription behaviors: transparent passthrough, missing-member errors, opaque type-minting.

use crate::builtins::test_support::{
    binds_module, lookup_module, parse_one, run, run_one, run_one_err, run_root_silent,
};
use crate::machine::core::run_root_storage;
use crate::machine::execute::KoanRuntime;
use crate::machine::model::{KObject, KType};
use crate::machine::KErrorKind;
use crate::parse::parse;

#[test]
fn transparent_ascription_returns_module() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE IntOrd = (LET compare = 0)\n\
         SIG OrderedSig = (VAL compare :Number)\n\
         LET IntOrdView = (IntOrd :! OrderedSig)",
    );
    // A view is a module value: `LET` binds it on the value channel (`bindings.data`).
    assert!(binds_module(scope, "IntOrdView"));
}

#[test]
fn ascription_missing_member_errors() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE Empty = (LET unrelated = 0)\n\
         SIG OrderedSig = (VAL compare :Number)",
    );
    let err = run_one_err(scope, parse_one("Empty :| OrderedSig"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("OrderedSig") && msg.contains("`compare`")),
        "expected ShapeError naming OrderedSig and the missing member, got {err}",
    );
}

#[test]
fn opaque_ascription_mints_distinct_module_type_per_application() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let src = "MODULE IntOrd = ((LET Carrier = Number) (LET compare = 0))\n\
         SIG OrderedSig = ((TYPE Carrier) (VAL compare :Number))\n\
         LET FirstAbstract = (IntOrd :| OrderedSig)\n\
         LET SecondAbstract = (IntOrd :| OrderedSig)";
    let exprs = parse(src).expect("parse should succeed");
    let mut runtime = KoanRuntime::new();
    let mut ids = Vec::new();
    for expr in exprs {
        ids.push(runtime.dispatch_in_scope(expr, scope));
    }
    runtime.execute().expect("scheduler should succeed");
    for (i, id) in ids.iter().enumerate() {
        if let Err(e) = runtime.result_error(*id) {
            panic!("expr {} errored: {}", i, e);
        }
    }
    let a = lookup_module(scope, "FirstAbstract");
    let b = lookup_module(scope, "SecondAbstract");
    let a_t = a.type_members.borrow().get("Carrier").cloned();
    let b_t = b.type_members.borrow().get("Carrier").cloned();
    // Post-collapse: opaque-ascription abstract-type members are minted as
    // `KType::AbstractType { source: Module(view), name }`.
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
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE IntOrd = (LET compare = 0)\n\
         SIG OrderedSig = (VAL compare :Number)\n\
         LET ViewMod = (IntOrd :! OrderedSig)",
    );
    let v = lookup_module(scope, "ViewMod");
    assert!(v.type_members.borrow().is_empty());
}

/// End-to-end example from [design/typing/modules.md](../../../../design/typing/modules.md).
#[test]
fn roadmap_example_int_ord_with_ordered_sig() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE IntOrd = ((LET Carrier = Number) (LET compare = 7))\n\
         SIG OrderedSig = ((TYPE Carrier) (VAL compare :Number))\n\
         LET IntOrdAbstract = (IntOrd :| OrderedSig)",
    );

    let abstract_mod = lookup_module(scope, "IntOrdAbstract");
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
        "opaque IntOrdAbstract.Carrier must not equal Number"
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
/// abstract identity, so `View.Tag` resolves to `Number`.
#[test]
fn opaque_view_reads_manifest_type_member_concretely() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE Impl = ((LET Tag = Number) (LET item = 5))\n\
         SIG TagSig = ((LET Tag = Number) (VAL item :Number))\n\
         LET View = (Impl :| TagSig)",
    );
    let view = lookup_module(scope, "View");
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
/// entry for it and `View.x` reads the underlying `Number` unwrapped.
#[test]
fn opaque_view_manifest_typed_val_slot_reads_concrete() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE Impl = ((LET Tag = Number) (LET x = 3))\n\
         SIG TagSig = ((LET Tag = Number) (VAL x :Tag))\n\
         LET View = (Impl :| TagSig)",
    );
    let view = lookup_module(scope, "View");
    assert!(
        view.slot_type_tags.borrow().get("x").is_none(),
        "a manifest-typed VAL slot must not be re-tagged in slot_type_tags",
    );
    let result = run_one(scope, parse_one("View.x"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 3.0),
        "View.x on a manifest-typed slot reads the underlying Number(3), got {:?}",
        result.ktype(),
    );
}

/// A module lacking a `TYPE`-declared abstract member fails the opaque (`:|`) satisfaction
/// check with the "missing type member" error.
#[test]
fn opaque_missing_abstract_member_rejected() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE Impl = (LET item = 0)\n\
         SIG Container = ((TYPE Elt) (VAL item :Number))",
    );
    let err = run_one_err(scope, parse_one("Impl :| Container"));
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
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE Impl = (LET item = 0)\n\
         SIG Container = ((TYPE Elt) (VAL item :Number))",
    );
    let err = run_one_err(scope, parse_one("Impl :! Container"));
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
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE Impl = ((LET Tag = Str) (LET item = 0))\n\
         SIG TagSig = ((LET Tag = Number) (VAL item :Number))",
    );
    let err = run_one_err(scope, parse_one("Impl :| TagSig"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("TagSig")
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
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE Impl = ((LET Tag = Number) (LET item = 0))\n\
         SIG TagSig = ((LET Tag = Number) (VAL item :Number))\n\
         LET View = (Impl :| TagSig)",
    );
    assert!(
        binds_module(scope, "View"),
        "a matching manifest member must satisfy the signature",
    );
}

/// An abstract member is presence-only: a module supplying `LET Elt = Str` for an abstract
/// `TYPE Elt` satisfies the signature regardless of the concrete type it chooses.
#[test]
fn abstract_member_bound_to_any_type_accepted() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE Impl = ((LET Elt = Str) (LET item = 0))\n\
         SIG Container = ((TYPE Elt) (VAL item :Number))\n\
         LET View = (Impl :| Container)",
    );
    assert!(
        binds_module(scope, "View"),
        "an abstract member supplied at any concrete type must satisfy the signature",
    );
}
