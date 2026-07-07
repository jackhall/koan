//! Tests for [`finalize_terminal`](super::NodeFinalize::finalize_terminal)'s Done-boundary gate: a
//! region-pure terminal severs its residence and releases the dying producer frame, while a value
//! that genuinely borrows into that frame keeps it pinned. The [`Weak`] census is the direct probe —
//! a released frame's `FrameStorage` upgrades to `None` once the last strong holder drops.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use super::NodeFinalize;
use crate::builtins::default_scope;
use crate::builtins::test_support::{parse_one, run, run_one, run_root_bare, run_root_silent};
use crate::machine::core::kfunction::action::{Action, BodyCtx};
use crate::machine::core::kfunction::body::{Body, ReturnContract};
use crate::machine::core::kfunction::KFunction;
use crate::machine::core::{run_root_storage, CarrierWitness, FrameSet, FrameStorage, FrameStorageExt, Scope};
use crate::machine::execute::KoanRuntime;
use crate::machine::model::types::{ExpressionSignature, KType, ReturnType, SignatureElement};
use crate::machine::model::values::{CarriedFamily, Module};
use crate::machine::model::{Carried, KObject};
use crate::machine::CallFrame;
use crate::witnessed::Sealed;

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
    let root = run_root_storage();
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
    let root = run_root_storage();
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
    let region = run_root_storage();
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
    let region = run_root_storage();
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

/// `Scope::adopt_sealed`'s severed re-home, Object channel: a region-pure scalar severed at the
/// Done boundary (its top node copied into an owned, frame-free backing) is adopted into a
/// consumer scope in a completely different frame, after the producer frame has already dropped —
/// the severed backing, not a region pin, is what keeps it readable.
#[test]
fn adopt_sealed_severed_object_survives_producer_drop() {
    let root = run_root_storage();
    let scope = default_scope(&root, Box::new(std::io::sink()));
    let producer = CallFrame::new_test(scope, None);

    let (carrier, weak) = resident_scalar(&producer, false);
    let runtime = KoanRuntime::new();
    let severed = runtime
        .finalize_terminal(carrier, Some(&producer), None)
        .expect("no declared return, no error");

    drop(producer);
    assert!(
        weak.upgrade().is_none(),
        "the producer frame is already released before adoption"
    );

    let consumer_storage = run_root_storage();
    let consumer = run_root_bare(&consumer_storage);
    let cell: Sealed<CarriedFamily, CarrierWitness> = Sealed::seal(severed);
    let adopted: Carried = consumer.adopt_sealed(&cell);

    match adopted {
        Carried::Object(KObject::Number(n)) => {
            assert_eq!(*n, 7.0, "severed value survives adoption")
        }
        other => panic!("expected the severed Number, got {:?}", other.ktype()),
    }
}

/// `Scope::adopt_sealed`'s severed re-home, Type channel: a severed `KType` (here a `KType::Module`
/// whose `&'a Scope` interior points into a **foreign** frame) is the dangerous case — `.clone()` is
/// shallow, so the re-homed `&'a KType` still borrows the foreign region. Adoption's mint must pin
/// that foreign frame into the consumer's arena *before* the re-home, so the module's child scope
/// reads back cleanly after every other handle on the foreign frame drops.
#[test]
fn adopt_sealed_severed_type_pins_foreign_region_after_producer_drop() {
    let foreign_storage = run_root_storage();
    let foreign_scope = run_root_bare(&foreign_storage);
    let foreign_child = foreign_storage
        .brand()
        .alloc_scope(Scope::child_under(foreign_scope));
    let module = foreign_storage
        .brand()
        .alloc_module(Module::new("M".to_string(), foreign_child));
    let foreign_weak = Rc::downgrade(&foreign_storage);

    let root = run_root_storage();
    let scope = default_scope(&root, Box::new(std::io::sink()));
    let producer = CallFrame::new_test(scope, None);
    let foreign_reach = FrameSet::singleton(Rc::clone(&foreign_storage));
    let carrier = producer.with_scope(|child| {
        let kt_ref = child.brand().alloc_ktype(KType::Module { module });
        child.resident_type_carrier(kt_ref, Some(&foreign_reach), false)
    });
    assert!(
        !carrier.witness().reach_covers(producer.region()),
        "the type borrows only into the foreign module region, not the producer's own"
    );

    let runtime = KoanRuntime::new();
    let severed = runtime
        .finalize_terminal(carrier, Some(&producer), None)
        .expect("no declared return, no error");
    drop(producer);

    let consumer_storage = run_root_storage();
    let consumer = run_root_bare(&consumer_storage);
    let cell: Sealed<CarriedFamily, CarrierWitness> = Sealed::seal(severed);
    let adopted: Carried = consumer.adopt_sealed(&cell);

    // Drop every other handle on the foreign frame: the consumer's minted arena set is now the
    // sole pin — Miri confirms no use-after-free on the read below.
    drop(foreign_storage);

    match adopted {
        Carried::Type(KType::Module { module }) => {
            assert_eq!(
                module.path, "M",
                "the re-homed module survives the foreign frame's drop"
            )
        }
        Carried::Type(other) => panic!("expected the severed Module type, got {}", other.name()),
        Carried::Object(_) => panic!("expected the severed Module type, got an Object"),
    }
    assert!(
        foreign_weak.upgrade().is_some(),
        "the consumer's minted reach still pins the foreign frame"
    );
}

