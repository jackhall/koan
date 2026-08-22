use crate::builtins::test_support::{TestRun, lookup_module, parse_one, type_name, value_name};
use crate::machine::ScopeId;
use crate::machine::model::ExpressionPart;
use crate::machine::model::KObject;
use crate::machine::model::Record;
use crate::machine::model::{
    KKind, KType, RecursiveGroupWindow, RelativeSchema, TypeNode, constructor_param_names,
};
use crate::machine::{ProgramStorage, program_storage, run_root_storage};

/// Resolve a SIG-declared type member's stored `KType` out of the signature's schema —
/// abstract members (`TYPE`) and manifest members (`LET`) both live there, classified by
/// representation at SIG finish.
fn member_type(
    scope: &crate::machine::Scope<'_>,
    registries: &crate::machine::model::RunRegistries,
    sig_name: &str,
    member_name: &str,
) -> KType {
    let types = &registries.types;
    let handle = scope
        .resolve_type(sig_name)
        .unwrap_or_else(|| panic!("{sig_name} must bind a type"));
    let schema = match types.node(handle) {
        TypeNode::Signature { schema, .. } => schema,
        _ => panic!("{sig_name} must bind a Signature, got {handle:?}"),
    };
    let member = type_name(member_name, registries);
    if let Some(kt) = schema.abstract_members.get(&member) {
        return *kt;
    }
    schema
        .manifest_members
        .get(&member)
        .copied()
        .unwrap_or_else(|| panic!("member `{member_name}` must live in {sig_name}'s type table"))
}

/// `TYPE Elt` binds `AbstractType { source: SENTINEL, name: "Elt" }` — a SIG-declared abstract
/// member's binder is the canonical sentinel (ruling 12), never a per-declaration id.
#[test]
fn bare_type_binds_abstract_member() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("SIG Container = ((TYPE Elt))");
    match test_run.types().node(member_type(
        scope,
        test_run.registries(),
        "Container",
        "Elt",
    )) {
        TypeNode::AbstractType { source, name, .. } => {
            assert_eq!(name, type_name("Elt", test_run.registries()));
            assert_eq!(source, ScopeId::SENTINEL);
        }
        _ => panic!("Elt must be an abstract member sourced at the canonical binder"),
    }
}

/// `TYPE (Type AS Wrap)` binds an `AbstractType` named `Wrap`, sourced at the canonical binder,
/// carrying `param_names == ["Type"]`.
#[test]
fn hk_type_binds_abstract_constructor() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("SIG Monad = ((TYPE (Type AS Wrap)))");
    match test_run
        .types()
        .node(member_type(scope, test_run.registries(), "Monad", "Wrap"))
    {
        TypeNode::AbstractType {
            source,
            name,
            param_names,
            ..
        } => {
            assert_eq!(name, type_name("Wrap", test_run.registries()));
            assert_eq!(source, ScopeId::SENTINEL);
            assert_eq!(param_names, vec![type_name("Type", test_run.registries())]);
        }
        _ => panic!("Wrap must be an abstract constructor member"),
    }
}

/// An abstract constructor member classifies as `KKind::TypeConstructor`; its first-order
/// sibling as `KKind::ProperType`.
#[test]
fn abstract_member_kind_tracks_parameters() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("SIG Monad = ((TYPE Elt) (TYPE (Type AS Wrap)))");
    let types = test_run.registry_handle();
    assert_eq!(
        member_type(scope, types.registries(), "Monad", "Wrap").kind_of(&types),
        KKind::TypeConstructor,
    );
    assert_eq!(
        member_type(scope, types.registries(), "Monad", "Elt").kind_of(&types),
        KKind::ProperType,
    );
}

/// `TYPE Elt` outside a SIG body errors.
#[test]
fn bare_type_outside_sig_errors() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("TYPE Elt");
    assert!(
        scope.resolve_type("Elt").is_none(),
        "TYPE outside a SIG body must not bind",
    );
}

/// `TYPE (Key Val AS Dict)` — two parameters before `AS` — declares an arity-2 constructor.
#[test]
fn hk_arity_above_one_declares() {
    let program = program_storage();
    let inner = hk_decl_body(&program, "TYPE (Key Val AS Dict)");
    let (param_names, member_name) =
        super::parse_hk_decl(&inner).expect("arity above 1 must declare");
    assert_eq!(param_names, vec!["Key".to_string(), "Val".to_string()]);
    assert_eq!(member_name, "Dict");
}

