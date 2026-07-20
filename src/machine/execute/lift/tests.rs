//! Tests for [`copy_carried`] — the witnessed-transfer copy hook. It structurally copies a
//! [`Carried`] into a destination region: the top node is re-allocated there, while the composite
//! spine shares its `Rc` payloads and a `KFunction` / first-class `Module` rides a bare
//! borrow preserved verbatim. No region anchor is embedded in the value — the regions a copied
//! value reaches are pinned by the carrier's witness set at the `transfer_into` layer, not here.

use super::*;
use crate::builtins::default_scope;
use crate::machine::core::{run_root_storage, FoldingBrand, KoanRegionExt};
use crate::machine::model::Held;
use crate::machine::model::KType;
use crate::machine::model::Record;
use crate::machine::model::{Carried, KObject};
use crate::machine::CallFrame;
use crate::witnessed::FoldedPlacement;
use std::rc::Rc;

/// A `KFunction` allocated into `home`'s region (its captured scope lives there), for the
/// borrow-preservation tests. The body is never run.
fn alloc_local_kf<'run>(home: &'run Rc<CallFrame>) -> &'run crate::machine::KFunction<'run> {
    use crate::machine::model::{ExpressionSignature, ReturnType, SignatureElement};
    use crate::machine::{Body, KFunction};
    // Capture the home frame's child scope (read at the brand), build the function there, and alloc it
    // into `home`'s region — where the captured scope genuinely lives — inside the open, so the re-homed
    // `&KFunction` escapes at `home`'s lifetime without a fixed-lifetime reattach. Mirrors a closure
    // capturing its defining scope in its own region.
    home.with_scope(|child| {
        let kf = KFunction::new(
            ExpressionSignature {
                return_type: ReturnType::Resolved(KType::Null),
                elements: vec![SignatureElement::Keyword("__INNER__".into())],
            },
            Body::Builtin(|ctx| {
                crate::machine::core::Action::done_resident(Carried::Object(
                    ctx.scope.brand().alloc_object(KObject::Null),
                ))
            }),
            child,
            None,
            None,
        );
        home.brand().alloc_function(kf)
    })
}

/// The top node of a relocated `Carried::Object` is a fresh allocation owned by `dest`, not the
/// source — that relocation (so the copy outlives the producer's dying frame) is the whole point.
#[test]
fn object_top_node_relocates_into_dest() {
    let root = run_root_storage();
    let scope = default_scope(&root, Box::new(std::io::sink()));
    let source = CallFrame::new(scope);
    let dest = CallFrame::new(scope);

    let obj: &KObject = source.brand().alloc_object(KObject::Number(2.5));
    let relocated = copy_carried(
        Carried::Object(obj),
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(dest.brand().handle())),
    );
    match relocated {
        Carried::Object(r) => {
            assert!(dest.region().owns_object(r), "relocated node lives in dest");
            assert!(
                !std::ptr::eq(r, obj),
                "top node is a fresh allocation, not the source"
            );
            assert!(
                matches!(r, KObject::Number(n) if *n == 2.5),
                "value preserved"
            );
        }
        Carried::Type(_) => panic!("expected an Object carrier"),
    }
}

/// A `List`'s inner `Rc<Vec<_>>` spine is shared, not deep-copied: relocating copies only the top
/// node, so the relocated list points at the same items allocation.
#[test]
fn list_relocation_shares_inner_rc() {
    let root = run_root_storage();
    let scope = default_scope(&root, Box::new(std::io::sink()));
    let source = CallFrame::new(scope);
    let dest = CallFrame::new(scope);

    let items = Rc::new(vec![
        Held::Object(KObject::Number(1.0)),
        Held::Object(KObject::Number(2.0)),
    ]);
    let list: &KObject = source
        .brand()
        .alloc_object_checked(KObject::list_with_type(Rc::clone(&items), KType::Any))
        .expect("a fresh owned List is always resident-in-self");
    let before = Rc::strong_count(&items);

    let relocated = copy_carried(
        Carried::Object(list),
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(dest.brand().handle())),
    );
    match relocated {
        Carried::Object(r @ KObject::List(out, _)) => {
            assert!(
                dest.region().owns_object(r),
                "relocated list node lives in dest"
            );
            assert!(
                Rc::ptr_eq(out, &items),
                "the items spine is shared, not copied"
            );
        }
        Carried::Object(other) => panic!("expected a List, got {:?}", other.ktype()),
        Carried::Type(_) => panic!("expected an Object carrier"),
    }
    assert_eq!(
        Rc::strong_count(&items),
        before + 1,
        "sharing bumps the Rc by one"
    );
}

