//! Tests for [`finalize_terminal`](super::NodeFinalize::finalize_terminal)'s Done boundary: a
//! terminal seals **as-is** — the boundary makes no memory decision — and the producer frame's
//! lifetime rides the delivery envelope's own host `Rc`, which the walk holds while it adopts into
//! each destination and releases when the envelope drops. The [`Weak`] census is the direct probe — a released
//! frame's `FrameStorage` upgrades to `None` once the last strong holder drops.

use crate::builtins::test_support::probe_symbol;
use std::cell::RefCell;
use std::rc::{Rc, Weak};

use super::NodeFinalize;
use crate::builtins::test_support::{TestRun, run_root_bare};
use crate::machine::AdoptSeam;
use crate::machine::CallFrame;
use crate::machine::core::{Action, BodyCtx};
use crate::machine::core::{
    CarrierWitness, FrameCoverage, FrameStorage, program_storage, run_root_storage,
};
use crate::machine::model::Scalar;
use crate::machine::model::Symbol;
use crate::machine::model::{Carried, KObject, RunRegistries};
use crate::machine::model::{KType, ReturnType, SignatureDraft, SignatureElement};
use crate::witnessed::{Delivered, Sealed};

/// Build a scalar carrier residing in `producer`'s region whose borrows reach that region exactly
/// when `borrows_into_home` — the exact carrier a resident-value read hands to finalize. The
/// description is minted by [`Scope::mint_born_here`], so home is an ordinary member of it or absent
/// from it, never a bit beside it. Returns the carrier (lifetime-erased, so it escapes the frame's
/// rank-2 scope open) and a [`Weak`] to the producer's `FrameStorage` for the liveness census.
fn resident_scalar(
    producer: &Rc<CallFrame>,
    borrows_into_home: bool,
) -> (
    crate::witnessed::Witnessed<crate::machine::model::CarriedFamily, CarrierWitness>,
    Weak<FrameStorage>,
) {
    let carrier = producer.with_scope(|child| {
        let obj = child.brand().alloc_scalar(Scalar::Number(7.0));
        child
            .seal_reaching(
                Carried::Object(obj),
                child.mint_born_here(borrows_into_home),
            )
            .unseal()
    });
    let weak = Rc::downgrade(&producer.storage_rc());
    (carrier, weak)
}

/// A region-pure scalar terminal (empty reach) seals as-is at the Done boundary and rides the
/// envelope's own host pin: that `Rc` keeps the producer's storage — hence the value — alive across
/// the producer shell's drop, and dropping the envelope releases the frame. Frame release is a
/// function of deliveries only, never of the value's reach.
#[test]
fn region_pure_scalar_rides_the_envelope_and_releases_at_envelope_drop() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let scope = test_run.scope;
    let producer = CallFrame::new(scope);

    let (carrier, weak) = resident_scalar(&producer, false);
    let delivered = Delivered::lift(
        crate::witnessed::Retained::from_sealed(Sealed::seal(carrier, producer.brand().handle())),
        producer.storage_rc(),
    );
    assert!(
        !delivered.open_at().has_reach_members(),
        "a region-pure scalar's reach names nothing — the carrier itself pins nothing, so \
         liveness is the envelope's"
    );

    // The delivery seed: the producer's storage rides the envelope out of the boundary, exactly as
    // the run loop hands it to the scheduler at finalize.
    let envelope = test_run
        .runtime
        .host
        .finalize_terminal(delivered, &producer.storage_rc(), None)
        .expect("no declared return, no error");

    drop(producer);
    assert!(
        weak.upgrade().is_some(),
        "the envelope's host pin keeps the producer's storage alive across the shell drop"
    );
    envelope.open(|carried| match carried {
        Carried::Object(KObject::Number(n)) => assert_eq!(*n, 7.0, "value rides the envelope"),
        other => panic!(
            "expected the retained Number, got {:?}",
            other.ktype(test_run.types())
        ),
    });
    drop(envelope);
    assert!(
        weak.upgrade().is_none(),
        "dropping the envelope releases the frame — a delivery fact, not a reach fact"
    );
}

