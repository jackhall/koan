//! Miri coverage for the unsafe sites: `*const Scope<'static>` lifetime-erasure
//! transmutes and `type_members` `RefCell` mutation under a held `&'a Module<'a>`
//! borrow. Each shape is exercised in isolation so a regression attributes to a
//! single site. See [`design/memory-model.md`](../../../../../design/memory-model.md).
use super::*;
use crate::builtins::default_scope;
use crate::machine::core::FrameStorage;
use crate::machine::model::types::{AbstractSource, KType};
use std::io::sink;
use std::ptr;
#[test]
fn module_child_scope_transmute_does_not_dangle() {
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(sink()));
    let module = region.region().alloc_module(Module::new("Test".into(), scope));
    let recovered = module.child_scope();
    assert!(ptr::eq(recovered, scope));
    // Re-borrow after a sibling alloc — tree borrows is sensitive to interleaved
    // mutation under live shared borrows.
    let _other = region
        .region()
        .alloc_object(crate::machine::model::values::KObject::Number(1.0));
    let recovered2 = module.child_scope();
    assert!(ptr::eq(recovered2, scope));
}

/// Covered independently of the module path because `ModuleSignature` lives on a different
/// sub-arena (`signatures`) — a regression in `alloc_signature` or `decl_scope` must
/// surface without the module path masking it.
#[test]
fn signature_decl_scope_transmute_does_not_dangle() {
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(sink()));
    let sig = region
        .region()
        .alloc_signature(ModuleSignature::new("OrderedSig".into(), scope));
    let recovered = sig.decl_scope();
    assert!(ptr::eq(recovered, scope));
    let _other = region
        .region()
        .alloc_object(crate::machine::model::values::KObject::Number(1.0));
    let recovered2 = sig.decl_scope();
    assert!(ptr::eq(recovered2, scope));
}

/// Opaque ascription mutates `type_members` after the surrounding `KObject` is alloc'd,
/// so the `&'a Module<'a>` borrow is live across the `borrow_mut` + insert. Tree
/// borrows is strict about interior mutation under a live shared borrow.
#[test]
fn module_type_members_refcell_mutation_with_held_module_ref() {
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(sink()));
    let module = region.region().alloc_module(Module::new("M".into(), scope));
    let scope_id = module.scope_id();
    {
        let mut tm = module.type_members.borrow_mut();
        tm.insert(
            "Type".into(),
            KType::AbstractType {
                source: AbstractSource::Module(module),
                name: "Type".into(),
            },
        );
    }
    let bound = module.type_members.borrow().get("Type").cloned();
    assert!(matches!(
        &bound,
        Some(KType::AbstractType { source, name })
            if source.scope_id() == scope_id && name == "Type"
    ));
}

/// `slot_type_tags` mutates after the surrounding `KObject` is alloc'd, same as
/// `type_members`: the `&'a Module<'a>` borrow is live across the `borrow_mut` +
/// insert, and tree borrows is strict about interior mutation under a live shared
/// borrow. Pinned independently so a regression attributes to this map's site.
#[test]
fn module_slot_type_tags_refcell_mutation_with_held_module_ref() {
    let region = FrameStorage::run_root();
    let scope = default_scope(&region, Box::new(sink()));
    let module = region.region().alloc_module(Module::new("M".into(), scope));
    let scope_id = module.scope_id();
    {
        let mut tags = module.slot_type_tags.borrow_mut();
        tags.insert(
            "zero".into(),
            KType::AbstractType {
                source: AbstractSource::Module(module),
                name: "Type".into(),
            },
        );
    }
    let bound = module.slot_type_tags.borrow().get("zero").cloned();
    assert!(matches!(
        &bound,
        Some(KType::AbstractType { source, name })
            if source.scope_id() == scope_id && name == "Type"
    ));
}

/// Build a `KTypeValue(KType::Module { module, frame })` whose `child_scope` lives in
/// a `CallFrame`, lift it against the dying frame, and assert the lifted carrier
/// carries the region anchor. Pins the unsafe site behind functor execution end-to-end.
#[test]
fn functor_per_call_module_lifts_correctly() {
    use crate::machine::core::kfunction::{Body, KFunction};
    use crate::machine::core::{CallFrame, KoanRegion as RA};
    use crate::machine::execute::lift_ktype_for_test;
    use crate::machine::model::types::{ExpressionSignature, KType, ReturnType, SignatureElement};
    use crate::machine::model::values::KObject;
    use std::rc::Rc;

    let outer_region = FrameStorage::run_root();
    let outer_scope = default_scope(&outer_region, Box::new(sink()));
    let frame: Rc<CallFrame> = CallFrame::new_test(outer_scope, None);

    // Borrow into the per-call region via raw-pointer roundtrip so the borrow doesn't
    // outlive `frame` for the borrow-checker (the SAFETY invariant on `CallFrame` —
    // region heap address is stable for the Rc's life — backs this).
    let region_ptr: *const RA = frame.region();
    let inner_region: &RA = unsafe { &*region_ptr };

    // Defeat `functions_is_empty()`'s fast path so the slow lift path runs.
    let kf = KFunction::new(
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Null),
            elements: vec![SignatureElement::Keyword("__SLOW__".into())],
        },
        Body::Builtin(|ctx| {
            crate::machine::core::kfunction::action::Action::Done(Ok(
                crate::machine::model::Carried::Object(
                    ctx.scope.region.alloc_object(KObject::Null),
                ),
            ))
        }),
        frame.scope(),
    );
    let _ = inner_region.alloc_function(kf);

    // Module's `child_scope` lives in `inner_region` — exactly the shape a functor
    // body's `MODULE Generated = (...)` produces. Lift must observe the region match.
    let inner_scope = inner_region.alloc_scope(crate::machine::core::Scope::child_under_module(
        frame.scope(),
        "Inner".into(),
    ));
    let module = inner_region.alloc_module(Module::new("Inner".into(), inner_scope));
    let m_type = KType::Module {
        module,
        frame: None,
    };

    let strong_before = Rc::strong_count(&frame.storage_rc());
    let lifted = lift_ktype_for_test(&m_type, &frame);
    match &lifted {
        KType::Module { frame: anchor, .. } => assert!(
            anchor.is_some(),
            "Module carrier whose child scope lives in the dying region must lift with frame=Some(rc)",
        ),
        other => panic!("expected lifted Module carrier, got {}", other.name()),
    }
    assert_eq!(
        Rc::strong_count(&frame.storage_rc()),
        strong_before + 1,
        "lifting a per-frame module must clone the dying frame's storage Rc once",
    );
    // Drop borrowers before `frame` so region teardown order is well-defined.
    drop(lifted);
    drop(m_type);
}
