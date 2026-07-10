//! Tests for [`finalize_terminal`](super::NodeFinalize::finalize_terminal)'s Done boundary: a
//! terminal seals **as-is** — the boundary makes no memory decision — and the producer frame's
//! lifetime rides the scheduler's retention hold (stood in for here by the delivery envelope's
//! host `Rc`), released when the hold drops. The [`Weak`] census is the direct probe — a released
//! frame's `FrameStorage` upgrades to `None` once the last strong holder drops.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use super::NodeFinalize;
use crate::builtins::default_scope;
use crate::builtins::test_support::{parse_one, run, run_one, run_root_bare, run_root_silent};
use crate::machine::core::kfunction::action::{Action, BodyCtx};
use crate::machine::core::kfunction::body::{Body, ReturnContract};
use crate::machine::core::kfunction::KFunction;
use crate::machine::core::{
    run_root_storage, CarrierWitness, FrameSet, FrameStorage, FrameStorageExt, Scope,
};
use crate::machine::execute::obligation::ReturnObligation;
use crate::machine::execute::KoanRuntime;
use crate::machine::model::types::{ExpressionSignature, KType, ReturnType, SignatureElement};
use crate::machine::model::values::Module;
use crate::machine::model::{Carried, KObject};
use crate::machine::CallFrame;
use crate::witnessed::Delivered;

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

/// A region-pure scalar terminal (empty reach) seals as-is at the Done boundary and rides the
/// retention hold: the envelope's host `Rc` (the hold's stand-in) keeps the producer's storage —
/// hence the value — alive across the producer shell's drop, and releasing the hold releases the
/// frame. Frame release is a function of deliveries only, never of the value's reach.
#[test]
fn region_pure_scalar_rides_retention_and_releases_at_hold_drop() {
    let root = run_root_storage();
    let scope = default_scope(&root, Box::new(std::io::sink()));
    let producer = CallFrame::new_test(scope, None);

    let (carrier, weak) = resident_scalar(&producer, false);
    assert!(
        !carrier.witness().reach_covers(None, producer.region()),
        "a region-pure scalar's reach names nothing"
    );
    assert!(
        carrier.witness().is_empty(),
        "the carrier pins nothing — liveness is retention's"
    );

    let runtime = KoanRuntime::new();
    let sealed = runtime
        .finalize_terminal(carrier, Some(&producer), None)
        .expect("no declared return, no error");
    // The retention seed: the producer's storage rides the envelope, exactly as the run loop hands
    // it to the scheduler at finalize.
    let envelope = Delivered::seal(sealed, producer.storage_rc());

    drop(producer);
    assert!(
        weak.upgrade().is_some(),
        "the retention hold keeps the producer's storage alive across the shell drop"
    );
    envelope.open(|carried| match carried {
        Carried::Object(KObject::Number(n)) => assert_eq!(*n, 7.0, "value rides the hold"),
        other => panic!("expected the retained Number, got {:?}", other.ktype()),
    });
    drop(envelope);
    assert!(
        weak.upgrade().is_none(),
        "releasing the hold releases the frame — a delivery fact, not a reach fact"
    );
}

