//! Primitive ascription behaviors: transparent passthrough, missing-member errors, opaque type-minting.

use crate::builtins::test_support::{
    TestRun, binds_module, lookup_module, parse_one, type_name, value_name,
};
use crate::machine::KErrorKind;
use crate::machine::model::{KObject, KType, TypeNode, render_label};
use crate::machine::{program_storage, run_root_storage};
use crate::parse::parse;

#[test]
fn transparent_ascription_returns_module() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
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
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "MODULE empty = (LET unrelated = 0)\n\
         SIG Ordered = (VAL compare :Number)",
    );
    let err = test_run.run_one_err(parse_one(&program, "empty :| Ordered"));
    // Ruling 12: a signature renders structurally, not by declared name — the diagnostic names
    // the interface `SIG (compare: Number)` and the missing member, not "Ordered".
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("does not satisfy signature") && msg.contains("`compare`")),
        "expected ShapeError naming the missing member `compare`, got {err}",
    );
}

#[test]
fn opaque_ascription_mints_distinct_module_type_per_application() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let src = "MODULE int_ord = ((LET Carrier = Number) (LET compare = 0))\n\
         SIG Ordered = ((TYPE Carrier) (VAL compare :Number))\n\
         LET first_abstract = (int_ord :| Ordered)\n\
         LET second_abstract = (int_ord :| Ordered)";
    let exprs = parse(program.brand(), src).expect("parse should succeed");
    let mut ids = Vec::new();
    for expr in exprs {
        ids.push(test_run.dispatch_watched_in(
            scope,
            crate::machine::model::WorkingExpression::from_ast(scope.brand(), expr),
        ));
    }
    test_run
        .runtime
        .execute()
        .expect("scheduler should succeed");
    for (i, id) in ids.iter().enumerate() {
        if let Err(e) = test_run.runtime.edge_result_error(*id) {
            panic!("expr {} errored: {}", i, e);
        }
    }
    let a = lookup_module(scope, "first_abstract", test_run.registries());
    let b = lookup_module(scope, "second_abstract", test_run.registries());
    let carrier = type_name("Carrier", test_run.registries());
    let a_t = a.type_members.get(&carrier).copied();
    let b_t = b.type_members.get(&carrier).copied();
    // An opaque-ascription abstract-type member mints as
    // `KType::AbstractType { name, nonce: Some(<view module's scope id>), .. }`.
    assert!(matches!(
        a_t.map(|h| test_run.types().node(h)),
        Some(TypeNode::AbstractType { name, .. }) if name == carrier
    ));
    assert!(matches!(
        b_t.map(|h| test_run.types().node(h)),
        Some(TypeNode::AbstractType { name, .. }) if name == carrier
    ));
    assert_ne!(
        a_t, b_t,
        "two opaque ascriptions must mint distinct module abstract types"
    );
}

#[test]
fn transparent_ascription_does_not_mint_module_types() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "MODULE int_ord = (LET compare = 0)\n\
         SIG Ordered = (VAL compare :Number)\n\
         LET view_mod = (int_ord :! Ordered)",
    );
    let v = lookup_module(scope, "view_mod", test_run.registries());
    assert!(v.type_members.is_empty());
}

/// End-to-end example from [design/typing/modules.md](../../../../design/typing/modules.md).
#[test]
fn roadmap_example_int_ord_with_ordered_sig() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "MODULE int_ord = ((LET Carrier = Number) (LET compare = 7))\n\
         SIG Ordered = ((TYPE Carrier) (VAL compare :Number))\n\
         LET int_ord_abstract = (int_ord :| Ordered)",
    );

    let abstract_mod = lookup_module(scope, "int_ord_abstract", test_run.registries());
    let minted = abstract_mod
        .type_members
        .get(&type_name("Carrier", test_run.registries()))
        .copied()
        .expect("opaque ascription should mint a Carrier member");
    match test_run.types().node(minted) {
        TypeNode::AbstractType { name, .. } => {
            assert_eq!(name, type_name("Carrier", test_run.registries()))
        }
        _ => panic!(
            "minted abstract type must be AbstractType, got {:?}",
            minted
        ),
    }
    assert_ne!(
        minted,
        KType::NUMBER,
        "opaque int_ord_abstract.Carrier must not equal Number"
    );
    let compare = abstract_mod.child_scope().lookup("compare");
    assert!(matches!(compare, Some(KObject::Number(n)) if *n == 7.0));
}

