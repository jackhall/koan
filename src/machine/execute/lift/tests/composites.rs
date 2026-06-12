//! `composites` arms of `lift_kobject`.

use super::*;
use crate::builtins::default_scope;
use crate::machine::model::types::{KType, NominalSchema, Record, RecursiveSet};
use crate::machine::model::values::Held;
use crate::machine::model::KObject;
use crate::machine::{CallArena, ScopeId};

use super::{alloc_local_kf, defeat_fast_path};

/// A singleton tagged set named `name`, for a lift-test carrier identity.
fn tagged_set<'run>(name: &str, scope_id: ScopeId) -> Rc<RecursiveSet<'run>> {
    RecursiveSet::singleton(
        name.into(),
        scope_id,
        NominalSchema::Tagged(std::collections::HashMap::new()),
    )
}

/// A singleton record-repr newtype (ex-struct) set named `name`, for a lift-test carrier
/// identity.
fn record_newtype_set<'run>(name: &str, scope_id: ScopeId) -> Rc<RecursiveSet<'run>> {
    RecursiveSet::singleton(
        name.into(),
        scope_id,
        NominalSchema::Newtype(Box::new(KType::Record(Box::new(Record::new())))),
    )
}

/// `List<Dict<KFunction>>` drives `any_descendant` recursion through both the
/// Dict arm and the nested-List arm down to the KFunction leaf.
#[test]
fn list_of_dict_with_kfunction_anchors_via_recursion() {
    use crate::machine::model::types::Serializable;
    use crate::machine::model::values::KKey;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    let mut inner_map: HashMap<Box<dyn Serializable<'_>>, KObject> = HashMap::new();
    inner_map.insert(
        Box::new(KKey::String("f".into())),
        KObject::KFunction(kf_ref, None),
    );
    let outer = KObject::list(vec![KObject::dict(inner_map)]);
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&outer, &dying);
    let count_after = Rc::strong_count(&dying);
    match &lifted {
        KObject::List(items, _) => match &items[0] {
            Held::Object(KObject::Dict(entries, _, _)) => match entries.values().next().unwrap() {
                Held::Object(KObject::KFunction(_, frame)) => assert!(frame.is_some()),
                other => panic!("expected nested KFunction, got {}", other.summarize()),
            },
            other => panic!("expected nested Dict, got {}", other.summarize()),
        },
        other => panic!("expected List, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
}

/// `List<Tagged<KFunction>>` exercises the Tagged recursion arm of
/// `any_descendant`.
#[test]
fn list_of_tagged_with_kfunction_anchors_via_recursion() {
    use crate::machine::ScopeId;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    let tagged = KObject::Tagged {
        tag: "T".into(),
        value: Rc::new(KObject::KFunction(kf_ref, None)),
        set: tagged_set("Carrier", ScopeId::next()),
        index: 0,
        type_args: std::rc::Rc::new(vec![]),
    };
    let outer = KObject::list(vec![tagged]);
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&outer, &dying);
    let count_after = Rc::strong_count(&dying);
    match &lifted {
        KObject::List(items, _) => match &items[0] {
            Held::Object(KObject::Tagged { value, .. }) => match &**value {
                KObject::KFunction(_, frame) => assert!(frame.is_some()),
                other => panic!("expected nested KFunction, got {:?}", other.ktype()),
            },
            other => panic!("expected nested Tagged, got {}", other.summarize()),
        },
        other => panic!("expected List, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
}

/// A descendant carrying its own `Some(rc)` anchor must short-circuit
/// `needs_lift` so the list reuses its Rc and the dying frame's count is
/// untouched.
#[test]
fn list_with_pre_anchored_variants_skips_them() {
    use crate::machine::core::kfunction::ArgumentBundle;
    use crate::machine::model::values::Module;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);
    let other = CallArena::new(scope, None);
    let kf_ref = alloc_local_kf(&dying);
    let module = Module::new("M".into(), dying.scope());
    let m_ref: &Module = dying.arena().alloc_module(module);

    let future = KFuture {
        parsed: KExpression::new(vec![]),
        function: kf_ref,
        bundle: ArgumentBundle {
            args: Record::new(),
        },
    };
    let items = Rc::new(vec![
        Held::Object(KObject::KFunction(kf_ref, Some(Rc::clone(&other)))),
        Held::Object(KObject::KFuture(future, Some(Rc::clone(&other)))),
        Held::Type(KType::Module {
            module: m_ref,
            frame: Some(Rc::clone(&other)),
        }),
    ]);
    let list = KObject::list_with_type(Rc::clone(&items), KType::Any);
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&list, &dying);
    let dying_after = Rc::strong_count(&dying);
    match &lifted {
        KObject::List(out, _) => assert!(
            Rc::ptr_eq(out, &items),
            "all pre-anchored ⇒ no needs_lift descendant ⇒ Rc reuse",
        ),
        other => panic!("expected List, got {:?}", other.ktype()),
    }
    assert_eq!(
        dying_after, before,
        "pre-anchored variants must not bump dying Rc"
    );
}

