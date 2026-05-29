//! Functor integration (module-system stage 2 — functor slice).

use crate::builtins::test_support::{parse_one, run, run_one, run_root_silent};
use crate::machine::model::{KObject, KType};
use crate::machine::{KErrorKind, RuntimeArena};
use crate::machine::execute::Scheduler;
use crate::parse::parse;

/// Test 1 — Functor returns a module. A FN with a sig-typed parameter whose body
/// declares `MODULE Result = (LET inner = 1)` produces a `KObject::KModule` whose
/// child scope carries `inner = 1`.
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
        "FN (MAKESET elem :OrderedSig) -> Module = (MODULE Result = (LET inner = 1))",
    );
    run(scope, "LET SetValue = (MAKESET IntOrdA)");

    let data = scope.bindings().data();
    let m = match data.get("SetValue").map(|(o, _)| *o) {
        Some(KObject::KTypeValue(KType::Module { module: m, frame: _ })) => *m,
        other => panic!("SetValue should be a module, got {:?}", other.map(|o| o.ktype())),
    };
    let inner = m.child_scope().bindings().data().get("inner").map(|(o, _)| *o);
    assert!(matches!(inner, Some(KObject::Number(n)) if *n == 1.0));
}

/// Test 2 — Functor body sees the signature-typed parameter. `(elem.compare)` inside
/// the body resolves through ATTR's KModule arm and reads `7` from the ascribed
/// IntOrd; that value lands in `S.sample`.
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
        "FN (MAKESET elem :OrderedSig) -> Module = (MODULE Result = (LET sample = (elem.compare)))",
    );
    run(scope, "LET SetValue = (MAKESET IntOrdA)");

    let data = scope.bindings().data();
    let m = match data.get("SetValue").map(|(o, _)| *o) {
        Some(KObject::KTypeValue(KType::Module { module: m, frame: _ })) => *m,
        other => panic!("SetValue should be a module, got {:?}", other.map(|o| o.ktype())),
    };
    let sample = m.child_scope().bindings().data().get("sample").map(|(o, _)| *o);
    assert!(matches!(sample, Some(KObject::Number(n)) if *n == 7.0));
}

/// Test 3 — Per-call generative semantics. Two functor invocations produce modules
/// whose `scope_id` differs, since each call's body runs in a fresh per-call frame
/// whose arena hands out a fresh scope address. The `Module::scope_id` is the
/// identity carrier `KType::UserType { kind: Module, .. }` would mint after `:|` opaque ascription;
/// asserting on the bare `scope_id`s themselves directly pins the per-call
/// generativity property without depending on multi-statement-FN-body forward refs
/// (which fold through `CONS` and don't share lexical bindings between statements).
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
        "FN (MAKESET elem :OrderedSig) -> Module = (MODULE Result = (LET inner = 1))",
    );
    run(scope, "LET SetOne = (MAKESET (IntOrdA))");
    run(scope, "LET SetTwo = (MAKESET (IntOrdA))");

    let data = scope.bindings().data();
    let m1 = match data.get("SetOne").map(|(o, _)| *o) {
        Some(KObject::KTypeValue(KType::Module { module: m, frame: _ })) => *m,
        other => panic!("SetOne should be a module, got ktype={:?}", other.map(|o| o.ktype())),
    };
    let m2 = match data.get("SetTwo").map(|(o, _)| *o) {
        Some(KObject::KTypeValue(KType::Module { module: m, frame: _ })) => *m,
        _ => panic!("SetTwo should be a module"),
    };
    // Per-call generativity: each invocation allocates a fresh `child_scope` in its
    // own per-call frame's arena, so `scope_id`s differ. After `:|` ascription this
    // would seed two distinct `KType::UserType { kind: Module, scope_id, .. }` values; the
    // identity carrier is what makes the abstract types incompatible across calls.
    assert_ne!(
        m1.scope_id(),
        m2.scope_id(),
        "two functor applications must produce modules with distinct scope_id",
    );
}

/// Test 4 — Dispatch admissibility filters non-conforming modules. An unascribed
/// `MODULE Empty` has an empty `compatible_sigs` set, so `accepts_part` for the
/// `SatisfiesSignature { sig_id }` slot rejects it and dispatch fails. (Also: ascribing
/// `Empty :! OrderedSig` would itself fail at shape-check time since `Empty` lacks a
/// `compare` member — verified by `ascription_missing_member_errors` above; the
/// admissibility-only path is what's pinned here.)
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
        "FN (MAKESET elem :OrderedSig) -> Module = (MODULE Result = (LET inner = 1))",
    );
    // Bind `IntOrd` (an unascribed module) under a Type-classified alias so the
    // auto-wrap pass triggers when the name appears in the SatisfiesSignature slot.
    // The LET partition guard requires module/signature carriers to ride
    // Type-classified binders only — a lowercase alias would be rejected at the
    // LET site (see design/typing/elaboration.md § Binding-map partition).
    //
    // PR C surface: cache-driven strict admission inspects `Unascribed`'s
    // resolved carrier type against the `SatisfiesSignature` slot and rejects
    // upfront (unascribed module doesn't satisfy the signature constraint).
    // The MAKESET overload falls out, no other bucket admits, and the post-walk
    // surfaces `DispatchFailed` — replacing the pre-PR-C bind-time
    // `TypeMismatch` that flowed from tentative-admit. See PR C surface-change
    // audit in `scratch/plan-unified-walk-pr-c.md`.
    run(scope, "LET Unascribed = IntOrd");
    let mut sched = Scheduler::new();
    sched.add_dispatch(parse_one("MAKESET Unascribed"), scope);
    let err = sched
        .execute()
        .expect_err("expected DispatchFailed at execute boundary");
    assert!(
        matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
        "expected DispatchFailed (PR C strict-only surface), got {err}",
    );
}

