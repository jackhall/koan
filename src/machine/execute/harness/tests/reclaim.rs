//! Reclamation invariants: what a finalize walk leaves behind.
//!
//! Delivery at finalize makes reclamation unconditional. A producer's walk adopts its terminal into
//! every waiting edge's destination region, so nothing downstream needs the producer's slot or its
//! region afterwards and the slot reclaims behind the walk — no retention count to reach zero, no
//! cascade to run over the slots it owned. Two consequences ride on that and are pinned here: a
//! finished program hands back every slot it allocated, and no notify list that outlives a walk
//! names an edge that walk released.

use crate::builtins::test_support::TestRun;
use crate::machine::core::{program_storage, run_root_storage};

/// Unconditional reclamation, end to end: run a program with nested blocks and spawned sub-slots, then
/// confirm the slot store's free list holds every index it ever minted. Finalize is the only event
/// that ends a slot and it reclaims unconditionally, so quiescence means an entirely free store.
#[test]
fn a_finished_program_reclaims_every_slot() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let root = test_run.scope;
    let runtime = &mut test_run.runtime;

    let exprs = super::working_all(
        &program,
        "LET x = 1\n\
         LET y = 2\n\
         LET z = (LET a = 3)",
    );
    for (i, e) in exprs.into_iter().enumerate() {
        runtime.dispatch_in_scope(e, root, i + 1);
    }
    runtime.execute().expect("program should run");

    let minted = runtime.len();
    assert!(minted > 0, "the program allocated slots to reclaim");
    assert_eq!(
        runtime.scheduler().free_list_len(),
        minted,
        "every slot a finished program allocated is back on the free list",
    );
}

/// Stale-name canary. A consumer releases each dep edge as soon as it has read the resident off it,
/// and a released edge's index is re-mintable — so a notify list that still named one would misfire
/// the next producer's delivery onto whatever took that index. Inv-C makes the walk (or the splice)
/// the one place a listed edge's name is dropped; this asserts nothing slipped past it.
#[test]
fn no_notify_list_names_a_released_edge() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let root = test_run.scope;
    let runtime = &mut test_run.runtime;

    let exprs = super::working_all(
        &program,
        "LET x = 1\n\
         LET y = 2\n\
         LET z = (LET a = 3)",
    );
    for (i, e) in exprs.into_iter().enumerate() {
        runtime.dispatch_in_scope(e, root, i + 1);
    }
    runtime.execute().expect("program should run");

    for (producer, listed) in runtime.scheduler().notify_list_iter() {
        for &edge in listed {
            assert!(
                !runtime.scheduler().edge_is_free(edge),
                "stale notify entry = slot {producer:?} still lists released edge {edge:?}",
            );
        }
    }
}
