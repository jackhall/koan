//! The sectioned-storage slate: run grouping, index lookup, and the alloc door
//! ([`Sectioned::build`]) — over a library-only profile (`RegionHost` frames, `u32` content), so no
//! embedder type is in scope. Reach counts are read as deltas off the thread-local
//! [`RegionMetrics`], scoped so a test's setup mints land outside its measured window.

use std::rc::Rc;

use super::super::*;

/// The library-only profile the sectioned slate runs over: owned `u32` content, which the payload
/// borrows out of a region.
struct SectionProfile;

impl StorageProfile for SectionProfile {
    type Families = (ValFamily, ());
    type FrameOwner = RegionHost<SectionProfile>;
}

type SectionFrame = RegionHost<SectionProfile>;

/// Owned scalar content — what a region stores and a cell payload borrows.
struct ValFamily;

reattachable! {
    ValFamily => u32,
}

impl Stored<SectionProfile> for ValFamily {
    fn cell(storage: &StorageOf<SectionProfile>) -> &FamilyArena<Self> {
        &storage.0
    }
}

fn frame() -> Rc<SectionFrame> {
    RegionHost::fresh(None)
}

/// Store `v` in `frame`'s region and hand back the co-located borrow — a cell payload.
fn store(frame: &Rc<SectionFrame>, v: u32) -> &u32 {
    RegionHandle::from_owner(&**frame).alloc_resident::<ValFamily>(v)
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
    let (reach, _bundle) = ReachDescription::mint(
        RegionHandle::from_owner(&**home),
        &[&PinBundle::singleton(Rc::clone(member))],
    );
    let mut coverage = StepCoverage::of(Rc::clone(home));
    coverage.absorb(StepCoverage::of(Rc::clone(member)));
    CellReach::Pinned { reach, coverage }
}

/// A cell that is fully owned at the destination.
fn owned(payload: &u32) -> CellInput<'_, &u32, SectionFrame> {
    CellInput {
        payload,
        reach: CellReach::Owned,
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
            CellInput {
                payload: values[2],
                reach: pinned_input(&source, &member),
            },
            CellInput {
                payload: values[3],
                reach: pinned_input(&source, &member),
            },
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
            CellInput {
                payload: values[0],
                reach: pinned_input(&source, &member),
            },
            owned(values[1]),
            CellInput {
                payload: values[2],
                reach: pinned_input(&source, &member),
            },
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
            CellInput {
                payload,
                reach: pinned_input(&source, &member),
            }
        });
    }
    let (container, _value_reach) = Sectioned::build(RegionHandle::from_owner(&*dest), inputs);

    assert_eq!(container.run_count(), 4);
    for index in 0..container.len() {
        let reach = container.reach_at(index).expect("index is covered");
        assert_eq!(reach.is_empty(), index % 2 == 0);
    }
    assert!(container.reach_at(container.len()).is_none());
    assert!(container.cell(container.len()).is_none());
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
            values.iter().map(|v| owned(v)).collect(),
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
        vec![CellInput {
            payload: value,
            reach: pinned_input(&source, &member),
        }],
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
        vec![CellInput {
            payload: value,
            reach: CellReach::Seed(StepCoverage::of(Rc::clone(&seeded))),
        }],
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
            CellInput {
                payload: values[0],
                reach: pinned_input(&left_home, &left_member),
            },
            CellInput {
                payload: values[1],
                reach: CellReach::Seed(StepCoverage::of(Rc::clone(&right))),
            },
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
        vec![CellInput {
            payload: value,
            reach: CellReach::Seed(StepCoverage::of(Rc::clone(&dest))),
        }],
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
            vec![
                CellInput {
                    payload: values[0],
                    reach: first,
                },
                CellInput {
                    payload: values[1],
                    reach: second,
                },
            ],
        )
    });

    assert!(container.is_single_run());
    assert!(std::ptr::eq(container.reach_at(1).unwrap(), value_reach));
    assert_eq!(metrics.reach_interned, 1);
    assert_eq!(metrics.reach_intern_hits, 2);
    assert_eq!(metrics.reach_retention_folds, 1);
}

/// An empty container is well-formed: no cells, no runs, and an empty value-level description.
#[test]
fn an_empty_container_has_no_runs() {
    let dest = frame();

    let (container, value_reach): (Sectioned<'_, &u32, SectionFrame>, _) =
        Sectioned::build(RegionHandle::from_owner(&*dest), Vec::new());

    assert!(container.is_empty());
    assert_eq!(container.run_count(), 0);
    assert!(container.reach_at(0).is_none());
    assert!(value_reach.is_empty());
}