/// A manifest SIG member (`LET Tag = Number`) reads *concretely* through an opaque
/// (`:|`) view: unlike an abstract `TYPE` member, opaque ascription mirrors its fixed
/// `KType` into the view's `type_members` verbatim rather than minting a per-call
/// abstract identity, so `view.Tag` resolves to `Number`.
#[test]
fn opaque_view_reads_manifest_type_member_concretely() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "MODULE implementation = ((LET Tag = Number) (LET item = 5))\n\
         SIG Tagged = ((LET Tag = Number) (VAL item :Number))\n\
         LET view = (implementation :| Tagged)",
    );
    let view = lookup_module(scope, "view", test_run.registries());
    let tag = view
        .type_members
        .get(&type_name("Tag", test_run.registries()))
        .copied();
    assert_eq!(
        tag,
        Some(KType::NUMBER),
        "manifest `LET Tag = Number` must mirror concretely into the opaque view, got {tag:?}",
    );
}

/// A VAL slot whose declared type is a *manifest* member (`VAL x :Tag` after
/// `LET Tag = Number`) resolves concrete: its declared type is `Number`, not a
/// `Sig`-rooted `AbstractType`, so opaque ascription records no `slot_type_tags`
/// entry for it and `view.x` reads the underlying `Number` unwrapped.
#[test]
fn opaque_view_manifest_typed_val_slot_reads_concrete() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "MODULE implementation = ((LET Tag = Number) (LET x = 3))\n\
         SIG Tagged = ((LET Tag = Number) (VAL x :Tag))\n\
         LET view = (implementation :| Tagged)",
    );
    let view = lookup_module(scope, "view", test_run.registries());
    assert!(
        view.slot_type_tags
            .get(&value_name("x", test_run.registries()))
            .is_none(),
        "a manifest-typed VAL slot must not be re-tagged in slot_type_tags",
    );
    let result = test_run.run_one(parse_one(&program, "view.x"));
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
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "MODULE implementation = (LET item = 0)\n\
         SIG Container = ((TYPE Elt) (VAL item :Number))",
    );
    let err = test_run.run_one_err(parse_one(&program, "implementation :| Container"));
    // Ruling 12: the signature is named structurally, not "Container".
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("does not satisfy signature") && msg.contains("missing type member `Elt`")),
        "expected the missing-type-member error, got {err}",
    );
}

/// The same absent abstract member is rejected through transparent (`:!`) ascription too.
#[test]
fn transparent_missing_abstract_member_rejected() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "MODULE implementation = (LET item = 0)\n\
         SIG Container = ((TYPE Elt) (VAL item :Number))",
    );
    let err = test_run.run_one_err(parse_one(&program, "implementation :! Container"));
    // Ruling 12: the signature is named structurally, not "Container".
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("does not satisfy signature") && msg.contains("missing type member `Elt`")),
        "expected the missing-type-member error, got {err}",
    );
}

/// A manifest member the module supplies at the wrong type (`LET Tag = Str` against a
/// signature fixing `LET Tag = Number`) is rejected with the "fixes it to" error.
#[test]
fn manifest_type_member_mismatch_rejected() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "MODULE implementation = ((LET Tag = Str) (LET item = 0))\n\
         SIG Tagged = ((LET Tag = Number) (VAL item :Number))",
    );
    let err = test_run.run_one_err(parse_one(&program, "implementation :| Tagged"));
    // Ruling 12: the signature is named structurally, not "Tagged".
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg)
            if msg.contains("does not satisfy signature")
                && msg.contains("type member `Tag`")
                && msg.contains("fixes it to")),
        "expected the manifest fixes-it-to error, got {err}",
    );
}

/// A manifest member the module supplies at the matching type (`LET Tag = Number` on both
/// sides) satisfies the signature.
#[test]
fn manifest_type_member_match_accepted() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
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
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
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

