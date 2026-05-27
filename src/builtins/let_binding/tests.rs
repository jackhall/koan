use std::collections::HashMap;
use std::rc::Rc;

use super::body;
use crate::builtins::default_scope;
use crate::builtins::test_support::run_root_bare;
use crate::machine::model::{KObject, KType};
use crate::machine::ArgumentBundle;
use crate::machine::execute::Scheduler;

#[test]
fn let_inserts_binding_into_scope() {
    use crate::machine::RuntimeArena;
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    let mut sched = Scheduler::new();
    let mut args = HashMap::new();
    args.insert("name".to_string(), Rc::new(KObject::KString("x".into())));
    args.insert("value".to_string(), Rc::new(KObject::Number(42.0)));

    let value = body(scope, &mut sched, ArgumentBundle { args }).expect_value("LET");
    assert!(matches!(value, KObject::Number(n) if *n == 42.0));
    let data = scope.bindings().data();
    let (entry, _) = data.get("x").expect("expected binding 'x'");
    assert!(matches!(entry, KObject::Number(n) if *n == 42.0));
}

/// Smoke test for LET's binder_name extractor: structural extraction of `parts[1]`
/// returns the bound name without requiring sub-dispatches.
#[test]
fn binder_name_extracts_let_name() {
    use crate::parse::parse;
    let mut exprs = parse("LET hello = 1").expect("parse should succeed");
    let expr = exprs.remove(0);
    let name = super::binder_name(&expr);
    assert_eq!(name.as_deref(), Some("hello"));
}

/// End-to-end install-then-clear: dispatch `LET x = 1` through the scheduler. The
/// binder_name hook installs `placeholders["x"] = NodeId(...)` before the body runs;
/// after the body finalizes via `bind_value`, the placeholder is removed.
#[test]
fn binder_name_install_then_body_finalize_clears_placeholder() {
    use crate::machine::RuntimeArena;
    use crate::machine::execute::Scheduler;
    use crate::builtins::default_scope;
    use crate::parse::parse;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let exprs = parse("LET hello = 1").unwrap();
    for e in exprs { sched.add_dispatch(e, scope); }
    sched.execute().unwrap();
    // After execute, placeholders should not contain "hello" — bind_value cleared it.
    assert!(scope.bindings().placeholders().get("hello").is_none());
    assert!(matches!(scope.lookup("hello"), Some(KObject::Number(n)) if *n == 1.0));
}

/// Phase 3: `LET T = T` is a trivially cyclic alias — the RHS references the binder
/// itself. Under index-gated resolution the self-reference is the degenerate "value
/// LET defined at the same lexical position as the reference" case: the producer's
/// `Ty` placeholder sits at index `i`, the consumer reads at cutoff `i`, and the
/// strict `b.idx < c` predicate makes the binding invisible — so the consumer
/// surfaces `UnboundName` rather than the old self-park cycle path. The
/// non-nominal-binder carve-out does not apply (LET is value-style gated).
/// Same surface as the Identifier-LHS form (`LET x = x`).
#[test]
fn let_t_cycle_errors() {
    use crate::machine::RuntimeArena;
    use crate::machine::execute::Scheduler;
    use crate::machine::{KErrorKind, SchedulerHandle};
    use crate::builtins::default_scope;
    use crate::parse::parse;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let exprs = parse("LET Ty = Ty").unwrap();
    let ids = sched.enter_block(scope.id, exprs, scope);
    sched.execute().expect("execute does not surface per-slot errors");
    let res = sched.read_result(ids[0]);
    match res {
        Err(e) => assert!(
            matches!(&e.kind, KErrorKind::UnboundName(name) if name == "Ty"),
            "expected UnboundName('Ty'), got {e}",
        ),
        Ok(v) => panic!("expected UnboundName error, got value {:?}", v.ktype()),
    }
}

