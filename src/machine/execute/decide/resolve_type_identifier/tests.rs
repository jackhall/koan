use super::*;
use crate::builtins::test_support::{TestRun, type_name, type_token};
use crate::machine::core::{program_storage, run_root_storage};

#[test]
fn resolve_type_expr_builtin_leaf_resolves_stably() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let types = test_run.registry_handle();
    let te = type_token("Number");
    let first = match scope.resolve_type_identifier(te, None, types.registries()) {
        TypeResolution::Done(resolved) => resolved,
        _ => panic!("expected Done"),
    };
    assert_eq!(first, KType::NUMBER);
    let second = match scope.resolve_type_identifier(te, None, types.registries()) {
        TypeResolution::Done(resolved) => resolved,
        _ => panic!("expected Done on second call"),
    };
    assert_eq!(first, second, "a re-resolve yields the same handle");
}

#[test]
fn resolve_type_expr_unbound_returns_unbound() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let types = test_run.registry_handle();
    let te = type_token("NotABuiltin");
    match scope.resolve_type_identifier(te, None, types.registries()) {
        TypeResolution::Unbound(_) => {}
        _ => panic!("expected Unbound for unknown leaf"),
    }
}

/// Pins the post-finalize path: a user type reached after a declaration finalizes resolves to
/// its sealed member handle, and re-resolves to the same one.
#[test]
fn resolve_type_expr_user_struct_resolves_after_finalize() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run("NEWTYPE Point = :{x :Number, y :Number}");
    let types = test_run.registry_handle();
    let te = type_token("Point");
    let kt = match scope.resolve_type_identifier(te, None, types.registries()) {
        TypeResolution::Done(resolved) => resolved,
        _ => panic!("expected Done after the declaration"),
    };
    match types.node(kt) {
        TypeNode::SetMember { name, .. } => {
            assert_eq!(name, type_name("Point", types.registries()))
        }
        _ => panic!("expected a sealed member node for Point"),
    }
    let kt2 = match scope.resolve_type_identifier(te, None, types.registries()) {
        TypeResolution::Done(resolved) => resolved,
        _ => panic!("expected Done on re-resolve"),
    };
    assert_eq!(kt, kt2);
}

/// Pins the walk shape against a regression that skips nested structurals: a declared slot at any
/// depth is a dependency the gate must see.
#[test]
fn user_type_refs_yields_nested_declared_slots_in_order() {
    let registries = crate::machine::model::RunRegistries::new();
    let types = &registries.types;
    let scope_id = crate::machine::core::ScopeId::next();
    let abstract_slot = |name: &str| {
        types.intern(TypeNode::AbstractType {
            source: scope_id,
            name: crate::builtins::test_support::type_name(name, &registries),
            param_names: Vec::new(),
            nonce: None,
        })
    };
    // Dict<Aa, List<Bb>>
    let kt = types.dict(abstract_slot("Aa"), types.list(abstract_slot("Bb")));
    let refs = user_type_refs(kt, types);
    let names: Vec<String> = refs
        .iter()
        .map(|r| crate::machine::model::render_label(r.name.symbol(), &registries))
        .collect();
    assert_eq!(names, vec!["Aa", "Bb"], "slots come back in walk order");
}

/// Member discipline: a sealed member is finished, so it is not a dependency — and the walk must
/// not descend its schema, which holds absolute handles and may be cyclic.
#[test]
fn user_type_refs_does_not_recurse_into_a_sealed_member() {
    use crate::machine::model::{RecursiveGroupWindow, RelativeSchema};
    let registries = crate::machine::model::RunRegistries::new();
    let types = &registries.types;
    let slot = types.intern(TypeNode::AbstractType {
        source: crate::machine::core::ScopeId::next(),
        name: crate::builtins::test_support::type_name("Carrier", &registries),
        param_names: Vec::new(),
        nonce: None,
    });
    let sealed = RecursiveGroupWindow::seal_singleton(
        type_token("Chain"),
        RelativeSchema::NewType(types.list(slot)),
        None,
        types,
    );
    assert!(
        user_type_refs(sealed, types).is_empty(),
        "a sealed member is finished and its schema is not walked",
    );
}

/// Pin against a regression that would push a spurious leaf onto the stack.
#[test]
fn user_type_refs_yields_nothing_for_leaf() {
    let types = crate::machine::model::TypeRegistry::new();
    assert!(user_type_refs(KType::NUMBER, &types).is_empty());
}

mod bare_leaf_resolution {
    use crate::builtins::test_support::{
        binder_name, mock_declaration_site, run_root_bare, type_name, type_token,
    };
    use crate::machine::core::run_root_storage;
    use crate::machine::core::{BindingIndex, DeclarationSite};
    use crate::machine::model::KType;
    use crate::machine::model::RunRegistries;
    use crate::machine::model::TypeResolution;

    #[test]
    fn builtin_synthesizes_type_carrier() {
        let region = run_root_storage();
        let scope = run_root_bare(&region);
        let registries = RunRegistries::new();
        let _ = scope.register_type_direct(
            type_name("Number", &registries),
            KType::NUMBER,
            DeclarationSite::BUILTIN,
            &registries,
            &mut crate::machine::WriteGate::for_test(),
        );
        let leaf = type_token("Number");
        match scope.resolve_type_identifier(leaf, None, &registries) {
            TypeResolution::Done(resolved) if resolved == KType::NUMBER => {}
            other => panic!("expected Done(Number), got {:?}", outcome_tag(&other)),
        }
    }