/// A parameter name repeated in one declaration is a shape error — the names key the
/// application record, so they must be distinct.
#[test]
fn hk_duplicate_parameter_name_errors() {
    let program = program_storage();
    let inner = hk_decl_body(&program, "TYPE (Key Key AS Dict)");
    let error = super::parse_hk_decl(&inner).expect_err("a duplicate parameter name must error");
    assert!(
        error.to_string().contains("duplicate parameter name `Key`"),
        "expected the duplicate-name message, got {error}",
    );
}

/// The parenthesized `(Param... AS Name)` group inside a parsed `TYPE` declaration.
fn hk_decl_body<'a>(
    program: &'a ProgramStorage,
    source: &str,
) -> crate::machine::model::KExpression<'a> {
    let expr = parse_one(program, source);
    match expr.parts.get(1).expect("TYPE decl part").value {
        ExpressionPart::Expression(inner) => *inner,
        other => panic!("expected a parenthesized decl, got {other:?}"),
    }
}

/// A `VAL item :Elt` slot after `TYPE Elt` records the abstract member as its declared type. The
/// slot lives in the signature's stored schema (`value_slots`), not the decl scope's type table.
#[test]
fn val_slot_after_type_records_abstract_member() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("SIG Container = ((TYPE Elt) (VAL item :Elt))");
    let handle = scope
        .resolve_type("Container")
        .expect("Container must bind a type");
    let item = match test_run.types().node(handle) {
        TypeNode::Signature { schema, .. } => schema
            .value_slots
            .get(&value_name("item", test_run.registries()))
            .copied()
            .expect("item must live in Container's stored schema value_slots"),
        _ => panic!("Container must bind a Signature, got {handle:?}"),
    };
    match test_run.types().node(item) {
        TypeNode::AbstractType { source, name, .. } => {
            assert_eq!(name, type_name("Elt", test_run.registries()));
            assert_eq!(source, ScopeId::SENTINEL);
        }
        _ => panic!("item's declared type must be the abstract Elt, got {item:?}"),
    }
}

/// End-to-end: a module ascribed to a SIG with a `TYPE Elt` member mints a per-call
/// `AbstractType` for `Elt` in its `type_members`, nonced on the view module's own `ScopeId`. The
/// `source` binder stays the canonical sentinel — generativity rides `nonce` alone — so the mint is
/// a distinct identity from the SIG-decl-time member it was threaded from.
#[test]
fn opaque_ascription_mints_module_abstract_for_type_member() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "MODULE implementation = ((LET Elt = Number) (LET item = 0))\n\
         SIG Container = ((TYPE Elt) (VAL item :Number))\n\
         LET view = (implementation :| Container)",
    );
    let view = lookup_module(scope, "view", test_run.registries());
    let elt = view
        .type_members
        .get(&type_name("Elt", test_run.registries()))
        .copied();
    let declared = member_type(scope, test_run.registries(), "Container", "Elt");
    match elt {
        Some(minted) => {
            match test_run.types().node(minted) {
                TypeNode::AbstractType {
                    source,
                    name,
                    nonce,
                    ..
                } => {
                    assert_eq!(name, type_name("Elt", test_run.registries()));
                    assert_eq!(source, ScopeId::SENTINEL);
                    assert_eq!(nonce, Some(view.scope_id()));
                }
                _ => panic!(
                    "Elt must mint an abstract type keyed on the view module, got {minted:?}"
                ),
            }
            assert_ne!(minted, declared, "the mint is not the declaration");
        }
        None => panic!("Elt must mint an abstract type keyed on the view module"),
    }
}