/// Retention-timeline acceptance (claim: *envelope pins at envelope drop*). A delivery envelope now
/// carries the terminal's owned **foreign** [`FrameCoverage`] bundle alongside the host frame `Rc`; the
/// bundle pins every region the value reaches and drops with the envelope. Seal an envelope whose
/// foreign bundle is the sole strong owner of a distinct region and confirm the region stays live
/// while the envelope lives, released when the envelope drops — the reach owned end-to-end, never
/// re-derived.
#[test]
fn delivery_envelope_foreign_bundle_releases_at_envelope_drop() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let producer = CallFrame::new(test_run.scope);
    // A distinct region the terminal reaches; the envelope's foreign bundle will be its sole owner.
    let foreign = run_root_storage();
    let weak = Rc::downgrade(&foreign);

    let (carrier, _producer_weak) = resident_scalar(&producer, false);
    let envelope = Delivered::seal_for_test(
        carrier,
        producer.storage_rc(),
        FrameCoverage::of(Rc::clone(&foreign)),
    );
    // The envelope's owned foreign bundle is now the sole strong owner of `foreign`.
    drop(foreign);
    assert!(
        weak.upgrade().is_some(),
        "the envelope's owned foreign bundle keeps the reached region alive"
    );
    drop(envelope);
    assert!(
        weak.upgrade().is_none(),
        "the foreign bundle releases when the envelope drops — owned to the envelope's life"
    );
}

/// A value that genuinely borrows into its producer frame carries that home membership through the
/// Done boundary unchanged — finalize seals as-is; membership is read only at a later copied re-home
/// mint, never as a lifecycle input. The frame's lifetime is the envelope's either way.
#[test]
fn home_borrowing_value_keeps_its_home_membership_and_rides_the_envelope() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let scope = test_run.scope;
    let producer = CallFrame::new(scope);

    let (carrier, weak) = resident_scalar(&producer, true);
    let delivered = Delivered::lift(
        crate::witnessed::Retained::from_sealed(Sealed::seal(carrier, producer.brand().handle())),
        producer.storage_rc(),
    );
    assert!(
        delivered.open_at().borrows_home(),
        "home is an ordinary member of the value's own description"
    );

    let envelope = test_run
        .runtime
        .host
        .finalize_terminal(delivered, &producer.storage_rc(), None)
        .expect("no declared return, no error");
    assert!(
        envelope.open_at().borrows_home(),
        "the membership survives the Done boundary verbatim"
    );

    drop(producer);
    assert!(
        weak.upgrade().is_some(),
        "the envelope's host pin — not the carrier — keeps the frame alive"
    );
    drop(envelope);
    assert!(
        weak.upgrade().is_none(),
        "dropping the envelope releases the frame"
    );
}

thread_local! {
    /// Per-thread census of every callee frame a [`probe_body`] call captured. Each test runs on its
    /// own thread, so the census is naturally isolated; a test clears it at entry for good measure and
    /// then asserts how many captured frames are still live after the run.
    static FRAME_CENSUS: RefCell<Vec<Weak<FrameStorage>>> = const { RefCell::new(Vec::new()) };
}

/// A test-only builtin `(PROBE)` — captures the frame its call runs in (its `region_owner`, downgraded
/// to a [`Weak`]) into [`FRAME_CENSUS`] and returns the region-pure scalar `1`. Registered inside a
/// user FN's body, it hands the test a handle to that FN's per-call frame so the run's frame lifetimes
/// become observable end-to-end.
fn probe_body<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    FRAME_CENSUS.with(|census| census.borrow_mut().push(ctx.scope.region_owner()));
    Action::done_resident(
        ctx.scope,
        Carried::Object(ctx.scope.brand().alloc_scalar(Scalar::Number(1.0))),
    )
}

/// Register `(PROBE)` — a nullary keyword builtin returning `Number` — into `scope`, against the
/// run's own registry.
fn register_probe<'a>(scope: &'a crate::machine::Scope<'a>, registries: &RunRegistries) {
    let signature = SignatureDraft {
        return_type: ReturnType::Resolved(KType::NUMBER),
        elements: vec![SignatureElement::Keyword(probe_symbol("PROBE"))],
    };
    crate::builtins::register_builtin(
        scope,
        signature,
        probe_body,
        registries,
        &mut crate::machine::WriteGate::for_test(),
    );
}

/// The number of captured frames still live — the retention census read.
fn live_frames() -> usize {
    FRAME_CENSUS.with(|census| {
        census
            .borrow()
            .iter()
            .filter(|weak| weak.upgrade().is_some())
            .count()
    })
}

/// End-to-end acceptance: a user FN returning a region-pure scalar releases its callee frame at call
/// end, not program end. The probe captures the callee frame from inside the body; after the call's
/// `Done` drains — with the root scope still very much alive — the captured frame is dead.
#[test]
fn user_fn_call_releases_callee_frame() {
    FRAME_CENSUS.with(|census| census.borrow_mut().clear());
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    register_probe(scope, test_run.registries());
    test_run.run("FN (GETONE) -> Number = (PROBE)");

    let result = test_run.run_one(test_run.parse_one("GETONE"));
    // The census reads frame *retention*, so release the drained slots that still hold their
    // terminals' producer frames; only a frame outliving the scheduler would survive this.
    test_run.reset_slots();
    assert!(
        matches!(result, KObject::Number(n) if *n == 1.0),
        "GETONE returns the probe's scalar"
    );
    assert_eq!(
        live_frames(),
        0,
        "GETONE's callee frame drops at call end while the root scope stays live"
    );
}

