use crate::builtins::default_scope;
use crate::machine::execute::KoanRuntime;
use crate::machine::model::{KObject, KType};

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
    use crate::builtins::default_scope;
    use crate::machine::core::FrameStorage;
    use crate::machine::execute::KoanRuntime;
    use crate::parse::parse;
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    let exprs = parse("LET hello = 1").unwrap();
    for e in exprs {
        runtime.dispatch_in_scope(e, scope);
    }
    runtime.execute().unwrap();
    assert!(scope.bindings().placeholders().get("hello").is_none());
    assert!(matches!(scope.lookup("hello"), Some(KObject::Number(n)) if *n == 1.0));
}

/// `LET T = T` is a trivially cyclic alias. Under index-gated resolution the
/// strict `b.idx < c` predicate makes the in-progress binding invisible so the
/// consumer surfaces `UnboundName` rather than self-parking on a cycle.
#[test]
fn let_t_cycle_errors() {
    use crate::builtins::default_scope;
    use crate::machine::core::FrameStorage;
    use crate::machine::execute::KoanRuntime;
    use crate::machine::KErrorKind;
    use crate::parse::parse;
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    let exprs = parse("LET Ty = Ty").unwrap();
    let ids = runtime.enter_block(scope.id, exprs, scope);
    runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let res = runtime.read_result_with(ids[0], |v| format!("{:?}", v.ktype()));
    match res {
        // The bare-leaf RHS resolves through the memoized type-expr bridge, whose miss
        // surfaces the elaborator's `unknown type name` diagnostic naming `Ty`. The
        // index-gated invisibility of the in-progress binding is what turns the cycle into
        // a miss rather than a self-park.
        Err(e) => assert!(
            matches!(&e.kind, KErrorKind::UnboundName(msg) if msg.contains("Ty")),
            "expected UnboundName naming Ty, got {e}",
        ),
        Ok(ktype) => panic!("expected UnboundName error, got value {ktype}"),
    }
}

/// `LET Foo = <non-type>` — Type-class LHS with a non-type RHS surfaces a
/// structured `TypeClassBindingExpectsType`. Covers Number and Str independently
/// so removing either primitive variant from the allowlist regresses here.
#[test]
fn let_type_class_with_non_type_value_errors() {
    use crate::machine::core::FrameStorage;
    use crate::machine::KErrorKind;
    use crate::parse::parse;
    for (src, expected) in [("LET Foo = 1", "Number"), ("LET Foo = \"hello\"", "Str")] {
        let region = FrameStorage::run_root();
        let scope = default_scope(&region, Box::new(std::io::sink()));
        let mut runtime = KoanRuntime::new();
        let exprs = parse(src).unwrap();
        let id = runtime.dispatch_in_scope(exprs.into_iter().next().unwrap(), scope);
        runtime
            .execute()
            .expect("execute does not surface per-slot errors");
        match runtime.read_result_with(id, |v| format!("{:?}", v.ktype())) {
            Err(e) => assert!(
                matches!(&e.kind, KErrorKind::TypeClassBindingExpectsType { name, got }
                    if name == "Foo" && got == expected),
                "expected TypeClassBindingExpectsType for {src:?}, got {e}",
            ),
            Ok(ktype) => panic!("expected bind-time error for {src:?}, got {ktype}"),
        }
    }
}

/// `LET Foo = Number` — Type-class LHS with a type RHS lands in `bindings.types`
/// via `register_type`, reachable through `Scope::resolve_type`.
#[test]
fn let_type_class_with_type_value_still_binds() {
    use crate::machine::core::FrameStorage;
    use crate::machine::model::KType;
    use crate::parse::parse;
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    let exprs = parse("LET Foo = Number").unwrap();
    let mut ids = Vec::new();
    for e in exprs {
        ids.push(runtime.dispatch_in_scope(e, scope));
    }
    runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let res = runtime.result_error(ids[0]);
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
    use crate::machine::core::FrameStorage;
    use crate::parse::parse;
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    let exprs = parse("LET foo = 1").unwrap();
    let mut ids = Vec::new();
    for e in exprs {
        ids.push(runtime.dispatch_in_scope(e, scope));
    }
    runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let res = runtime.result_error(ids[0]);
    assert!(res.is_ok(), "expected bind to succeed, got {:?}", res.err());
    let data = scope.bindings().data();
    let (entry, _, _) = data.get("foo").expect("expected binding 'foo'");
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
    use crate::machine::core::FrameStorage;
    use crate::machine::KErrorKind;
    use crate::parse::parse;
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    let mut runtime = KoanRuntime::new();
    let exprs = parse("LET :(LIST OF Number) = 1").unwrap();
    let mut ids = Vec::new();
    for e in exprs {
        ids.push(runtime.dispatch_in_scope(e, scope));
    }
    runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let res = runtime.read_result_with(ids[0], |v| format!("{:?}", v.ktype()));
    match res {
        Err(e) => assert!(
            matches!(&e.kind, KErrorKind::ShapeError(_)),
            "expected ShapeError, got {e}",
        ),
        Ok(ktype) => panic!("expected shape error, got value {ktype}"),
    }
}

