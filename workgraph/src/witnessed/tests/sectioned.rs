//! The sectioned-storage slate: run grouping, index lookup, and the alloc door
//! ([`Sectioned::build`]) — over a library-only profile (`RegionHost` frames, `u32` content), so no
//! embedder type is in scope. Reach counts are read as deltas off the thread-local
//! [`RegionMetrics`], scoped so a test's setup mints land outside its measured window.

use std::rc::Rc;

use super::super::*;

/// The library-only profile the sectioned slate runs over: `u32` cells, resident in a region and
/// handed to the door as the `&'a CellFamily::At<'a>` references it takes.
struct SectionProfile;

impl StorageProfile for SectionProfile {
    type Families = (CellFamily, ());
    type FrameOwner = RegionHost<SectionProfile>;
}

type SectionFrame = RegionHost<SectionProfile>;

/// The stand-in cell family: owned scalar content, so `At<'r>` is lifetime-free and a cell
/// reference is the plain `&'a u32` a region hands back.
struct CellFamily;

reattachable! {
    CellFamily => u32,
}

impl Stored<SectionProfile> for CellFamily {
    fn cell(storage: &StorageOf<SectionProfile>) -> &FamilyArena<Self> {
        &storage.0
    }
}

fn frame() -> Rc<SectionFrame> {
    RegionHost::fresh(None)
}

/// Store `v` in `frame`'s region and hand back the co-located cell reference the door takes.
fn store(frame: &Rc<SectionFrame>, v: u32) -> &u32 {
    RegionHandle::from_owner(&**frame).alloc_resident::<CellFamily>(v)
}

/// The reach counters' movement across `body`. A delta rather than a [`reset_region_metrics`]
/// window, whose zeroing of the live-region gauge the test's own frames would underflow on drop.
fn reach_delta<R>(body: impl FnOnce() -> R) -> (R, RegionMetrics) {
    let before = region_metrics();
    let result = body();
    let after = region_metrics();
    (
        result,
        RegionMetrics {
            reach_interned: after.reach_interned - before.reach_interned,
            reach_intern_hits: after.reach_intern_hits - before.reach_intern_hits,
            reach_retention_folds: after.reach_retention_folds - before.reach_retention_folds,
            ..RegionMetrics::default()
        },
    )
}

/// A value living in `home`'s region whose borrows reach `member`'s — the stored description a
/// [`CellReach::Pinned`] input hands the door, paired with the coverage that pins it.
fn pinned_input<'r>(
    home: &'r Rc<SectionFrame>,
    member: &Rc<SectionFrame>,
) -> CellReach<'r, SectionFrame> {
    let reach = ReachDescription::mint_resident(
        RegionHandle::from_owner(&**home),
        &[&PinBundle::singleton(Rc::clone(member))],
    );
    let mut coverage = StepCoverage::of(Rc::clone(home));
    coverage.absorb(StepCoverage::of(Rc::clone(member)));
    CellReach::Pinned { reach, coverage }
}

/// A cell that is fully owned at the destination.
fn owned<'a>(payload: &'a u32) -> CellInput<'a, 'a, CellFamily, SectionFrame> {
    cell(payload, CellReach::Owned)
}

/// A cell with an explicit reach verdict. Its weight is its own content, so a container's
/// [`Sectioned::weight`] is the sum of the values this slate stores in it.
fn cell<'a, 'r>(
    payload: &'a u32,
    reach: CellReach<'r, SectionFrame>,
) -> CellInput<'a, 'r, CellFamily, SectionFrame> {
    CellInput {
        payload,
        reach,
        weight: u64::from(*payload),
    }
}

/// Whether `description`'s members are exactly `expected`, compared by owner identity.
fn members_are(
    description: &ReachDescription<SectionFrame>,
    expected: &[&Rc<SectionFrame>],
) -> bool {
    let members = description.members();
    members.len() == expected.len()
        && expected
            .iter()
            .all(|want| members.iter().any(|got| Rc::ptr_eq(got, want)))
}