/// A transparent view minted **inside a per-call frame** and returned from it. The re-tagged
/// `Module` is allocated into the call's own region while the source module's child scope stays in
/// the run root, so a view's residence and the scope it borrows are genuinely different regions.
/// The return crosses a relocation seam that carries the
/// module reference verbatim (a borrow leaf is never rebuilt), so a claim that the call's region is
/// released would free the very storage the returned value points at. Reading the view's member back
/// after the call frame is gone is the use-after-free check; it is a Miri audit-slate test because a
/// normal build reads the freed bytes back intact.
#[test]
fn a_returned_transparent_view_keeps_the_region_it_was_minted_in() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = (LET compare = 7)",
    );
    test_run.run("FN (VIEWIT) -> Module = (int_ord :! Ordered)");
    test_run.run("LET view = (VIEWIT)");

    let m = lookup_module(scope, "view", test_run.registries());
    assert!(
        matches!(m.child_scope().lookup("compare"), Some(KObject::Number(n)) if *n == 7.0),
        "the returned view reads its member back after its minting frame is gone",
    );
}

/// An opaque view's child scope carries the view's type interface directly: the ascription seeds
/// its `types` table with exactly the per-call abstract mints and the signature's manifest members,
/// and nothing of the source's representation. That table is what a `USING` window over the view
/// borrows, so the seeding is the whole opacity story — the hidden type is absent, not masked.
/// `Module::type_members` mirrors it, as it does for a plain `MODULE`.
#[test]
fn opaque_view_scope_holds_exactly_the_views_type_members() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Boxed = ((TYPE Elem) (LET Tag = Str) (VAL zero :Elem) (VAL label :Tag))\n\
         MODULE int_ord = ((LET Elem = Number) (LET Tag = Str) (LET Hidden = Bool) \
                           (LET zero = 0) (LET label = \"n\"))\n\
         LET sealed = (int_ord :| Boxed)",
    );
    let view = lookup_module(scope, "sealed", test_run.registries());
    let mut seeded: Vec<(String, KType)> = view
        .child_scope()
        .bindings()
        .iter_types()
        .into_iter()
        .map(|(n, kt)| (render_label(n.symbol(), test_run.registries()), kt))
        .collect();
    seeded.sort_by(|a, b| a.0.cmp(&b.0));
    let names: Vec<&str> = seeded.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        vec!["Elem", "Tag"],
        "the view scope holds the signature's members and not the source's `Hidden`",
    );

    let elem = seeded[0].1;
    assert!(
        matches!(test_run.types().node(elem), TypeNode::AbstractType { name, nonce, .. }
            if name == type_name("Elem", test_run.registries()) && nonce == Some(view.scope_id())),
        "the abstract member is seeded as this view's per-call mint",
    );
    assert_eq!(
        seeded[1].1,
        KType::STR,
        "a manifest member is seeded at its fixed identity",
    );
    for (name, kt) in &seeded {
        assert_eq!(
            view.type_members
                .get(&type_name(name, test_run.registries()))
                .copied(),
            Some(*kt),
            "`type_members` mirrors the seeded scope entry for `{name}`",
        );
    }
}

/// Two ascriptions of one module seed *distinct* mints into their two view scopes — the generativity
/// the nonce buys, read on the channel the `USING` window uses rather than through the mirror.
#[test]
fn two_opaque_ascriptions_seed_distinct_mints() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Pointed = ((TYPE Elem) (VAL zero :Elem))\n\
         MODULE int_ord = ((LET Elem = Number) (LET zero = 0))\n\
         LET first = (int_ord :| Pointed)\n\
         LET second = (int_ord :| Pointed)",
    );
    let seeded = |name: &str| {
        lookup_module(scope, name, test_run.registries())
            .child_scope()
            .bindings()
            .lookup_type(type_name("Elem", test_run.registries()), None)
            .and_then(|hit| hit.bound())
            .expect("each view scope is seeded with its own `Elem`")
    };
    assert_ne!(
        seeded("first"),
        seeded("second"),
        "two opaque ascriptions must seed distinct abstract identities",
    );
}