/// Acceptance retention measurement: a 100-element list literal over region-pure call results keeps
/// the aggregate live and readable while every one of the 100 producer frames is released.
/// **Measured retention: 100 callee frames minted → 0 live after the run.** Before the empty-reach
/// change each escaped scalar pinned its whole per-call arena for the program's life, so the census
/// would read 100 live here; the finalize sever drives it to 0. The producers are identical, so the
/// hundredth buys the audit nothing the fifth did not; the slate runs the five-call mixed twin below
/// and this width measurement stays under plain `cargo test`.
#[test]
fn aggregate_of_call_results_releases_every_producer_frame() {
    FRAME_CENSUS.with(|census| census.borrow_mut().clear());
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    register_probe(scope, test_run.registries());
    test_run.run("FN (GETONE) -> Number = (PROBE)");

    let calls = vec!["(GETONE)"; 100].join(" ");
    test_run.run(&format!("LET results = [{calls}]"));

    // The aggregate is live and complete...
    let results = test_run.run_one(test_run.parse_one("results"));
    match results {
        KObject::List(items, _) => {
            assert_eq!(items.elements().len(), 100, "all 100 results retained")
        }
        other => panic!("expected a 100-element List, got {:?}", other.ktype()),
    }
    // ...while every producer frame the 100 calls minted has dropped.
    let total = FRAME_CENSUS.with(|census| census.borrow().len());
    assert_eq!(total, 100, "each call captured its own callee frame");
    assert_eq!(
        live_frames(),
        0,
        "all 100 producer arenas released — the escaped scalars no longer pin them"
    );
}

/// The slate's dep-envelope census: five real scheduler steps, each producing into its own per-call
/// frame, aggregated by one consumer step that opens all five delivered deps at once. The cells
/// **mix** the two escape shapes the seam selects between — a region-pure scalar
/// (`Residence::Copied`, rebuilt into the aggregate's own region) and a plain-data record
/// (escape with **copy**: totally rebuilt by `copy_object_into` via `fold_cells`'s per-cell seam
/// selection, and because no field borrows anything, no run of the rebuilt record names its producer)
/// — so one run pins both verdicts against the same consumer open. Every producer arena is gone
/// while the aggregate still reads correctly: a use-after-free under tree borrows the moment the
/// redundancy claim is wrong, and a lifetime leak the moment a fold re-pins a producer it copied
/// out of.
// Pins the copy/release mechanism; the `seam-force-pin` build pins the record cells and retains
// their frames, so this cannot hold there. The equivalence battery proves language-output
// invisibility separately.
#[cfg(not(feature = "seam-force-pin"))]
#[test]
fn aggregate_of_mixed_call_results_releases_every_producer_frame() {
    FRAME_CENSUS.with(|census| census.borrow_mut().clear());
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    register_probe(scope, test_run.registries());
    test_run.run(
        "FN (GETONE) -> Number = (PROBE)\n\
         FN (GETREC) -> :{acc :Number, tag :Number} = ({acc = 1, tag = (PROBE)})",
    );

    const CALLS: usize = 5;
    test_run.run("LET results = [(GETONE) (GETREC) (GETONE) (GETREC) (GETONE)]");

    let results = test_run.run_one(test_run.parse_one("results"));
    match results {
        KObject::List(items, _) => {
            assert_eq!(items.elements().len(), CALLS, "all five results retained");
            for (i, item) in items.elements().iter().enumerate() {
                match (i % 2, item.object()) {
                    (0, KObject::Number(n)) => {
                        assert_eq!(*n, 1.0, "the scalar cell survives its producer's release")
                    }
                    (1, KObject::Record(substrate, _)) => {
                        match substrate.field(Symbol::of("acc")).map(|h| h.object()) {
                            Some(KObject::Number(n)) => {
                                assert_eq!(*n, 1.0, "the acc field survives the total copy")
                            }
                            _ => panic!("expected field acc: Number"),
                        }
                    }
                    (_, other) => panic!(
                        "cell {i}: expected an alternating scalar / record, got {}",
                        other.ktype().name(test_run.registries())
                    ),
                }
            }
        }
        other => panic!(
            "expected a {CALLS}-element List, got {}",
            other.ktype().name(test_run.registries())
        ),
    }
    let total = FRAME_CENSUS.with(|census| census.borrow().len());
    assert_eq!(total, CALLS, "each call captured its own producer frame");
    assert_eq!(
        live_frames(),
        0,
        "every producer arena released — the escaped scalars and the records' total copies pin none"
    );
}