/// Adjacency decides sharing: equal-reach neighbours collapse into one run, and a differing cell
/// between them opens a new one.
#[test]
fn adjacent_cells_of_equal_reach_share_a_run() {
    let dest = frame();
    let source = frame();
    let member = frame();
    let values: Vec<&u32> = (0..5).map(|v| store(&dest, v)).collect();

    let (container, _value_reach) = Sectioned::build(
        RegionHandle::from_owner(&*dest),
        vec![
            owned(values[0]),
            owned(values[1]),
            cell(values[2], pinned_input(&source, &member)),
            cell(values[3], pinned_input(&source, &member)),
            owned(values[4]),
        ],
    );

    assert_eq!(container.len(), 5);
    assert_eq!(container.run_count(), 3);
    let spans: Vec<_> = container.runs().map(|(span, _)| span).collect();
    assert_eq!(spans, vec![0..2, 2..4, 4..5]);
}

/// The same reach in non-adjacent runs makes two run entries naming **one** interned description.
#[test]
fn non_adjacent_runs_share_one_interned_description() {
    let dest = frame();
    let source = frame();
    let member = frame();
    let values: Vec<&u32> = (0..3).map(|v| store(&dest, v)).collect();

    let (container, _value_reach) = Sectioned::build(
        RegionHandle::from_owner(&*dest),
        vec![
            cell(values[0], pinned_input(&source, &member)),
            owned(values[1]),
            cell(values[2], pinned_input(&source, &member)),
        ],
    );

    assert_eq!(container.run_count(), 3);
    let reaches: Vec<_> = container.runs().map(|(_, reach)| reach).collect();
    assert!(std::ptr::eq(reaches[0], reaches[2]));
    assert!(!std::ptr::eq(reaches[0], reaches[1]));
}

/// Alternating owned and borrowing cells degrade to runs of length one — the per-cell-envelope cost
/// floor, and `reach_at` still answers exactly at every index.
#[test]
fn alternating_reach_degrades_to_length_one_runs() {
    let dest = frame();
    let source = frame();
    let member = frame();
    let values: Vec<&u32> = (0..4).map(|v| store(&dest, v)).collect();

    let mut inputs = Vec::new();
    for (index, payload) in values.iter().copied().enumerate() {
        inputs.push(if index % 2 == 0 {
            owned(payload)
        } else {
            cell(payload, pinned_input(&source, &member))
        });
    }
    let (container, _value_reach) = Sectioned::build(RegionHandle::from_owner(&*dest), inputs);

    assert_eq!(container.run_count(), 4);
    for index in 0..container.len() {
        let reach = container.reach_at(index).expect("index is covered");
        assert_eq!(reach.is_empty(), index % 2 == 0);
    }
    assert!(container.reach_at(container.len()).is_none());
    assert!(container.project(container.len()).is_none());
}

/// The single-run fast path: an all-owned container is one empty-reach run, and its value-level
/// description is the region's empty singleton — the whole build costs one interned description.
#[test]
fn all_owned_is_one_empty_run() {
    let dest = frame();
    let values: Vec<&u32> = (0..3).map(|v| store(&dest, v)).collect();

    let ((container, value_reach), metrics) = reach_delta(|| {
        Sectioned::build(
            RegionHandle::from_owner(&*dest),
            values.iter().copied().map(owned).collect(),
        )
    });

    assert!(container.is_single_run());
    assert_eq!(container.reach_at(2).map(|r| r.is_empty()), Some(true));
    assert!(value_reach.is_empty());
    assert!(std::ptr::eq(container.reach_at(0).unwrap(), value_reach));
    // One description for the whole build (the empty singleton); every later mint is a hit.
    assert_eq!(metrics.reach_interned, 1);
    assert_eq!(metrics.reach_intern_hits, 3);
    assert_eq!(metrics.reach_retention_folds, 1);
}

