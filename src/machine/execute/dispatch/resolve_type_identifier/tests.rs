use super::*;
use crate::builtins::test_support::run_root_silent;
use crate::machine::core::run_root_storage;

#[test]
fn resolve_type_expr_builtin_leaf_caches() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let te = TypeIdentifier::leaf("Number".into());
    let first = match scope.resolve_type_identifier(&te, None) {
        TypeResolution::Done(resolved) => resolved,
        _ => panic!("expected Done"),
    };
    assert_eq!(*first, KType::Number);
    let second = match scope.resolve_type_identifier(&te, None) {
        TypeResolution::Done(resolved) => resolved,
        _ => panic!("expected Done on second call"),
    };
    assert!(
        std::ptr::eq(first, second),
        "second call should hit the memo"
    );
}

#[test]
fn resolve_type_expr_unbound_returns_unbound() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let te = TypeIdentifier::leaf("NotABuiltin".into());
    match scope.resolve_type_identifier(&te, None) {
        TypeResolution::Unbound(_) => {}
        _ => panic!("expected Unbound for unknown leaf"),
    }
}

/// Pins the post-finalize memo path: a user type reached after STRUCT
/// finalize lands in the cache.
#[test]
fn resolve_type_expr_user_struct_caches_after_finalize() {
    use crate::builtins::test_support::run;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "NEWTYPE Point = :{x :Number, y :Number}");
    let te = TypeIdentifier::leaf("Point".into());
    let kt = match scope.resolve_type_identifier(&te, None) {
        TypeResolution::Done(resolved) => resolved,
        _ => panic!("expected Done after STRUCT declaration"),
    };
    match kt {
        KType::SetRef { set, index } => assert_eq!(set.member(*index).name, "Point"),
        _ => panic!("expected SetRef for Point"),
    }
    let kt2 = match scope.resolve_type_identifier(&te, None) {
        TypeResolution::Done(resolved) => resolved,
        _ => panic!("expected Done on memo hit"),
    };
    assert!(std::ptr::eq(kt, kt2));
}

/// A singleton record-repr newtype `SetRef` named `name`.
fn struct_setref(name: &str) -> KType {
    use crate::machine::model::Record;
    use crate::machine::model::{NominalSchema, RecursiveSet};
    let set = RecursiveSet::singleton(
        name.into(),
        NominalSchema::NewType(Box::new(KType::record(Box::new(Record::new())))),
    );
    KType::SetRef { set, index: 0 }
}

/// Pins recursion shape against a regression that skips nested structurals.
#[test]
fn ktype_user_refs_yields_nested_structural_refs_in_order() {
    let user_a = struct_setref("A");
    let user_b = struct_setref("B");
    let (set_a, set_b) = match (&user_a, &user_b) {
        (KType::SetRef { set: a, .. }, KType::SetRef { set: b, .. }) => {
            (std::rc::Rc::clone(a), std::rc::Rc::clone(b))
        }
        _ => panic!("expected SetRefs"),
    };
    // Dict<A, List<B>>
    let kt = KType::dict(Box::new(user_a), Box::new(KType::list(Box::new(user_b))));
    let refs: Vec<_> = KTypeUserRefs::of(&kt).collect();
    match refs.as_slice() {
        [UserTypeRef::Member {
            set: first,
            name: first_name,
        }, UserTypeRef::Member {
            set: second,
            name: second_name,
        }] => {
            assert!(std::rc::Rc::ptr_eq(first, &set_a), "first ref is A's set");
            assert_eq!(*first_name, "A");
            assert!(std::rc::Rc::ptr_eq(second, &set_b), "second ref is B's set");
            assert_eq!(*second_name, "B");
        }
        _ => panic!("expected two Member refs in order"),
    }
}

/// SCC discipline: the iterator must not descend into a `SetRef` member's schema —
/// the outer `SetRef` is yielded, the inner stays invisible.
#[test]
fn ktype_user_refs_does_not_recurse_into_user_type_payload() {
    use crate::machine::model::{NominalSchema, RecursiveSet};
    let inner = struct_setref("Inner");
    let outer_set =
        RecursiveSet::singleton("Outer".into(), NominalSchema::NewType(Box::new(inner)));
    let outer = KType::SetRef {
        set: std::rc::Rc::clone(&outer_set),
        index: 0,
    };
    let refs: Vec<_> = KTypeUserRefs::of(&outer).collect();
    match refs.as_slice() {
        [UserTypeRef::Member { set, name }] => {
            assert!(std::rc::Rc::ptr_eq(set, &outer_set), "yields the outer set");
            assert_eq!(*name, "Outer");
        }
        _ => panic!("expected exactly the outer Member ref"),
    }
}

/// Pin against a regression that would push a spurious leaf onto the stack.
#[test]
fn ktype_user_refs_yields_nothing_for_leaf() {
    let mut iter = KTypeUserRefs::of(&KType::Number);
    assert!(iter.next().is_none());
}