/// `Scope::adopt_carried` at the retaining seam, on a delivered object: the value rides its retention
/// hold (the envelope's host) across the producer shell's drop, and the copy-free adoption
/// materializes that host into the consumer's arena — so after the envelope itself drops, the
/// consumer's minted set is the sole owner of the producer's storage and the adopted read stays live.
///
/// The envelope the adoption consumes is the one `finalize_terminal` returns, unsplit: value and
/// coverage travel as one delivery envelope from the Done boundary to the seam, so the ordering the
/// adoption depends on is the library verb's (`Delivered::adopt_into`) and not a pairing this call
/// site assembles.
#[test]
fn retaining_adopt_object_rides_retention_across_producer_shell_drop() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let scope = test_run.scope;
    let producer = CallFrame::new(scope);

    let (carrier, weak) = resident_scalar(&producer, false);
    let cell = test_run
        .runtime
        .host
        .finalize_terminal(
            Delivered::lift(
                crate::witnessed::Retained::from_sealed(Sealed::seal(
                    carrier,
                    producer.brand().handle(),
                )),
                producer.storage_rc(),
            ),
            &producer.storage_rc(),
            None,
        )
        .expect("no declared return, no error");

    drop(producer);
    assert!(
        weak.upgrade().is_some(),
        "the envelope's host pin keeps the producer's storage alive for the adoption"
    );

    let consumer_storage = run_root_storage();
    let consumer = run_root_bare(&consumer_storage);
    let adopted: Carried = consumer.adopt_carried(&cell, AdoptSeam::Retaining);

    // Drop the envelope: the consumer's minted arena set (the materialized host member) is now the
    // sole owner of the producer's storage.
    drop(cell);
    assert!(
        weak.upgrade().is_some(),
        "the consumer's minted reach pins the producer past the envelope's release"
    );
    match adopted {
        Carried::Object(KObject::Number(n)) => {
            assert_eq!(*n, 7.0, "adopted value reads live under the minted pin")
        }
        other => panic!(
            "expected the adopted Number, got {:?}",
            other.ktype(test_run.types())
        ),
    }
}

/// The pass-through acceptance criterion: a value returned unmodified through the Done boundary
/// rides by reference. Finalize clones nothing and allocates nothing — the read on the consumer
/// side is the birth allocation, byte-for-byte the same address — and the only refcount the
/// delivery pays is the envelope's single frame-level retention bump.
#[test]
fn done_passthrough_rides_by_reference_without_clone_or_refcount() {
    let program = program_storage();
    let root = run_root_storage();
    let test_run = TestRun::silent(&program, &root);
    let scope = test_run.scope;
    let producer = CallFrame::new(scope);

    let (carrier, birth_addr) = producer.with_scope(|child| {
        let obj = child.brand().alloc_scalar(Scalar::Number(7.0));
        let addr = obj as *const KObject as usize;
        (child.seal_resident(Carried::Object(obj)).unseal(), addr)
    });
    let storage = producer.storage_rc();
    let count_before = Rc::strong_count(&storage);

    let delivered = Delivered::lift(
        crate::witnessed::Retained::from_sealed(Sealed::seal(carrier, producer.brand().handle())),
        producer.storage_rc(),
    );
    assert_eq!(
        Rc::strong_count(&storage),
        count_before + 1,
        "the delivery pays exactly one frame-level bump — the envelope's host pin"
    );

    let envelope = test_run
        .runtime
        .host
        .finalize_terminal(delivered, &producer.storage_rc(), None)
        .expect("no declared return, no error");
    assert_eq!(
        Rc::strong_count(&storage),
        count_before + 1,
        "the Done boundary itself pays no refcount — the envelope passes straight through"
    );

    drop(producer);
    envelope.open(|carried| match carried {
        Carried::Object(obj) => {
            assert_eq!(
                obj as *const KObject as usize, birth_addr,
                "the pass-through reads back the birth allocation — no deep_clone anywhere \
                 between production and delivery"
            );
            assert!(
                matches!(obj, KObject::Number(n) if *n == 7.0),
                "and the value is intact"
            );
        }
        Carried::Type(other) => panic!(
            "expected the passed-through Number, got {}",
            other.name(test_run.registries())
        ),
        Carried::UnresolvedType(ti) => {
            panic!(
                "expected the passed-through Number, got {}",
                crate::machine::model::render_label(ti.symbol(), test_run.registries())
            )
        }
    });
}