/// A pinned input's run names its members **and** its home region, as an ordinary member.
#[test]
fn a_pinned_run_names_its_members_and_its_home() {
    let dest = frame();
    let source = frame();
    let member = frame();
    let value = store(&dest, 7);

    let (container, value_reach) = Sectioned::build(
        RegionHandle::from_owner(&*dest),
        vec![cell(value, pinned_input(&source, &member))],
    );

    let run_reach = container.reach_at(0).expect("index is covered");
    assert!(members_are(run_reach, &[&source, &member]));
    // A single-run container's value-level description is that run's own entry, by interning.
    assert!(std::ptr::eq(run_reach, value_reach));
}

/// A seed input's run is exactly the coverage the caller declared — no stored description involved.
#[test]
fn a_seed_run_is_its_declared_coverage() {
    let dest = frame();
    let seeded = frame();
    let value = store(&dest, 7);

    let (container, _value_reach) = Sectioned::build(
        RegionHandle::from_owner(&*dest),
        vec![cell(
            value,
            CellReach::Seed(StepCoverage::of(Rc::clone(&seeded))),
        )],
    );

    assert!(members_are(
        container.reach_at(0).expect("index is covered"),
        &[&seeded]
    ));
}

/// The value-level description is the union over the runs.
#[test]
fn the_value_level_description_is_the_union_over_runs() {
    let dest = frame();
    let (left_home, left_member) = (frame(), frame());
    let right = frame();
    let values: Vec<&u32> = (0..2).map(|v| store(&dest, v)).collect();

    let (container, value_reach) = Sectioned::build(
        RegionHandle::from_owner(&*dest),
        vec![
            cell(values[0], pinned_input(&left_home, &left_member)),
            cell(
                values[1],
                CellReach::Seed(StepCoverage::of(Rc::clone(&right))),
            ),
        ],
    );

    assert_eq!(container.run_count(), 2);
    assert!(members_are(
        value_reach,
        &[&left_home, &left_member, &right]
    ));
}

/// The union accumulates the **pre-mint** source bundles: a cell borrowing into `dest` itself keeps
/// home in the value-level description. Folding the bundles the mints hand back instead would drop
/// it, because the self rule strips `dest` from a returned bundle while leaving it in the
/// description.
#[test]
fn a_cell_borrowing_home_keeps_home_in_the_value_level_description() {
    let dest = frame();
    let value = store(&dest, 7);

    let (container, value_reach) = Sectioned::build(
        RegionHandle::from_owner(&*dest),
        vec![cell(
            value,
            CellReach::Seed(StepCoverage::of(Rc::clone(&dest))),
        )],
    );

    assert!(container
        .reach_at(0)
        .expect("index is covered")
        .borrows_home());
    assert!(value_reach.borrows_home());
    assert!(members_are(value_reach, &[&dest]));
}

/// Two pinned inputs of equal reach cost one description and one retention fold: the second mint is
/// an intern hit, and a hit skips the fold.
#[test]
fn equal_reach_inputs_cost_one_description_and_one_fold() {
    let dest = frame();
    let source = frame();
    let member = frame();
    let values: Vec<&u32> = (0..2).map(|v| store(&dest, v)).collect();
    // Mint the input's own description outside the measured window.
    let first = pinned_input(&source, &member);
    let second = pinned_input(&source, &member);

    let ((container, value_reach), metrics) = reach_delta(|| {
        Sectioned::build(
            RegionHandle::from_owner(&*dest),
            vec![cell(values[0], first), cell(values[1], second)],
        )
    });

    assert!(container.is_single_run());
    assert!(std::ptr::eq(container.reach_at(1).unwrap(), value_reach));
    assert_eq!(metrics.reach_interned, 1);
    assert_eq!(metrics.reach_intern_hits, 2);
    assert_eq!(metrics.reach_retention_folds, 1);
}

