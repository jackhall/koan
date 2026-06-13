//! Functor integration: module-typed parameters, signature-bound dispatch,
//! per-call generativity.

use crate::builtins::test_support::{parse_one, run, run_one, run_root_silent};
use crate::machine::execute::Scheduler;
use crate::machine::model::{KObject, KType};
use crate::machine::{KErrorKind, RuntimeArena};
use crate::parse::parse;

#[test]
fn functor_returns_a_module() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         MODULE IntOrd = (LET compare = 7)",
    );
    run(scope, "LET IntOrdA = (IntOrd :! OrderedSig)");
    run(
        scope,
        "FN (MAKESET elem :OrderedSig) -> Module = (MODULE Generated = (LET inner = 1))",
    );
    run(scope, "LET SetValue = (MAKESET IntOrdA)");

    let m = match scope.resolve_type("SetValue") {
        Some(KType::Module {
            module: m,
            frame: _,
        }) => *m,
        _ => panic!("SetValue should be a module identity in types"),
    };
    let inner = m
        .child_scope()
        .bindings()
        .data()
        .get("inner")
        .map(|(o, _)| *o);
    assert!(matches!(inner, Some(KObject::Number(n)) if *n == 1.0));
}

#[test]
fn functor_body_reads_signature_typed_parameter() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         MODULE IntOrd = (LET compare = 7)",
    );
    run(scope, "LET IntOrdA = (IntOrd :! OrderedSig)");
    run(
        scope,
        "FN (MAKESET elem :OrderedSig) -> Module = (MODULE Generated = (LET sample = (elem.compare)))",
    );
    run(scope, "LET SetValue = (MAKESET IntOrdA)");

    let m = match scope.resolve_type("SetValue") {
        Some(KType::Module {
            module: m,
            frame: _,
        }) => *m,
        _ => panic!("SetValue should be a module identity in types"),
    };
    let sample = m
        .child_scope()
        .bindings()
        .data()
        .get("sample")
        .map(|(o, _)| *o);
    assert!(matches!(sample, Some(KObject::Number(n)) if *n == 7.0));
}

/// Per-call generativity: two invocations produce modules with distinct `scope_id`.
/// Asserts on bare `scope_id`s rather than on minted abstract types, which would
/// require multi-statement-FN-body forward refs that don't share lexical bindings.
#[test]
fn functor_application_is_generative() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         MODULE IntOrd = (LET compare = 7)",
    );
    run(scope, "LET IntOrdA = (IntOrd :! OrderedSig)");
    run(
        scope,
        "FN (MAKESET elem :OrderedSig) -> Module = (MODULE Generated = (LET inner = 1))",
    );
    run(scope, "LET SetOne = (MAKESET (IntOrdA))");
    run(scope, "LET SetTwo = (MAKESET (IntOrdA))");

    let m1 = match scope.resolve_type("SetOne") {
        Some(KType::Module {
            module: m,
            frame: _,
        }) => *m,
        _ => panic!("SetOne should be a module identity in types"),
    };
    let m2 = match scope.resolve_type("SetTwo") {
        Some(KType::Module {
            module: m,
            frame: _,
        }) => *m,
        _ => panic!("SetTwo should be a module identity in types"),
    };
    assert_ne!(
        m1.scope_id(),
        m2.scope_id(),
        "two functor applications must produce modules with distinct scope_id",
    );
}

/// Dispatch admissibility rejects an unascribed module against a
/// `Signature { sig, .. }` (constraint-role) slot.
#[test]
fn functor_rejects_unascribed_module_argument() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         MODULE IntOrd = (LET compare = 7)",
    );
    run(
        scope,
        "FN (MAKESET elem :OrderedSig) -> Module = (MODULE Generated = (LET inner = 1))",
    );
    // Type-classified binder so the auto-wrap pass triggers in the
    // `Signature { .. }` slot. The LET partition guard requires module carriers
    // to ride Type-classified names (design/typing/elaboration.md § Binding-map
    // partition).
    run(scope, "LET Unascribed = IntOrd");
    let mut sched = Scheduler::new();
    let root = sched.add_dispatch(parse_one("MAKESET Unascribed"), scope);
    sched
        .execute()
        .expect("a dispatch failure is slot-terminal, not a fatal execute error");
    let err = sched
        .read_result(root)
        .err()
        .expect("expected a DispatchFailed in the dispatch slot");
    assert!(
        matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
        "expected DispatchFailed, got {err}",
    );
}