/// A value that genuinely borrows into its producer frame carries the `borrows_host` bit through
/// the Done boundary unchanged — finalize seals as-is; the bit is read only at a later copied
/// re-home mint, never as a lifecycle input. The frame's lifetime is retention's either way.
#[test]
fn home_borrowing_value_keeps_its_bit_and_rides_retention() {
    let root = run_root_storage();
    let scope = default_scope(&root, Box::new(std::io::sink()));
    let producer = CallFrame::new_test(scope, None);

    let (carrier, weak) = resident_scalar(&producer, true);
    assert!(
        carrier.witness().borrows_host(),
        "the home-borrow bit rides the carrier"
    );

    let runtime = KoanRuntime::new();
    let sealed = runtime
        .finalize_terminal(carrier, Some(&producer), None)
        .expect("no declared return, no error");
    assert!(
        sealed.witness().borrows_host(),
        "the bit survives the Done boundary verbatim"
    );
    let envelope = Delivered::seal(sealed, producer.storage_rc());

    drop(producer);
    assert!(
        weak.upgrade().is_some(),
        "the retention hold — not the carrier — keeps the frame alive"
    );
    drop(envelope);
    assert!(
        weak.upgrade().is_none(),
        "dropping the hold releases the frame"
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

/// `Scope::adopt_sealed` on a delivered object: the value rides its retention hold (the envelope's
/// host) across the producer shell's drop, and the copy-free adoption materializes that host into
/// the consumer's arena — so after the envelope itself drops, the consumer's minted set is the
/// sole owner of the producer's storage and the adopted read stays live.
#[test]
fn adopt_sealed_object_rides_retention_across_producer_shell_drop() {
    let root = run_root_storage();
    let scope = default_scope(&root, Box::new(std::io::sink()));
    let producer = CallFrame::new_test(scope, None);

    let (carrier, weak) = resident_scalar(&producer, false);
    let runtime = KoanRuntime::new();
    let sealed = runtime
        .finalize_terminal(carrier, Some(&producer), None)
        .expect("no declared return, no error");
    let cell = Delivered::seal(sealed, producer.storage_rc());

    drop(producer);
    assert!(
        weak.upgrade().is_some(),
        "the retention hold keeps the producer's storage alive for the adoption"
    );

    let consumer_storage = run_root_storage();
    let consumer = run_root_bare(&consumer_storage);
    let adopted: Carried = consumer.adopt_sealed(&cell);

    // Drop the hold: the consumer's minted arena set (the materialized host member) is now the
    // sole owner of the producer's storage.
    drop(cell);
    assert!(
        weak.upgrade().is_some(),
        "the consumer's minted reach pins the producer past the hold's release"
    );
    match adopted {
        Carried::Object(KObject::Number(n)) => {
            assert_eq!(*n, 7.0, "adopted value reads live under the minted pin")
        }
        other => panic!("expected the adopted Number, got {:?}", other.ktype()),
    }
}

/// `Scope::adopt_sealed` on a delivered type: a `KType` whose interior (here a `KType::Module`'s
/// `&'a Scope`) points into a **foreign** frame is the dangerous case — adoption's mint must pin
/// both the foreign reach and the producer residence into the consumer's arena, so the module's
/// child scope reads back cleanly after every other handle on the foreign frame drops.
#[test]
fn adopt_sealed_type_pins_foreign_region_after_producer_drop() {
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
        let evidence = crate::machine::core::StoredReach {
            foreign: Some(&foreign_reach),
            borrows_into_home: false,
        };
        let kt_ref = child
            .alloc_ktype_reaching(KType::Module { module }, &evidence)
            .expect("module is covered by foreign_reach");
        child.resident_type_carrier(kt_ref, Some(&foreign_reach), false)
    });
    assert!(
        !carrier.witness().reach_covers(None, producer.region()),
        "the type borrows only into the foreign module region, not the producer's own"
    );

    let runtime = KoanRuntime::new();
    let sealed = runtime
        .finalize_terminal(carrier, Some(&producer), None)
        .expect("no declared return, no error");
    let cell = Delivered::seal(sealed, producer.storage_rc());
    drop(producer);

    let consumer_storage = run_root_storage();
    let consumer = run_root_bare(&consumer_storage);
    let adopted: Carried = consumer.adopt_sealed(&cell);

    // Drop the hold and every other handle on the foreign frame: the consumer's minted arena set is
    // now the sole pin — Miri confirms no use-after-free on the read below.
    drop(cell);
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

/// The pass-through acceptance criterion: a value returned unmodified through the Done boundary
/// rides by reference. Finalize clones nothing and allocates nothing — the read on the consumer
/// side is the birth allocation, byte-for-byte the same address — and the only refcount the
/// delivery pays is the envelope's single frame-level retention bump.
#[test]
fn done_passthrough_rides_by_reference_without_clone_or_refcount() {
    let root = run_root_storage();
    let scope = default_scope(&root, Box::new(std::io::sink()));
    let producer = CallFrame::new_test(scope, None);

    let (carrier, birth_addr) = producer.with_scope(|child| {
        let obj = child.brand().alloc_object(KObject::Number(7.0));
        let addr = obj as *const KObject as usize;
        (child.resident_value_carrier(obj, None, false), addr)
    });
    let storage = producer.storage_rc();
    let count_before = Rc::strong_count(&storage);

    let runtime = KoanRuntime::new();
    let sealed = runtime
        .finalize_terminal(carrier, Some(&producer), None)
        .expect("no declared return, no error");
    assert_eq!(
        Rc::strong_count(&storage),
        count_before,
        "the Done boundary itself pays no refcount"
    );

    let envelope = Delivered::seal(sealed, producer.storage_rc());
    assert_eq!(
        Rc::strong_count(&storage),
        count_before + 1,
        "the delivery pays exactly one frame-level bump — the retention hold"
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
        Carried::Type(other) => panic!("expected the passed-through Number, got {}", other.name()),
    });
}

/// A declared-return re-stamp whose carrier holds a **type** value passes through un-relocated on
/// the type channel: the check runs at a shared brand under the producer ∪ home-owner pin, and the
/// carrier returns verbatim — nothing is minted into the home region for a value that never moves
/// there.
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
    // `borrows_into_home = true` marks the value as home-borrowing; the type channel must pass the
    // carrier through with the bit and the reach untouched either way.
    let carrier = producer.with_scope(|child| {
        let evidence = crate::machine::core::StoredReach {
            foreign: Some(&foreign_reach),
            borrows_into_home: true,
        };
        let kt_ref = child
            .alloc_ktype_reaching(KType::Module { module }, &evidence)
            .expect("module is covered by foreign_reach");
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
    let obligation = ReturnObligation::seal(ReturnContract::Function(kf_ref));

    let foreign_count_before = Rc::strong_count(&foreign_storage);

    let runtime = KoanRuntime::new();
    let checked = runtime
        .finalize_terminal(carrier, Some(&producer), Some(&obligation))
        .expect("declared type Any matches the carried Module -- no mismatch");

    assert_eq!(
        Rc::strong_count(&foreign_storage),
        foreign_count_before,
        "the type channel passes the carrier through un-relocated -- no mint bumps the foreign \
         frame's refcount, and no set holds it until the home region dies"
    );
    assert!(
        checked
            .witness()
            .reach_covers(None, foreign_storage.region()),
        "the checked carrier still names the foreign region its reach always named"
    );
    assert!(
        checked.witness().borrows_host(),
        "the home-borrow bit passes through the type channel verbatim"
    );
}