/// Two `:|` applications of one SIG mint distinct opaque slot types: each ascription allocates a
/// fresh child scope, so the per-application `nonce` differs even though `source` and name agree.
#[test]
fn two_ascriptions_of_one_sig_mint_distinct_slot_types() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "MODULE implementation = ((LET Elt = Number) (LET item = 0))\n\
         SIG Container = ((TYPE Elt) (VAL item :Number))\n\
         LET one = (implementation :| Container)\n\
         LET two = (implementation :| Container)",
    );
    let elt = |view_name: &str| {
        lookup_module(scope, view_name, test_run.registries())
            .type_members
            .get(&type_name("Elt", test_run.registries()))
            .copied()
            .expect("each view mints Elt")
    };
    let (one, two) = (elt("one"), elt("two"));
    assert!(matches!(
        test_run.types().node(one),
        TypeNode::AbstractType { .. }
    ));
    assert_ne!(
        one, two,
        "each `:|` application mints its own opaque Elt identity",
    );
    assert_ne!(one.digest(), two.digest());
}

/// Assert `kt` is a type constructor — a declared family (`SetMember`) or a SIG's abstract
/// constructor slot (`AbstractType`) — whose parameter names equal `expected`; returns its name.
fn assert_type_constructor(
    kt: KType,
    expected: &[&str],
    registries: &crate::machine::model::RunRegistries,
) -> String {
    let types = &registries.types;
    let want: Vec<_> = expected.iter().map(|s| type_name(s, registries)).collect();
    let param_names = constructor_param_names(kt, types)
        .unwrap_or_else(|| panic!("expected a type constructor, got {kt:?}"));
    assert_eq!(param_names, want);
    match types.node(kt) {
        TypeNode::SetMember { name, .. } => {
            crate::machine::model::render_label(name.symbol(), registries)
        }
        TypeNode::AbstractType { name, .. } => {
            crate::machine::model::render_label(name.symbol(), registries)
        }
        _ => panic!("expected a type constructor, got {kt:?}"),
    }
}

/// A root-scope-bound `Wrap` TypeConstructor member, sealed through the real declaration window.
fn wrap_type_constructor(registries: &crate::machine::model::RunRegistries) -> KType {
    let window = RecursiveGroupWindow::new(vec![(
        type_name("Wrap", registries),
        KKind::TypeConstructor,
    )]);
    window
        .fill_member(
            0,
            RelativeSchema::TypeConstructor {
                schema: crate::machine::model::TypeMemberMap::default(),
                param_names: vec![type_name("Type", registries)],
            },
            &registries.types,
        )
        .expect("a singleton window seals on its sole fill")
        .members[0]
}

/// Pins the dispatch path for an FN return type `:(Number AS Wrap)` against a
/// root-scope-bound TypeConstructor — the `AS` keyworded builtin lowers it to a
/// `ConstructorApply` carrier.
#[test]
fn fn_return_type_constructor_apply_root_scope() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let wrap = wrap_type_constructor(test_run.registries());
    scope.register_builtin_type(
        type_name("Wrap", test_run.registries()),
        wrap,
        test_run.registries(),
        &mut crate::machine::WriteGate::for_test(),
    );
    let id = test_run.dispatch_in_scope(
        crate::machine::model::WorkingExpression::from_ast(
            scope.brand(),
            parse_one(
                &program,
                "LET pure = FN (PURE a :Number) -> :(Number AS Wrap) = (1)",
            ),
        ),
        scope,
    );
    let edge = test_run.runtime.install_edge_for_test(id, scope);
    test_run.runtime.execute().expect("scheduler should run");
    match test_run.runtime.edge_result_error(edge) {
        Ok(()) => {}
        Err(e) => panic!("FN with :(Number AS Wrap) return failed: {}", e),
    }
    let pure = scope.expect_value("pure");
    let f = match pure {
        KObject::KFunction(f) => *f,
        other => panic!("pure not KFunction: {:?}", other.ktype()),
    };
    use crate::machine::model::ReturnType;
    match f.signature.return_type() {
        ReturnType::Resolved(handle) => match test_run.types().node(handle) {
            TypeNode::ConstructorApply { arguments, .. } => {
                assert_eq!(
                    arguments,
                    Record::from_pairs([(
                        crate::machine::model::Symbol::of("Type"),
                        KType::NUMBER
                    )]),
                );
            }
            _ => panic!("expected Resolved(ConstructorApply), got {:?}", handle),
        },
        other => panic!("expected Resolved(ConstructorApply), got {:?}", other),
    }
}