/// Two functors share a keyword `MAKESET` but differ on parameter sig
/// (`OrderedSig` vs `HashedSig`); dispatch routes by the argument's satisfied sig.
#[test]
fn functor_overloads_dispatch_by_signature_bound_param() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         SIG HashedSig = (VAL hash :Number)\n\
         MODULE IntOrd = (LET compare = 7)\n\
         MODULE IntHash = (LET hash = 11)",
    );
    run(
        scope,
        "LET IntOrdA = (IntOrd :! OrderedSig)\n\
         LET IntHashA = (IntHash :! HashedSig)",
    );
    run(
        scope,
        "FN (MAKESET elem :OrderedSig) -> Module = (MODULE Generated = (LET tag = 1))",
    );
    run(
        scope,
        "FN (MAKESET elem :HashedSig) -> Module = (MODULE Generated = (LET tag = 2))",
    );
    run(scope, "LET OrdSet = (MAKESET (IntOrdA))");
    run(scope, "LET HashSet = (MAKESET (IntHashA))");

    let mo = match scope.resolve_type("OrdSet") {
        Some(KType::Module {
            module: m,
            frame: _,
        }) => *m,
        _ => panic!("OrdSet not a module identity in types"),
    };
    let mh = match scope.resolve_type("HashSet") {
        Some(KType::Module {
            module: m,
            frame: _,
        }) => *m,
        _ => panic!("HashSet not a module identity in types"),
    };
    let to = mo
        .child_scope()
        .bindings()
        .data()
        .get("tag")
        .map(|(o, _)| *o);
    let th = mh
        .child_scope()
        .bindings()
        .data()
        .get("tag")
        .map(|(o, _)| *o);
    assert!(
        matches!(to, Some(KObject::Number(n)) if *n == 1.0),
        "OrderedSig call should pick body with tag=1, got {:?}",
        to.map(|o| o.ktype())
    );
    assert!(
        matches!(th, Some(KObject::Number(n)) if *n == 2.0),
        "HashedSig call should pick body with tag=2, got {:?}",
        th.map(|o| o.ktype())
    );
}

/// `:!` (transparent) populates `compatible_sigs` the same way `:|` (opaque) does,
/// and the body still reads the underlying member through the view.
#[test]
fn transparent_ascription_satisfies_signature_bound_slot() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         MODULE IntOrd = (LET compare = 7)",
    );
    run(scope, "LET IntView = (IntOrd :! OrderedSig)");
    run(
        scope,
        "FN (MAKESET elem :OrderedSig) -> Module = (MODULE Generated = (LET sample = (elem.compare)))",
    );
    run(scope, "LET SetValue = (MAKESET IntView)");

    let m = match scope.resolve_type("SetValue") {
        Some(KType::Module {
            module: m,
            frame: _,
        }) => *m,
        _ => panic!("SetValue should be a module identity in types"),
    };
    let sample = m
        .child_scope()
        .bindings()
        .data()
        .get("sample")
        .map(|(o, _)| *o);
    assert!(matches!(sample, Some(KObject::Number(n)) if *n == 7.0));
}

/// A bare Type-classified argument (`MAKESET IntOrdA`) auto-wraps to a value lookup
/// just like the lowercase-identifier and parens-wrapped forms do.
#[test]
fn functor_argument_bare_type_token_auto_wraps() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         MODULE IntOrd = (LET compare = 7)",
    );
    run(scope, "LET IntOrdA = (IntOrd :! OrderedSig)");
    run(
        scope,
        "FN (MAKESET elem :OrderedSig) -> Module = \
         (MODULE Generated = (LET sample = (elem.compare)))",
    );
    run(scope, "LET SetValue = (MAKESET IntOrdA)");

    let m = match scope.resolve_type("SetValue") {
        Some(KType::Module {
            module: m,
            frame: _,
        }) => *m,
        _ => panic!("SetValue should be a module identity in types"),
    };
    let sample = m
        .child_scope()
        .bindings()
        .data()
        .get("sample")
        .map(|(o, _)| *o);
    assert!(matches!(sample, Some(KObject::Number(n)) if *n == 7.0));
}

