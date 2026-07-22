//! Tests for [`copy_carried`] — the witnessed-transfer copy hook. It structurally copies a
//! [`Carried`] into a destination region: the top node is re-allocated there, while the composite
//! spine shares its `Rc` payloads and a `KFunction` / first-class `Module` rides a bare
//! borrow preserved verbatim. No region anchor is embedded in the value — the regions a copied
//! value reaches are pinned by the carrier's witness set at the `transfer_into` layer, not here.

use super::*;
use crate::builtins::test_support::TestRun;
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
    let types = crate::machine::model::TypeRegistry::new();
    home.with_scope(|child| {
        let kf = KFunction::new(
            ExpressionSignature {
                return_type: ReturnType::Resolved(KType::NULL),
                elements: vec![SignatureElement::Keyword("__INNER__".into())],
            },
            Body::Builtin(|ctx| {
                crate::machine::core::Action::done_resident(Carried::Object(
                    ctx.scope.brand().alloc_object(KObject::Null),
                ))
            }),
            child,
            false,
            &types,
        );
        home.brand().alloc_function(kf)
    })
}

/// The top node of a relocated `Carried::Object` is a fresh allocation owned by `dest`, not the
/// source — that relocation (so the copy outlives the producer's dying frame) is the whole point.
#[test]
fn object_top_node_relocates_into_dest() {
    let root = run_root_storage();
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
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
        Carried::Type(_) | Carried::UnresolvedType(_) => panic!("expected an Object carrier"),
    }
}

/// A `List`'s inner `Rc<Vec<_>>` spine is shared, not deep-copied: relocating copies only the top
/// node, so the relocated list points at the same items allocation.
#[test]
fn list_relocation_shares_inner_rc() {
    let root = run_root_storage();
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
    let source = CallFrame::new(scope);
    let dest = CallFrame::new(scope);
    let types = test_run.types.clone();

    let items = Rc::new(vec![
        Held::Object(KObject::Number(1.0)),
        Held::Object(KObject::Number(2.0)),
    ]);
    let list: &KObject = source
        .brand()
        .alloc_object_checked(
            KObject::list_with_type(Rc::clone(&items), KType::LIST_OF_ANY),
            &types,
        )
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
        Carried::Type(_) | Carried::UnresolvedType(_) => panic!("expected an Object carrier"),
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
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
    let source = CallFrame::new(scope);
    let dest = CallFrame::new(scope);
    let types = test_run.types.clone();

    let mut map: HashMap<KKey, Held> = HashMap::new();
    map.insert(KKey::String("a".into()), Held::Object(KObject::Number(1.0)));
    let entries = Rc::new(map);
    let dict: &KObject = source
        .brand()
        .alloc_object_checked(
            KObject::dict_with_type(Rc::clone(&entries), KType::DICT_ANY_ANY),
            &types,
        )
        .expect("a fresh owned Dict is always resident-in-self");

    let relocated = copy_carried(
        Carried::Object(dict),
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(dest.brand().handle())),
    );
    match relocated {
        Carried::Object(r @ KObject::Dict(out, _)) => {
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
        Carried::Type(_) | Carried::UnresolvedType(_) => panic!("expected an Object carrier"),
    }
}

