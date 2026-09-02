use crate::builtins::test_support::lookup_type;
use crate::builtins::test_support::{TestRun, type_name, type_token, value_name};
use crate::machine::model::{KObject, KType};

#[test]
fn binder_name_extracts_let_name() {
    use crate::builtins::test_support::parse_one;
    use crate::machine::program_storage;
    let program = program_storage();
    let labels = crate::machine::model::LabelInterner::new();
    let expr = parse_one(&program, &labels, "LET hello = 1");
    let name = crate::machine::model::binder::identifier_part_binder_name(&expr)
        .expect("`LET hello = 1` names a binder");
    assert_eq!(name.bind_kind(), crate::machine::model::BindKind::Value);
    assert_eq!(labels.render(name.symbol()), "hello");
}

/// End-to-end claim-then-finalize: statement submission claims the name's slot from the
/// cached binder plan before the body runs; the value write commits and retires it at apply.
#[test]
fn binder_name_install_then_body_finalize_clears_placeholder() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::{program_storage, run_root_storage};
    use crate::parse::parse;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let exprs = parse(
        program.brand(),
        &test_run.registries().labels,
        "LET hello = 1",
    )
    .unwrap();
    for e in exprs {
        test_run.dispatch_in_scope(
            crate::machine::model::WorkingExpression::from_ast(scope.brand(), e),
            scope,
        );
    }
    test_run.runtime.execute().unwrap();
    assert!(
        scope
            .bindings()
            .pending_value(value_name("hello", test_run.registries()))
            .is_none()
    );
    assert!(matches!(scope.lookup("hello"), Some(KObject::Number(n)) if *n == 1.0));
}

