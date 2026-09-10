//! The value channel over **both** representations. Every test here runs its ladder twice — once
//! against a keyed store and once against a slotted one over the same names — because the two are
//! meant to differ in how a name is addressed and in nothing else: the visibility rule, the
//! bind-once rule and the claim rules have one implementation each.

use std::rc::Rc;

use super::super::*;
use crate::builtins::test_support::{binder_name, value_name};
use crate::machine::ProducerId;
use crate::machine::model::RunRegistries;
use crate::machine::model::{Carried, KObject, Scalar, SlotLayout, ValueSymbol};
use crate::memory::{FrameStorageExt, RegionBrand, Sealed, run_root_storage};

/// A three-binder body: `a` at position 1, `b` at 2, `c` at 3 — the positions statements `0..3` of
/// a block submit at. Spelled as source so the layout comes off the same statement reader a real
/// body's does.
const BODY: &str = "((LET c = 1) (LET a = 2) (LET b = 3))";

fn names(registries: &RunRegistries) -> [ValueSymbol; 3] {
    [
        value_name("a", registries),
        value_name("b", registries),
        value_name("c", registries),
    ]
}

/// [`BODY`]'s layout, homed in program storage — where a real body's is, and outliving every frame
/// region a test opens under it.
fn body_layout<'p>(
    program: crate::memory::ProgramBrand<'p>,
    registries: &RunRegistries,
) -> &'p SlotLayout<'p> {
    let body = crate::parse::parse(program, &registries.labels, BODY)
        .expect("parse")
        .into_iter()
        .next()
        .expect("one statement");
    let layout = SlotLayout::of_body(program.region(), &body);
    assert_eq!(layout.len(), 3, "the fixture body binds three names");
    layout
}

/// The two stores under test, each over its own region: the keyed one, and a slotted one sized by
/// `layout`. A test body runs against both, so a divergence in either direction fails.
///
/// Each store is bumped into the region it is over, so the pair reaches `check` at one lifetime —
/// the shape a `Scope` holds its own tables at, and the only one `Bindings`' invariance admits.
fn both<'p>(layout: &'p SlotLayout<'p>, check: impl for<'r> Fn(&'r Bindings<'r>, RegionBrand<'r>)) {
    let keyed_storage = run_root_storage();
    let keyed = keyed_storage.brand();
    check(keyed.allocator().in_place(Bindings::new(keyed)), keyed);

    let slotted_storage = run_root_storage();
    let slotted = slotted_storage.brand();
    check(
        slotted
            .allocator()
            .in_place(Bindings::slotted(slotted, layout)),
        slotted,
    );
}

/// Seal a number as resident in `region` — the shape a bind door hands the write verb.
fn number<'a>(region: RegionBrand<'a>, n: f64) -> SealedValue<'a> {
    let value: &KObject = region.alloc_scalar(Scalar::Number(n));
    Sealed::seal(
        region.seal_resident(Carried::Object(value)),
        region.handle(),
    )
}

fn gate() -> crate::machine::WriteGate {
    crate::machine::WriteGate::for_test()
}

/// The whole life of a name in one cell: miss, then `Parked` on the in-flight binder, then `Bound`
/// — with the claim retired by the commit that replaced it, on both representations.
#[test]
fn one_cell_answers_miss_parked_and_bound() {
    let program = crate::memory::program_storage();
    let registries = RunRegistries::new();
    both(
        body_layout(program.brand(), &registries),
        |bindings, region| {
            let [a, ..] = names(&registries);
            assert!(
                bindings.lookup_value(a, None).is_none(),
                "unwritten is a miss"
            );
            assert!(bindings.has_no_claims());

            bindings
                .install_placeholder(
                    binder_name("a", &registries),
                    ProducerId::for_test(7),
                    BindingIndex::value(2),
                    &registries,
                    &mut gate(),
                )
                .expect("a fresh name admits a claim");
            assert!(matches!(
                bindings.lookup_value(a, None),
                Some(NameLookup::Parked(id)) if id == ProducerId::for_test(7),
            ));
            assert!(!bindings.has_no_claims(), "a standing claim is not ready");

            bindings
                .write_value(
                    a,
                    BindingIndex::value(2),
                    number(region, 1.0),
                    &registries,
                    &mut gate(),
                )
                .expect("the binder's own commit");
            assert!(matches!(
                bindings.lookup_value(a, None),
                Some(NameLookup::Bound(_))
            ));
            assert!(
                bindings.has_no_claims(),
                "the commit retired the claim it satisfied",
            );

            // The zero-mask retirement path finds nothing left and leaves the binding standing.
            bindings.retire_claims(BindingIndex::value(2), &mut gate());
            assert!(matches!(
                bindings.lookup_value(a, None),
                Some(NameLookup::Bound(_))
            ));
        },
    );
}

/// The positional rule is the representation's business only in *where* the position is read from:
/// a binder at position `i` is invisible at cutoff `i` and visible at `i + 1`, claimed or bound.
#[test]
fn visibility_gates_on_the_binders_own_position() {
    let program = crate::memory::program_storage();
    let registries = RunRegistries::new();
    both(
        body_layout(program.brand(), &registries),
        |bindings, region| {
            let [a, b, _] = names(&registries);
            // `a` binds at 2, `b` at 3 — the fixture body's own positions.
            bindings
                .install_placeholder(
                    binder_name("b", &registries),
                    ProducerId::for_test(9),
                    BindingIndex::value(3),
                    &registries,
                    &mut gate(),
                )
                .expect("a fresh name admits a claim");
            bindings
                .write_value(
                    a,
                    BindingIndex::value(2),
                    number(region, 1.0),
                    &registries,
                    &mut gate(),
                )
                .expect("a fresh name admits a bind");

            assert!(
                bindings.lookup_value(a, Some(2)).is_none(),
                "2 < 2 is false"
            );
            assert!(matches!(
                bindings.lookup_value(a, Some(3)),
                Some(NameLookup::Bound(_))
            ));
            assert!(
                bindings.lookup_value(b, Some(3)).is_none(),
                "a claim gates too"
            );
            assert!(matches!(
                bindings.lookup_value(b, Some(4)),
                Some(NameLookup::Parked(_))
            ));
            // Off-chain readers see everything.
            assert!(bindings.lookup_value(a, None).is_some());
        },
    );
}