/// Stage 1.6: `LET Foo = <non-type>` — Type-class LHS with a non-type RHS. The
/// bind-time check fires before the value reaches storage, producing a structured
/// `TypeClassBindingExpectsType` rather than the downstream `UnboundName` /
/// `ShapeError` the old "bind silently" path eventually surfaced. Covers Number
/// and Str independently — the blocklist's `matches!` arm carries one variant per
/// primitive, so removing any single variant must surface here.
#[test]
fn let_type_class_with_non_type_value_errors() {
    use crate::machine::RuntimeArena;
    use crate::machine::KErrorKind;
    use crate::parse::parse;
    // Post-collapse: `TypeClassBindingExpectsType.got` is the pre-rendered type name
    // (e.g. `"Number"`) rather than a `KType` value — keeps `KError` lifetime-free.
    for (src, expected) in [("LET Foo = 1", "Number"), ("LET Foo = \"hello\"", "Str")] {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let exprs = parse(src).unwrap();
        let id = sched.add_dispatch(exprs.into_iter().next().unwrap(), scope);
        sched.execute().expect("execute does not surface per-slot errors");
        match sched.read_result(id) {
            Err(e) => assert!(
                matches!(&e.kind, KErrorKind::TypeClassBindingExpectsType { name, got }
                    if name == "Foo" && got == expected),
                "expected TypeClassBindingExpectsType for {src:?}, got {e}",
            ),
            Ok(v) => panic!("expected bind-time error for {src:?}, got {:?}", v.ktype()),
        }
    }
}

/// Stage 1.7: `LET Foo = Number` — Type-class LHS with a type RHS. Storage now
/// lives in `bindings.types` (via `register_type`), reachable through
/// `Scope::resolve_type`. Regression guard that the blocklist doesn't reject the
/// good case and that the storage flip lands on the right map.
#[test]
fn let_type_class_with_type_value_still_binds() {
    use crate::machine::RuntimeArena;
    use crate::machine::model::KType;
    use crate::parse::parse;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let exprs = parse("LET Foo = Number").unwrap();
    let mut ids = Vec::new();
    for e in exprs {
        ids.push(sched.add_dispatch(e, scope));
    }
    sched.execute().expect("execute does not surface per-slot errors");
    let res = sched.read_result(ids[0]);
    assert!(res.is_ok(), "expected bind to succeed, got {:?}", res.err());
    let kt = scope
        .resolve_type("Foo")
        .expect("expected type binding 'Foo' in bindings.types");
    assert_eq!(*kt, KType::Number, "expected Number, got {:?}", kt);
}

/// Stage 1.6: `LET foo = 1` (lowercase, Identifier overload) is untouched by
/// the new check — it doesn't go through the `KTypeValue(_)` arm at all.
#[test]
fn let_identifier_lhs_with_non_type_still_binds() {
    use crate::machine::RuntimeArena;
    use crate::parse::parse;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let exprs = parse("LET foo = 1").unwrap();
    let mut ids = Vec::new();
    for e in exprs {
        ids.push(sched.add_dispatch(e, scope));
    }
    sched.execute().expect("execute does not surface per-slot errors");
    let res = sched.read_result(ids[0]);
    assert!(res.is_ok(), "expected bind to succeed, got {:?}", res.err());
    let data = scope.bindings().data();
    let (entry, _) = data.get("foo").expect("expected binding 'foo'");
    assert!(
        matches!(entry, KObject::Number(n) if *n == 1.0),
        "expected Number(1.0), got {:?}",
        entry.ktype(),
    );
}

/// Stage 1.6: `LET List<Number> = 1` — parameterized binder name is rejected by
/// the structural-shape check, which fires before the primitive blocklist.
/// Regression guard for ordering.
#[test]
fn let_parameterized_type_lhs_still_shape_errors() {
    use crate::machine::RuntimeArena;
    use crate::machine::KErrorKind;
    use crate::parse::parse;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let exprs = parse("LET :(List Number) = 1").unwrap();
    let mut ids = Vec::new();
    for e in exprs {
        ids.push(sched.add_dispatch(e, scope));
    }
    sched.execute().expect("execute does not surface per-slot errors");
    let res = sched.read_result(ids[0]);
    match res {
        Err(e) => assert!(
            matches!(&e.kind, KErrorKind::ShapeError(_)),
            "expected ShapeError, got {e}",
        ),
        Ok(v) => panic!("expected shape error, got value {:?}", v.ktype()),
    }
}

/// Stage 3.1 dual-write: `LET IntOrdA = (IntOrd :| OrderedSig)` writes the alias
/// into `bindings.types` (via `register_nominal`) AND `bindings.data` at the same
/// scope. The identity preserves the ORIGINAL module's `(scope_id, path)` rather
/// than minting a fresh nominal — aliasing is type-equivalent.
#[test]
fn let_type_class_with_module_carrier_dual_writes() {
    use crate::machine::RuntimeArena;
    use crate::machine::model::KType;
    use crate::builtins::test_support::run;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    run(
        scope,
        "MODULE IntOrd = (LET compare = 0)\n\
         SIG OrderedSig = (VAL compare :Number)\n\
         LET IntOrdA = (IntOrd :| OrderedSig)",
    );
    let types = scope.bindings().types();
    let (kt, _) = types
        .get("IntOrdA")
        .expect("IntOrdA should be in bindings.types");
    // Post-collapse: MODULE aliases dual-write `KType::Module { .. }` rather than
    // the old `UserType { kind: Module, .. }` indirection.
    assert!(matches!(**kt, KType::Module { .. }));
    drop(types);
    let data = scope.bindings().data();
    let (obj, _) = data
        .get("IntOrdA")
        .expect("IntOrdA should be in bindings.data");
    assert!(matches!(obj, KObject::KTypeValue(KType::Module { module: _, frame: _ })));
}

