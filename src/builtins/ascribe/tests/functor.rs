//! Functor integration: module-typed parameters, signature-bound dispatch,
//! per-call generativity.

use crate::builtins::test_support::{TestRun, lookup_module, type_name, value_name};
use crate::machine::KErrorKind;
use crate::machine::model::{KObject, KType, TypeNode};
use crate::machine::{program_storage, run_root_storage};
use crate::parse::parse;

#[test]
fn functor_returns_a_module() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = (LET compare = 7)",
    );
    test_run.run("LET int_ord_a = (int_ord :! Ordered)");
    test_run.run("FN (MAKESET elem :Ordered) -> Module = (MODULE generated = (LET inner = 1))");
    test_run.run("LET set_value = (MAKESET int_ord_a)");

    let m = lookup_module(scope, "set_value", test_run.registries());
    let inner = m.child_scope().lookup("inner");
    assert!(matches!(inner, Some(KObject::Number(n)) if *n == 1.0));
}

#[test]
fn functor_body_reads_signature_typed_parameter() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = (LET compare = 7)",
    );
    test_run.run("LET int_ord_a = (int_ord :! Ordered)");
    test_run.run(
        "FN (MAKESET elem :Ordered) -> Module = (MODULE generated = (LET sample = (elem.compare)))",
    );
    test_run.run("LET set_value = (MAKESET int_ord_a)");

    let m = lookup_module(scope, "set_value", test_run.registries());
    let sample = m.child_scope().lookup("sample");
    assert!(matches!(sample, Some(KObject::Number(n)) if *n == 7.0));
}

/// Per-call generativity: two invocations produce modules with distinct `scope_id`.
/// Asserts on bare `scope_id`s rather than on minted abstract types, which would
/// require multi-statement-FN-body forward refs that don't share lexical bindings.
#[test]
fn functor_application_is_generative() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = (LET compare = 7)",
    );
    test_run.run("LET int_ord_a = (int_ord :! Ordered)");
    test_run.run("FN (MAKESET elem :Ordered) -> Module = (MODULE generated = (LET inner = 1))");
    test_run.run("LET set_one = (MAKESET (int_ord_a))");
    test_run.run("LET set_two = (MAKESET (int_ord_a))");

    let m1 = lookup_module(scope, "set_one", test_run.registries());
    let m2 = lookup_module(scope, "set_two", test_run.registries());
    assert_ne!(
        m1.scope_id(),
        m2.scope_id(),
        "two functor applications must produce modules with distinct scope_id",
    );
}

/// Generativity in its abstract-type form: a module-returning FN whose body opaquely ascribes
/// (`:|`) mints a fresh abstract type per application, so two calls yield modules whose `Carrier`
/// type members are distinct `KType::AbstractType` carriers. Compare
/// [`functor_application_is_generative`], which pins the same property on bare `scope_id`s.
///
/// Miri audit-slate: this is the opaque shape of the escaping-module retention discipline. Each view
/// is born inside its call's own region carrying every byte it names — its path, both member maps'
/// keys, and their member tables' bucket arrays — and every read below happens after that frame is gone,
/// probing each map by content with a `&str` built at the read site rather than the key the draft was
/// assembled from. A release claim that freed the call's region would free the very storage those
/// reads walk, which only tree borrows observes: a normal build reads the freed bytes back intact.
#[test]
fn functor_application_mints_distinct_abstract_types() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    // `VAL zero :Carrier` is typed against the SIG's abstract member, so each view's scope is born
    // holding a `zero` coerced to that view's own mint — read back off the value below.
    let src = "SIG Ordered = ((TYPE Carrier) (VAL zero :Carrier) (VAL compare :Number))\n\
               MODULE int_ord = ((LET Carrier = Number) (LET zero = 0) (LET compare = 7))\n\
               FN (MAKESET er :Ordered) -> Module = (er :| Ordered)\n\
               LET set_one = (MAKESET int_ord)\n\
               LET set_two = (MAKESET int_ord)";
    let exprs =
        parse(program.brand(), &test_run.registries().labels, src).expect("parse should succeed");
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
            panic!("expr {i} errored: {e}");
        }
    }

    let one = lookup_module(scope, "set_one", test_run.registries());
    let two = lookup_module(scope, "set_two", test_run.registries());
    let carrier = type_name("Carrier", test_run.registries());
    let one_carrier = one.type_members.get(&carrier).copied();
    let two_carrier = two.type_members.get(&carrier).copied();
    assert!(
        matches!(one_carrier.map(|h| test_run.types().node(h)), Some(TypeNode::AbstractType { name, .. }) if name == carrier),
        "the first application must mint an abstract Carrier, got {one_carrier:?}",
    );
    assert!(
        matches!(two_carrier.map(|h| test_run.types().node(h)), Some(TypeNode::AbstractType { name, .. }) if name == carrier),
        "the second application must mint an abstract Carrier, got {two_carrier:?}",
    );
    assert_ne!(
        one_carrier, two_carrier,
        "two applications of a module-returning FN must mint distinct abstract types",
    );

    // The coerced member values and the bumped path, read out of the same dead call regions.
    assert_eq!(
        one.child_scope().lookup("zero").map(KObject::ktype),
        one_carrier,
        "the abstract-typed VAL slot's member is born carrying that application's own minted Carrier",
    );
    assert_eq!(
        two.child_scope().lookup("zero").map(KObject::ktype),
        two_carrier,
        "and the second application's member names its own mint, not the first's",
    );
    assert_eq!(
        (one.path, two.path),
        ("int_ord", "int_ord"),
        "each view's path reads back out of the region it was minted in",
    );
}

