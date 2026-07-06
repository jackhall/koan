//! Tests for [`finalize_terminal`](super::NodeFinalize::finalize_terminal)'s Done-boundary gate: a
//! region-pure terminal severs its residence and releases the dying producer frame, while a value
//! that genuinely borrows into that frame keeps it pinned. The [`Weak`] census is the direct probe —
//! a released frame's `FrameStorage` upgrades to `None` once the last strong holder drops.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use super::NodeFinalize;
use crate::builtins::default_scope;
use crate::builtins::test_support::{parse_one, run, run_one, run_root_silent};
use crate::machine::core::kfunction::action::{Action, BodyCtx};
use crate::machine::core::{CarrierWitness, FrameStorage};
use crate::machine::execute::KoanRuntime;
use crate::machine::model::types::{ExpressionSignature, KType, ReturnType, SignatureElement};
use crate::machine::model::{Carried, KObject};
use crate::machine::CallFrame;

/// Build a scalar carrier residing in `producer`'s region with the given home-omitted foreign reach
/// and `borrows_into_home` bit — the exact carrier a resident-value read hands to finalize. Returns
/// the carrier (lifetime-erased, so it escapes the frame's rank-2 scope open) and a [`Weak`] to the
/// producer's `FrameStorage` for the liveness census.
fn resident_scalar(
    producer: &Rc<CallFrame>,
    borrows_into_home: bool,
) -> (
    crate::witnessed::Witnessed<crate::machine::model::values::CarriedFamily, CarrierWitness>,
    Weak<FrameStorage>,
) {
    let carrier = producer.with_scope(|child| {
        let obj = child.brand().alloc_object(KObject::Number(7.0));
        child.resident_value_carrier(obj, None, borrows_into_home)
    });
    let weak = Rc::downgrade(&producer.storage_rc());
    (carrier, weak)
}

/// A region-pure scalar terminal (empty reach) is severed at the Done boundary: finalize copies the
/// top node into an owned backing and drops the producer-residence pin, so once the caller drops the
/// producer `CallFrame` the frame's `FrameStorage` is released — well before program end — while the
/// value stays readable through the severed carrier's owned backing.
#[test]
fn region_pure_scalar_releases_producer_frame() {
    let root = FrameStorage::run_root();
    let scope = default_scope(&root, Box::new(std::io::sink()));
    let producer = CallFrame::new_test(scope, None);

    let (carrier, weak) = resident_scalar(&producer, false);
    assert!(
        !carrier.witness().reach_covers(producer.region()),
        "a region-pure scalar's reach names nothing — it only pins its residence frame"
    );
    assert!(
        carrier.witness().covers(producer.region()),
        "the residence Frame pin still keeps the region live before the sever"
    );

    let runtime = KoanRuntime::new();
    let sealed = runtime
        .finalize_terminal(carrier, Some(&producer), None)
        .expect("no declared return, no error");

    // The severed carrier holds an owned backing, not the frame — dropping the producer shell releases
    // its storage.
    drop(producer);
    assert!(
        weak.upgrade().is_none(),
        "the producer frame drops once the region-pure scalar stops pinning it"
    );
    sealed.with(|carried| match carried {
        Carried::Object(KObject::Number(n)) => assert_eq!(*n, 7.0, "value survives the sever"),
        other => panic!("expected the severed Number, got {:?}", other.ktype()),
    });
}

/// A value that genuinely borrows into its producer frame (the `borrows_into_home` bit set, so its
/// reach names the frame) is **not** severed: finalize seals it as-is, and the returned carrier keeps
/// the frame alive after the producer shell drops. The soundness counterweight to the sever — a home
/// borrow must never be released early.
#[test]
fn home_borrowing_value_keeps_producer_frame() {
    let root = FrameStorage::run_root();
    let scope = default_scope(&root, Box::new(std::io::sink()));
    let producer = CallFrame::new_test(scope, None);

    let (carrier, weak) = resident_scalar(&producer, true);
    assert!(
        carrier.witness().reach_covers(producer.region()),
        "the home-borrow bit materializes the producer region into reach"
    );

    let runtime = KoanRuntime::new();
    let sealed = runtime
        .finalize_terminal(carrier, Some(&producer), None)
        .expect("no declared return, no error");

    drop(producer);
    assert!(
        weak.upgrade().is_some(),
        "the sealed carrier's reach still pins the producer frame after the shell drops"
    );

    // Releasing the carrier drops the last pin, so the frame finally dies — no leak.
    drop(sealed);
    assert!(
        weak.upgrade().is_none(),
        "dropping the last carrier releases the frame"
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
fn probe_body<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
    FRAME_CENSUS.with(|census| census.borrow_mut().push(ctx.scope.region_owner()));
    Action::done_resident(Carried::Object(
        ctx.scope.brand().alloc_object(KObject::Number(1.0)),
    ))
}

/// Register `(PROBE)` — a nullary keyword builtin returning `Number` — into `scope`.
fn register_probe<'a>(scope: &'a crate::machine::Scope<'a>) {
    let signature = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Number),
        elements: vec![SignatureElement::Keyword("PROBE".into())],
    };
    crate::builtins::register_builtin(scope, "PROBE", signature, probe_body);
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
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    register_probe(scope);
    run(scope, "FN (GETONE) -> Number = (PROBE)");

    let result = run_one(scope, parse_one("GETONE"));
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
/// would read 100 live here; the finalize sever drives it to 0.
#[test]
fn aggregate_of_call_results_releases_every_producer_frame() {
    FRAME_CENSUS.with(|census| census.borrow_mut().clear());
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    register_probe(scope);
    run(scope, "FN (GETONE) -> Number = (PROBE)");

    let calls = vec!["(GETONE)"; 100].join(" ");
    run(scope, &format!("LET results = [{calls}]"));

    // The aggregate is live and complete...
    let results = run_one(scope, parse_one("results"));
    match results {
        KObject::List(items, _) => assert_eq!(items.len(), 100, "all 100 results retained"),
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