/// `LET T = T` is a trivially cyclic alias. Under index-gated resolution the
/// strict `b.idx < c` predicate makes the in-progress binding invisible so the
/// consumer surfaces `UnboundName` rather than self-parking on a cycle.
#[test]
fn let_t_cycle_errors() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::KErrorKind;
    use crate::machine::program_storage;
    use crate::machine::run_root_storage;
    use crate::parse::parse;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let types = test_run.registry_handle();
    let exprs = parse(
        program.brand(),
        &test_run.registries().labels,
        "LET Ty = Ty",
    )
    .unwrap();
    let runtime = &mut test_run.runtime;
    let exprs = exprs
        .into_iter()
        .map(|e| crate::machine::model::WorkingExpression::from_ast(scope.brand(), e))
        .collect();
    let ids = runtime.enter_block(scope.id, exprs, scope);
    let edge = runtime.install_edge_for_test(ids[0], scope);
    runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let res = runtime.read_edge_result_with(edge, |v| format!("{:?}", v.ktype(&types)));
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
    use crate::machine::KErrorKind;
    use crate::machine::program_storage;
    use crate::machine::run_root_storage;
    use crate::parse::parse;
    for (src, expected) in [("LET Foo = 1", "Number"), ("LET Foo = \"hello\"", "Str")] {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let scope = test_run.scope;
        let exprs = parse(program.brand(), &test_run.registries().labels, src).unwrap();
        let id = test_run.dispatch_in_scope(
            crate::machine::model::WorkingExpression::from_ast(
                scope.brand(),
                exprs.into_iter().next().unwrap(),
            ),
            scope,
        );
        let edge = test_run.runtime.install_edge_for_test(id, scope);
        test_run
            .runtime
            .execute()
            .expect("execute does not surface per-slot errors");
        let types = test_run.registry_handle();
        match test_run
            .runtime
            .read_edge_result_with(edge, |v| format!("{:?}", v.ktype(&types)))
        {
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
    use crate::machine::model::KType;
    use crate::machine::program_storage;
    use crate::machine::run_root_storage;
    use crate::parse::parse;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let exprs = parse(
        program.brand(),
        &test_run.registries().labels,
        "LET Foo = Number",
    )
    .unwrap();
    let mut ids = Vec::new();
    for e in exprs {
        ids.push(test_run.dispatch_in_scope(
            crate::machine::model::WorkingExpression::from_ast(scope.brand(), e),
            scope,
        ));
    }
    let edges: Vec<_> = ids
        .iter()
        .map(|id| test_run.runtime.install_edge_for_test(*id, scope))
        .collect();
    test_run
        .runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let res = test_run.runtime.edge_result_error(edges[0]);
    assert!(res.is_ok(), "expected bind to succeed, got {:?}", res.err());
    let kt = lookup_type(scope, "Foo").expect("expected type binding 'Foo' in bindings.types");
    assert_eq!(kt, KType::NUMBER, "expected Number, got {:?}", kt);
}

/// `LET foo = 1` (lowercase, Identifier overload) doesn't go through the
/// `Held::Type` arm and so isn't subject to the type-class allowlist.
#[test]
fn let_identifier_lhs_with_non_type_still_binds() {
    use crate::machine::program_storage;
    use crate::machine::run_root_storage;
    use crate::parse::parse;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let exprs = parse(
        program.brand(),
        &test_run.registries().labels,
        "LET foo = 1",
    )
    .unwrap();
    let mut ids = Vec::new();
    for e in exprs {
        ids.push(test_run.dispatch_in_scope(
            crate::machine::model::WorkingExpression::from_ast(scope.brand(), e),
            scope,
        ));
    }
    let edges: Vec<_> = ids
        .iter()
        .map(|id| test_run.runtime.install_edge_for_test(*id, scope))
        .collect();
    test_run
        .runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let res = test_run.runtime.edge_result_error(edges[0]);
    assert!(res.is_ok(), "expected bind to succeed, got {:?}", res.err());
    let entry = scope.lookup("foo").expect("expected binding 'foo'");
    assert!(
        matches!(entry, KObject::Number(n) if *n == 1.0),
        "expected Number(1.0), got {:?}",
        entry.ktype(),
    );
}

/// A binder position captures a name token, so a `:(…)` type expression there matches no LET
/// overload at all. The refusal is a dispatch non-match, not a bind-time check over a lowered
/// handle — the binder never resolves, so there is no rendering to report.
#[test]
fn let_parameterized_type_lhs_matches_no_overload() {
    use crate::machine::KErrorKind;
    use crate::machine::program_storage;
    use crate::machine::run_root_storage;
    use crate::parse::parse;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let exprs = parse(
        program.brand(),
        &test_run.registries().labels,
        "LET :(LIST OF Number) = 1",
    )
    .unwrap();
    let mut ids = Vec::new();
    for e in exprs {
        ids.push(test_run.dispatch_in_scope(
            crate::machine::model::WorkingExpression::from_ast(scope.brand(), e),
            scope,
        ));
    }
    let edges: Vec<_> = ids
        .iter()
        .map(|id| test_run.runtime.install_edge_for_test(*id, scope))
        .collect();
    test_run
        .runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let types = test_run.registry_handle();
    let res = test_run
        .runtime
        .read_edge_result_with(edges[0], |v| format!("{:?}", v.ktype(&types)));
    match res {
        Err(e) => {
            assert!(
                matches!(&e.kind, KErrorKind::DispatchFailed { .. }),
                "expected DispatchFailed, got {e}",
            );
            assert!(
                !e.to_string().contains("LIST OF"),
                "the binder never lowers, so no diagnostic renders one: {e}",
            );
        }
        Ok(ktype) => panic!("expected a dispatch failure, got value {ktype}"),
    }
}

/// `LET Pt = Point` writes a `types[Pt]` entry equal to `types[Point]` —
/// aliasing preserves the original `UserType` identity rather than minting a
/// fresh one from the alias name.
#[test]
fn let_aliases_struct_preserves_type_identity() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::model::KType;
    use crate::machine::program_storage;
    use crate::machine::run_root_storage;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "NEWTYPE Point = :{x :Number, y :Number}\n\
         LET Pt = Point",
    );
    let types = scope.bindings().types();
    let pt: KType = types
        .get(&type_name("Pt", test_run.registries()))
        .map(|(kt, _)| *kt)
        .expect("Pt should be bound in bindings.types after alias");
    let point: KType = types
        .get(&type_name("Point", test_run.registries()))
        .map(|(kt, _)| *kt)
        .expect("Point should be bound in bindings.types");
    assert_eq!(pt, point, "alias must preserve type identity field-wise");
}

/// A lowercase-name `LET` inside a SIG body surfaces a `ShapeError` directing
/// the user to `VAL`. The check fires only for the value-route, so
/// `LET Carrier = Number` and module-alias forms keep working inside SIG bodies.
#[test]
fn let_lowercase_in_sig_body_rejected_with_val_diagnostic() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::KErrorKind;
    use crate::machine::program_storage;
    use crate::machine::run_root_storage;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let _err = test_run.run_one_err(test_run.parse_one("SIG Bad = (LET compare = 0)"));
    // `Bad` is a Type token and `data` keys by `ValueSymbol`, so no key spells it; what the
    // rejection means for the value table is that it stayed empty.
    assert!(
        scope.bindings().data().is_empty(),
        "SIG with lowercase-LET in body must not bind",
    );
    // Verify the diagnostic shape directly against a synthetic SIG scope — the
    // outer SIG's error is a combine-propagated shape error and doesn't carry
    // the inner diagnostic text.

    let sig_scope = scope.alloc_child_under_sig(type_token("SyntheticForTest"));
    let err = test_run.run_one_err_in(sig_scope, test_run.parse_one("LET compare = 0"));
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

