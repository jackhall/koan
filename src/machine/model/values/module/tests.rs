//! Behavioral coverage for the module store: the born door's round trip
//! ([`Module::alloc_at_child_scope`]) and the bump-hosted member maps read back through the
//! resulting borrow. The underlying erase-store / re-anchor UB shapes are pinned library-side in the
//! workgraph slate's born-door group; these run under plain `cargo test`. See
//! [`design/memory-model.md`](../../../../../design/memory-model.md).
use super::*;
use crate::builtins::test_support::{TestRun, type_name};
use crate::machine::core::{FrameStorageExt, program_storage, run_root_storage};
use std::ptr;
/// The child-scope borrow a module carries reads back co-located after the door's round trip, and
/// keeps reading back once a sibling allocation has appended to the same region.
#[test]
fn module_child_scope_reads_back_after_the_born_store() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let types = test_run.types();
    let draft = ModuleDraft::empty();
    let self_sig = types.signature(SigSchema::raw_self_sig(scope, &draft));
    let module = Module::alloc_at_child_scope("Test", scope, draft, self_sig);
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

/// The member map reads back by content through the born door: the draft's owned keys are re-homed
/// as bumped `&str` and the table lands in the same region as the value, so a probe built at the
/// read site (a shorter-lived `&str`) hits. Tree borrows is strict about reads through a borrow that
/// stayed live across a sibling allocation, so the probe runs after one.
#[test]
fn module_members_read_back_through_the_bumped_maps() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let types = test_run.types();
    let mut draft = ModuleDraft::empty();
    let member = types.intern(TypeNode::AbstractType {
        source: scope.id,
        name: type_name("Type", test_run.registries()),
        param_names: Vec::new(),
        nonce: None,
    });
    draft
        .type_members
        .insert(type_name("Type", test_run.registries()), member);
    let self_sig = types.signature(SigSchema::raw_self_sig(scope, &draft));
    let module = Module::alloc_at_child_scope("M", scope, draft, self_sig);
    let _other = region
        .brand()
        .alloc_scalar(crate::machine::model::Scalar::Number(1.0));
    assert_eq!(module.path, "M");
    let handle = module
        .type_members
        .get(&type_name("Type", test_run.registries()))
        .copied()
        .expect("the Type member was assembled into the map");
    match types.node(handle) {
        TypeNode::AbstractType { source, name, .. } => {
            assert_eq!(source, scope.id);
            assert_eq!(name, type_name("Type", test_run.registries()));
        }
        _ => panic!("expected an AbstractType member, got {handle:?}"),
    }
}

/// A bare module's self-sig is derived from its (here empty) body by [`SigSchema::raw_self_sig`]
/// before the value exists, so reading it back off the built module yields an empty interface.
#[test]
fn bare_module_self_sig_is_empty_after_raw_seal() {
    let program = program_storage();
    let region = run_root_storage();
    let test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    let types = test_run.types();
    let draft = ModuleDraft::empty();
    let self_sig = types.signature(SigSchema::raw_self_sig(scope, &draft));
    let module = Module::alloc_at_child_scope("Bare", scope, draft, self_sig);
    let sig = module.self_sig(types);
    assert!(sig.abstract_members.is_empty());
    assert!(sig.manifest_members.is_empty());
    assert!(sig.value_slots.is_empty());
}
