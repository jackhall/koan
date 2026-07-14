//! Miri coverage for the unsafe sites: `*const Scope<'static>` lifetime-erasure
//! transmutes and `type_members` `RefCell` mutation under a held `&'a Module<'a>`
//! borrow. Each shape is exercised in isolation so a regression attributes to a
//! single site. See [`design/memory-model.md`](../../../../../design/memory-model.md).
use super::*;
use crate::builtins::default_scope;
use crate::machine::core::{run_root_storage, FrameStorageExt};
use crate::machine::model::types::KType;
use std::io::sink;
use std::ptr;
#[test]
fn module_child_scope_transmute_does_not_dangle() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(sink()));
    let module = region
        .brand()
        .alloc_module(Module::new("Test".into(), scope));
    let recovered = module.child_scope();
    assert!(ptr::eq(recovered, scope));
    // Re-borrow after a sibling alloc — tree borrows is sensitive to interleaved
    // mutation under live shared borrows.
    let _other = region
        .brand()
        .alloc_object(crate::machine::model::values::KObject::Number(1.0));
    let recovered2 = module.child_scope();
    assert!(ptr::eq(recovered2, scope));
}

/// Covered independently of the module path because `ModuleSignature` lives on a different
/// sub-arena (`signatures`) — a regression in `alloc_signature` or `decl_scope` must
/// surface without the module path masking it.
#[test]
fn signature_decl_scope_transmute_does_not_dangle() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(sink()));
    let sig = region
        .brand()
        .alloc_signature(ModuleSignature::new("Ordered".into(), scope));
    let recovered = sig.decl_scope();
    assert!(ptr::eq(recovered, scope));
    let _other = region
        .brand()
        .alloc_object(crate::machine::model::values::KObject::Number(1.0));
    let recovered2 = sig.decl_scope();
    assert!(ptr::eq(recovered2, scope));
}

/// Opaque ascription mutates `type_members` after the surrounding `KObject` is alloc'd,
/// so the `&'a Module<'a>` borrow is live across the `borrow_mut` + insert. Tree
/// borrows is strict about interior mutation under a live shared borrow.
#[test]
fn module_type_members_refcell_mutation_with_held_module_ref() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(sink()));
    let module = region.brand().alloc_module(Module::new("M".into(), scope));
    let scope_id = module.scope_id();
    {
        let mut tm = module.type_members.borrow_mut();
        tm.insert(
            "Type".into(),
            KType::AbstractType {
                source: module.scope_id(),
                name: "Type".into(),
            },
        );
    }
    let bound = module.type_members.borrow().get("Type").cloned();
    assert!(matches!(
        &bound,
        Some(KType::AbstractType { source, name })
            if *source == scope_id && name == "Type"
    ));
}

/// `slot_type_tags` mutates after the surrounding `KObject` is alloc'd, same as
/// `type_members`: the `&'a Module<'a>` borrow is live across the `borrow_mut` +
/// insert, and tree borrows is strict about interior mutation under a live shared
/// borrow. Pinned independently so a regression attributes to this map's site.
#[test]
fn module_slot_type_tags_refcell_mutation_with_held_module_ref() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(sink()));
    let module = region.brand().alloc_module(Module::new("M".into(), scope));
    let scope_id = module.scope_id();
    {
        let mut tags = module.slot_type_tags.borrow_mut();
        tags.insert(
            "zero".into(),
            KType::AbstractType {
                source: module.scope_id(),
                name: "Type".into(),
            },
        );
    }
    let bound = module.slot_type_tags.borrow().get("zero").cloned();
    assert!(matches!(
        &bound,
        Some(KType::AbstractType { source, name })
            if *source == scope_id && name == "Type"
    ));
}

/// A bare `Module::new` never sealed still answers `self_sig()` — the accessor lazily derives
/// the schema from the (here empty) body via the fallback, so direct constructions in tests
/// need no explicit seal.
#[test]
fn bare_module_self_sig_falls_back_to_raw_derivation() {
    let region = run_root_storage();
    let scope = default_scope(&region, Box::new(sink()));
    let module = region
        .brand()
        .alloc_module(Module::new("Bare".into(), scope));
    let sig = module.self_sig();
    assert!(sig.abstract_members.is_empty());
    assert!(sig.manifest_members.is_empty());
    assert!(sig.value_slots.is_empty());
}