/// An unascribed module is admitted by a constraint-role `Signature { sig, .. }` slot iff its
/// self-sig structurally satisfies the signature — no ascription required. `int_ord = (LET
/// compare = 7)` structurally satisfies `Ordered = (VAL compare :Number)`, so the call
/// succeeds and produces the generated module.
#[test]
fn functor_admits_unascribed_module_structurally() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = (LET compare = 7)",
    );
    test_run.run("FN (MAKESET elem :Ordered) -> Module = (MODULE generated = (LET inner = 1))");
    // Type-classified binder so the auto-wrap pass triggers in the
    // `Signature { .. }` slot. The LET partition guard requires module carriers
    // to ride Type-classified names (design/typing/elaboration.md § Binding-map
    // partition).
    test_run.run("LET unascribed = int_ord");
    test_run.run("LET set_value = (MAKESET unascribed)");

    let m = lookup_module(scope, "set_value", test_run.registries());
    let inner = m.child_scope().lookup("inner");
    assert!(
        matches!(inner, Some(KObject::Number(n)) if *n == 1.0),
        "generated module should carry inner=1, got {:?}",
        inner.map(|o| o.ktype())
    );
}

/// A module that does *not* structurally satisfy the slot's signature is a dispatch non-match:
/// `no_compare = (LET other = 1)` lacks the `compare` slot `Ordered` requires, so `MAKESET`
/// finds no admitting overload and the slot terminates in `DispatchFailed`.
#[test]
fn functor_rejects_structurally_unsatisfying_module() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE no_compare = (LET other = 1)",
    );
    test_run.run("FN (MAKESET elem :Ordered) -> Module = (MODULE generated = (LET inner = 1))");
    test_run.run("LET arg = no_compare");
    let root = test_run.dispatch_watched_in(
        scope,
        crate::machine::model::WorkingExpression::from_ast(
            scope.brand(),
            test_run.parse_one("MAKESET arg"),
        ),
    );
    test_run
        .runtime
        .execute()
        .expect("a dispatch failure is slot-terminal, not a fatal execute error");
    let err = test_run
        .runtime
        .edge_result_error(root)
        .expect_err("expected a DispatchFailed in the dispatch slot");
    assert!(
        matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
        "expected DispatchFailed, got {err}",
    );
}

