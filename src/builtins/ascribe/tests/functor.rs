//! Functor integration: module-typed parameters, signature-bound dispatch,
//! per-call generativity.

use crate::builtins::test_support::{lookup_module, parse_one, run, run_one, run_root_silent};
use crate::machine::model::{KObject, KType};
use crate::machine::run_root_storage;
use crate::machine::KErrorKind;
use crate::machine::KoanRuntime;
use crate::parse::parse;

#[test]
fn functor_returns_a_module() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = (LET compare = 7)",
    );
    run(scope, "LET int_ord_a = (int_ord :! Ordered)");
    run(
        scope,
        "FN (MAKESET elem :Ordered) -> Module = (MODULE generated = (LET inner = 1))",
    );
    run(scope, "LET set_value = (MAKESET int_ord_a)");

    let m = lookup_module(scope, "set_value");
    let inner = m
        .child_scope()
        .bindings()
        .data()
        .get("inner")
        .map(|(o, _, _)| *o);
    assert!(matches!(inner, Some(KObject::Number(n)) if *n == 1.0));
}

#[test]
fn functor_body_reads_signature_typed_parameter() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = (LET compare = 7)",
    );
    run(scope, "LET int_ord_a = (int_ord :! Ordered)");
    run(
        scope,
        "FN (MAKESET elem :Ordered) -> Module = (MODULE generated = (LET sample = (elem.compare)))",
    );
    run(scope, "LET set_value = (MAKESET int_ord_a)");

    let m = lookup_module(scope, "set_value");
    let sample = m
        .child_scope()
        .bindings()
        .data()
        .get("sample")
        .map(|(o, _, _)| *o);
    assert!(matches!(sample, Some(KObject::Number(n)) if *n == 7.0));
}

/// Per-call generativity: two invocations produce modules with distinct `scope_id`.
/// Asserts on bare `scope_id`s rather than on minted abstract types, which would
/// require multi-statement-FN-body forward refs that don't share lexical bindings.
#[test]
fn functor_application_is_generative() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = (LET compare = 7)",
    );
    run(scope, "LET int_ord_a = (int_ord :! Ordered)");
    run(
        scope,
        "FN (MAKESET elem :Ordered) -> Module = (MODULE generated = (LET inner = 1))",
    );
    run(scope, "LET set_one = (MAKESET (int_ord_a))");
    run(scope, "LET set_two = (MAKESET (int_ord_a))");

    let m1 = lookup_module(scope, "set_one");
    let m2 = lookup_module(scope, "set_two");
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
#[test]
fn functor_application_mints_distinct_abstract_types() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let src = "SIG Ordered = ((TYPE Carrier) (VAL compare :Number))\n\
               MODULE int_ord = ((LET Carrier = Number) (LET compare = 7))\n\
               FN (MAKESET er :Ordered) -> Module = (er :| Ordered)\n\
               LET set_one = (MAKESET int_ord)\n\
               LET set_two = (MAKESET int_ord)";
    let exprs = parse(src).expect("parse should succeed");
    let mut runtime = KoanRuntime::new();
    let mut ids = Vec::new();
    for expr in exprs {
        ids.push(runtime.dispatch_in_scope(expr, scope));
    }
    runtime.execute().expect("scheduler should succeed");
    for (i, id) in ids.iter().enumerate() {
        if let Err(e) = runtime.result_error(*id) {
            panic!("expr {i} errored: {e}");
        }
    }

    let one = lookup_module(scope, "set_one");
    let two = lookup_module(scope, "set_two");
    let one_carrier = one.type_members.borrow().get("Carrier").cloned();
    let two_carrier = two.type_members.borrow().get("Carrier").cloned();
    assert!(
        matches!(&one_carrier, Some(KType::AbstractType { name, .. }) if name == "Carrier"),
        "the first application must mint an abstract Carrier, got {one_carrier:?}",
    );
    assert!(
        matches!(&two_carrier, Some(KType::AbstractType { name, .. }) if name == "Carrier"),
        "the second application must mint an abstract Carrier, got {two_carrier:?}",
    );
    assert_ne!(
        one_carrier, two_carrier,
        "two applications of a module-returning FN must mint distinct abstract types",
    );
}