/// A `Dict`'s inner `Rc<HashMap<_>>` is likewise shared through relocation.
#[test]
fn dict_relocation_shares_inner_rc() {
    use crate::machine::model::KKey;
    use std::collections::HashMap;
    let root = run_root_storage();
    let scope = default_scope(&root, Box::new(std::io::sink()));
    let source = CallFrame::new(scope);
    let dest = CallFrame::new(scope);

    let mut map: HashMap<KKey, Held> = HashMap::new();
    map.insert(KKey::String("a".into()), Held::Object(KObject::Number(1.0)));
    let entries = Rc::new(map);
    let dict: &KObject = source
        .brand()
        .alloc_object_checked(KObject::dict_with_type(
            Rc::clone(&entries),
            KType::Any,
            KType::Any,
        ))
        .expect("a fresh owned Dict is always resident-in-self");

    let relocated = copy_carried(
        Carried::Object(dict),
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(dest.brand().handle())),
    );
    match relocated {
        Carried::Object(r @ KObject::Dict(out, _, _)) => {
            assert!(
                dest.region().owns_object(r),
                "relocated dict node lives in dest"
            );
            assert!(
                Rc::ptr_eq(out, &entries),
                "the entries map is shared, not copied"
            );
        }
        Carried::Object(other) => panic!("expected a Dict, got {:?}", other.ktype()),
        Carried::Type(_) => panic!("expected an Object carrier"),
    }
}

/// A `Tagged` shares both its `value` and its `RecursiveSet` `Rc` through relocation, and the tag
/// rides along unchanged.
#[test]
fn tagged_relocation_shares_value_and_set_rc() {
    use crate::machine::model::{NominalSchema, RecursiveSet};
    use crate::machine::ScopeId;
    let root = run_root_storage();
    let scope = default_scope(&root, Box::new(std::io::sink()));
    let source = CallFrame::new(scope);
    let dest = CallFrame::new(scope);

    let inner = Rc::new(KObject::Number(42.0));
    let set = RecursiveSet::singleton(
        "Maybe".into(),
        ScopeId::next(),
        NominalSchema::TypeConstructor {
            schema: std::collections::HashMap::new(),
            param_names: Vec::new(),
        },
    );
    let tagged: &KObject = source
        .brand()
        .alloc_object_checked(KObject::Tagged {
            tag: "Just".into(),
            value: Rc::clone(&inner),
            set: Rc::clone(&set),
            index: 0,
            type_args: Rc::new(Record::new()),
        })
        .expect("a fresh owned Tagged is always resident-in-self");

    let relocated = copy_carried(
        Carried::Object(tagged),
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(dest.brand().handle())),
    );
    match relocated {
        Carried::Object(
            r @ KObject::Tagged {
                tag,
                value,
                set: out_set,
                ..
            },
        ) => {
            assert!(
                dest.region().owns_object(r),
                "relocated tagged node lives in dest"
            );
            assert_eq!(tag, "Just");
            assert!(Rc::ptr_eq(value, &inner), "the wrapped value is shared");
            assert!(Rc::ptr_eq(out_set, &set), "the RecursiveSet is shared");
        }
        Carried::Object(other) => panic!("expected a Tagged, got {:?}", other.ktype()),
        Carried::Type(_) => panic!("expected an Object carrier"),
    }
}