/// Two functors share a keyword `MAKESET` but differ on parameter sig
/// (`Ordered` vs `Hashed`); dispatch routes by the argument's satisfied sig.
#[test]
fn functor_overloads_dispatch_by_signature_bound_param() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Ordered = (VAL compare :Number)\n\
         SIG Hashed = (VAL hash :Number)\n\
         MODULE int_ord = (LET compare = 7)\n\
         MODULE int_hash = (LET hash = 11)",
    );
    test_run.run(
        "LET int_ord_a = (int_ord :! Ordered)\n\
         LET int_hash_a = (int_hash :! Hashed)",
    );
    test_run.run("FN (MAKESET elem :Ordered) -> Module = (MODULE generated = (LET tag = 1))");
    test_run.run("FN (MAKESET elem :Hashed) -> Module = (MODULE generated = (LET tag = 2))");
    test_run.run("LET ord_set = (MAKESET (int_ord_a))");
    test_run.run("LET hash_set = (MAKESET (int_hash_a))");

    let mo = lookup_module(scope, "ord_set", test_run.registries());
    let mh = lookup_module(scope, "hash_set", test_run.registries());
    let to = mo.child_scope().lookup("tag");
    let th = mh.child_scope().lookup("tag");
    assert!(
        matches!(to, Some(KObject::Number(n)) if *n == 1.0),
        "Ordered call should pick body with tag=1, got {:?}",
        to.map(|o| o.ktype())
    );
    assert!(
        matches!(th, Some(KObject::Number(n)) if *n == 2.0),
        "Hashed call should pick body with tag=2, got {:?}",
        th.map(|o| o.ktype())
    );
}

/// A `:!` (transparent) view structurally satisfies the slot's signature exactly as a `:|`
/// (opaque) view does, and the body still reads the underlying member through the view.
#[test]
fn transparent_ascription_satisfies_signature_bound_slot() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = (LET compare = 7)",
    );
    test_run.run("LET int_view = (int_ord :! Ordered)");
    test_run.run(
        "FN (MAKESET elem :Ordered) -> Module = (MODULE generated = (LET sample = (elem.compare)))",
    );
    test_run.run("LET set_value = (MAKESET int_view)");

    let m = lookup_module(scope, "set_value", test_run.registries());
    let sample = m.child_scope().lookup("sample");
    assert!(matches!(sample, Some(KObject::Number(n)) if *n == 7.0));
}

/// The monad program of record: a `NEWTYPE (Type AS Wrapper)` family, a SIG whose `pure` VAL
/// slot returns `:(Number AS Wrap)`, and a module supplying `Wrap = Wrapper` plus a `pure` whose
/// body constructs `Wrapper (x)`. Returned as a source string reused by the end-to-end tests.
fn monad_program() -> &'static str {
    "NEWTYPE (Type AS Wrapper)\n\
     SIG Monad = ((TYPE (Type AS Wrap)) (VAL pure :(FN :{x :Number} -> :(Number AS Wrap))))\n\
     MODULE id_monad = ((LET Wrap = Wrapper) \
     (LET pure = FN (PURE x :Number) -> :(Number AS Wrapper) = (Wrapper (x))))"
}

/// `id_monad :| Monad` succeeds: `substitute_sig_members` substitutes the SIG's `Wrap` slot to
/// the module's `Wrapper` and descends the `pure` VAL slot's `ConstructorApply` return type, so
/// the module's `pure` (returning `:(Number AS Wrapper)`) satisfies the substituted
/// `:(Number AS Wrap)` slot end to end.
#[test]
fn hk_value_slot_satisfies_after_substitution() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(monad_program());
    test_run.run("LET view = (id_monad :| Monad)");
    assert!(
        matches!(
            lookup_module(scope, "view", test_run.registries()),
            m if m
                .child_scope()
                .bindings()
                .data()
                .get(&value_name("pure", test_run.registries()))
                .is_some()
        ),
        "id_monad must satisfy Monad and bind a view module carrying `pure`",
    );
}

/// `(id_monad.pure {x = 3.0})` runs the module's `pure`, whose declared return type
/// `:(Number AS Wrapper)` is checked via `matches_value` against the constructed
/// `Wrapper (x)` — the per-call return check passing on an identity-wrapper value.
#[test]
fn pure_call_passes_return_check() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(monad_program());
    let result = test_run.run_one(test_run.parse_one("id_monad.pure {x = 3.0}"));
    match result {
        KObject::Wrapped { inner, type_id } => {
            assert!(
                matches!(
                    test_run.types().node(*type_id),
                    TypeNode::ConstructorApply { .. }
                ),
                "pure must return an identity-wrapper value, got {:?}",
                type_id,
            );
            assert!(matches!(inner.payload(), KObject::Number(n) if *n == 3.0));
        }
        other => panic!("expected Wrapped from pure, got {:?}", other.ktype()),
    }
}