/// End-to-end smoke for a monad-shaped signature: `TYPE (Type AS Wrap)` precedes
/// `VAL pure` so the inner `:(Number AS Wrap)` resolves synchronously against the
/// SIG decl-scope's `bindings.types["Wrap"]` entry.
#[test]
fn monad_signature_smoke() {
    use crate::machine::model::Symbol;
    use crate::parse::parse;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let src = "SIG Monad = ((TYPE (Type AS Wrap)) \
         (VAL pure :(FN (x :Number) -> :(Number AS Wrap))))";
    let exprs = parse(
        program.brand(),
        &crate::machine::model::LabelInterner::new(),
        src,
    )
    .expect("parse should succeed");
    {
        let mut ids = Vec::new();
        for expr in exprs {
            ids.push(test_run.dispatch_in_scope(
                crate::machine::model::WorkingExpression::from_ast(scope.brand(), expr),
                scope,
            ));
        }
        let edges: Vec<_> = ids
            .iter()
            .map(|id| test_run.runtime.install_edge_for_test(*id, scope))
            .collect();
        match test_run.runtime.execute() {
            Ok(()) => {}
            Err(e) => panic!("scheduler errored: {}", e),
        }
        for (i, edge) in edges.iter().enumerate() {
            if let Err(e) = test_run.runtime.edge_result_error(*edge) {
                panic!("expr {} errored: {}", i, e);
            }
        }
    }
    let registries = test_run.registries();
    let types = &registries.types;
    let handle = scope.resolve_type("Monad").expect("Monad must bind a type");
    let schema = match types.node(handle) {
        TypeNode::Signature { schema, .. } => schema,
        _ => panic!("Monad must bind a Signature KType, got {:?}", handle),
    };
    let wrap_kt = schema
        .abstract_members
        .get(&type_name("Wrap", registries))
        .copied()
        .expect("Wrap must live in Monad's stored schema abstract_members");
    assert_type_constructor(wrap_kt, &["Type"], registries);
    // A SIG-body `VAL pure :T` slot lives in the signature's stored schema (`value_slots`),
    // carrying the declared type directly.
    let pure = schema
        .value_slots
        .get(&value_name("pure", registries))
        .copied()
        .expect("pure must live in Monad's stored schema value_slots");
    match types.node(pure) {
        TypeNode::KFunction { params, ret } => {
            assert_eq!(params.get(Symbol::of("x")).copied(), Some(KType::NUMBER));
            assert_eq!(params.len(), 1);
            match types.node(ret) {
                TypeNode::ConstructorApply {
                    constructor,
                    arguments,
                } => {
                    assert_type_constructor(constructor, &["Type"], registries);
                    assert_eq!(
                        arguments,
                        Record::from_pairs([(
                            crate::machine::model::Symbol::of("Type"),
                            KType::NUMBER
                        )]),
                    );
                }
                _ => panic!(
                    "pure return type must be ConstructorApply(Wrap, {{Type = Number}}), got {:?}",
                    ret,
                ),
            }
        }
        _ => panic!("pure must be a Function type, got {:?}", pure),
    }
}

/// `(M.Wrap)` after opaque ascription resolves through the module's `type_members` to the
/// per-call-minted constructor variant. The module supplies the higher-kinded abstract `Wrap`
/// slot with a real arity-1 constructor (`LET Wrap = Wrapper`) — a proper type would fail the
/// slot's kind and parameter-name check.
#[test]
fn module_attr_access_returns_type_constructor() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "NEWTYPE (Type AS Wrapper)\n\
         SIG Monad = ((TYPE (Type AS Wrap)))\n\
         MODULE int_list = ((LET Wrap = Wrapper))\n\
         LET mo = (int_list :| Monad)",
    );
    let mo = lookup_module(scope, "mo", test_run.registries());
    let wrap_t = mo
        .type_members
        .get(&type_name("Wrap", test_run.registries()))
        .copied();
    match wrap_t {
        Some(kt) => {
            let name = assert_type_constructor(kt, &["Type"], test_run.registries());
            assert_eq!(name, "Wrap");
        }
        other => panic!(
            "expected TypeConstructor in type_members[Wrap], got {:?}",
            other
        ),
    }
}