/// An unascribed module is admitted by a constraint-role `Signature { sig, .. }` slot iff its
/// self-sig structurally satisfies the signature — no ascription required. `int_ord = (LET
/// compare = 7)` structurally satisfies `Ordered = (VAL compare :Number)`, so the call
/// succeeds and produces the generated module.
#[test]
fn functor_admits_unascribed_module_structurally() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = (LET compare = 7)",
    );
    run(
        scope,
        "FN (MAKESET elem :Ordered) -> Module = (MODULE generated = (LET inner = 1))",
    );
    // Type-classified binder so the auto-wrap pass triggers in the
    // `Signature { .. }` slot. The LET partition guard requires module carriers
    // to ride Type-classified names (design/typing/elaboration.md § Binding-map
    // partition).
    run(scope, "LET unascribed = int_ord");
    run(scope, "LET set_value = (MAKESET unascribed)");

    let m = lookup_module(scope, "set_value");
    let inner = m
        .child_scope()
        .bindings()
        .data()
        .get("inner")
        .map(|(o, _, _)| *o);
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
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE no_compare = (LET other = 1)",
    );
    run(
        scope,
        "FN (MAKESET elem :Ordered) -> Module = (MODULE generated = (LET inner = 1))",
    );
    run(scope, "LET arg = no_compare");
    let mut runtime = KoanRuntime::new();
    let root = runtime.dispatch_in_scope(parse_one("MAKESET arg"), scope);
    runtime
        .execute()
        .expect("a dispatch failure is slot-terminal, not a fatal execute error");
    let err = runtime
        .result_error(root)
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
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Ordered = (VAL compare :Number)\n\
         SIG Hashed = (VAL hash :Number)\n\
         MODULE int_ord = (LET compare = 7)\n\
         MODULE int_hash = (LET hash = 11)",
    );
    run(
        scope,
        "LET int_ord_a = (int_ord :! Ordered)\n\
         LET int_hash_a = (int_hash :! Hashed)",
    );
    run(
        scope,
        "FN (MAKESET elem :Ordered) -> Module = (MODULE generated = (LET tag = 1))",
    );
    run(
        scope,
        "FN (MAKESET elem :Hashed) -> Module = (MODULE generated = (LET tag = 2))",
    );
    run(scope, "LET ord_set = (MAKESET (int_ord_a))");
    run(scope, "LET hash_set = (MAKESET (int_hash_a))");

    let mo = lookup_module(scope, "ord_set");
    let mh = lookup_module(scope, "hash_set");
    let to = mo
        .child_scope()
        .bindings()
        .data()
        .get("tag")
        .map(|(o, _, _)| *o);
    let th = mh
        .child_scope()
        .bindings()
        .data()
        .get("tag")
        .map(|(o, _, _)| *o);
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
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = (LET compare = 7)",
    );
    run(scope, "LET int_view = (int_ord :! Ordered)");
    run(
        scope,
        "FN (MAKESET elem :Ordered) -> Module = (MODULE generated = (LET sample = (elem.compare)))",
    );
    run(scope, "LET set_value = (MAKESET int_view)");

    let m = lookup_module(scope, "set_value");
    let sample = m
        .child_scope()
        .bindings()
        .data()
        .get("sample")
        .map(|(o, _, _)| *o);
    assert!(matches!(sample, Some(KObject::Number(n)) if *n == 7.0));
}

/// The monad program of record: a `NEWTYPE (Type AS Wrapper)` family, a SIG whose `pure` VAL
/// slot returns `:(Number AS Wrap)`, and a module supplying `Wrap = Wrapper` plus a `pure` whose
/// body constructs `Wrapper (x)`. Returned as a source string reused by the end-to-end tests.
fn monad_program() -> &'static str {
    "NEWTYPE (Type AS Wrapper)\n\
     SIG Monad = ((TYPE (Type AS Wrap)) (VAL pure :(FN (x :Number) -> :(Number AS Wrap))))\n\
     MODULE id_monad = ((LET Wrap = Wrapper) \
     (LET pure = (FN (PURE x :Number) -> :(Number AS Wrapper) = (Wrapper (x)))))"
}

/// `id_monad :| Monad` succeeds: `substitute_sig_members` substitutes the SIG's `Wrap` slot to
/// the module's `Wrapper` and descends the `pure` VAL slot's `ConstructorApply` return type, so
/// the module's `pure` (returning `:(Number AS Wrapper)`) satisfies the substituted
/// `:(Number AS Wrap)` slot end to end.
#[test]
fn hk_value_slot_satisfies_after_substitution() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, monad_program());
    run(scope, "LET view = (id_monad :| Monad)");
    assert!(
        matches!(
            lookup_module(scope, "view"),
            m if m.child_scope().bindings().data().get("pure").is_some()
        ),
        "id_monad must satisfy Monad and bind a view module carrying `pure`",
    );
}