/// An FN bound to a Type-class name errors at the LET site: a function is a value,
/// and a Type-class binder admits only a type carrier, so `bindings.types` never
/// holds a callable.
#[test]
fn let_type_class_with_plain_function_rejects() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::KErrorKind;
    use crate::machine::program_storage;
    use crate::machine::run_root_storage;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let err =
        test_run.run_one_err(test_run.parse_one("LET Plain = FN (PP x :Number) -> Number = (x)"));
    match &err.kind {
        KErrorKind::ShapeError(message) => assert!(
            message.contains("Plain") && message.contains("plain"),
            "the diagnostic should name the binder and suggest the value-classified \
             respelling, got {message}",
        ),
        _ => panic!("expected the value-classified-respelling diagnostic, got {err}"),
    }
}

/// SIG-body `LET Tag = Number` is a *manifest* type member: the RHS type is bound
/// verbatim (concrete `Number`), not re-tagged to an `AbstractType` identity. The
/// SIG-body reject fires only for the value-route, so a Type-class `LET` routes
/// through `register_type` and binds the resolved type unconditionally. `=` inside a
/// SIG body means manifest; abstract members use `TYPE` (which has no RHS).
#[test]
fn let_type_class_in_sig_body_binds_manifest() {
    use crate::builtins::test_support::{TestRun, type_name};
    use crate::machine::program_storage;
    use crate::machine::run_root_storage;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("SIG WithTag = ((LET Tag = Number) (VAL zero :Number))");
    use crate::machine::model::TypeNode;
    let handle = lookup_type(scope, "WithTag").expect("WithTag should bind a type");
    let schema = match test_run.types().node(handle) {
        TypeNode::Signature { schema, .. } => schema,
        _ => panic!("WithTag should be a Signature KType, got {:?}", handle),
    };
    let bound = schema
        .manifest_members
        .get(&type_name("Tag", test_run.registries()))
        .copied()
        .expect(
            "Tag binding should survive in the SIG schema's manifest members after manifest LET",
        );
    assert_eq!(
        bound,
        KType::NUMBER,
        "SIG-local `LET Tag = Number` binds the concrete `Number`, not an AbstractType, got {:?}",
        bound,
    );
}

/// A Type-classified SIG alias `LET Po = Ordered` writes the *same* unified
/// `KType::Signature` identity into `bindings.types[Po]` as `Ordered` carries,
/// so `:Po` and `:Ordered` are dispatch-identical. Pins the merged-variant
/// LET path: the generic `Held::Type(kt)` arm shared with struct/union/module
/// aliases, with no separate signature-only install branch.
#[test]
fn let_type_class_signature_alias_preserves_identity() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::program_storage;
    use crate::machine::run_root_storage;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("SIG Ordered = (VAL compare :Number)\nLET Po = Ordered");
    use crate::machine::model::TypeNode;
    let original = lookup_type(scope, "Ordered").expect("Ordered type binding");
    let aliased = lookup_type(scope, "Po").expect("Po type binding");
    assert!(
        matches!(test_run.types().node(aliased), TypeNode::Signature { .. }),
        "Po must alias to a Signature KType, got {:?}",
        aliased,
    );
    assert_eq!(
        original, aliased,
        "alias `Po` must carry the same signature identity as `Ordered`",
    );
}

/// Partition guard regression site: a value-classified binder name with a
/// module RHS rejects at the LET site. See design/typing/elaboration.md
/// § Binding-map partition. A module is a value, so a *Type*-classified binder is the wrong
/// spelling for one — whatever RHS produced it. The diagnostic names the snake_case respelling.
#[test]
fn let_type_class_lhs_with_module_rhs_rejects() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::KErrorKind;
    use crate::machine::program_storage;
    use crate::machine::run_root_storage;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = ((LET compare = 7))",
    );
    let err = test_run.run_one_err(test_run.parse_one("LET IntOrdView = (int_ord :! Ordered)"));
    match &err.kind {
        KErrorKind::ShapeError(msg) => {
            assert!(
                msg.contains("IntOrdView") && msg.contains("module"),
                "expected diagnostic naming the binder and 'module', got: {msg}",
            );
            assert!(
                msg.contains("int_ord_view"),
                "expected diagnostic to suggest the snake_case respelling, got: {msg}",
            );
        }
        _ => panic!("expected ShapeError, got {err}"),
    }
}

/// Companion to `let_type_class_lhs_with_module_rhs_rejects` — pinned
/// independently because the cross-kind guard classifies a module value and a
/// `KType::Signature` on separate arms.
#[test]
fn let_value_class_lhs_with_signature_rhs_rejects() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::KErrorKind;
    use crate::machine::program_storage;
    use crate::machine::run_root_storage;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("SIG Ordered = (VAL compare :Number)");
    let err = test_run.run_one_err(test_run.parse_one("LET sig_alias = Ordered"));
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

