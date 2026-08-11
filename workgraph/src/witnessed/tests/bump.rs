//! The bump-door slate: what [`FoldedPlacement::fold_and_bump`] composes, what it retains, and what
//! it costs in bytes — over a library-only profile with an **empty family list**, which is the
//! acceptance criterion made observable. Nothing here declares a storage policy or any residence
//! check, yet a value holding an `&'b` back into its own region stores fine.
//!
//! Reach counts are read as deltas off the thread-local [`RegionMetrics`], scoped so a test's setup
//! mints land outside its measured window (mirroring the sectioned slate).

use std::ptr;
use std::rc::Rc;

use allocator_api2::vec::Vec as BumpVec;
use hashbrown::{DefaultHashBuilder, HashMap};

use super::super::*;

/// The profile the bump slate runs over: **no families at all**. A region still holds its bump, its
/// reach side table and its union bundle, which is everything the door touches.
struct BumpProfile;

impl StorageProfile for BumpProfile {
    type FrameOwner = RegionHost<BumpProfile>;
}

type BumpFrame = RegionHost<BumpProfile>;

/// The operand / product family: text living in a region's bump.
struct WordFamily;

/// A value that holds an `&'r` **back into its own region** — the shape a lifetime-typed cell cannot store
/// without erasing the borrow and auditing it back, and the bump stores with neither.
struct SpanFamily;

reattachable! {
    WordFamily => &'r str,
    SpanFamily => &'r (&'r str, usize),
}

fn frame() -> Rc<BumpFrame> {
    RegionHost::fresh(None)
}

/// The reach counters' movement across `body` — a delta rather than a [`reset_region_metrics`]
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

/// An operand carrier: `text` living in `home`'s region, its borrows reaching every frame in
/// `members`. Built through [`Opened::adopted`] — the same constructor
/// [`Sectioned::project`](super::super::Sectioned::project) parts a cell through — so the operand
/// arrives exactly as a real one does.
fn operand<'b>(
    text: &'b str,
    home: &'b Rc<BumpFrame>,
    members: &[&Rc<BumpFrame>],
) -> Opened<'b, WordFamily, Carrier<BumpFrame>> {
    let bundles: Vec<PinBundle<BumpFrame>> = members
        .iter()
        .map(|m| PinBundle::singleton(Rc::clone(m)))
        .collect();
    let refs: Vec<&PinBundle<BumpFrame>> = bundles.iter().collect();
    let reach = ReachDescription::mint_resident(RegionHandle::from_owner(&**home), &refs);
    Opened::adopted(text, Carrier::new(reach))
}

/// The door, over a forged placement — a unit test has no enclosing fold engine to mint one.
fn placement(dest: &Rc<BumpFrame>) -> FoldedPlacement<'_, BumpProfile> {
    FoldedPlacement::forge_for_test(RegionHandle::from_owner(&**dest))
}

/// Whether `description`'s members are exactly `expected`, compared by owner identity.
fn members_are(description: &ReachDescription<BumpFrame>, expected: &[&Rc<BumpFrame>]) -> bool {
    let members = description.members();
    members.len() == expected.len()
        && expected
            .iter()
            .all(|want| members.iter().any(|got| Rc::ptr_eq(got, want)))
}

/// A door call with no operands: the product's reach is empty *structurally* — there is no coverage
/// claim for a call site to write — and its residence is still recorded.
#[test]
fn no_operands_yields_empty_reach_hosted_in_the_destination() {
    let dest = frame();
    let product = placement(&dest).fold_and_bump::<WordFamily, WordFamily, BumpFrame>(
        &[],
        |bump, operands| {
            assert!(operands.is_empty(), "no operands were passed");
            bump.text("koan")
        },
    );

    assert_eq!(product.value(), "koan");
    assert!(!product.has_reach_members(), "nothing composed in");
    assert!(product.with_home_region(|home| ptr::eq(home, dest.region())));
    assert_eq!(
        dest.region().retained_reach_len(),
        0,
        "an empty reach retains nothing"
    );
}

/// An operand living somewhere else contributes its own members **and** its home — nothing else
/// would pin the region it lives in — and the destination's union bundle grows to own both.
#[test]
fn a_foreign_operand_contributes_its_members_and_its_home() {
    let dest = frame();
    let source = frame();
    let member = frame();
    let word = operand("in-flight", &source, &[&member]);

    let product = placement(&dest)
        .fold_and_bump::<WordFamily, WordFamily, BumpFrame>(&[&word], |bump, operands| {
            bump.text(operands[0])
        });

    assert_eq!(product.value(), "in-flight");
    assert!(product.with_reach(|reach| members_are(reach, &[&source, &member])));
    assert!(
        product.with_home_region(|home| ptr::eq(home, dest.region())),
        "the product lives where it was minted"
    );
    assert_eq!(
        dest.region().retained_reach_len(),
        2,
        "the destination owns a pin on each region the product reaches"
    );
}