/// `(id_monad.pure {x = 3.0})` runs the module's `pure`, whose declared return type
/// `:(Number AS Wrapper)` is checked via `matches_value` against the constructed
/// `Wrapper (x)` — the per-call return check passing on an identity-wrapper value.
#[test]
fn pure_call_passes_return_check() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, monad_program());
    let result = run_one(scope, parse_one("id_monad.pure {x = 3.0}"));
    match result {
        KObject::Wrapped { inner, type_id } => {
            assert!(
                matches!(**type_id, KType::ConstructorApply { .. }),
                "pure must return an identity-wrapper value, got {:?}",
                type_id,
            );
            assert!(matches!(inner.get(), KObject::Number(n) if *n == 3.0));
        }
        other => panic!("expected Wrapped from pure, got {:?}", other.ktype()),
    }
}

/// A bare Type-classified argument (`MAKESET int_ord_a`) auto-wraps to a value lookup
/// just like the lowercase-identifier and parens-wrapped forms do.
#[test]
fn functor_argument_bare_type_token_auto_wraps() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = (LET compare = 7)",
    );
    run(scope, "LET int_ord_a = (int_ord :! Ordered)");
    run(
        scope,
        "FN (MAKESET elem :Ordered) -> Module = \
         (MODULE generated = (LET sample = (elem.compare)))",
    );
    run(scope, "LET set_value = (MAKESET int_ord_a)");

    let m = lookup_module(scope, "set_value");
    let sample = m
        .child_scope()
        .bindings()
        .data()
        .get("sample")
        .map(|(o, _, _)| *o);
    assert!(matches!(sample, Some(KObject::Number(n)) if *n == 7.0));
}

/// Two opaque ascriptions of a module satisfying a SIG with `TYPE (Type AS Wrap)`
/// mint distinct per-call `TypeConstructor` slots —
/// the higher-kinded analogue of `functor_application_is_generative`.
#[test]
fn opaque_ascription_mints_fresh_type_constructor_per_call() {
    use crate::machine::model::KKind;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let src = "NEWTYPE (Type AS Wrapper)\n\
               SIG Monad = ((TYPE (Type AS Wrap)))\n\
               MODULE int_list = ((LET Wrap = Wrapper))\n\
               LET first = (int_list :| Monad)\n\
               LET second = (int_list :| Monad)";
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
    let a = lookup_module(scope, "first");
    let b = lookup_module(scope, "second");
    let a_wrap = a.type_members.borrow().get("Wrap").cloned();
    let b_wrap = b.type_members.borrow().get("Wrap").cloned();
    let is_type_constructor = |kt: &Option<KType>| {
        matches!(
            kt,
            Some(KType::SetRef { set, index }) if set.member(*index).kind == KKind::TypeConstructor
        )
    };
    assert!(is_type_constructor(&a_wrap));
    assert!(is_type_constructor(&b_wrap));
    // Identity is the content digest, but an opaque-ascription set is *generative*: each
    // application folds its per-call nonce (the view module's `scope_id`) into the set digest,
    // so the two sets digest apart even though their member content is identical. The origin
    // scope_ids differ because they ARE those distinct nonces.
    match (&a_wrap, &b_wrap) {
        (
            Some(KType::SetRef {
                set: aset,
                index: ai,
            }),
            Some(KType::SetRef {
                set: bset,
                index: bi,
            }),
        ) => {
            assert_ne!(
                aset.member(*ai).scope_id,
                bset.member(*bi).scope_id,
                "two opaque ascriptions must mint TypeConstructor slots with distinct scope_id",
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
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    // Plain `LET` plus `LET = FN` so the re-bind walk hits both the `data` write
    // and the `KFunction → functions` mirror.
    run(
        scope,
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = ((LET compare = 7) (LET helper = (FN (HELP x :Number) -> Number = (x))))\n\
         LET held = (int_ord :| Ordered)",
    );
    let held = lookup_module(scope, "held");

    // Churn the run-root region, then re-ascribe to allocate a second re-bind
    // scope. The original `held` must still walk through to its own pair.
    run(scope, "FN (CHURNCALL) -> Number = (1)");
    for _ in 0..20 {
        run_one(scope, parse_one("CHURNCALL"));
    }
    run(scope, "LET held2 = (int_ord :| Ordered)");

    let child = held.child_scope();
    let inner = child.bindings().data();
    assert!(
        matches!(inner.get("compare").map(|(o, _, _)| *o), Some(KObject::Number(n)) if *n == 7.0),
        "held.child_scope().compare must still read 7.0 after subsequent churn",
    );
    assert!(
        matches!(
            inner.get("helper").map(|(o, _, _)| *o),
            Some(KObject::KFunction(_))
        ),
        "held.child_scope().helper must still resolve to a KFunction after churn",
    );
}
