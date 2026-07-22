use super::*;
use crate::builtins::test_support::TestRun;
use crate::machine::core::run_root_storage;

#[test]
fn resolve_type_expr_builtin_leaf_caches() {
    let region = run_root_storage();
    let test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let types = test_run.types.clone();
    let te = TypeIdentifier::leaf("Number".into());
    let first = match scope.resolve_type_identifier(&te, None, &types) {
        TypeResolution::Done(resolved) => resolved,
        _ => panic!("expected Done"),
    };
    assert_eq!(first, KType::NUMBER);
    let second = match scope.resolve_type_identifier(&te, None, &types) {
        TypeResolution::Done(resolved) => resolved,
        _ => panic!("expected Done on second call"),
    };
    assert_eq!(first, second, "second call should hit the memo");
}

#[test]
fn resolve_type_expr_unbound_returns_unbound() {
    let region = run_root_storage();
    let test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let types = test_run.types.clone();
    let te = TypeIdentifier::leaf("NotABuiltin".into());
    match scope.resolve_type_identifier(&te, None, &types) {
        TypeResolution::Unbound(_) => {}
        _ => panic!("expected Unbound for unknown leaf"),
    }
}

/// Pins the post-finalize memo path: a user type reached after a declaration finalizes lands in
/// the cache as its sealed member handle.
#[test]
fn resolve_type_expr_user_struct_caches_after_finalize() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run("NEWTYPE Point = :{x :Number, y :Number}");
    let types = test_run.types.clone();
    let te = TypeIdentifier::leaf("Point".into());
    let kt = match scope.resolve_type_identifier(&te, None, &types) {
        TypeResolution::Done(resolved) => resolved,
        _ => panic!("expected Done after the declaration"),
    };
    match types.node(kt) {
        TypeNode::SetMember { name, .. } => assert_eq!(name, "Point"),
        _ => panic!("expected a sealed member node for Point"),
    }
    let kt2 = match scope.resolve_type_identifier(&te, None, &types) {
        TypeResolution::Done(resolved) => resolved,
        _ => panic!("expected Done on memo hit"),
    };
    assert_eq!(kt, kt2);
}

/// Pins the walk shape against a regression that skips nested structurals: a relative sibling at
/// any depth is a dependency the gate must see.
#[test]
fn user_type_refs_yields_nested_siblings_in_order() {
    let types = crate::machine::model::TypeRegistry::new();
    let first = types.intern(TypeNode::Sibling(0));
    let second = types.intern(TypeNode::Sibling(1));
    // Dict<Sibling(0), List<Sibling(1)>>
    let kt = types.dict(first, types.list(second));
    let refs = user_type_refs(kt, &types);
    match refs.as_slice() {
        [UserTypeRef::Sibling { index: a }, UserTypeRef::Sibling { index: b }] => {
            assert_eq!((*a, *b), (0, 1), "siblings come back in walk order");
        }
        _ => panic!("expected two sibling refs in order"),
    }
}