/// The run-level self rule: an operand already resident in the destination is covered by the
/// destination's own liveness, so its home is **not** folded in — while its own members still are.
#[test]
fn a_co_resident_operand_does_not_name_the_destination() {
    let dest = frame();
    let member = frame();
    let word = operand("resident", &dest, &[&member]);

    let product = placement(&dest)
        .fold_and_bump::<WordFamily, WordFamily, BumpFrame>(&[&word], |bump, operands| {
            bump.text(operands[0])
        });

    assert!(product.with_reach(|reach| members_are(reach, &[&member])));
    assert!(
        !product.reach_covers(dest.region()),
        "living in the destination is not borrowing into it"
    );
    assert!(!product.borrows_home());
}

/// Two operands compose as a union under outer-chain subsumption: a member kept alive by another
/// member's owner chain is dropped.
#[test]
fn two_operands_compose_under_subsumption() {
    let dest = frame();
    let outer = frame();
    let inner = RegionHost::<BumpProfile>::fresh(Some(Rc::clone(&outer)));
    let left = operand("left", &dest, &[&inner]);
    let right = operand("right", &dest, &[&outer]);

    let product = placement(&dest)
        .fold_and_bump::<WordFamily, WordFamily, BumpFrame>(&[&left, &right], |bump, operands| {
            bump.text(&format!("{}+{}", operands[0], operands[1]))
        });

    assert_eq!(product.value(), "left+right");
    assert!(
        product.with_reach(|reach| members_are(reach, &[&inner])),
        "the inner frame's chain already pins the outer one, so the outer member is subsumed"
    );
    assert!(
        product.with_reach(|reach| reach.pins_region(outer.region())),
        "and the surviving member genuinely reports it pinned"
    );
}

/// A second door call over the same reach is an **intern hit**: the description already exists in
/// the destination, and the region's union already pins its members, so nothing folds twice.
#[test]
fn a_repeat_reach_interns_and_folds_no_second_retention() {
    let dest = frame();
    let source = frame();
    let word = operand("shared", &source, &[]);

    let (_first, first_metrics) = reach_delta(|| {
        placement(&dest)
            .fold_and_bump::<WordFamily, WordFamily, BumpFrame>(&[&word], |bump, operands| {
                bump.text(operands[0])
            });
    });
    assert_eq!(first_metrics.reach_interned, 1);
    assert_eq!(first_metrics.reach_retention_folds, 1);

    let (_second, second_metrics) = reach_delta(|| {
        placement(&dest)
            .fold_and_bump::<WordFamily, WordFamily, BumpFrame>(&[&word], |bump, operands| {
                bump.text(operands[0])
            });
    });
    assert_eq!(second_metrics.reach_intern_hits, 1);
    assert_eq!(second_metrics.reach_interned, 0);
    assert_eq!(
        second_metrics.reach_retention_folds, 0,
        "the region already pins everything this description names"
    );
    assert_eq!(dest.region().retained_reach_len(), 1);
}

/// The figure is **reserved chunk capacity**: it covers at least what was stored, it never shrinks,
/// and a door call that bumps nothing reserves nothing. Asserted as lower bounds rather than exact
/// totals — chunk sizing is the allocator's own policy, and the whole point of the figure is that it
/// includes the padding and unused tail a pin would retain along with the live bytes.
#[test]
fn bump_capacity_reports_reserved_chunks() {
    let dest = frame();
    assert_eq!(
        dest.region().bump_capacity(),
        0,
        "a fresh bump reserves no chunk"
    );

    placement(&dest).fold_and_bump::<SpanFamily, WordFamily, BumpFrame>(&[], |bump, _| {
        bump.value(("koan", 4usize))
    });
    let live_after_value = size_of::<(&str, usize)>();
    let after_value = dest.region().bump_capacity();
    assert!(
        after_value >= live_after_value,
        "capacity covers at least what is stored, plus whatever chunk floor the allocator reserved"
    );

    placement(&dest).fold_and_bump::<WordFamily, WordFamily, BumpFrame>(&[], |bump, _| {
        let block: Vec<u64> = (0..4096).collect();
        let _stored: &[u64] = bump.slice(&block);
        "done"
    });
    let live_after_slice = live_after_value + 4096 * size_of::<u64>();
    let after_slice = dest.region().bump_capacity();
    assert!(
        after_slice >= live_after_slice && after_slice >= after_value,
        "a slice's whole span is reserved, and capacity is monotonic"
    );

    placement(&dest)
        .fold_and_bump::<WordFamily, WordFamily, BumpFrame>(&[], |bump, _| bump.text("tail"));
    let after_text = dest.region().bump_capacity();
    assert!(
        after_text >= live_after_slice + "tail".len() && after_text >= after_slice,
        "text's byte length is reserved too"
    );

    // A door call that bumps nothing — the constructor returns owned `'static` data — reserves no
    // chunk, so the reading is unchanged.
    placement(&dest).fold_and_bump::<WordFamily, WordFamily, BumpFrame>(&[], |_bump, _| "literal");
    assert_eq!(
        dest.region().bump_capacity(),
        after_text,
        "a door call that stores nothing reserves nothing"
    );
}