/// Test 5 — Sig-typed-parameter overload selection. Two functors share a keyword
/// (`MAKESET`) but differ on parameter sig (`OrderedSig` vs `HashedSig`); a call
/// with an OrderedSig-conforming module routes to the first body, a HashedSig one to
/// the second.
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
        "FN (MAKESET elem :OrderedSig) -> Module = (MODULE Result = (LET tag = 1))",
    );
    run(
        scope,
        "FN (MAKESET elem :HashedSig) -> Module = (MODULE Result = (LET tag = 2))",
    );
    run(scope, "LET OrdSet = (MAKESET (IntOrdA))");
    run(scope, "LET HashSet = (MAKESET (IntHashA))");

    let data = scope.bindings().data();
    let mo = match data.get("OrdSet").map(|(o, _)| *o) { Some(KObject::KTypeValue(KType::Module { module: m, frame: _ })) => *m, _ => panic!("OrdSet not module") };
    let mh = match data.get("HashSet").map(|(o, _)| *o) { Some(KObject::KTypeValue(KType::Module { module: m, frame: _ })) => *m, _ => panic!("HashSet not module") };
    let to = mo.child_scope().bindings().data().get("tag").map(|(o, _)| *o);
    let th = mh.child_scope().bindings().data().get("tag").map(|(o, _)| *o);
    assert!(matches!(to, Some(KObject::Number(n)) if *n == 1.0),
            "OrderedSig call should pick body with tag=1, got {:?}", to.map(|o| o.ktype()));
    assert!(matches!(th, Some(KObject::Number(n)) if *n == 2.0),
            "HashedSig call should pick body with tag=2, got {:?}", th.map(|o| o.ktype()));
}

/// Test 6 — Transparent ascription satisfies `SatisfiesSignature`. Pins that `:!`
/// (transparent) populates `compatible_sigs` the same way `:|` (opaque) does — the
/// functor's sig-typed slot accepts a `:!`-ascribed module, and the body still reads
/// the underlying member through the view.
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
        "FN (MAKESET elem :OrderedSig) -> Module = (MODULE Result = (LET sample = (elem.compare)))",
    );
    run(scope, "LET SetValue = (MAKESET IntView)");

    let data = scope.bindings().data();
    let m = match data.get("SetValue").map(|(o, _)| *o) {
        Some(KObject::KTypeValue(KType::Module { module: m, frame: _ })) => *m,
        other => panic!("SetValue should be a module, got {:?}", other.map(|o| o.ktype())),
    };
    let sample = m.child_scope().bindings().data().get("sample").map(|(o, _)| *o);
    assert!(matches!(sample, Some(KObject::Number(n)) if *n == 7.0));
}

/// Test 7 — bare Type-token argument auto-wraps into a value-lookup. A `LET`-bound
/// Type-classified name (`IntOrdA`) passed as `MAKESET IntOrdA` should resolve to its
/// bound `KModule` the same way the lowercase-identifier and parens-wrapped forms do.
/// Pins the auto-wrap extension to Type-tokens via the `BareTypeLeaf` fast lane.
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
         (MODULE Result = (LET sample = (elem.compare)))",
    );
    run(scope, "LET SetValue = (MAKESET IntOrdA)");

    let data = scope.bindings().data();
    let m = match data.get("SetValue").map(|(o, _)| *o) {
        Some(KObject::KTypeValue(KType::Module { module: m, frame: _ })) => *m,
        other => panic!("SetValue should be a module, got {:?}", other.map(|o| o.ktype())),
    };
    let sample = m.child_scope().bindings().data().get("sample").map(|(o, _)| *o);
    assert!(matches!(sample, Some(KObject::Number(n)) if *n == 7.0));
}