/// Bind-once, and one claim per name: a second bind rebinds, and a claim naming a different binder
/// collides while the same binder's re-entry is idempotent.
#[test]
fn a_bound_name_rebinds_and_a_foreign_claim_collides() {
    let program = crate::memory::program_storage();
    let registries = RunRegistries::new();
    both(
        body_layout(program.brand(), &registries),
        |bindings, region| {
            let [a, _b, _] = names(&registries);
            bindings
                .install_placeholder(
                    binder_name("b", &registries),
                    ProducerId::for_test(1),
                    BindingIndex::value(3),
                    &registries,
                    &mut gate(),
                )
                .expect("a fresh name admits a claim");
            bindings
                .install_placeholder(
                    binder_name("b", &registries),
                    ProducerId::for_test(1),
                    BindingIndex::value(3),
                    &registries,
                    &mut gate(),
                )
                .expect("the same stamp arriving twice is not a second declaration");
            assert!(
                bindings
                    .install_placeholder(
                        binder_name("b", &registries),
                        ProducerId::for_test(2),
                        BindingIndex::value(3),
                        &registries,
                        &mut gate(),
                    )
                    .is_err(),
                "a different binder claiming a held name is a Rebind",
            );

            bindings
                .write_value(
                    a,
                    BindingIndex::value(2),
                    number(region, 1.0),
                    &registries,
                    &mut gate(),
                )
                .expect("a fresh name admits a bind");
            assert!(
                bindings
                    .write_value(
                        a,
                        BindingIndex::value(2),
                        number(region, 2.0),
                        &registries,
                        &mut gate()
                    )
                    .is_err(),
                "bindings are bind-once",
            );
            assert!(
                bindings
                    .install_placeholder(
                        binder_name("a", &registries),
                        ProducerId::for_test(3),
                        BindingIndex::value(2),
                        &registries,
                        &mut gate(),
                    )
                    .is_err(),
                "a committed name admits no claim",
            );
        },
    );
}

/// A binder that terminalizes without committing leaves nothing behind: its claim retires and the
/// readiness gate reads clear again.
#[test]
fn an_unsatisfied_binder_retires_its_claim() {
    let program = crate::memory::program_storage();
    let registries = RunRegistries::new();
    both(
        body_layout(program.brand(), &registries),
        |bindings, _region| {
            let [_, b, _] = names(&registries);
            bindings
                .install_placeholder(
                    binder_name("b", &registries),
                    ProducerId::for_test(5),
                    BindingIndex::value(3),
                    &registries,
                    &mut gate(),
                )
                .expect("a fresh name admits a claim");
            assert!(!bindings.has_no_claims());

            bindings.retire_claims(BindingIndex::value(3), &mut gate());
            assert!(bindings.has_no_claims(), "the statement dropped its claim");
            assert!(bindings.lookup_value(b, None).is_none());
            assert!(bindings.pending_value(b).is_none());
            assert!(bindings.pending_names(&registries).is_empty());
        },
    );
}

/// The capture snapshot reads both states off the same channel: a committed binding travels with
/// the position its binder wrote at, and a standing claim travels as a producer to wait on.
#[test]
fn the_capture_snapshot_carries_positions_and_claims() {
    let program = crate::memory::program_storage();
    let registries = RunRegistries::new();
    both(
        body_layout(program.brand(), &registries),
        |bindings, region| {
            let [a, b, _] = names(&registries);
            bindings
                .write_value(
                    a,
                    BindingIndex::value(2),
                    number(region, 1.0),
                    &registries,
                    &mut gate(),
                )
                .expect("a fresh name admits a bind");
            bindings
                .install_placeholder(
                    binder_name("b", &registries),
                    ProducerId::for_test(4),
                    BindingIndex::value(3),
                    &registries,
                    &mut gate(),
                )
                .expect("a fresh name admits a claim");

            let visible = bindings.visible_for_capture(None, crate::memory::Global);
            assert_eq!(
                visible
                    .data
                    .iter()
                    .map(|(name, at, _)| (*name, at.index().idx))
                    .collect::<Vec<_>>(),
                vec![(a, 2)],
                "the bound entry travels with its own lexical position",
            );
            assert_eq!(visible.claims.to_vec(), vec![ProducerId::for_test(4)]);
            assert_eq!(bindings.iter_data().len(), 1);
            assert_eq!(bindings.bound_value_count(), 1);
            assert!(bindings.is_value_bound(a));
            assert!(!bindings.is_value_bound(b));
        },
    );
}

/// A slotted store's layout is reachable through the façade, and a keyed one has none — which is
/// what the copy engine reads to decide the representation of the scope it builds.
#[test]
fn only_a_slotted_store_publishes_a_layout() {
    let program = crate::memory::program_storage();
    let registries = RunRegistries::new();
    let storage = run_root_storage();
    assert!(Bindings::new(storage.brand()).layout().is_none());

    let layout = body_layout(program.brand(), &registries);
    let slotted = Bindings::slotted(storage.brand(), layout);
    assert_eq!(slotted.layout().map(SlotLayout::len), Some(3));
    // The `Rc` keeps the storage alive for the borrow above, and states which region the tables
    // sit in.
    drop(Rc::clone(&storage));
}