    #[test]
    fn unbound_returns_unbound() {
        let region = run_root_storage();
        let scope = run_root_bare(&region);
        let registries = RunRegistries::new();
        let leaf = type_name("Missing", &registries);
        match scope.resolve_type_identifier(leaf, None, &registries) {
            // The bridge surfaces the elaborator's `unknown type name` diagnostic, which
            // names the leaf rather than carrying the bare name.
            TypeResolution::Unbound(message) => assert!(
                message.contains("Missing"),
                "expected an unbound message naming `Missing`, got: {message}",
            ),
            other => panic!("expected Unbound, got {:?}", outcome_tag(&other)),
        }
    }

    /// A bare leaf naming a member of an open window resolves to that member's relative sibling
    /// handle, which the gate refuses to admit: it parks on the declaration's producer instead,
    /// then admits once the window seals and the identity write clears the placeholder. Handing
    /// the relative handle to a consumer would leak a window-scoped index into a window-free
    /// context.
    #[test]
    fn mid_window_member_parks_then_resolves() {
        use crate::machine::ProducerId;

        use crate::machine::model::Record;

        let region = run_root_storage();
        let outer = run_root_bare(&region);
        let scope = announced_module(outer, &["Node"]);
        let window = scope.own_declaration_window().expect("announced");
        let registries = RunRegistries::new();
        // Mark the binder in-flight: the type-side placeholder the finalize gate reads, naming the
        // producer node a consumer parks on.
        scope
            .install_placeholder(
                binder_name("Node", &registries),
                ProducerId::for_test(7),
                BindingIndex::value(0),
                &registries,
                &mut crate::machine::WriteGate::for_test(),
            )
            .expect("placeholder install");

        let types = &registries.types;
        let leaf = type_token("Node");
        match scope.resolve_type_identifier(leaf, None, &registries) {
            TypeResolution::Park(producers) => {
                assert_eq!(
                    producers,
                    vec![ProducerId::for_test(7)],
                    "parks on the single producer"
                );
            }
            other => panic!("expected Park mid-window, got {:?}", outcome_tag(&other)),
        }

        // Seal: fill the member and bind the sealed handle where the declarator's finalize would —
        // the `types` write clears the placeholder with it. The re-resolve now admits, off the
        // sealed window rather than the binding, so it sees the identity regardless of where in the
        // body the sealing statement sits.
        let sealed = window
            .fill(
                0,
                types.record(Record::from_pairs([(
                    crate::machine::model::Symbol::of("x"),
                    KType::NUMBER,
                )])),
                scope.brand(),
                types,
            )
            .expect("the only member's fill seals the window");
        let member = sealed.member(0).expect("the sole member");
        crate::machine::core::bindings::WriteOp::Type {
            name: type_name("Node", &registries),
            kt: member,
            site: mock_declaration_site(0),
            policy: crate::machine::core::bindings::TypeWritePolicy::UpsertEqual,
            builtin_shadow_guard: true,
        }
        .apply(
            scope,
            &registries,
            &mut crate::machine::WriteGate::for_test(),
        )
        .expect("install the sealed identity");

        match scope.resolve_type_identifier(leaf, None, &registries) {
            TypeResolution::Done(resolved) => assert_eq!(resolved, member),
            other => panic!(
                "expected Done(member) after seal, got {:?}",
                outcome_tag(&other)
            ),
        }
    }

    /// Nearest-window scoping: an in-flight declaration of the *same name* in an enclosing module
    /// body must not capture a reference elaborated inside a nested one. Only the nearest window is
    /// consulted, and the producers a consumer parks on are read off the scope carrying *that*
    /// window — so the outer body's placeholder is never what this reference waits on.
    #[test]
    fn a_same_named_declaration_in_an_outer_body_does_not_capture() {
        use crate::machine::ProducerId;

        let region = run_root_storage();
        let root = run_root_bare(&region);
        // An outer module body with an in-flight `Node` of its own.
        let outer = announced_module(root, &["Node"]);
        let registries = RunRegistries::new();
        outer
            .install_placeholder(
                binder_name("Node", &registries),
                ProducerId::for_test(11),
                BindingIndex::value(0),
                &registries,
                &mut crate::machine::WriteGate::for_test(),
            )
            .expect("placeholder install");

        // The elaborating scope is a *different* module body that also announces `Node`, with no
        // pending marker of its own.
        let inner = announced_module(outer, &["Node"]);

        let leaf = type_token("Node");
        match inner.resolve_type_identifier(leaf, None, &registries) {
            TypeResolution::Unbound(_) => {}
            other => panic!(
                "the outer same-named declaration must not capture this reference, got {:?}",
                outcome_tag(&other),
            ),
        }
    }

    /// A module-body child announcing `members`, for the fixtures above.
    fn announced_module<'a>(
        parent: &'a crate::machine::Scope<'a>,
        members: &[&str],
    ) -> &'a crate::machine::Scope<'a> {
        let mut announced = crate::machine::model::AnnouncedData::default();
        for member in members {
            announced.announce(crate::builtins::test_support::type_token(member));
        }
        parent.alloc_child_under_module(Some(announced))
    }

    fn outcome_tag(c: &TypeResolution<KType>) -> &'static str {
        match c {
            TypeResolution::Done(_) => "Done",
            TypeResolution::Park(_) => "Park",
            TypeResolution::Unbound(_) => "Unbound",
        }
    }
}