/// `LET Pt = Point` writes a `types[Pt]` entry equal to `types[Point]` —
/// aliasing preserves the original `UserType` identity rather than minting a
/// fresh one from the alias name.
#[test]
fn let_aliases_struct_preserves_type_identity() {
    use crate::builtins::test_support::run;
    use crate::machine::core::FrameStorage;
    use crate::machine::model::KType;
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(std::io::sink()));
    run(
        scope,
        "NEWTYPE Point = :{x :Number, y :Number}\n\
         LET Pt = Point",
    );
    let types = scope.bindings().types();
    let pt: &KType = types
        .get("Pt")
        .map(|(kt, _, _)| *kt)
        .expect("Pt should be in bindings.types after alias");
    let point: &KType = types
        .get("Point")
        .map(|(kt, _, _)| *kt)
        .expect("Point should be in bindings.types");
    assert_eq!(*pt, *point, "alias must preserve type identity field-wise");
}

/// A lowercase-name `LET` inside a SIG body surfaces a `ShapeError` directing
/// the user to `VAL`. The check fires only for the value-route, so
/// `LET Carrier = Number` and module-alias forms keep working inside SIG bodies.
#[test]
fn let_lowercase_in_sig_body_rejected_with_val_diagnostic() {
    use crate::builtins::test_support::{parse_one, run_one_err, run_root_silent};
    use crate::machine::core::FrameStorage;
    use crate::machine::KErrorKind;
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    let _err = run_one_err(scope, parse_one("SIG Bad = (LET compare = 0)"));
    assert!(
        scope.bindings().data().get("Bad").is_none(),
        "SIG with lowercase-LET in body must not bind",
    );
    // Verify the diagnostic shape directly against a synthetic SIG scope — the
    // outer SIG's error is a combine-propagated shape error and doesn't carry
    // the inner diagnostic text.
    use crate::machine::Scope;
    let sig_scope = region.brand().alloc_scope(Scope::child_under_sig(
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
    use crate::machine::core::FrameStorage;
    use crate::machine::KErrorKind;
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
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
/// allowlist arm and registers *type-side*: the binding lands in `bindings.types`
/// as a `KType::KFunctor { body: Some(f) }`, never in `bindings.data`. The carried
/// body is the callable a later `:(MyF {…})` / `MyF {…}` application invokes.
#[test]
fn let_type_class_with_functor_admits() {
    use crate::builtins::test_support::{run, run_root_silent};
    use crate::machine::core::FrameStorage;
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\n\
         LET MyF = (FUNCTOR (MAKESET Er :OrderedSig) -> Module = (MODULE Generated = (LET inner = 1)))",
    );
    assert!(
        scope.lookup("MyF").is_none(),
        "MyF must NOT be value-bound — a functor name registers type-side",
    );
    let kt = scope
        .resolve_type("MyF")
        .expect("MyF must be type-bound — the functor lands in bindings.types");
    assert!(
        matches!(kt, KType::KFunctor { body: Some(f), .. } if f.is_functor),
        "MyF should resolve type-side to a KFunctor carrying the callable body, got {:?}",
        kt,
    );
}

/// `LET f = (FUNCTOR …)` (lowercase / value-class name) is an error: a functor
/// lives in the type namespace only and must never land in `bindings.data`. The
/// value-route guard fires before `bind_value`, and the diagnostic redirects to a
/// Type-classified identifier. Companion to `let_type_class_with_functor_admits`,
/// which pins the legal uppercase form.
#[test]
fn let_value_class_with_functor_rejects() {
    use crate::builtins::test_support::{parse_one, run, run_one_err, run_root_silent};
    use crate::machine::core::FrameStorage;
    use crate::machine::KErrorKind;
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(scope, "SIG OrderedSig = (VAL compare :Number)");
    let err = run_one_err(
        scope,
        parse_one("LET f = (FUNCTOR (MAKESET Er :OrderedSig) -> Module = (MODULE Generated = (LET inner = 1)))"),
    );
    match &err.kind {
        KErrorKind::ShapeError(msg) => {
            assert!(
                msg.contains("functor") && msg.contains('f') && msg.contains("value-class"),
                "expected diagnostic naming the binder and 'value-class', got: {msg}",
            );
            assert!(
                msg.contains("Type-classified") && msg.contains('F'),
                "expected diagnostic to suggest a Type-classified rewrite, got: {msg}",
            );
        }
        _ => panic!("expected ShapeError, got {err}"),
    }
    assert!(
        scope.lookup("f").is_none(),
        "a rejected lowercase functor must not land in bindings.data",
    );
}

/// SIG-body `LET <Type-class> = ...` keeps working — the SIG-body reject only
/// fires for the value-route, and `LET Carrier = Number` routes through
/// `register_type`. Inside a SIG body the bound `KType` is the name-bearing
/// `AbstractType { source: Sig(decl_scope), name }` rather than the collapsed
/// underlying type, so a later `VAL :Carrier` records that the slot *names* the
/// abstract member.
#[test]
fn let_type_class_in_sig_body_still_works() {
    use crate::builtins::test_support::{run, run_root_silent};
    use crate::machine::core::FrameStorage;
    use crate::machine::model::types::AbstractSource;
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG WithType = ((LET Carrier = Number) (VAL zero :Number))",
    );
    let s = match scope.resolve_type("WithType") {
        Some(KType::Signature { sig, .. }) => *sig,
        other => panic!("WithType should be a Signature KType, got {:?}", other),
    };
    let decl_scope = s.decl_scope();
    let bound = decl_scope
        .resolve_type("Carrier")
        .expect("Carrier binding should survive in SIG types map after Type-class LET");
    match bound {
        KType::AbstractType {
            source: AbstractSource::Sig(id),
            name,
        } => {
            assert_eq!(name, "Carrier");
            assert_eq!(
                *id, decl_scope.id,
                "Sig source must key on the decl_scope id"
            );
        }
        other => panic!(
            "SIG-local `LET Carrier = Number` should bind a Sig-rooted AbstractType, got {:?}",
            other
        ),
    }
}

/// A Type-classified SIG alias `LET Po = OrderedSig` writes the *same* unified
/// `KType::Signature` identity into `bindings.types[Po]` as `OrderedSig` carries,
/// so `:Po` and `:OrderedSig` are dispatch-identical. Pins the merged-variant
/// LET path: the generic `KTypeValue(kt)` arm shared with struct/union/module
/// aliases, with no separate signature-only install branch.
#[test]
fn let_type_class_signature_alias_preserves_identity() {
    use crate::builtins::test_support::{run, run_root_silent};
    use crate::machine::core::FrameStorage;
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(
        scope,
        "SIG OrderedSig = (VAL compare :Number)\nLET Po = OrderedSig",
    );
    let original = scope
        .resolve_type("OrderedSig")
        .expect("OrderedSig type binding");
    let aliased = scope.resolve_type("Po").expect("Po type binding");
    assert!(
        matches!(aliased, KType::Signature { .. }),
        "Po must alias to a Signature KType, got {:?}",
        aliased,
    );
    assert_eq!(
        *original, *aliased,
        "alias `Po` must carry the same signature identity as `OrderedSig`",
    );
}

/// Partition guard regression site: a value-classified binder name with a
/// module RHS rejects at the LET site. See design/typing/elaboration.md
/// § Binding-map partition.
#[test]
fn let_value_class_lhs_with_module_rhs_rejects() {
    use crate::builtins::test_support::{parse_one, run, run_one_err, run_root_silent};
    use crate::machine::core::FrameStorage;
    use crate::machine::KErrorKind;
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
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
    use crate::machine::core::FrameStorage;
    use crate::machine::KErrorKind;
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
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