mod bare_leaf_resolution {
    use crate::builtins::test_support::run_root_bare;
    use crate::machine::core::run_root_storage;
    use crate::machine::core::BindingIndex;
    use crate::machine::model::KType;
    use crate::machine::model::TypeIdentifier;
    use crate::machine::model::TypeResolution;

    #[test]
    fn builtin_synthesizes_type_carrier() {
        let region = run_root_storage();
        let scope = run_root_bare(&region);
        scope.register_type("Number".into(), KType::Number, BindingIndex::BUILTIN);
        let leaf = TypeIdentifier::leaf("Number".to_string());
        match scope.resolve_type_identifier(&leaf, None) {
            TypeResolution::Done(resolved) if *resolved == KType::Number => {}
            other => panic!("expected Done(Number), got {:?}", outcome_tag(&other)),
        }
    }

    #[test]
    fn unbound_returns_unbound() {
        let region = run_root_storage();
        let scope = run_root_bare(&region);
        let leaf = TypeIdentifier::leaf("Missing".to_string());
        match scope.resolve_type_identifier(&leaf, None) {
            // The bridge surfaces the elaborator's `unknown type name` diagnostic, which
            // names the leaf rather than carrying the bare name.
            TypeResolution::Unbound(message) => assert!(
                message.contains("Missing"),
                "expected an unbound message naming `Missing`, got: {message}",
            ),
            other => panic!("expected Unbound, got {:?}", outcome_tag(&other)),
        }
    }

    /// A bare leaf naming a member caught mid-seal — its `SetRef` identity is
    /// pre-installed but the member is still `pending` and a value-side placeholder
    /// stands in for the producer — parks rather than handing back the half-sealed
    /// identity, then resolves once the member fills and the placeholder clears. This
    /// is the regression the bridge-routed leaf closes: the prior synchronous resolver
    /// returned the pre-installed `SetRef` while the schema was still empty.
    #[test]
    fn mid_seal_member_parks_then_resolves() {
        use crate::machine::core::BindingIndex;
        use crate::machine::core::Bindings;
        use crate::machine::core::NodeId;
        use crate::machine::model::Record;
        use crate::machine::model::{KKind, NominalMember, NominalSchema, RecursiveSet};

        let region = run_root_storage();
        let scope = run_root_bare(&region);
        // Pre-install a singleton set whose one member is still `pending` (schema
        // unfilled) and bind its external `SetRef` into `bindings.types`, mirroring the
        // `RECURSIVE TYPES` pre-install window.
        let member = NominalMember::pending("Node".into(), KKind::NewType);
        let set = std::rc::Rc::new(RecursiveSet::new(vec![member]));
        scope.preinstall_identity(
            "Node".into(),
            KType::SetRef {
                set: std::rc::Rc::clone(&set),
                index: 0,
            },
            BindingIndex::value(0),
        );
        // Mark the binder in-flight (the `pending_types` name the finalize gate reads)
        // and install a value-side placeholder for the producer node to park on.
        let bindings: &Bindings<'_> = scope.bindings();
        let pending_guard = bindings.insert_pending_type("Node".into());
        scope
            .install_placeholder(
                "Node".into(),
                NodeId(7),
                BindingIndex::value(0),
                crate::machine::BindKind::Type,
            )
            .expect("placeholder install");

        let leaf = TypeIdentifier::leaf("Node".to_string());
        match scope.resolve_type_identifier(&leaf, None) {
            TypeResolution::Park(producers) => {
                assert_eq!(producers, vec![NodeId(7)], "parks on the single producer");
            }
            other => panic!("expected Park mid-seal, got {:?}", outcome_tag(&other)),
        }

        // Seal: fill the member, drop the in-flight guard. The re-resolve now admits
        // (the name is no longer in `pending_types`) and hands back the sealed carrier.
        set.fill_member(
            0,
            NominalSchema::NewType(Box::new(KType::record(Box::new(Record::from_pairs([(
                "x".to_string(),
                KType::Number,
            )]))))),
        );
        drop(pending_guard);

        match scope.resolve_type_identifier(&leaf, None) {
            TypeResolution::Done(resolved) => match resolved {
                KType::SetRef { set: s, index } => {
                    assert_eq!(s.member(*index).name, "Node");
                }
                other => panic!("expected SetRef after seal, got {other:?}"),
            },
            other => {
                panic!(
                    "expected Done(SetRef) after seal, got {:?}",
                    outcome_tag(&other)
                )
            }
        }
    }

    fn outcome_tag(c: &TypeResolution<&KType>) -> &'static str {
        match c {
            TypeResolution::Done(_) => "Done",
            TypeResolution::Park(_) => "Park",
            TypeResolution::Unbound(_) => "Unbound",
        }
    }
}