/// A `KFunction` rides a *bare* borrow preserved verbatim — relocation copies the reference, never
/// the closure (which may reference anything reachable from its captured scope). The borrow points
/// back into the source region; the carrier's witness set keeps that region alive.
#[test]
fn kfunction_borrow_preserved_verbatim() {
    let root = run_root_storage();
    let scope = default_scope(&root, Box::new(std::io::sink()));
    let source = CallFrame::new(scope);
    let dest = CallFrame::new(scope);

    let kf_ref = alloc_local_kf(&source);
    let obj: &KObject = source
        .brand()
        .alloc_object_checked(KObject::KFunction(kf_ref))
        .expect("f was just allocated into region\'s own region");

    let relocated = copy_carried(
        Carried::Object(obj),
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(dest.brand().handle())),
    );
    match relocated {
        Carried::Object(r @ KObject::KFunction(f)) => {
            assert!(
                dest.region().owns_object(r),
                "the KFunction node relocated into dest"
            );
            assert!(
                std::ptr::eq(*f, kf_ref),
                "the function borrow is preserved verbatim"
            );
        }
        Carried::Object(other) => panic!("expected a KFunction, got {:?}", other.ktype()),
        Carried::Type(_) => panic!("expected an Object carrier"),
    }
}

/// A recursive `SetRef` *type* value (a self-recursive newtype) relocates by sharing the whole
/// `RecursiveSet` `Rc` — no copy — and stays navigable afterward: the member's self-edge
/// `SetLocal(0)` still resolves back through the relocated set. Guards against a type value
/// escaping the region that built it with a dangling self-reference (cf. `recursive_tagged_match`).
#[test]
fn type_recursive_setref_relocates_and_navigates() {
    use crate::machine::model::{NominalSchema, Record, RecursiveSet};
    use crate::machine::ScopeId;
    let root = run_root_storage();
    let scope = default_scope(&root, Box::new(std::io::sink()));
    let dest = CallFrame::new(scope);

    // A self-recursive `Tree` whose `children` field is `List(SetLocal(0))` — the shape a
    // `NEWTYPE Tree = :{children :(LIST OF Tree)}` seals into.
    let set = RecursiveSet::singleton(
        "Tree".into(),
        ScopeId::next(),
        NominalSchema::NewType(Box::new(KType::record(Box::new(Record::from_pairs(vec![
            ("children".into(), KType::list(Box::new(KType::SetLocal(0)))),
        ]))))),
    );
    let type_value = KType::SetRef {
        set: Rc::clone(&set),
        index: 0,
    };
    let before = Rc::strong_count(&set);

    let relocated = copy_carried(
        Carried::Type(&type_value),
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(dest.brand().handle())),
    );
    assert_eq!(
        Rc::strong_count(&set),
        before + 1,
        "the set travels by Rc::clone"
    );
    match relocated {
        Carried::Type(KType::SetRef {
            set: out_set,
            index,
        }) => {
            assert!(
                Rc::ptr_eq(out_set, &set),
                "lift shares the same RecursiveSet allocation"
            );
            // Navigable: the member's self-edge `SetLocal(0)` survives the relocation.
            let borrow = out_set.member(*index).schema();
            match borrow.as_ref() {
                Some(NominalSchema::NewType(repr)) => match repr.as_ref() {
                    KType::Record { fields, .. } => assert_eq!(
                        fields.get("children"),
                        Some(&KType::list(Box::new(KType::SetLocal(0)))),
                        "the relocated Tree's self-reference is still SetLocal(0)",
                    ),
                    other => panic!("expected a record repr, got {other:?}"),
                },
                other => panic!("expected a navigable NewType schema, got {other:?}"),
            }
        }
        Carried::Type(other) => panic!("expected a SetRef type, got {other:?}"),
        Carried::Object(_) => panic!("expected a Type carrier"),
    }
}