/// Two opaque ascriptions of a module satisfying a SIG with `LET Wrap =
/// (TEMPLATE T)` mint distinct per-call `TypeConstructor` slots —
/// the higher-kinded analogue of `functor_application_is_generative`.
#[test]
fn opaque_ascription_mints_fresh_type_constructor_per_call() {
    use crate::machine::model::types::KKind;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let src = "SIG MonadSig = ((LET Wrap = (TEMPLATE Type)))\n\
               MODULE IntList = ((LET Wrap = Number))\n\
               LET First = (IntList :| MonadSig)\n\
               LET Second = (IntList :| MonadSig)";
    let exprs = parse(src).expect("parse should succeed");
    let mut sched = Scheduler::new();
    let mut ids = Vec::new();
    for expr in exprs {
        ids.push(sched.add_dispatch(expr, scope));
    }
    sched.execute().expect("scheduler should succeed");
    for (i, id) in ids.iter().enumerate() {
        if let Err(e) = sched.read_result(*id) {
            panic!("expr {} errored: {}", i, e);
        }
    }
    let a = match scope.resolve_type("First") {
        Some(KType::Module {
            module: m,
            frame: _,
        }) => *m,
        _ => panic!("First should be a module identity in types"),
    };
    let b = match scope.resolve_type("Second") {
        Some(KType::Module {
            module: m,
            frame: _,
        }) => *m,
        _ => panic!("Second should be a module identity in types"),
    };
    let a_wrap = a.type_members.borrow().get("Wrap").cloned();
    let b_wrap = b.type_members.borrow().get("Wrap").cloned();
    let is_type_constructor = |kt: &Option<KType<'_>>| {
        matches!(
            kt,
            Some(KType::SetRef { set, index }) if set.member(*index).kind == KKind::TypeConstructor
        )
    };
    assert!(is_type_constructor(&a_wrap));
    assert!(is_type_constructor(&b_wrap));
    // Identity is the `(set ptr, index)` pair — two ascriptions mint distinct sets, so the
    // members' origin scope_ids differ and the slots compare unequal.
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
/// survive subsequent arena churn under tree borrows.
#[test]
fn opaque_ascription_re_binds_do_not_alias_unsoundly() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    // Plain `LET` plus `LET = FN` so the re-bind walk hits both the `data` write
    // and the `KFunction → functions` mirror.
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         MODULE IntOrd = ((LET compare = 7) (LET helper = (FN (HELP x :Number) -> Number = (x))))\n\
         LET Held = (IntOrd :| OrderedSig)",
    );
    // `Held` is a type-only `:|`-ascribed module alias — its identity lives in `types`.
    let held = match scope.resolve_type("Held") {
        Some(KType::Module {
            module: m,
            frame: _,
        }) => *m,
        other => panic!("Held should be a module identity in types, got {other:?}"),
    };

    // Churn the run-root arena, then re-ascribe to allocate a second re-bind
    // scope. The original `held` must still walk through to its own pair.
    run(scope, "FN (CHURNCALL) -> Number = (1)");
    for _ in 0..20 {
        run_one(scope, parse_one("CHURNCALL"));
    }
    run(scope, "LET Held2 = (IntOrd :| OrderedSig)");

    let child = held.child_scope();
    let inner = child.bindings().data();
    assert!(
        matches!(inner.get("compare").map(|(o, _)| *o), Some(KObject::Number(n)) if *n == 7.0),
        "held.child_scope().compare must still read 7.0 after subsequent churn",
    );
    assert!(
        matches!(
            inner.get("helper").map(|(o, _)| *o),
            Some(KObject::KFunction(_, _))
        ),
        "held.child_scope().helper must still resolve to a KFunction after churn",
    );
}