/// A declared-return re-stamp whose carrier holds a **type** value passes through un-relocated on
/// the type channel: the merge composes against an unwitnessed home operand (`merge(w, ∅) == w`),
/// so nothing is minted into the home region for a value that never moves there. Before this was
/// gated on the carried discriminant, the merge always built a witnessed home operand when the home
/// owner resolved, minting the type's foreign reach into the home arena and orphaning it there for
/// the home region's whole life even though the type-channel result discarded that composed witness.
#[test]
fn type_passthrough_declared_return_mints_nothing_into_home() {
    let foreign_storage = run_root_storage();
    let foreign_scope = run_root_bare(&foreign_storage);
    let foreign_child = foreign_storage
        .brand()
        .alloc_scope(Scope::child_under(foreign_scope));
    let module = foreign_storage
        .brand()
        .alloc_module(Module::new("M".to_string(), foreign_child));

    let root = run_root_storage();
    let scope = default_scope(&root, Box::new(std::io::sink()));
    let producer = CallFrame::new_test(scope, None);
    let foreign_reach = FrameSet::singleton(Rc::clone(&foreign_storage));
    // `borrows_into_home = true` so the carrier's reach already covers the producer region -- this
    // keeps the pre-merge triage's sever branch from firing (it only severs when the home owner
    // resolves *and* the reach doesn't already cover the producer), so the carrier the merge sees is
    // exactly the one `resident_type_carrier` built, with no extra reach-clone muddying the refcount.
    let carrier = producer.with_scope(|child| {
        let kt_ref = child.brand().alloc_ktype(KType::Module { module });
        child.resident_type_carrier(kt_ref, Some(&foreign_reach), true)
    });

    // A declared return of `Any` matches any carried type, so the merge takes the no-mismatch path;
    // the home owner (`home_storage`) resolves via the FN's captured scope, so `home_owner.is_some()`.
    let home_storage = run_root_storage();
    let home_scope = run_root_bare(&home_storage);
    let signature = ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![],
    };
    let kfunc = KFunction::new(
        signature,
        Body::Builtin(probe_body),
        home_scope,
        None,
        None,
        false,
    );
    let kf_ref = home_storage.brand().alloc_function(kfunc);
    let contract = Some(ReturnContract::Function(kf_ref));

    let foreign_count_before = Rc::strong_count(&foreign_storage);

    let runtime = KoanRuntime::new();
    let checked = runtime
        .finalize_terminal(carrier, Some(&producer), contract)
        .expect("declared type Any matches the carried Module -- no mismatch");

    assert_eq!(
        Rc::strong_count(&foreign_storage),
        foreign_count_before,
        "the type channel passes the carrier through un-relocated -- no mint bumps the foreign \
         frame's refcount, and no set holds it until the home region dies"
    );
    assert!(
        checked.witness().covers(foreign_storage.region()),
        "the checked carrier still covers the foreign region its reach always named"
    );
}
