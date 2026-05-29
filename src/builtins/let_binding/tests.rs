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

#[test]
fn binder_name_extracts_let_name() {
    use crate::parse::parse;
    let mut exprs = parse("LET hello = 1").expect("parse should succeed");
    let expr = exprs.remove(0);
    let name = super::binder_name(&expr);
    assert_eq!(name.as_deref(), Some("hello"));
}

/// End-to-end install-then-clear: the binder_name hook installs a placeholder
/// before the body runs; `bind_value` clears it on finalize.
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
    assert!(scope.bindings().placeholders().get("hello").is_none());
    assert!(matches!(scope.lookup("hello"), Some(KObject::Number(n)) if *n == 1.0));
}

/// `LET T = T` is a trivially cyclic alias. Under index-gated resolution the
/// strict `b.idx < c` predicate makes the in-progress binding invisible so the
/// consumer surfaces `UnboundName` rather than self-parking on a cycle.
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

/// `LET Foo = <non-type>` — Type-class LHS with a non-type RHS surfaces a
/// structured `TypeClassBindingExpectsType`. Covers Number and Str independently
/// so removing either primitive variant from the allowlist regresses here.
#[test]
fn let_type_class_with_non_type_value_errors() {
    use crate::machine::RuntimeArena;
    use crate::machine::KErrorKind;
    use crate::parse::parse;
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

/// `LET Foo = Number` — Type-class LHS with a type RHS lands in `bindings.types`
/// via `register_type`, reachable through `Scope::resolve_type`.
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

/// `LET foo = 1` (lowercase, Identifier overload) doesn't go through the
/// `KTypeValue(_)` arm and so isn't subject to the type-class allowlist.
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

/// Parameterized binder names hit the structural shape check, which fires
/// before the type-class allowlist — regression guard for ordering.
#[test]
fn let_parameterized_type_lhs_still_shape_errors() {
    use crate::machine::RuntimeArena;
    use crate::machine::KErrorKind;
    use crate::parse::parse;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let mut sched = Scheduler::new();
    let exprs = parse("LET :(LIST OF Number) = 1").unwrap();
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

/// `LET Pt = Point` writes a `types[Pt]` entry equal to `types[Point]` —
/// aliasing preserves the original `UserType` identity rather than minting a
/// fresh one from the alias name.
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

/// A lowercase-name `LET` inside a SIG body surfaces a `ShapeError` directing
/// the user to `VAL`. The check fires only for the value-route, so
/// `LET Type = Number` and module-alias forms keep working inside SIG bodies.
#[test]
fn let_lowercase_in_sig_body_rejected_with_val_diagnostic() {
    use crate::builtins::test_support::{parse_one, run_one_err, run_root_silent};
    use crate::machine::RuntimeArena;
    use crate::machine::KErrorKind;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let _err = run_one_err(scope, parse_one("SIG Bad = (LET compare = 0)"));
    assert!(
        scope.bindings().data().get("Bad").is_none(),
        "SIG with lowercase-LET in body must not bind",
    );
    // Verify the diagnostic shape directly against a synthetic SIG scope — the
    // outer SIG's error is a combine-propagated shape error and doesn't carry
    // the inner diagnostic text.
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

/// Plain FN bound to a Type-class name errors at the LET site — a plain
/// `KFunction` carries neither `KTypeValue`, nominal identity, nor the
/// `is_functor` flag, so the allowlist rejects it.
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

/// FUNCTOR-flagged KFunction admits as a Type-class LET RHS via the third
/// allowlist arm. The binding lands in `bindings.data` (FUNCTOR isn't a
/// nominal-identity carrier).
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

/// SIG-body `LET <Type-class> = ...` keeps working — the SIG-body reject only
/// fires for the value-route, and `LET Type = Number` routes through
/// `register_type`.
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

/// Partition guard regression site: a value-classified binder name with a
/// module RHS rejects at the LET site. See design/typing/elaboration.md
/// § Binding-map partition.
#[test]
fn let_value_class_lhs_with_module_rhs_rejects() {
    use crate::builtins::test_support::{parse_one, run, run_one_err, run_root_silent};
    use crate::machine::KErrorKind;
    use crate::machine::RuntimeArena;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
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

/// Companion to `let_value_class_lhs_with_module_rhs_rejects` — pinned
/// independently because the predicate matches `KType::Module` and
/// `KType::Signature` on separate arms.
#[test]
fn let_value_class_lhs_with_signature_rhs_rejects() {
    use crate::builtins::test_support::{parse_one, run, run_one_err, run_root_silent};
    use crate::machine::KErrorKind;
    use crate::machine::RuntimeArena;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "SIG OrderedSig = (VAL compare :Number)");
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