/// The product rests like any other open: [`Opened::reseal`] and re-open round-trip both facts the
/// carrier holds — what the value reaches, and where it lives.
#[test]
fn the_product_reseals_and_reopens_with_its_pairing_intact() {
    let dest = frame();
    let source = frame();
    let member = frame();
    let word = operand("travelling", &source, &[&member]);

    let product = placement(&dest)
        .fold_and_bump::<WordFamily, WordFamily, BumpFrame>(&[&word], |bump, operands| {
            bump.text(operands[0])
        });

    let resealed = product.reseal();
    let reopened = resealed.open_at();
    assert_eq!(reopened.value(), "travelling");
    assert!(reopened.reach_covers(member.region()));
    assert!(reopened.reach_covers(source.region()));
    assert!(reopened.with_home_region(|home| ptr::eq(home, dest.region())));
}

/// **The acceptance criterion, made observable.** A value stored through the door holds an `&'b`
/// into the very region it lives in. [`BumpProfile`] declares no family at all, so no residence
/// check ran — the bump needs none, because it erases nothing.
#[test]
fn a_stored_value_may_borrow_its_own_region_with_no_residence_audit() {
    let dest = frame();

    let span = placement(&dest).fold_and_bump::<SpanFamily, WordFamily, BumpFrame>(
        &[],
        |bump, _operands| {
            // The text lands in `dest`'s bump, and the pair stored beside it borrows straight back
            // into that same region.
            let text: &str = bump.text("self-referential");
            bump.value((text, text.len()))
        },
    );

    let (text, length) = *span.value();
    assert_eq!(text, "self-referential");
    assert_eq!(length, "self-referential".len());
    assert!(
        !span.has_reach_members(),
        "borrowing its own region is residence, not reach"
    );
    assert!(span.with_home_region(|home| ptr::eq(home, dest.region())));
}

/// The keyed index shape: a table built and placed in the region's bump, read back by key. Glue-free
/// elements are the whole admission criterion — nothing here declares a family, a storage policy or
/// an audit, exactly as for the other primitives.
#[test]
fn a_bumped_map_indexes_its_entries() {
    let dest = frame();
    let handle = RegionHandle::<BumpProfile>::from_owner(&*dest);

    let index = handle
        .allocator()
        .frozen_table([(10u32, 0usize), (20, 1), (30, 2)]);

    assert_eq!(index.len(), 3);
    assert!(!index.is_empty());
    assert_eq!(index.get(&20), Some(&1));
    assert_eq!(index.get(&99), None, "an absent key indexes nothing");
    let mut pairs: Vec<(u32, usize)> = index.iter().map(|(k, v)| (*k, *v)).collect();
    pairs.sort_unstable();
    assert_eq!(
        pairs,
        vec![(10, 0), (20, 1), (30, 2)],
        "iteration covers every pair, in an order the door does not promise"
    );
}

/// An empty table is a legitimate index.
#[test]
fn an_empty_bumped_map_holds_nothing() {
    let dest = frame();
    let handle = RegionHandle::<BumpProfile>::from_owner(&*dest);

    let index = handle.allocator().frozen_table(Vec::<(u32, usize)>::new());

    assert!(index.is_empty());
    assert_eq!(index.len(), 0);
    assert_eq!(index.get(&0), None);
}

/// A table may key on bytes from **its own region**, like every other bumped value — and region
/// death is chunk release, with the table's own `Drop` never run. Under Miri's leak check that is
/// the acceptance criterion made observable: the bucket array is bump memory, so forgoing its
/// deallocation strands nothing.
#[test]
fn a_bumped_map_keys_on_its_own_regions_bytes_and_dies_with_it() {
    let dest = frame();
    {
        let handle = RegionHandle::<BumpProfile>::from_owner(&*dest);
        let names: Vec<&str> = ["alpha", "beta", "gamma"]
            .into_iter()
            .map(|name| handle.allocator().text(name))
            .collect();
        let index = handle
            .allocator()
            .frozen_table(names.iter().copied().zip(0usize..));

        assert_eq!(index.get(&"beta"), Some(&1));
        assert_eq!(index.len(), 3);
    }
    drop(dest);
}