/// A container is `Copy` and `Drop`-free: both its slices are bumped into the region, so it is
/// region state a holder *names* rather than owns. That is what lets a frame teardown release a
/// container with the region's chunk instead of walking it.
#[test]
fn a_container_is_copy_and_drop_free() {
    fn assert_copy<T: Copy>(_: &T) {}

    let dest = frame();
    let values: Vec<&u32> = (0..3).map(|v| store(&dest, v)).collect();
    let (container, _value_reach) = Sectioned::build(
        RegionHandle::from_owner(&*dest),
        values.iter().copied().map(owned).collect(),
    );

    assert!(!std::mem::needs_drop::<
        Sectioned<'_, CellFamily, SectionFrame>,
    >());
    assert_copy(&container);
    // A copy names the same region state, so it reads identically.
    let duplicate = container;
    assert_eq!(duplicate.len(), container.len());
    assert!(std::ptr::eq(
        duplicate.reach_at(0).unwrap(),
        container.reach_at(0).unwrap()
    ));
}

/// The parting seam hands a cell out **bundled** with exactly its own run's reach — never the
/// container's union — and the pairing is one value, not two the caller must keep aligned.
#[test]
fn project_bundles_a_cell_with_exactly_its_run_reach() {
    let dest = frame();
    let source = frame();
    let member = frame();
    let values: Vec<&u32> = (0..2).map(|v| store(&dest, v + 1)).collect();

    let (container, value_reach) = Sectioned::build(
        RegionHandle::from_owner(&*dest),
        vec![
            owned(values[0]),
            cell(values[1], pinned_input(&source, &member)),
        ],
    );

    // The container's own reach names both source regions; the owned cell's does not.
    assert!(members_are(value_reach, &[&source, &member]));

    let plain = container.project(0).expect("index is covered");
    assert_eq!(*plain.value(), 1);
    assert!(!plain.reach_covers(source.region()));

    let borrowing = container.project(1).expect("index is covered");
    assert_eq!(*borrowing.value(), 2);
    assert!(borrowing.reach_covers(source.region()));
    assert!(borrowing.reach_covers(member.region()));
    // Release-exact: the parted cell resides in `dest` but does not borrow into it.
    assert!(!borrowing.borrows_home());
}

/// An empty container is well-formed: no cells, no runs, and an empty value-level description.
#[test]
fn an_empty_container_has_no_runs() {
    let dest = frame();

    let (container, value_reach): (Sectioned<'_, CellFamily, SectionFrame>, _) =
        Sectioned::build(RegionHandle::from_owner(&*dest), Vec::new());

    assert!(container.is_empty());
    assert_eq!(container.run_count(), 0);
    assert!(container.reach_at(0).is_none());
    assert!(value_reach.is_empty());
}

/// The door folds the cells' input weights into one stored container total, independent of how the
/// reach verdicts partition them into runs.
#[test]
fn weight_sums_the_cells_across_runs() {
    let dest = frame();
    let source = frame();
    let member = frame();
    let values: Vec<&u32> = [3u32, 40, 500]
        .into_iter()
        .map(|v| store(&dest, v))
        .collect();

    let (container, _value_reach) = Sectioned::build(
        RegionHandle::from_owner(&*dest),
        vec![
            owned(values[0]),
            cell(values[1], pinned_input(&source, &member)),
            owned(values[2]),
        ],
    );

    // Three runs, one total: the fixture weighs each cell as its own content.
    assert_eq!(container.run_count(), 3);
    assert_eq!(container.weight(), 543);
}

/// An empty container weighs nothing, and the fold saturates rather than wrapping — an immense
/// total reads as immense.
#[test]
fn weight_is_zero_when_empty_and_saturates_when_immense() {
    let dest = frame();
    let handle = RegionHandle::from_owner(&*dest);

    let (empty, _): (Sectioned<'_, CellFamily, SectionFrame>, _) =
        Sectioned::build(handle, Vec::new());
    assert_eq!(empty.weight(), 0);

    let payload = store(&dest, 0);
    let huge = || CellInput::<CellFamily, SectionFrame> {
        payload,
        reach: CellReach::Owned,
        weight: u64::MAX,
    };
    let (saturated, _) = Sectioned::build(handle, vec![huge(), huge()]);
    assert_eq!(saturated.weight(), u64::MAX);
}