/// A bare Type-classified argument (`MAKESET int_ord_a`) auto-wraps to a value lookup
/// just like the lowercase-identifier and parens-wrapped forms do.
#[test]
fn functor_argument_bare_type_token_auto_wraps() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = (LET compare = 7)",
    );
    test_run.run("LET int_ord_a = (int_ord :! Ordered)");
    test_run.run(
        "FN (MAKESET elem :Ordered) -> Module = \
         (MODULE generated = (LET sample = (elem.compare)))",
    );
    test_run.run("LET set_value = (MAKESET int_ord_a)");

    let m = lookup_module(scope, "set_value", test_run.registries());
    let sample = m.child_scope().lookup("sample");
    assert!(matches!(sample, Some(KObject::Number(n)) if *n == 7.0));
}

/// Two opaque ascriptions of a module satisfying a SIG with `TYPE (Type AS Wrap)`
/// mint distinct per-call `TypeConstructor` slots —
/// the higher-kinded analogue of `functor_application_is_generative`.
#[test]
fn opaque_ascription_mints_fresh_type_constructor_per_call() {
    use crate::machine::model::KKind;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let src = "NEWTYPE (Type AS Wrapper)\n\
               SIG Monad = ((TYPE (Type AS Wrap)))\n\
               MODULE int_list = ((LET Wrap = Wrapper))\n\
               LET first = (int_list :| Monad)\n\
               LET second = (int_list :| Monad)";
    let exprs =
        parse(program.brand(), &test_run.registries().labels, src).expect("parse should succeed");
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
    let a = lookup_module(scope, "first", test_run.registries());
    let b = lookup_module(scope, "second", test_run.registries());
    let wrap = type_name("Wrap", test_run.registries());
    let a_wrap = a.type_members.get(&wrap).copied();
    let b_wrap = b.type_members.get(&wrap).copied();
    let is_type_constructor = |kt: Option<KType>| {
        matches!(
            kt.map(|h| test_run.types().node(h)),
            Some(TypeNode::SetMember { kind, .. }) if kind == KKind::TypeConstructor
        )
    };
    assert!(is_type_constructor(a_wrap));
    assert!(is_type_constructor(b_wrap));
    // Identity is the content digest, but an opaque-ascription mint is *generative*: each
    // application folds its per-call nonce (the view module's `scope_id`) into the component
    // digest, so the two members digest apart even though their member content is identical.
    match (
        a_wrap.map(|h| test_run.types().node(h)),
        b_wrap.map(|h| test_run.types().node(h)),
    ) {
        (
            Some(TypeNode::SetMember {
                scc_digest: a_scc, ..
            }),
            Some(TypeNode::SetMember {
                scc_digest: b_scc, ..
            }),
        ) => {
            assert_ne!(
                a_scc, b_scc,
                "two opaque ascriptions must mint TypeConstructor members with distinct component digests",
            );
        }
        _ => unreachable!("matched above"),
    }
    assert_ne!(
        a_wrap, b_wrap,
        "two opaque ascriptions must mint distinct TypeConstructor types",
    );
}

/// Miri audit-slate: the held `&Module` plus its re-bound child scope must
/// survive subsequent region churn under tree borrows.
#[test]
fn opaque_ascription_re_binds_do_not_alias_unsoundly() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    // Plain `LET` plus `LET = FN` so the re-bind walk hits both the `data` replay
    // and the `functions` bucket replay — and the signature declares all three surfaces,
    // since a view holds only what its signature names.
    test_run.run(
        "SIG Ordered = ((VAL compare :Number) \
         (VAL helper :(FN :{x :Number} -> Number)) \
         (FN (HELP x :Number) -> Number))\n\
         MODULE int_ord = ((LET compare = 7) (LET helper = FN (HELP x :Number) -> Number = (x)))\n\
         LET held = (int_ord :| Ordered)",
    );
    let held = lookup_module(scope, "held", test_run.registries());

    // Churn the run-root region, then re-ascribe to allocate a second re-bind
    // scope. The original `held` must still walk through to its own pair.
    test_run.run("FN (CHURNCALL) -> Number = (1)");
    for _ in 0..20 {
        test_run.run_one(test_run.parse_one("CHURNCALL"));
    }
    test_run.run("LET held2 = (int_ord :| Ordered)");

    let child = held.child_scope();
    assert!(
        matches!(child.lookup("compare"), Some(KObject::Number(n)) if *n == 7.0),
        "held.child_scope().compare must still read 7.0 after subsequent churn",
    );
    assert!(
        matches!(child.lookup("helper"), Some(KObject::KFunction(_))),
        "held.child_scope().helper must still resolve to a KFunction after churn",
    );
}