/// Stage 3.1 aliasing-preserves-identity: `LET Pt = Point` writes a `types[Pt]`
/// entry that equals `types[Point]` field-wise — `Pt` and `Point` lower to the
/// same `UserType` (same kind, scope_id, name="Point"). The alias binder name
/// `Pt` is for value-side lookup only; the type identity stays Point's. Token
/// classification requires the binder to carry at least one lowercase letter
/// to read as a type-class name.
#[test]
fn let_aliases_struct_preserves_type_identity() {
    use crate::machine::RuntimeArena;
    use crate::machine::model::KType;
    use crate::builtins::test_support::run;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    run(
        scope,
        "STRUCT Point = (x :Number, y :Number)\n\
         LET Pt = Point",
    );
    let types = scope.bindings().types();
    let (pt, _): (&KType, _) = types
        .get("Pt")
        .copied()
        .expect("Pt should be in bindings.types after alias");
    let (point, _): (&KType, _) = types
        .get("Point")
        .copied()
        .expect("Point should be in bindings.types");
    assert_eq!(*pt, *point, "alias must preserve type identity field-wise");
}

/// A lowercase-name `LET` inside a SIG body must surface a focused `ShapeError`
/// directing the user to `VAL`. The check fires only for the value-route
/// (neither Type-class LET nor a nominal-identity carrier alias); `LET Type =
/// Number` and `LET MyMod = (Some :| Sig)` keep working inside SIG bodies.
#[test]
fn let_lowercase_in_sig_body_rejected_with_val_diagnostic() {
    use crate::builtins::test_support::{parse_one, run_one_err, run_root_silent};
    use crate::machine::RuntimeArena;
    use crate::machine::KErrorKind;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    // Outer parse-and-execute: the SIG body errors via `(LET compare = 0)`. The
    // inner body's error propagates through the SIG outer Combine; the SIG node
    // itself does not bind.
    let _err = run_one_err(scope, parse_one("SIG Bad = (LET compare = 0)"));
    // The diagnostic for the lowercase-LET-in-SIG rejection lives on a child
    // node; the outer SIG node's error is a combine-propagated shape error. The
    // assertion below pins the SIG itself didn't bind — the migration-loud
    // observable. The diagnostic text is best-effort discoverable via a debug
    // run; the integration smoke is what blocks regressions.
    assert!(
        scope.bindings().data().get("Bad").is_none(),
        "SIG with lowercase-LET in body must not bind",
    );
    // Verify the diagnostic shape by running the LET directly against a
    // synthetic SIG-classified scope. The strict-reject check fires at body time
    // when the nearest non-`Anonymous` enclosing scope is `ScopeKind::Sig`.
    use crate::machine::Scope;
    let sig_scope = arena.alloc_scope(Scope::child_under_sig(
        scope,
        "SyntheticForTest".to_string(),
    ));
    let err = run_one_err(sig_scope, parse_one("LET compare = 0"));
    match &err.kind {
        KErrorKind::ShapeError(msg) => {
            assert!(
                msg.contains("VAL") && msg.contains("compare"),
                "expected diagnostic mentioning VAL and slot name, got: {msg}",
            );
        }
        _ => panic!("expected ShapeError, got something else"),
    }
}

/// Stage-5 allowlist regression — plain FN bound to a Type-class name now
/// errors at the LET site. Pre-Stage-5 the denylist accepted this case (no
/// primitive / container match) and silently landed the function in `data`;
/// the allowlist's three-arm test rejects it as `TypeClassBindingExpectsType`
/// because a plain `KFunction` carries neither `KTypeValue`, nominal identity,
/// nor the `is_functor` flag.
#[test]
fn let_type_class_with_plain_function_rejects() {
    use crate::builtins::test_support::{parse_one, run_one_err, run_root_silent};
    use crate::machine::KErrorKind;
    use crate::machine::RuntimeArena;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let err = run_one_err(
        scope,
        parse_one("LET Plain = (FN (PP x :Number) -> Number = (x))"),
    );
    match &err.kind {
        KErrorKind::TypeClassBindingExpectsType { name, .. } => {
            assert_eq!(name, "Plain", "binder name should surface in diagnostic");
        }
        _ => panic!("expected TypeClassBindingExpectsType, got {err}"),
    }
}