/// A module is a value, so a value-classified LET of a module RHS binds it into `data` like any
/// other object value. The cross-kind exclusion means exactly one map holds the name.
#[test]
fn let_value_class_with_module_rhs_binds_value_side() {
    use crate::builtins::test_support::{TestRun, binds_module};
    use crate::machine::program_storage;
    use crate::machine::run_root_storage;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "SIG Ordered = (VAL compare :Number)\n\
         MODULE int_ord = (LET compare = 7)\n\
         LET view = (int_ord :! Ordered)",
    );
    assert!(
        binds_module(scope, "view"),
        "a module RHS under a value-classified name binds the module value",
    );
    assert!(
        lookup_type(scope, "view").is_none(),
        "the name is committed to `data` xor `types` — nothing lands in `types`",
    );
}

/// The binder never resolves, so a name that happens to spell a builtin type differs from a fresh
/// one in exactly one way: it is already bound. `Str` (a leaf), `List` and `Dict` (names that
/// used to lower to parameterized nodes and so answered no bare name at all) all report the same
/// `Rebind`, and no diagnostic renders the lowered type.
#[test]
fn let_builtin_type_names_are_uniformly_already_bound() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::KErrorKind;
    use crate::machine::program_storage;
    use crate::machine::run_root_storage;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;

    test_run.run("LET Foo = Number");
    assert!(
        lookup_type(scope, "Foo").is_some(),
        "a fresh Type-classed binder binds",
    );

    for name in ["Str", "List", "Dict"] {
        let source = format!("LET {name} = Number");
        let err = test_run.run_one_err(test_run.parse_one(&source));
        assert!(
            matches!(&err.kind, KErrorKind::Rebind { name: bound } if bound == name),
            "expected `LET {name} = Number` to report Rebind, got {err}",
        );
        let rendered = err.to_string();
        assert!(
            !rendered.contains("LIST OF") && !rendered.contains("MAP "),
            "no spelling reports a rendered lowered type: {rendered}",
        );
    }
}

/// The same uniformity across the sibling declarators: their `name` slots are `TypeNameToken`, so
/// a builtin-spelled name is the ordinary already-bound question there too. `MODULE` keeps its
/// respelling diagnostic and names the token as written.
#[test]
fn sibling_declarators_report_the_same_uniform_refusal() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::KErrorKind;
    use crate::machine::program_storage;
    use crate::machine::run_root_storage;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);

    let err = test_run.run_one_err(test_run.parse_one("NEWTYPE List = Number"));
    assert!(
        matches!(&err.kind, KErrorKind::Rebind { name } if name == "List"),
        "expected NEWTYPE Rebind, got {err}",
    );

    let err = test_run.run_one_err(test_run.parse_one("UNION Dict = (Some :Number None :Null)"));
    assert!(
        matches!(&err.kind, KErrorKind::Rebind { name } if name == "Dict"),
        "expected UNION Rebind, got {err}",
    );

    let err = test_run.run_one_err(test_run.parse_one("MODULE List = ((LET x = 1))"));
    match &err.kind {
        KErrorKind::ShapeError(message) => {
            assert!(
                message.contains("List") && !message.contains("LIST OF"),
                "the respelling diagnostic names the token as written: {message}",
            );
        }
        _ => panic!("expected the MODULE respelling ShapeError, got {err}"),
    }
}

/// The partition guard's respelling suggestion has to be a token the writer can actually type.
/// `point` capitalizes to a Type name and is offered; `t` capitalizes to `T`, which is one
/// uppercase letter with no lowercase and so classifies as neither keyword nor type name — the
/// diagnostic states the rule and offers nothing rather than naming a parse error.
#[test]
fn the_type_class_respelling_is_offered_only_when_it_classifies() {
    use crate::builtins::test_support::TestRun;
    use crate::machine::KErrorKind;
    use crate::machine::program_storage;
    use crate::machine::run_root_storage;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);

    let named = test_run.run_one_err(test_run.parse_one("LET point = Number"));
    assert!(
        matches!(&named.kind, KErrorKind::ShapeError(msg) if msg.contains("e.g. `Point`")),
        "expected the capitalized respelling, got {named}",
    );

    let unspellable = test_run.run_one_err(test_run.parse_one("LET t = Number"));
    assert!(
        matches!(&unspellable.kind, KErrorKind::ShapeError(msg)
            if msg.contains("at least one lowercase letter") && !msg.contains("e.g.")),
        "expected the rule with no suggestion, got {unspellable}",
    );
}
