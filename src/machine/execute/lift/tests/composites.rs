//! `composites` arms of `lift_kobject`.

use super::*;
use crate::builtins::default_scope;
use crate::machine::model::KObject;
use crate::machine::CallArena;

use super::{alloc_local_kf, defeat_fast_path};

/// `any_descendant`'s Dict recursion arm (136) and List None-recursion arm
/// (177) only fire when a Dict / List sits inside another composite at lift
/// time. `List<Dict<KFunction>>` triggers both: the outer list rebuild walks
/// each item through `needs_lift` → `any_descendant`, which recurses into
/// Dict, which recurses into the KFunction leaf.
#[test]
fn list_of_dict_with_kfunction_anchors_via_recursion() {
    use crate::machine::model::types::Serializable;
    use crate::machine::model::values::KKey;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    let mut inner_map: HashMap<Box<dyn Serializable>, KObject> = HashMap::new();
    inner_map.insert(
        Box::new(KKey::String("f".into())),
        KObject::KFunction(kf_ref, None),
    );
    let outer = KObject::List(Rc::new(vec![KObject::Dict(Rc::new(inner_map))]));
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&outer, &dying);
    let count_after = Rc::strong_count(&dying);
    match &lifted {
        KObject::List(items) => match &items[0] {
            KObject::Dict(entries) => match entries.values().next().unwrap() {
                KObject::KFunction(_, frame) => assert!(frame.is_some()),
                other => panic!("expected nested KFunction, got {:?}", other.ktype()),
            },
            other => panic!("expected nested Dict, got {:?}", other.ktype()),
        },
        other => panic!("expected List, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
}

/// `any_descendant`'s Tagged recursion arm (137). `List<Tagged<KFunction>>`
/// walks the outer list, recurses into Tagged's `value`, finds the KFunction.
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
        scope_id: ScopeId::next(),
        name: "Carrier".into(),
    };
    let outer = KObject::List(Rc::new(vec![tagged]));
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&outer, &dying);
    let count_after = Rc::strong_count(&dying);
    match &lifted {
        KObject::List(items) => match &items[0] {
            KObject::Tagged { value, .. } => match &**value {
                KObject::KFunction(_, frame) => assert!(frame.is_some()),
                other => panic!("expected nested KFunction, got {:?}", other.ktype()),
            },
            other => panic!("expected nested Tagged, got {:?}", other.ktype()),
        },
        other => panic!("expected List, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
}

/// `needs_lift`'s pre-anchored short-circuit arms (164, 169, 171) — when a
/// List descendant already carries its own `Some(rc)` anchor, the predicate
/// must return `Some(false)` and the list must NOT mark them as needing lift.
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
        bundle: ArgumentBundle { args: HashMap::new() },
    };
    let items = Rc::new(vec![
        KObject::KFunction(kf_ref, Some(Rc::clone(&other))),
        KObject::KFuture(future, Some(Rc::clone(&other))),
        KObject::KModule(m_ref, Some(Rc::clone(&other))),
    ]);
    let list = KObject::List(Rc::clone(&items));
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&list, &dying);
    let dying_after = Rc::strong_count(&dying);
    match &lifted {
        KObject::List(out) => assert!(
            Rc::ptr_eq(out, &items),
            "all pre-anchored ⇒ no needs_lift descendant ⇒ Rc reuse",
        ),
        other => panic!("expected List, got {:?}", other.ktype()),
    }
    assert_eq!(dying_after, before, "pre-anchored variants must not bump dying Rc");
}