/// Stage-5 allowlist regression — FUNCTOR-flagged KFunction admits as a
/// Type-class LET RHS. The `is_functor: true` flag flips the third arm of
/// `is_admissible_type_class_rhs`; the binding lands in `bindings.data` via
/// the fallthrough `bind_value` (FUNCTOR isn't a nominal-identity carrier).
#[test]
fn let_type_class_with_functor_admits() {
    use crate::builtins::test_support::{run, run_root_silent};
    use crate::machine::RuntimeArena;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         LET MyF = (FUNCTOR (MAKESET Er :OrderedSig) -> Module = (MODULE Result = (LET inner = 1)))",
    );
    let obj = scope
        .lookup("MyF")
        .expect("MyF must be value-bound — allowlist admits the functor");
    assert!(
        matches!(obj, KObject::KFunction(f, _) if f.is_functor),
        "MyF should resolve to a FUNCTOR-flagged KFunction, got {:?}",
        obj.ktype(),
    );
}

/// SIG-body `LET <Type-class> = ...` keeps working post-VAL — the strict reject
/// only fires for the value-route. `LET Type = Number` lands on `register_type`,
/// not `bind_value`, so the SIG-body gate doesn't fire.
#[test]
fn let_type_class_in_sig_body_still_works() {
    use crate::builtins::test_support::{run, run_root_silent};
    use crate::machine::RuntimeArena;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "SIG WithType = ((LET Type = Number) (VAL zero :Number))");
    let s = match scope.bindings().data().get("WithType").map(|(o, _)| *o) {
        Some(KObject::KTypeValue(KType::Signature(s))) => *s,
        other => panic!("WithType should be a signature, got {:?}", other.map(|o| o.ktype())),
    };
    let types = s.decl_scope().bindings().types();
    assert!(
        types.contains_key("Type"),
        "Type binding should survive in SIG types map after Type-class LET",
    );
}

/// LET partition guard (design/typing/elaboration.md § Binding home and the
/// dual-map): `LET <name> = <m>` where `name` is value-classified (lowercase-
/// leading) and the RHS evaluates to a module value must reject at the LET
/// site. Module / signature carriers belong on Type-classified identifiers
/// only; this test pins the diagnostic so the partition rule has a regression
/// site.
#[test]
fn let_value_class_lhs_with_module_rhs_rejects() {
    use crate::builtins::test_support::{parse_one, run, run_one_err, run_root_silent};
    use crate::machine::KErrorKind;
    use crate::machine::RuntimeArena;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    // Set up a module value `IntOrd`. The lowercase rebind `int_ord` is the
    // partition violation — lowercase binder, module RHS.
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         MODULE IntOrd = ((LET compare = 7))",
    );
    let err = run_one_err(scope, parse_one("LET int_ord = (IntOrd :! OrderedSig)"));
    match &err.kind {
        KErrorKind::ShapeError(msg) => {
            assert!(
                msg.contains("int_ord") && msg.contains("module"),
                "expected diagnostic naming the binder and 'module', got: {msg}",
            );
            assert!(
                msg.contains("Type-classified"),
                "expected diagnostic to redirect to Type-classified identifier, got: {msg}",
            );
        }
        _ => panic!("expected ShapeError, got {err}"),
    }
}

/// Companion to `let_value_class_lhs_with_module_rhs_rejects`: signature carrier
/// on the RHS surface fires the same partition rejection. Pinned independently
/// because the predicate matches `KType::Module` and `KType::Signature` separately.
#[test]
fn let_value_class_lhs_with_signature_rhs_rejects() {
    use crate::builtins::test_support::{parse_one, run, run_one_err, run_root_silent};
    use crate::machine::KErrorKind;
    use crate::machine::RuntimeArena;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "SIG OrderedSig = (VAL compare :Number)");
    // `OrderedSig` is a Type-classified token; resolving it through `value_lookup`
    // returns the signature carrier. The lowercase binder + signature RHS hits
    // the partition guard.
    let err = run_one_err(scope, parse_one("LET sig_alias = OrderedSig"));
    match &err.kind {
        KErrorKind::ShapeError(msg) => {
            assert!(
                msg.contains("sig_alias") && msg.contains("signature"),
                "expected diagnostic naming the binder and 'signature', got: {msg}",
            );
        }
        _ => panic!("expected ShapeError, got {err}"),
    }
}