/// Module-system stage 2 Workstream B: two opaque ascriptions of a module that
/// satisfies a SIG declaring `LET Wrap = (TYPE_CONSTRUCTOR T)` mint distinct
/// per-call `KType::UserType { kind: TypeConstructor, .. }` values under each
/// resulting module's `type_members[Wrap]`. Mirror of
/// `functor_application_is_generative` — pins the abstraction-barrier property
/// for higher-kinded slots.
#[test]
fn opaque_ascription_mints_fresh_type_constructor_per_call() {
    use crate::machine::model::types::UserTypeKind;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let src = "SIG MonadSig = ((LET Wrap = (TYPE_CONSTRUCTOR Type)))\n\
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
    let data = scope.bindings().data();
    let a = match data.get("First").map(|(o, _)| *o) {
        Some(KObject::KTypeValue(KType::Module { module: m, frame: _ })) => *m,
        _ => panic!("First should be a module"),
    };
    let b = match data.get("Second").map(|(o, _)| *o) {
        Some(KObject::KTypeValue(KType::Module { module: m, frame: _ })) => *m,
        _ => panic!("Second should be a module"),
    };
    let a_wrap = a.type_members.borrow().get("Wrap").cloned();
    let b_wrap = b.type_members.borrow().get("Wrap").cloned();
    // Both wraps must be UserType(TypeConstructor) — the SIG slot kind, not
    // the default Module kind.
    assert!(matches!(
        &a_wrap,
        Some(KType::UserType { kind: UserTypeKind::TypeConstructor { .. }, .. })
    ));
    assert!(matches!(
        &b_wrap,
        Some(KType::UserType { kind: UserTypeKind::TypeConstructor { .. }, .. })
    ));
    // Per-call generativity: two opaque ascriptions get distinct scope_ids on the
    // minted slot, even though the SIG and source module are the same. The manual
    // `UserTypeKind::PartialEq` ignores `param_names`, so the equality test below
    // is gated on `(scope_id, name)` — exactly the abstraction-barrier property.
    match (&a_wrap, &b_wrap) {
        (
            Some(KType::UserType { scope_id: aid, .. }),
            Some(KType::UserType { scope_id: bid, .. }),
        ) => {
            assert_ne!(
                aid, bid,
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

/// Miri audit-slate: pins the opaque-ascription re-bind path under tree borrows.
/// `body_opaque` allocates a fresh child scope, mirrors the source module's bindings
/// into it via `try_bulk_install_from` (which replays each entry through `try_apply`
/// so a `KFunction` entry exercises the `functions`-map mirror as well as
/// the plain `data` write), and builds the resulting `Module` over the captured
/// scope. The captured-reference shape is the per-call analogue of the
/// `module_child_scope_transmute_does_not_dangle` site, so the slate needs an
/// end-to-end pin that the re-bind walk plus the held `&Module` survive subsequent
/// arena churn under tree borrows.
#[test]
fn opaque_ascription_re_binds_do_not_alias_unsoundly() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    // The source module carries a plain `LET` plus a `LET = FN` so the
    // `try_bulk_install_from` walk hits the `KFunction → functions` mirror
    // (`LET <name> = (FN ...)` is the canonical shape for module-member functions per
    // `module_member_function_via_let_fn` in `module_def.rs`) as well as the plain
    // `data` write path. The SIG only requires `compare`; `helper` is a non-required
    // FN member that still rides through the re-bind walk.
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         MODULE IntOrd = ((LET compare = 7) (LET helper = (FN (HELP x :Number) -> Number = (x))))\n\
         LET Held = (IntOrd :| OrderedSig)",
    );
    // Extract the module pointer *before* further dispatches — `bindings().data()`
    // returns a `Ref<_>` and holding it across a `run` would block the RefCell
    // writes the new dispatches need.
    let held = {
        let data = scope.bindings().data();
        match data.get("Held").map(|(o, _)| *o) {
            Some(KObject::KTypeValue(KType::Module { module: m, frame: _ })) => *m,
            other => panic!("Held should be a module, got {:?}", other.map(|o| o.ktype())),
        }
    };

    // Subsequent allocations and FN calls churn the run-root arena. The re-bound
    // child scope (and the `&Module` pointing at it) must keep both maps live
    // across that churn.
    run(scope, "FN (CHURNCALL) -> Number = (1)");
    for _ in 0..20 {
        run_one(scope, parse_one("CHURNCALL"));
    }
    // Re-ascribe a second time to allocate another re-bind scope; the original
    // `held` reference must still walk through to its own data/functions pair.
    run(scope, "LET Held2 = (IntOrd :| OrderedSig)");

    // Read both binding kinds back through the held module's child scope. The
    // `compare` slot tests the plain `data` mirror; the `helper` slot tests the
    // `KFunction → functions` mirror written by `try_apply`.
    let child = held.child_scope();
    let inner = child.bindings().data();
    assert!(
        matches!(inner.get("compare").map(|(o, _)| *o), Some(KObject::Number(n)) if *n == 7.0),
        "held.child_scope().compare must still read 7.0 after subsequent churn",
    );
    assert!(
        matches!(inner.get("helper").map(|(o, _)| *o), Some(KObject::KFunction(_, _))),
        "held.child_scope().helper must still resolve to a KFunction after churn",
    );
}