/// Unanchored KFuture inside a list whose function captured the dying scope
/// drives the rebuild.
#[test]
fn list_with_unanchored_kfuture_anchors() {
    use crate::machine::core::kfunction::ArgumentBundle;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    let future = KFuture {
        parsed: KExpression::new(vec![]),
        function: kf_ref,
        bundle: ArgumentBundle {
            args: Record::new(),
        },
    };
    let list = KObject::list(vec![KObject::KFuture(future, None)]);
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&list, &dying);
    let count_after = Rc::strong_count(&dying);
    match &lifted {
        KObject::List(out, _) => {
            assert!(matches!(
                &out[0],
                Held::Object(KObject::KFuture(_, Some(_)))
            ))
        }
        other => panic!("expected List, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
}

/// Unanchored KModule whose child scope is the dying arena, inside a list.
#[test]
fn list_with_unanchored_kmodule_anchors() {
    use crate::machine::model::values::Module;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);
    let module = Module::new("LocalM".into(), dying.scope());
    let m_ref: &Module = dying.arena().alloc_module(module);

    let list = KObject::list_of_held(vec![Held::Type(KType::Module {
        module: m_ref,
        frame: None,
    })]);
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&list, &dying);
    let count_after = Rc::strong_count(&dying);
    match &lifted {
        KObject::List(out, _) => assert!(matches!(
            &out[0],
            Held::Type(KType::Module { frame: Some(_), .. }),
        )),
        other => panic!("expected List, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
}

/// Wrapped (record-repr newtype) and KExpression are `needs_lift` leaves, so a list of only
/// those must reuse its Rc.
#[test]
fn list_with_wrapped_and_kexpression_descendants_clones_rc() {
    use crate::machine::model::values::NonWrappedRef;
    use crate::machine::ScopeId;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);

    let record = KObject::record(Record::new());
    let type_id: &KType = arena.alloc_ktype(KType::SetRef {
        set: record_newtype_set("S", ScopeId::next()),
        index: 0,
    });
    let s = KObject::Wrapped {
        inner: NonWrappedRef::peel(&record),
        type_id,
    };
    let e = KObject::KExpression(KExpression::new(vec![]));
    let items = Rc::new(vec![Held::Object(s), Held::Object(e)]);
    let list = KObject::list_with_type(Rc::clone(&items), KType::Any);
    let before = Rc::strong_count(&items);

    let lifted = lift_kobject(&list, &dying);
    let count_after = Rc::strong_count(&items);
    match &lifted {
        KObject::List(out, _) => assert!(Rc::ptr_eq(out, &items)),
        other => panic!("expected List, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
}

/// A list of non-borrowing leaves must lift via `Rc::clone`; rebuilding would
/// over-allocate.
#[test]
fn list_no_descendants_clones_rc() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);

    let items = Rc::new(vec![
        Held::Object(KObject::Number(1.0)),
        Held::Object(KObject::Number(2.0)),
    ]);
    let list = KObject::list_with_type(Rc::clone(&items), KType::Any);
    let before = Rc::strong_count(&items);

    let lifted = lift_kobject(&list, &dying);
    let count_after = Rc::strong_count(&items);
    match lifted {
        KObject::List(out, _) => assert!(
            Rc::ptr_eq(&out, &items),
            "non-borrowing list must reuse the inner Rc"
        ),
        other => panic!("expected List, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1, "Rc::clone bumps count by 1");
}

#[test]
fn list_with_local_kfunction_rebuilds_and_anchors() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    let list = KObject::list(vec![KObject::KFunction(kf_ref, None)]);
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&list, &dying);
    let count_after = Rc::strong_count(&dying);
    match lifted {
        KObject::List(out, _) => match &out[0] {
            Held::Object(KObject::KFunction(_, frame)) => assert!(
                frame.is_some(),
                "nested KFunction must anchor on dying frame's Rc",
            ),
            other => panic!("expected nested KFunction, got {}", other.summarize()),
        },
        other => panic!("expected List, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1, "one anchored descendant ⇒ +1 Rc");
}

/// Dict counterpart of `list_no_descendants_clones_rc`.
#[test]
fn dict_no_descendants_clones_rc() {
    use crate::machine::model::types::Serializable;
    use crate::machine::model::values::KKey;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);

    let mut map: HashMap<Box<dyn Serializable<'_>>, Held> = HashMap::new();
    map.insert(
        Box::new(KKey::String("a".into())),
        Held::Object(KObject::Number(1.0)),
    );
    let entries = Rc::new(map);
    let dict = KObject::dict_with_type(Rc::clone(&entries), KType::Any, KType::Any);
    let before = Rc::strong_count(&entries);

    let lifted = lift_kobject(&dict, &dying);
    let count_after = Rc::strong_count(&entries);
    match lifted {
        KObject::Dict(out, _, _) => assert!(
            Rc::ptr_eq(&out, &entries),
            "non-borrowing dict must reuse the inner Rc",
        ),
        other => panic!("expected Dict, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
}

/// Dict counterpart of `list_with_local_kfunction_rebuilds_and_anchors`.
#[test]
fn dict_with_local_kfunction_rebuilds_and_anchors() {
    use crate::machine::model::types::Serializable;
    use crate::machine::model::values::KKey;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    let mut map: HashMap<Box<dyn Serializable<'_>>, KObject> = HashMap::new();
    map.insert(
        Box::new(KKey::String("f".into())),
        KObject::KFunction(kf_ref, None),
    );
    let dict = KObject::dict(map);
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&dict, &dying);
    let count_after = Rc::strong_count(&dying);
    match lifted {
        KObject::Dict(out, _, _) => {
            let v = out.values().next().expect("one entry");
            match v {
                Held::Object(KObject::KFunction(_, frame)) => assert!(frame.is_some()),
                other => panic!("expected nested KFunction, got {}", other.summarize()),
            }
        }
        other => panic!("expected Dict, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
}

/// Tagged wrapping a non-borrowing value must reuse the inner `Rc` *and* preserve
/// `(scope_id, name)` identity through the no-rebuild branch.
#[test]
fn tagged_no_borrow_clones_inner_rc() {
    use crate::machine::ScopeId;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);

    let inner = Rc::new(KObject::Number(42.0));
    let sid = ScopeId::next();
    let set = tagged_set("Maybe", sid);
    let tagged = KObject::Tagged {
        tag: "Just".into(),
        value: Rc::clone(&inner),
        set: Rc::clone(&set),
        index: 0,
        type_args: std::rc::Rc::new(vec![]),
    };
    let before = Rc::strong_count(&inner);

    let lifted = lift_kobject(&tagged, &dying);
    let count_after = Rc::strong_count(&inner);
    match lifted {
        KObject::Tagged {
            tag,
            value,
            set: lifted_set,
            ..
        } => {
            assert!(
                Rc::ptr_eq(&value, &inner),
                "no-borrow Tagged must reuse inner Rc"
            );
            assert_eq!(tag, "Just");
            assert!(
                Rc::ptr_eq(&lifted_set, &set),
                "lift shares the RecursiveSet by Rc::clone"
            );
        }
        other => panic!("expected Tagged, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
}

/// Tagged wrapping a borrowing KFunction must rebuild and share the same `RecursiveSet`
/// (`Rc::clone`) through the rebuild branch.
#[test]
fn tagged_with_local_kfunction_rebuilds_and_anchors() {
    use crate::machine::ScopeId;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    let sid = ScopeId::next();
    let set = tagged_set("Carrier", sid);
    let tagged = KObject::Tagged {
        tag: "Wrap".into(),
        value: Rc::new(KObject::KFunction(kf_ref, None)),
        set: Rc::clone(&set),
        index: 0,
        type_args: std::rc::Rc::new(vec![]),
    };
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&tagged, &dying);
    let count_after = Rc::strong_count(&dying);
    match lifted {
        KObject::Tagged {
            tag,
            value,
            set: lifted_set,
            ..
        } => {
            assert!(Rc::ptr_eq(&lifted_set, &set), "lift shares the set");
            assert_eq!(tag, "Wrap");
            assert_eq!(lifted_set.member(0).name, "Carrier");
            assert_eq!(lifted_set.member(0).scope_id, sid);
            match &*value {
                KObject::KFunction(_, frame) => assert!(frame.is_some()),
                other => panic!("expected nested KFunction, got {:?}", other.ktype()),
            }
        }
        other => panic!("expected Tagged, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
}

/// A `KTypeValue` carrying a *recursive* `SetRef` (a self-recursive STRUCT type value)
/// lifts across the dying arena by `Rc::clone` of the whole set — no copy, no anchor. After
/// lift the set is still navigable (its self-edge `SetLocal` resolves back to the lifted
/// member). Mirrors `recursive_tagged_match_no_uaf`: the type value escapes the call arena
/// that built it without UAF.
#[test]
fn recursive_setref_type_value_lifts_by_rc_clone() {
    use crate::machine::model::types::{KKind, NominalMember, NominalSchema};
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);

    // A self-recursive `Tree` whose `children` field is `List(SetLocal(0))` — the shape a
    // `NEWTYPE Tree = :{children :(LIST OF Tree)}` seals into.
    let member = NominalMember::pending("Tree".into(), ScopeId::next(), KKind::Newtype);
    member.fill(NominalSchema::Newtype(Box::new(KType::Record(Box::new(
        Record::from_pairs(vec![(
            "children".into(),
            KType::List(Box::new(KType::SetLocal(0))),
        )]),
    )))));
    let set = Rc::new(RecursiveSet::new(vec![member]));
    let type_value = KType::SetRef {
        set: Rc::clone(&set),
        index: 0,
    };
    let before = Rc::strong_count(&set);

    let lifted = lift_ktype(&type_value, &dying);

    // Lift `Rc::clone`d the set — the strong count rose, and the lifted value's set is the
    // same allocation, so the recursive group travels as one unit.
    assert_eq!(Rc::strong_count(&set), before + 1);
    match &lifted {
        KType::SetRef {
            set: lifted_set,
            index,
        } => {
            assert!(
                Rc::ptr_eq(lifted_set, &set),
                "lift must share the same RecursiveSet allocation",
            );
            // Navigable: the member's self-edge `SetLocal(0)` survives the lift.
            let borrow = lifted_set.member(*index).schema();
            match borrow.as_ref() {
                Some(NominalSchema::Newtype(repr)) => match repr.as_ref() {
                    KType::Record(fields) => assert_eq!(
                        fields.get("children"),
                        Some(&KType::List(Box::new(KType::SetLocal(0))))
                    ),
                    other => panic!("expected a record repr after lift, got {other:?}"),
                },
                other => panic!("expected a navigable Newtype schema after lift, got {other:?}"),
            }
        }
        other => panic!("expected a SetRef type after lift, got {other:?}"),
    }
}

/// A recursive record-repr-newtype *value* (`KObject::Wrapped` whose `type_id` is a `SetRef`
/// into a self-recursive set) lifts across the dying arena: its `inner` record rides an `Rc`
/// (lift-stable by `Rc::clone`) and its `type_id` is the declaration-stable `SetRef`, so the
/// recursive group stays navigable (the `children` field type is the self-edge `SetLocal(0)`).
/// Builds the value directly so the assertion targets the lift path without FN-dispatch
/// incidentals. Unlike the retired `KObject::Struct` (which carried the set `Rc` directly and
/// bumped its strong count on lift), the `Wrapped` reaches the set through its `&'run` `type_id`,
/// so lift copies that reference rather than `Rc::clone`ing the set — the assertion is
/// navigability, not a refcount bump.
#[test]
fn recursive_newtype_value_lifts_and_navigates() {
    use crate::machine::model::types::{KKind, NominalMember, NominalSchema};
    use crate::machine::model::values::NonWrappedRef;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);

    let member = NominalMember::pending("Tree".into(), ScopeId::next(), KKind::Newtype);
    member.fill(NominalSchema::Newtype(Box::new(KType::Record(Box::new(
        Record::from_pairs(vec![(
            "children".into(),
            KType::List(Box::new(KType::SetLocal(0))),
        )]),
    )))));
    let set = Rc::new(RecursiveSet::new(vec![member]));
    let record = KObject::record(Record::from_pairs(vec![(
        "children".to_string(),
        KObject::list(vec![]),
    )]));
    let type_id: &KType = arena.alloc_ktype(KType::SetRef {
        set: Rc::clone(&set),
        index: 0,
    });
    let tree_value = KObject::Wrapped {
        inner: NonWrappedRef::peel(&record),
        type_id,
    };

    let lifted = lift_kobject(&tree_value, &dying);

    match &lifted {
        KObject::Wrapped {
            type_id: lifted_type_id,
            ..
        } => match lifted_type_id {
            KType::SetRef {
                set: lifted_set,
                index,
            } => {
                assert!(
                    Rc::ptr_eq(lifted_set, &set),
                    "lift shares the set allocation through type_id"
                );
                let borrow = lifted_set.member(*index).schema();
                match borrow.as_ref() {
                    Some(NominalSchema::Newtype(repr)) => match repr.as_ref() {
                        KType::Record(fields) => assert_eq!(
                            fields.get("children"),
                            Some(&KType::List(Box::new(KType::SetLocal(0)))),
                            "the lifted Tree's self-reference must still be SetLocal(0)",
                        ),
                        other => panic!("expected a record repr, got {other:?}"),
                    },
                    other => panic!("expected a navigable Newtype schema, got {other:?}"),
                }
            }
            other => panic!("expected a SetRef type_id, got {other:?}"),
        },
        other => panic!("expected a lifted Wrapped value, got {:?}", other.ktype()),
    }
}