/// Member discipline: a sealed member is finished, so it is not a dependency — and the walk must
/// not descend its schema, which holds absolute handles and may be cyclic.
#[test]
fn user_type_refs_does_not_recurse_into_a_sealed_member() {
    use crate::machine::model::{RecursiveGroupWindow, RelativeSchema};
    let types = crate::machine::model::TypeRegistry::new();
    let sealed = RecursiveGroupWindow::seal_singleton(
        "Chain".into(),
        RelativeSchema::NewType(types.list(types.intern(TypeNode::Sibling(0)))),
        None,
        &types,
    );
    assert!(
        user_type_refs(sealed, &types).is_empty(),
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
    use crate::builtins::test_support::{mock_declaration_site, run_root_bare};
    use crate::machine::core::run_root_storage;
    use crate::machine::core::{BindingIndex, DeclarationSite};
    use crate::machine::model::KType;
    use crate::machine::model::TypeIdentifier;
    use crate::machine::model::TypeRegistry;
    use crate::machine::model::TypeResolution;
    use crate::machine::model::{KKind, RecursiveGroupWindow, RelativeSchema};

    #[test]
    fn builtin_synthesizes_type_carrier() {
        let region = run_root_storage();
        let scope = run_root_bare(&region);
        scope.register_type("Number".into(), KType::NUMBER, DeclarationSite::BUILTIN);
        let types = TypeRegistry::new();
        let leaf = TypeIdentifier::leaf("Number".to_string());
        match scope.resolve_type_identifier(&leaf, None, &types) {
            TypeResolution::Done(resolved) if resolved == KType::NUMBER => {}
            other => panic!("expected Done(Number), got {:?}", outcome_tag(&other)),
        }
    }

    #[test]
    fn unbound_returns_unbound() {
        let region = run_root_storage();
        let scope = run_root_bare(&region);
        let types = TypeRegistry::new();
        let leaf = TypeIdentifier::leaf("Missing".to_string());
        match scope.resolve_type_identifier(&leaf, None, &types) {
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
    /// handle, which the gate refuses to memoize: it parks on the declaration's producer instead,
    /// then admits once the window seals and the in-flight guard clears. Caching the relative
    /// handle would leak a window-scoped index into a later, window-free lookup.
    #[test]
    fn mid_window_member_parks_then_resolves() {
        use crate::machine::core::Bindings;
        use crate::machine::core::NodeId;
        use crate::machine::core::Scope;
        use crate::machine::model::Record;

        let region = run_root_storage();
        let outer = run_root_bare(&region);
        let window = RecursiveGroupWindow::new(vec![("Node".into(), KKind::NewType)], None);
        let scope = outer
            .brand()
            .alloc_scope(Scope::child_recursive_group(outer, window.clone()));
        // Mark the binder in-flight (the `pending_types` name the finalize gate reads) and install
        // a value-side placeholder for the producer node to park on.
        let bindings: &Bindings<'_> = scope.bindings();
        let pending_guard = bindings.insert_pending_type("Node".into());
        scope
            .install_placeholder(
                "Node".into(),
                NodeId(7),
                BindingIndex::value(0),
                crate::machine::model::BindKind::Type,
            )
            .expect("placeholder install");

        let types = TypeRegistry::new();
        let leaf = TypeIdentifier::leaf("Node".to_string());
        match scope.resolve_type_identifier(&leaf, None, &types) {
            TypeResolution::Park(producers) => {
                assert_eq!(producers, vec![NodeId(7)], "parks on the single producer");
            }
            other => panic!("expected Park mid-window, got {:?}", outcome_tag(&other)),
        }

        // Seal: fill the member, drop the in-flight guard, and bind the sealed handle where the
        // declarator's finalize would. The re-resolve now admits.
        let sealed = window
            .fill_member(
                0,
                RelativeSchema::NewType(
                    types.record(Record::from_pairs([("x".to_string(), KType::NUMBER)])),
                ),
                &types,
            )
            .expect("the only member's fill seals the window");
        drop(pending_guard);
        scope
            .register_nominal_upsert(
                "Node".into(),
                sealed.members[0],
                mock_declaration_site(7, 0),
            )
            .expect("install the sealed identity");

        match scope.resolve_type_identifier(&leaf, None, &types) {
            TypeResolution::Done(resolved) => assert_eq!(resolved, sealed.members[0]),
            other => panic!(
                "expected Done(member) after seal, got {:?}",
                outcome_tag(&other)
            ),
        }
    }

    /// Shadowing: an in-flight declaration of the *same name* in an unrelated window must not
    /// capture a sibling reference minted against this one. The gate resolves the index against
    /// the nearest window and then requires the pending scope to carry that same window, so the
    /// inner declaration's own window — which is a different allocation — never matches.
    ///
    /// This is what the pre-flip gate got from pointer-equality on the set allocation; window
    /// identity carries exactly the same guarantee.
    #[test]
    fn a_same_named_declaration_in_another_window_does_not_capture() {
        use crate::machine::core::Bindings;
        use crate::machine::core::NodeId;
        use crate::machine::core::Scope;

        let region = run_root_storage();
        let root = run_root_bare(&region);
        // An outer scope with an in-flight `Node` belonging to its *own* window.
        let other_window = RecursiveGroupWindow::new(vec![("Node".into(), KKind::NewType)], None);
        let outer = root
            .brand()
            .alloc_scope(Scope::child_recursive_group(root, other_window));
        let outer_bindings: &Bindings<'_> = outer.bindings();
        let _outer_guard = outer_bindings.insert_pending_type("Node".into());
        outer
            .install_placeholder(
                "Node".into(),
                NodeId(11),
                BindingIndex::value(0),
                crate::machine::model::BindKind::Type,
            )
            .expect("placeholder install");

        // The elaborating scope carries a *different* window that also announces `Node`, with no
        // pending marker of its own.
        let inner_window = RecursiveGroupWindow::new(vec![("Node".into(), KKind::NewType)], None);
        let inner = outer
            .brand()
            .alloc_scope(Scope::child_recursive_group(outer, inner_window));

        let types = TypeRegistry::new();
        let leaf = TypeIdentifier::leaf("Node".to_string());
        match inner.resolve_type_identifier(&leaf, None, &types) {
            TypeResolution::Done(_) => {}
            other => panic!(
                "the outer same-named declaration must not capture this reference, got {:?}",
                outcome_tag(&other),
            ),
        }
    }

    fn outcome_tag(c: &TypeResolution<KType>) -> &'static str {
        match c {
            TypeResolution::Done(_) => "Done",
            TypeResolution::Park(_) => "Park",
            TypeResolution::Unbound(_) => "Unbound",
        }
    }
}