/// A `Tagged` shares its `value` `Rc` through relocation, and its tag and interned `identity` type
/// handle ride along unchanged.
#[test]
fn tagged_relocation_shares_value_and_identity() {
    use crate::machine::core::ScopeId;
    use crate::machine::model::TypeNode;
    let root = run_root_storage();
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
    let source = CallFrame::new(scope);
    let dest = CallFrame::new(scope);
    let types = test_run.types.clone();

    let inner = Rc::new(KObject::Number(42.0));
    // The value's own type handle: a `Maybe` constructor applied to `Number` — the shape a tagged
    // union member's `identity` interns to.
    let ctor = types.intern(TypeNode::AbstractType {
        source: ScopeId::from_raw(0, 0x11),
        name: "Maybe".into(),
        param_names: vec!["T".into()],
        nonce: None,
    });
    let identity =
        types.constructor_apply(ctor, Record::from_pairs([("T".to_string(), KType::NUMBER)]));
    let tagged: &KObject = source
        .brand()
        .alloc_object_checked(
            KObject::Tagged {
                tag: "Just".into(),
                value: Rc::clone(&inner),
                identity,
            },
            &types,
        )
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
                identity: out_identity,
            },
        ) => {
            assert!(
                dest.region().owns_object(r),
                "relocated tagged node lives in dest"
            );
            assert_eq!(tag, "Just");
            assert!(Rc::ptr_eq(value, &inner), "the wrapped value is shared");
            assert_eq!(
                *out_identity, identity,
                "the identity handle rides along unchanged"
            );
        }
        Carried::Object(other) => panic!("expected a Tagged, got {:?}", other.ktype()),
        Carried::Type(_) | Carried::UnresolvedType(_) => panic!("expected an Object carrier"),
    }
}

/// A `KFunction` rides a *bare* borrow preserved verbatim — relocation copies the reference, never
/// the closure (which may reference anything reachable from its captured scope). The borrow points
/// back into the source region; the carrier's witness set keeps that region alive.
#[test]
fn kfunction_borrow_preserved_verbatim() {
    let root = run_root_storage();
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
    let source = CallFrame::new(scope);
    let dest = CallFrame::new(scope);
    let types = test_run.types.clone();

    let kf_ref = alloc_local_kf(&source);
    let obj: &KObject = source
        .brand()
        .alloc_object_checked(KObject::KFunction(kf_ref), &types)
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
        Carried::Type(_) | Carried::UnresolvedType(_) => panic!("expected an Object carrier"),
    }
}

/// A recursive newtype's sealed member *type* handle relocates by copying its digest, and stays
/// navigable afterward: reading the relocated handle back through the registry still finds the
/// member's `children` field self-referencing the sealed `Tree` member. Guards against a relocated
/// type value losing its recursive self-edge.
#[test]
fn type_recursive_member_relocates_and_navigates() {
    use crate::machine::model::{NodeSchema, RecursiveGroupWindow, RelativeSchema, TypeNode};
    let root = run_root_storage();
    let test_run = TestRun::silent(&root);
    let scope = test_run.scope;
    let dest = CallFrame::new(scope);
    let types = crate::machine::model::TypeRegistry::new();

    // A self-recursive `Tree` whose `children` field is `List(Tree)` — the shape a
    // `NEWTYPE Tree = :{children :(LIST OF Tree)}` seals into. The self-edge starts as `Sibling(0)`
    // and seals to the member's own absolute handle.
    let tree = RecursiveGroupWindow::seal_singleton(
        "Tree".into(),
        RelativeSchema::NewType(types.record(Record::from_pairs([(
            "children".to_string(),
            types.list(types.intern(TypeNode::Sibling(0))),
        )]))),
        None,
        &types,
    );
    let type_value = tree;

    let relocated = copy_carried(
        Carried::Type(type_value),
        FoldingBrand::in_fold_closure(FoldedPlacement::forge_for_test(dest.brand().handle())),
    );
    match relocated {
        Carried::Type(out) => {
            assert_eq!(
                out, tree,
                "relocation copies the member's digest handle unchanged"
            );
            // Navigable: reading the relocated handle back finds the member's `children` field
            // self-referencing the sealed `Tree` member.
            match types.node(out) {
                TypeNode::SetMember {
                    schema: NodeSchema::NewType(repr),
                    ..
                } => match types.node(repr) {
                    TypeNode::Record { fields } => assert_eq!(
                        fields.get("children"),
                        Some(&types.list(tree)),
                        "the relocated Tree's children field self-references the sealed Tree member",
                    ),
                    _ => panic!("expected a record repr, got {}", repr.name(&types)),
                },
                _ => panic!("expected a navigable NewType member, got {}", out.name(&types)),
            }
        }
        Carried::UnresolvedType(ti) => {
            panic!(
                "expected a member type, got the unlowered name {}",
                ti.render()
            )
        }
        Carried::Object(_) => panic!("expected a Type carrier"),
    }
}
