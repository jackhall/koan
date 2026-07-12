use super::*;
use crate::builtins::test_support::run_root_silent;
use crate::machine::core::run_root_storage;

#[test]
fn resolve_type_expr_builtin_leaf_caches() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let te = TypeIdentifier::leaf("Number".into());
    let first = match scope.resolve_type_identifier(&te, None) {
        TypeResolution::Done(resolved) => resolved.kt,
        _ => panic!("expected Done"),
    };
    assert_eq!(*first, KType::Number);
    let second = match scope.resolve_type_identifier(&te, None) {
        TypeResolution::Done(resolved) => resolved.kt,
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
        TypeResolution::Done(resolved) => resolved.kt,
        _ => panic!("expected Done after STRUCT declaration"),
    };
    match kt {
        KType::SetRef { set, index } => assert_eq!(set.member(*index).name, "Point"),
        _ => panic!("expected SetRef for Point"),
    }
    let kt2 = match scope.resolve_type_identifier(&te, None) {
        TypeResolution::Done(resolved) => resolved.kt,
        _ => panic!("expected Done on memo hit"),
    };
    assert!(std::ptr::eq(kt, kt2));
}

/// A singleton record-repr newtype `SetRef` named `name` at `scope_id`.
fn struct_setref<'step>(name: &str, scope_id: ScopeId) -> KType<'step> {
    use crate::machine::model::types::{NominalSchema, RecursiveSet};
    use crate::machine::model::Record;
    let set = RecursiveSet::singleton(
        name.into(),
        scope_id,
        NominalSchema::NewType(Box::new(KType::Record(Box::new(Record::new())))),
    );
    KType::SetRef { set, index: 0 }
}

/// Pins recursion shape against a regression that skips nested structurals.
#[test]
fn ktype_user_refs_yields_nested_structural_refs_in_order() {
    let a_id = ScopeId::next();
    let b_id = ScopeId::next();
    let user_a = struct_setref("A", a_id);
    let user_b = struct_setref("B", b_id);
    // Dict<A, List<B>>
    let kt = KType::Dict(Box::new(user_a), Box::new(KType::List(Box::new(user_b))));
    let refs: Vec<(ScopeId, String)> = KTypeUserRefs::of(&kt)
        .map(|(id, n)| (id, n.to_string()))
        .collect();
    assert_eq!(refs, vec![(a_id, "A".into()), (b_id, "B".into())]);
}

/// SCC discipline: the iterator must not descend into a `SetRef` member's schema —
/// the outer `SetRef` is yielded, the inner stays invisible.
#[test]
fn ktype_user_refs_does_not_recurse_into_user_type_payload() {
    use crate::machine::model::types::{NominalSchema, RecursiveSet};
    let outer_id = ScopeId::next();
    let inner_id = ScopeId::next();
    let inner = struct_setref("Inner", inner_id);
    let outer = {
        let set = RecursiveSet::singleton(
            "Outer".into(),
            outer_id,
            NominalSchema::NewType(Box::new(inner)),
        );
        KType::SetRef { set, index: 0 }
    };
    let refs: Vec<(ScopeId, String)> = KTypeUserRefs::of(&outer)
        .map(|(id, n)| (id, n.to_string()))
        .collect();
    assert_eq!(refs, vec![(outer_id, "Outer".into())]);
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
    use crate::machine::core::StoredReach;
    use crate::machine::model::ast::TypeIdentifier;
    use crate::machine::model::types::TypeResolution;
    use crate::machine::model::KType;
    use crate::machine::TypeHit;

    #[test]
    fn builtin_synthesizes_type_carrier() {
        let region = run_root_storage();
        let scope = run_root_bare(&region);
        scope.register_type(
            "Number".into(),
            KType::Number,
            BindingIndex::BUILTIN,
            StoredReach::for_test(None, false),
        );
        let leaf = TypeIdentifier::leaf("Number".to_string());
        match scope.resolve_type_identifier(&leaf, None) {
            TypeResolution::Done(resolved) if *resolved.kt == KType::Number => {}
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
        use crate::machine::core::kfunction::NodeId;
        use crate::machine::core::BindingIndex;
        use crate::machine::core::{Bindings, PendingTypeEntry};
        use crate::machine::model::ast::KExpression;
        use crate::machine::model::types::{KKind, NominalMember, NominalSchema, RecursiveSet};
        use crate::machine::model::Record;

        let region = run_root_storage();
        let scope = run_root_bare(&region);
        // Pre-install a singleton set whose one member is still `pending` (schema
        // unfilled) and bind its external `SetRef` into `bindings.types`, mirroring the
        // `RECURSIVE TYPES` pre-install window.
        let member = NominalMember::pending("Node".into(), scope.id, KKind::NewType);
        let set = std::rc::Rc::new(RecursiveSet::new(vec![member]));
        scope.preinstall_identity(
            "Node".into(),
            KType::SetRef {
                set: std::rc::Rc::clone(&set),
                index: 0,
            },
            BindingIndex::value(0),
        );
        // Mark the binder in-flight (the `pending_types` entry the finalize gate reads)
        // and install a value-side placeholder for the producer node to park on.
        let bindings: &Bindings<'_> = scope.bindings();
        let pending_guard = bindings.insert_pending_type(
            "Node".into(),
            PendingTypeEntry {
                kind: KKind::NewType,
                scope_id: scope.id,
                schema_expr: KExpression::new(Vec::new()),
            },
        );
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
        set.member(0)
            .fill(NominalSchema::NewType(Box::new(KType::Record(Box::new(
                Record::from_pairs([("x".to_string(), KType::Number)]),
            )))));
        drop(pending_guard);

        match scope.resolve_type_identifier(&leaf, None) {
            TypeResolution::Done(resolved) => match resolved.kt {
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

    fn outcome_tag(c: &TypeResolution<TypeHit<'_>>) -> &'static str {
        match c {
            TypeResolution::Done(_) => "Done",
            TypeResolution::Park(_) => "Park",
            TypeResolution::Unbound(_) => "Unbound",
        }
    }
}
