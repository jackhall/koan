//! Behavioral coverage for the module store: the born door's round trip
//! ([`Module::alloc_at_child_scope`]) and the `RefCell` maps' mutation under a held
//! `&'a Module<'a>` borrow. The underlying erase-store / re-anchor and
//! interior-write-through-the-re-anchor UB shapes are pinned library-side in the workgraph slate's
//! born-door group; these run under plain `cargo test`. See
//! [`design/memory-model.md`](../../../../../design/memory-model.md).
use super::*;
use crate::builtins::test_support::TestRun;
use crate::machine::core::{program_storage, run_root_storage, FrameStorageExt};
use std::ptr;
/// The child-scope borrow a module carries reads back co-located after the door's round trip, and
/// keeps reading back once a sibling allocation has appended to the same region.
#[test]
fn module_child_scope_reads_back_after_the_born_store() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let module = Module::alloc_at_child_scope("Test".into(), scope);
    let recovered = module.child_scope();
    assert!(ptr::eq(recovered, scope));
    // Re-borrow after a sibling alloc — tree borrows is sensitive to interleaved
    // mutation under live shared borrows.
    let _other = region
        .brand()
        .alloc_scalar(crate::machine::model::Scalar::Number(1.0));
    let recovered2 = module.child_scope();
    assert!(ptr::eq(recovered2, scope));
}

/// Opaque ascription mutates `type_members` after the surrounding `KObject` is alloc'd,
/// so the `&'a Module<'a>` borrow is live across the `borrow_mut` + insert. Tree
/// borrows is strict about interior mutation under a live shared borrow.
#[test]
fn module_type_members_refcell_mutation_with_held_module_ref() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let types = &test_run.types;
    let module = Module::alloc_at_child_scope("M".into(), scope);
    let scope_id = module.scope_id();
    {
        let mut tm = module.type_members.borrow_mut();
        tm.insert(
            "Type".into(),
            types.intern(TypeNode::AbstractType {
                source: module.scope_id(),
                name: "Type".into(),
                param_names: Vec::new(),
                nonce: None,
            }),
        );
    }
    let handle = module
        .type_members
        .borrow()
        .get("Type")
        .copied()
        .expect("the Type member was just inserted");
    match types.node(handle) {
        TypeNode::AbstractType { source, name, .. } => {
            assert_eq!(source, scope_id);
            assert_eq!(name.as_str(), "Type");
        }
        _ => panic!("expected an AbstractType member, got {handle:?}"),
    }
}

/// `slot_type_tags` mutates after the surrounding `KObject` is alloc'd, same as
/// `type_members`: the `&'a Module<'a>` borrow is live across the `borrow_mut` +
/// insert, and tree borrows is strict about interior mutation under a live shared
/// borrow. Pinned independently so a regression attributes to this map's site.
#[test]
fn module_slot_type_tags_refcell_mutation_with_held_module_ref() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let types = &test_run.types;
    let module = Module::alloc_at_child_scope("M".into(), scope);
    let scope_id = module.scope_id();
    {
        let mut tags = module.slot_type_tags.borrow_mut();
        tags.insert(
            "zero".into(),
            types.intern(TypeNode::AbstractType {
                source: module.scope_id(),
                name: "Type".into(),
                param_names: Vec::new(),
                nonce: None,
            }),
        );
    }
    let handle = module
        .slot_type_tags
        .borrow()
        .get("zero")
        .copied()
        .expect("the zero tag was just inserted");
    match types.node(handle) {
        TypeNode::AbstractType { source, name, .. } => {
            assert_eq!(source, scope_id);
            assert_eq!(name.as_str(), "Type");
        }
        _ => panic!("expected an AbstractType tag, got {handle:?}"),
    }
}

/// A bare module's self-sig is derived from its (here empty) body by [`SigSchema::raw_self_sig`]
/// and sealed at mint, so reading it back through the sealed cell yields an empty interface.
#[test]
fn bare_module_self_sig_is_empty_after_raw_seal() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let types = &test_run.types;
    let module = Module::alloc_at_child_scope("Bare".into(), scope);
    module.seal_self_sig(SigSchema::raw_self_sig(module), types);
    let sig = module.self_sig(types);
    assert!(sig.abstract_members.is_empty());
    assert!(sig.manifest_members.is_empty());
    assert!(sig.value_slots.is_empty());
}