/// `needs_lift`'s KFuture None arm (170) — unanchored KFuture inside a list
/// whose function captured the dying scope drives the rebuild.
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
        bundle: ArgumentBundle { args: HashMap::new() },
    };
    let list = KObject::List(Rc::new(vec![KObject::KFuture(future, None)]));
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&list, &dying);
    let count_after = Rc::strong_count(&dying);
    match &lifted {
        KObject::List(out) => assert!(matches!(&out[0], KObject::KFuture(_, Some(_)))),
        other => panic!("expected List, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
}

/// `needs_lift`'s KModule None arm (172–174) — unanchored KModule whose
/// child scope is the dying arena, inside a list.
#[test]
fn list_with_unanchored_kmodule_anchors() {
    use crate::machine::model::values::Module;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);
    let module = Module::new("LocalM".into(), dying.scope());
    let m_ref: &Module = dying.arena().alloc_module(module);

    let list = KObject::List(Rc::new(vec![KObject::KModule(m_ref, None)]));
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&list, &dying);
    let count_after = Rc::strong_count(&dying);
    match &lifted {
        KObject::List(out) => assert!(matches!(&out[0], KObject::KModule(_, Some(_)))),
        other => panic!("expected List, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
}

/// `needs_lift`'s `Struct | KExpression => Some(false)` arm (176) — Struct
/// and KExpression descendants inside a List are leaves to needs_lift, so
/// the list must reuse its Rc (no rebuild) when those are its only contents.
#[test]
fn list_with_struct_and_kexpression_descendants_clones_rc() {
    use crate::machine::ScopeId;
    use indexmap::IndexMap;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);

    let fields: IndexMap<String, KObject> = IndexMap::new();
    let s = KObject::Struct {
        name: "S".into(),
        scope_id: ScopeId::next(),
        fields: Rc::new(fields),
    };
    let e = KObject::KExpression(KExpression::new(vec![]));
    let items = Rc::new(vec![s, e]);
    let list = KObject::List(Rc::clone(&items));
    let before = Rc::strong_count(&items);

    let lifted = lift_kobject(&list, &dying);
    let count_after = Rc::strong_count(&items);
    match &lifted {
        KObject::List(out) => assert!(Rc::ptr_eq(out, &items)),
        other => panic!("expected List, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
}

/// List of non-borrowing leaves must lift via `Rc::clone` — the rebuild branch
/// would over-allocate and break the fast-path/needs_lift invariant.
#[test]
fn list_no_descendants_clones_rc() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    defeat_fast_path(&dying);

    let items = Rc::new(vec![KObject::Number(1.0), KObject::Number(2.0)]);
    let list = KObject::List(Rc::clone(&items));
    let before = Rc::strong_count(&items);

    let lifted = lift_kobject(&list, &dying);
    let count_after = Rc::strong_count(&items);
    match lifted {
        KObject::List(out) => assert!(
            Rc::ptr_eq(&out, &items),
            "non-borrowing list must reuse the inner Rc"
        ),
        other => panic!("expected List, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1, "Rc::clone bumps count by 1");
}

/// List containing a KFunction whose captured scope is the dying arena must rebuild
/// the list and anchor the inner KFunction on the dying frame's Rc.
#[test]
fn list_with_local_kfunction_rebuilds_and_anchors() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    let list = KObject::List(Rc::new(vec![KObject::KFunction(kf_ref, None)]));
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&list, &dying);
    let count_after = Rc::strong_count(&dying);
    match lifted {
        KObject::List(out) => match &out[0] {
            KObject::KFunction(_, frame) => assert!(
                frame.is_some(),
                "nested KFunction must anchor on dying frame's Rc",
            ),
            other => panic!("expected nested KFunction, got {:?}", other.ktype()),
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

    let mut map: HashMap<Box<dyn Serializable>, KObject> = HashMap::new();
    map.insert(Box::new(KKey::String("a".into())), KObject::Number(1.0));
    let entries = Rc::new(map);
    let dict = KObject::Dict(Rc::clone(&entries));
    let before = Rc::strong_count(&entries);

    let lifted = lift_kobject(&dict, &dying);
    let count_after = Rc::strong_count(&entries);
    match lifted {
        KObject::Dict(out) => assert!(
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

    let mut map: HashMap<Box<dyn Serializable>, KObject> = HashMap::new();
    map.insert(
        Box::new(KKey::String("f".into())),
        KObject::KFunction(kf_ref, None),
    );
    let dict = KObject::Dict(Rc::new(map));
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&dict, &dying);
    let count_after = Rc::strong_count(&dying);
    match lifted {
        KObject::Dict(out) => {
            let v = out.values().next().expect("one entry");
            match v {
                KObject::KFunction(_, frame) => assert!(frame.is_some()),
                other => panic!("expected nested KFunction, got {:?}", other.ktype()),
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
    let tagged = KObject::Tagged {
        tag: "Just".into(),
        value: Rc::clone(&inner),
        scope_id: sid,
        name: "Maybe".into(),
    };
    let before = Rc::strong_count(&inner);

    let lifted = lift_kobject(&tagged, &dying);
    let count_after = Rc::strong_count(&inner);
    match lifted {
        KObject::Tagged { tag, value, scope_id, name } => {
            assert!(Rc::ptr_eq(&value, &inner), "no-borrow Tagged must reuse inner Rc");
            assert_eq!(tag, "Just");
            assert_eq!(name, "Maybe");
            assert_eq!(scope_id, sid);
        }
        other => panic!("expected Tagged, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
}

/// Tagged wrapping a borrowing KFunction must rebuild and propagate
/// `(scope_id, name)` unchanged through the rebuild branch.
#[test]
fn tagged_with_local_kfunction_rebuilds_and_anchors() {
    use crate::machine::ScopeId;
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(std::io::sink()));
    let dying = CallArena::new(scope, None);
    let kf_ref = alloc_local_kf(&dying);

    let sid = ScopeId::next();
    let tagged = KObject::Tagged {
        tag: "Wrap".into(),
        value: Rc::new(KObject::KFunction(kf_ref, None)),
        scope_id: sid,
        name: "Carrier".into(),
    };
    let before = Rc::strong_count(&dying);

    let lifted = lift_kobject(&tagged, &dying);
    let count_after = Rc::strong_count(&dying);
    match lifted {
        KObject::Tagged { tag, value, scope_id, name } => {
            assert_eq!(tag, "Wrap");
            assert_eq!(name, "Carrier");
            assert_eq!(scope_id, sid);
            match &*value {
                KObject::KFunction(_, frame) => assert!(frame.is_some()),
                other => panic!("expected nested KFunction, got {:?}", other.ktype()),
            }
        }
        other => panic!("expected Tagged, got {:?}", other.ktype()),
    }
    assert_eq!(count_after, before + 1);
}