/// The **mutable** seam, where [`BumpAllocator::frozen_table`] is the write-once one: a table built over
/// [`BumpAllocator`] and then churned — grown past several resizes, overwritten in place, removed
/// from, retained over — with every bucket array it ever allocates landing in the region's chunks.
///
/// Under tree borrows this is the acceptance criterion for handing the raw allocator out, and the
/// two reallocation paths are **both** the point: bumpalo satisfies `grow` in place when the old
/// allocation is the newest one out of the chunk, and falls back to allocate-copy-abandon when it is
/// not. The retagging each performs has to stay sound against the reads that follow. A second table
/// interleaved with the first is what forces the fallback — an embedder's tables share one bump, so
/// the allocation a table is about to grow is rarely the newest one — and a bumped `&str` between
/// the inserts stands in for the key text a real embedder re-homes as it writes.
///
/// Under the leak check it is the second criterion: every abandoned bucket array is bump memory the
/// region releases whole, so the tables' un-run `Drop`s strand nothing.
#[test]
fn a_bump_backed_table_survives_growth_overwrite_and_removal() {
    let dest = frame();
    {
        let handle = RegionHandle::<BumpProfile>::from_owner(&*dest);
        let before = dest.region().bump_capacity();
        let mut table: HashMap<u32, u64, DefaultHashBuilder, BumpAllocator<'_>> =
            HashMap::new_in(handle.allocator());
        let mut sibling: HashMap<u32, &str, DefaultHashBuilder, BumpAllocator<'_>> =
            HashMap::new_in(handle.allocator());

        // Enough inserts to force several geometric resizes off hashbrown's initial capacity, with
        // a sibling table and loose bytes taking the chunk's tail in between, so neither table's
        // bucket array is the newest allocation when it needs to grow.
        for key in 0..512u32 {
            table.insert(key, u64::from(key) * 3);
            sibling.insert(key, handle.allocator().text(&format!("key_{key}")));
        }
        assert_eq!(table.len(), 512);
        assert_eq!(table.get(&511), Some(&1533));
        assert_eq!(sibling.get(&511), Some(&"key_511"));

        // In-place overwrite: the slot is reused, so peak occupancy is unchanged.
        for key in 0..512u32 {
            table.insert(key, u64::from(key));
        }
        assert_eq!(table.get(&511), Some(&511));

        for key in (0..512u32).step_by(2) {
            assert_eq!(table.remove(&key), Some(u64::from(key)));
        }
        table.retain(|key, _| key % 3 != 0);
        assert!(table.keys().all(|key| key % 2 == 1 && key % 3 != 0));
        // Re-inserting into the tombstoned slots reuses them rather than growing again.
        for key in (0..512u32).step_by(2) {
            table.insert(key, u64::from(key) * 7);
        }
        assert_eq!(table.get(&510), Some(&3570));

        // A growable vec over the same allocator: `grow` past several reallocations, then a shrink,
        // which the bump answers by abandoning bytes rather than returning them.
        let mut run: BumpVec<u32, BumpAllocator<'_>> = BumpVec::new_in(handle.allocator());
        run.extend(0..4096u32);
        assert_eq!(run.iter().copied().sum::<u32>(), (0..4096u32).sum());
        run.truncate(16);
        run.shrink_to_fit();
        assert_eq!(run.len(), 16);

        assert!(
            dest.region().bump_capacity() > before,
            "every bucket array and vec buffer the churn allocated is priced by the region's own \
             capacity figure, with no counted door involved"
        );
    }
    drop(dest);
}

/// [`FoldedPlacement::allocator`] writes to the destination the enclosing fold composed its witness
/// over, with no storage policy and no erase/re-anchor round trip.
/// A value written through it may hold an `&'b` back into that very region, which is the whole point
/// of the bump: a lifetime-typed cell cannot, because its slot type would have to name `'r`.
#[test]
fn a_fold_allocator_writes_a_self_referential_value_into_its_own_destination() {
    let dest = frame();
    let placement = placement(&dest);

    let text: &str = placement.allocator().text("koan");
    // The stored value borrows the bytes bumped a line above — same region, no audit, no erasure.
    let cell: &&str = placement.allocator().value(text);

    assert_eq!(*cell, "koan");
    assert!(
        dest.region().bump_capacity() >= "koan".len() + size_of::<&str>(),
        "both the bytes and the cell pointing at them land in the destination's bump"
    );
    assert_eq!(
        dest.region().retained_reach_len(),
        0,
        "a bare byte write composes no reach and retains nothing"
    );
}
