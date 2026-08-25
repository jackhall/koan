//! Miri slate (tree borrows) for the lifetime-erasure carrier. Every test carries a *real* borrow
//! into the erased store and reads it back after the original binding drops, so the erase → reattach
//! → read round-trip pins the lifetime-fabricated read under tree borrows. Names only stand-in
//! families and a stand-in cart (`TestCart`: a region-backing `Vec` plus an `outer` ancestor
//! chain), never a koan type. Fails on UB, not values. The escape-can't-compile guards live as
//! `compile_fail` doctests on [`Witnessed::with`] / [`Witnessed::map`] / [`Witnessed::yoke`].

use std::cell::Cell;
use std::rc::Rc;

use super::*;

/// The abstract-shape slate: home as an ordinary member of the envelope's pins, the copy-versus-pin
/// source claim, duplication, the mint's self rule, the three carrier states and their transform
/// verbs, and the finish-surface fold — over a library-only profile.
mod shapes;

/// The reach side table's interning slate: get-or-mint keyed on the canonical member set, the
/// skipped retention fold on a hit, and the per-region empty singleton.
mod intern;

/// The pin-ring slate: the debug-mode detector at [`Region::retain_reach`], over the mutual pin, the
/// chain-mediated ring, and the two shapes that are not rings at all.
mod pin_cycles;

/// The sectioned-storage slate: run grouping, index lookup, and the alloc door.
mod sectioned;

/// The bump-door slate: reach composition and retention at [`FoldedPlacement::fold_and_bump`], the
/// occupancy reader, and a region-self-referential store with no residence audit.
mod bump;

/// The bump-residence slate: what a same-region value bumped through
/// [`BumpAllocator::in_place`] and a crossing one built through [`RegionHandle::bump_born_with`]
/// store, over an invariant family that names its own region and a parent in another one.
mod born;

/// Covariant stand-in — a shared reference whose lifetime the borrow checker cannot track across
/// the `'static` store.
struct RefFamily;

/// Invariant stand-in — the case that actually matters. `Cell<&'r u32>` is invariant in `'r`
/// (interior mutability over a `'r` reference), exactly like koan's `Scope` binding table.
struct InvFamily;

/// Mutable-scope-plus-pool family: a carrier holding a mutable "scope" slot AND a cart-coherent
/// "pool" (both share `'r` — the region). Stands in for koan's continuation, whose `map` binds a
/// cart-coherent value into the invariant scope slot.
struct ScopeFamily;
struct ScopeAndPool<'r> {
    scope: Cell<Option<&'r u32>>,
    pool: &'r [u32],
}

/// Non-`Copy` stand-in: a boxed borrow. `At<'r>` is a `Box<&'r u32>`, which (like koan's boxed
/// `NodeContinuation`) is consumed by value, not copied — the case [`SealedExtern::open`] admits and
/// [`Sealed::open`]'s `At<'static>: Copy` bound excludes.
struct BoxFamily;

/// Fat-pointer stand-in: a boxed `dyn FnOnce` continuation over a captured borrow. Unlike
/// `BoxFamily` (a thin `Box`), the carrier is a **two-word** data + vtable pointer, so the `retype`
/// runs over a fat pointer — the shape an embedder's stored continuation takes.
struct DynContinuationFamily;
type TestContinuation<'r> = Box<dyn FnOnce() -> u32 + 'r>;

// Each stand-in is one type generic only in `'r` with a lifetime-independent layout; the shared
// `reattachable!` macro discharges the obligation.
reattachable! {
    RefFamily => &'r u32,
    InvFamily => Cell<&'r u32>,
    ScopeFamily => ScopeAndPool<'r>,
}
// These three carry drop glue, so they rest on the owned tier (`SealedPinned`) and take the
// `droppable` arm, which emits no `DropFree`.
reattachable! {
    droppable
    BoxFamily => Box<&'r u32>,
    DynContinuationFamily => TestContinuation<'r>,
    TieredContinuationFamily => TieredContinuation<'r>,
    GlueProbeFamily => GlueProbe<'r>,
}

/// Two-tier stand-in: the shape koan's stored `ContinuationCall` takes — an enum whose arms are a
/// **borrowed** fat pointer into the pinned region (a `Copy` closure bumped there, called by
/// reference) and an owning boxed one. The `Box` arm gives the whole enum drop glue, so the family
/// is droppable and rests on the owned tier even when the value in hand is the borrowed arm.
struct TieredContinuationFamily;

enum TieredContinuation<'r> {
    /// The bumped tier: a `&dyn Fn` resident in the region the seal's pin holds, so the retype runs
    /// over a fat pointer whose *referent* — not just the pointer — is lifetime-fabricated.
    Bumped(&'r (dyn Fn() -> u32 + 'r)),
    /// The owning tier, alongside it in the same enum, so the discriminant crosses the retype too.
    #[allow(dead_code)]
    Boxed(Box<dyn FnOnce() -> u32 + 'r>),
}

/// A region-only profile for the tiered carrier: the closure lives in the bump, nothing typed does.
struct TieredProfile;

impl StorageProfile for TieredProfile {
    type FrameOwner = RegionHost<TieredProfile>;
}

/// A droppable *and* region-pointing stand-in — the shape the owned tier exists for. Its
/// destructor dereferences `last`, a borrow into a region, so it is only sound to drop while that
/// region is still pinned.
struct GlueProbe<'r> {
    last: &'r u32,
    seen: Rc<Cell<u32>>,
}

/// Family for [`GlueProbe`] — a reference plus an `Rc`, layout identical for every `'r`.
struct GlueProbeFamily;

impl Drop for GlueProbe<'_> {
    fn drop(&mut self) {
        self.seen.set(*self.last);
    }
}

/// Cart stand-in for the witness-with-a-region cases (`yoke` / the composed merge): a backing `Vec` (the
/// "region") plus an `outer` link, mirroring `FrameStorage`'s region + ancestor-pin chain without
/// naming a koan type. Held by `Rc`, so the backing's heap address is stable (a `StableDeref`); a
/// descendant's `outer` chain keeps its ancestors' backings alive, exactly the relation the
/// `PinBundle` union reads.
struct TestCart {
    backing: Vec<u32>,
    outer: Option<Rc<TestCart>>,
}

// SAFETY: the backing lives inside the `Rc`-owned `TestCart` at a fixed heap address for the whole
// life of the `Rc`, so a value built from `&backing` is pinned by the witness.
unsafe impl RegionOwner for TestCart {
    type Region = [u32];
    fn region(&self) -> &[u32] {
        &self.backing
    }
}

// SAFETY: `pins_region` walks self's own region and its `outer` ancestor chain; holding self's
// `Rc` holds each ancestor `Rc` in turn, so every region the walk reports pinned stays live and
// fixed-address while self is held.
unsafe impl PinsRegion for TestCart {
    fn pins_region(&self, region: &[u32]) -> bool {
        let mut node = self;
        loop {
            if std::ptr::eq(&node.backing[..], region) {
                return true;
            }
            match &node.outer {
                Some(outer) => node = outer,
                None => return false,
            }
        }
    }

    #[cfg(debug_assertions)]
    fn for_each_pinned_region(&self, visit: &mut dyn FnMut(&[u32])) {
        let mut node = self;
        loop {
            visit(&node.backing[..]);
            match &node.outer {
                Some(outer) => node = outer,
                None => return,
            }
        }
    }
}

/// The witness-less primitive: [`Erased`] storage over a real borrow.
#[test]
fn erased_roundtrip() {
    let backing = [7u32, 8, 9];
    let erased: Erased<RefFamily> = Erased::erase(&backing[0]);
    // SAFETY: `backing` is held live to the end of the test, pinning the re-anchored borrow.
    let reattached: &u32 = unsafe { erased.reattach() };
    assert_eq!(*reattached, 7);

    // Re-read the borrow to catch a tree-borrows regression.
    assert_eq!(*reattached, 7);
}

/// `with_branded_ref`: re-anchor a reference-to-an-erased-store behind the rank-2 brand and copy a
/// scalar out — the region's store-side read. The fabricated `'b` stays confined to the closure
/// because `R` is a copied scalar that cannot name it.
#[test]
fn branded_ref_reads_erased_store() {
    let backing = [11u32, 22, 33];
    let stored: <RefFamily as Reattachable>::At<'static> =
        erase_to_static::<RefFamily>(&backing[0]);
    let value: u32 = with_branded_ref::<RefFamily, _>(&stored, |reref| **reref);
    assert_eq!(value, 11);
}

/// Covariant carrier round-trips after the original borrow drops; the bundled witness keeps it live.
/// The rank-2 closure returns a copied scalar (`'b`-independent), so nothing escapes.
#[test]
fn covariant_roundtrip_witness_only() {
    let backing: Rc<Vec<u32>> = Rc::new(vec![7, 8, 9]);
    let w: Witnessed<RefFamily, Rc<Vec<u32>>> = {
        let borrow: &u32 = &backing[0]; // original binding...
        Witnessed::from_erased(Erased::erase(borrow), Rc::clone(&backing))
    }; // ...dropped here; only the witness `Rc` inside `w` keeps `backing[0]` alive.
    drop(backing); // drop the other handle too — `w`'s witness is now the sole owner.
    assert_eq!(w.with(|r| **r), 7);
}

/// The load-bearing test: invariant carrier, original dropped, read via the witness pin through the
/// sound rank-2 accessor.
#[test]
fn invariant_roundtrip_witness_only() {
    let backing: Rc<Vec<u32>> = Rc::new(vec![10, 20, 30]);
    let w: Witnessed<InvFamily, Rc<Vec<u32>>> = {
        let cell: Cell<&u32> = Cell::new(&backing[1]);
        Witnessed::from_erased(Erased::erase(cell), Rc::clone(&backing))
    };
    drop(backing); // witness is sole owner now.
    assert_eq!(w.with(|c| *c.get()), 20);
    // Read again to catch a tree-borrows regression on the reattached view.
    assert_eq!(w.with(|c| *c.get()), 20);
}

/// `Witnessed::map` as branded projection: run the continuation inside the brand and bind
/// `&pool[i]` (a genuine `'b` ref, cart-coherent) into the invariant scope slot — the exact write
/// `with` rejects — then re-seal and read. Original dropped; Miri must stay clean.
#[test]
fn continuation_binds_cart_coherent_value_via_map() {
    let backing: Rc<Vec<u32>> = Rc::new(vec![11, 22, 33]);
    let pre: Witnessed<ScopeFamily, Rc<Vec<u32>>> = {
        let carrier = ScopeAndPool {
            scope: Cell::new(None),
            pool: &backing[..],
        };
        Witnessed::from_erased(Erased::erase(carrier), Rc::clone(&backing))
    };
    let post: Witnessed<ScopeFamily, Rc<Vec<u32>>> = pre.map(|c, _token: FoldToken<'_>| {
        c.scope.set(Some(&c.pool[2]));
        c
    });
    drop(backing); // witness is now the sole owner of the pool.
    assert_eq!(post.with(|c| *c.scope.get().unwrap()), 33);
}

/// Same-brand mutation is sound: set the cell to a value read out of the *same* branded cell — stays
/// within `'b`, no escape, no external ref. (Writing an external region ref is correctly rejected by
/// the rank-2 bound; that path needs `map`.)
#[test]
fn invariant_same_brand_mutation() {
    let backing: Rc<Vec<u32>> = Rc::new(vec![100, 200, 300]);
    let w: Witnessed<InvFamily, Rc<Vec<u32>>> =
        Witnessed::from_erased(Erased::erase(Cell::new(&backing[0])), Rc::clone(&backing));
    let got = w.with(|c| {
        let here = c.get();
        c.set(here);
        *c.get()
    });
    assert_eq!(got, 100);
    drop(backing);
    assert_eq!(w.with(|c| *c.get()), 100);
}

/// `yoke`: the carrier is sourced from the witness's own region inside the `for<'b>` closure, so its
/// reference is region-derived by construction. Read back after the original cart handle drops — the
/// bundled witness pins the backing the reference points into.
#[test]
fn yoke_sources_carrier_from_witness_region() {
    let cart: Rc<TestCart> = Rc::new(TestCart {
        backing: vec![5, 6, 7],
        outer: None,
    });
    let w: Witnessed<RefFamily, Rc<TestCart>> =
        Witnessed::yoke(Rc::clone(&cart), |region| &region[2]);
    drop(cart); // the bundled witness is now the sole owner of the backing.
    assert_eq!(w.with(|r| **r), 7);
    // Read again to catch a tree-borrows regression on the reattached view.
    assert_eq!(w.with(|r| **r), 7);
}

/// The pinned merge, reconstructed over the crate-private composition engine — the test door for
/// [`PinBundle`]'s [`ComposeWitness`] semantics (subsumption collapse, set union). The engine's
/// `fold` both projects the product and composes the witness inside one brand; the composition here
/// is the generic self-contained one, which owns what it names and so threads nothing out.
fn merge_for_test<T, B, P>(
    left: Witnessed<T, PinBundle<TestCart>>,
    right: Witnessed<B, PinBundle<TestCart>>,
    pin: &Rc<TestCart>,
    f: impl for<'b> FnOnce(T::At<'b>, B::At<'b>, FoldToken<'b>) -> P::At<'b>,
) -> Witnessed<P, PinBundle<TestCart>>
where
    T: Reattachable + DropFree,
    B: Reattachable + DropFree,
    P: Reattachable + DropFree,
    PinBundle<TestCart>: ComposeWitness<B>,
{
    let (out, ()) = left.merge_composed(right, pin, |l, r, left_view, right_view, token| {
        let witness = PinBundle::compose(l, r, &right_view);
        (f(left_view, right_view, token), witness, ())
    });
    out
}

/// The composed merge as the function-into-scope composition: a witnessed `ScopeFamily` carrier in
/// the *descendant* cart binds, at the shared brand, a witnessed `&u32` sourced from the *ancestor*
/// cart. The result is sealed under the descendant, whose `outer` chain keeps the ancestor backing
/// alive after both call handles drop. Miri must stay clean reading the bound ancestor ref back.
#[test]
fn compose_binds_ancestor_ref_into_descendant_scope() {
    let ancestor: Rc<TestCart> = Rc::new(TestCart {
        backing: vec![100, 200],
        outer: None,
    });
    let descendant: Rc<TestCart> = Rc::new(TestCart {
        backing: vec![1, 2, 3],
        outer: Some(Rc::clone(&ancestor)),
    });
    // Scope carrier in the descendant: empty slot, pool = the descendant's own region. Lifted into
    // the set world so `merge` composes totally.
    let scope_w: Witnessed<ScopeFamily, PinBundle<TestCart>> =
        Witnessed::yoke(Rc::clone(&descendant), |region| ScopeAndPool {
            scope: Cell::new(None),
            pool: region,
        })
        .rewitness(PinBundle::singleton(Rc::clone(&descendant)));
    // Function stand-in: a reference sourced from the ancestor's region.
    let fn_w: Witnessed<RefFamily, PinBundle<TestCart>> =
        Witnessed::yoke(Rc::clone(&ancestor), |region| &region[1])
            .rewitness(PinBundle::singleton(Rc::clone(&ancestor)));
    let merged: Witnessed<ScopeFamily, PinBundle<TestCart>> =
        merge_for_test::<ScopeFamily, RefFamily, ScopeFamily>(
            scope_w,
            fn_w,
            &descendant,
            |scope, func, _token: FoldToken<'_>| {
                scope.scope.set(Some(func));
                scope
            },
        );
    // Subsumption collapses the union to the descendant (whose `outer` chain already pins the
    // ancestor).
    assert!(matches!(
        merged.witness().members(),
        [only] if Rc::ptr_eq(only, &descendant)
    ));
    // `merged`'s witness is the descendant clone; its `outer` chain still pins the ancestor backing
    // the bound `&200` points into.
    drop(descendant);
    drop(ancestor);
    assert_eq!(merged.with(|c| *c.scope.get().unwrap()), 200);
}

/// The composed merge unions two unrelated carts into a two-member set — under the set currency
/// there is no failure verdict (unlike a single-region witness, which could not represent the
/// combined pin).
#[test]
fn compose_keeps_unrelated_carts_as_a_two_member_set() {
    let a: Rc<TestCart> = Rc::new(TestCart {
        backing: vec![1],
        outer: None,
    });
    let b: Rc<TestCart> = Rc::new(TestCart {
        backing: vec![2],
        outer: None,
    });
    let wa: Witnessed<RefFamily, PinBundle<TestCart>> =
        Witnessed::yoke(Rc::clone(&a), |r| &r[0]).rewitness(PinBundle::singleton(Rc::clone(&a)));
    let wb: Witnessed<RefFamily, PinBundle<TestCart>> =
        Witnessed::yoke(Rc::clone(&b), |r| &r[0]).rewitness(PinBundle::singleton(Rc::clone(&b)));
    let merged = merge_for_test::<RefFamily, RefFamily, RefFamily>(
        wa,
        wb,
        &a,
        |l, _r, _token: FoldToken<'_>| l,
    );
    assert_eq!(
        merged.witness().members().len(),
        2,
        "neither cart pins the other, so both remain in the set"
    );
}

/// [`Sealed::open_at`] + [`Opened::reseal`]: the borrow-tied in-use state. The step lifetime `'b`
/// rides the pin borrow, so the read cannot outlive the frame, and the resealed carrier's own
/// bundled witness keeps the pointee live once every original handle drops. (`open_at` copies the
/// value out, so it is a `Copy`-family verb — the invariant-`Cell` stress lives on the rank-2
/// `with` / `map` round-trips.)
#[test]
fn open_at_reseal_roundtrip() {
    let backing: Rc<Vec<u32>> = Rc::new(vec![5, 6, 7]);
    let sealed: Sealed<RefFamily, Rc<Vec<u32>>> = {
        let borrow: &u32 = &backing[2];
        Sealed::seal_bundled(Witnessed::from_erased(
            Erased::erase(borrow),
            Rc::clone(&backing),
        ))
    };
    // Pin held across the open; `'b` rides this borrow.
    let pin = Rc::clone(&backing);
    drop(backing); // the seal's bundled witness + `pin` keep the pointee live.
    let resealed: Sealed<RefFamily, Rc<Vec<u32>>> = {
        let opened = sealed.open_at();
        assert_eq!(*opened.value(), 7);
        opened.reseal()
    };
    // The resealed carrier carries its own cloned `Rc` witness, so it stays live after `pin` drops.
    drop(pin);
    assert_eq!(resealed.open(|r| *r), 7);
    // Read again to catch a tree-borrows regression on the reattached view.
    assert_eq!(resealed.open(|r| *r), 7);
}

/// [`SealedExtern::open`] — the **consuming, externally-witnessed** rank-2 open, distinct from the
/// bundled-witness [`Sealed::open`]. The invariant value is read back after its original binding
/// drops: the separately-held witness pins the pointee for the call, and the `for<'b>` brand
/// confines the read. Fails on UB, not values.
#[test]
fn sealed_extern_open_externally_witnessed() {
    let backing: Rc<Vec<u32>> = Rc::new(vec![5, 6, 7]);
    let sealed: SealedExtern<InvFamily> = {
        // Erase a real, invariant borrow; the original `Cell` binding drops at the block end, so the
        // read below rides only the lifetime-fabricated reattach the witness pins.
        let borrow: &u32 = &backing[1];
        SealedExtern::erase(Cell::new(borrow))
    };
    // Witness held across the open (a clone separate from the carrier — the externally-witnessed
    // model, where bundling it would be a redundant owner). The brand confines the read to the call.
    let seen: u32 = sealed.open(&backing, |cell: Cell<&u32>| *cell.get());
    assert_eq!(seen, 6);
    // Re-read the region through a sibling `Rc` after the open to catch a tree-borrows regression.
    let _again: &u32 = &backing[2];
}

/// [`SealedPinned::open`] over a **non-`Copy`** carrier: a `Box<&u32>` moves through the seal and is
/// consumed by the open, so the owned tier admits the boxed continuation shape the Copy tier's
/// `DropFree` bound excludes. The trivial extern operand is what a caller with nothing to zip
/// passes — the tier has one open verb. Fails on UB, not values.
#[test]
fn sealed_pinned_open_consumes_non_copy() {
    let backing: Rc<Vec<u32>> = Rc::new(vec![10, 20]);
    let sealed: SealedPinned<BoxFamily, Rc<Vec<u32>>> = {
        let borrow: &u32 = &backing[0];
        SealedPinned::erase(Box::new(borrow), Rc::clone(&backing))
    };
    let operand: SealedExtern<RefFamily> = SealedExtern::erase(&backing[1]);
    let seen: u32 = sealed.open(
        operand,
        &backing,
        |_within, boxed: Box<&u32>, other: &u32| **boxed + *other,
    );
    assert_eq!(seen, 30);
    let _again: &u32 = &backing[1];
}

/// [`SealedPinned::open`] over a **fat-pointer** carrier: a `Box<dyn FnOnce>` continuation is
/// **invoked inside the brand**, so the retype runs over a two-word data + vtable pointer (the
/// stored-continuation shape) and tree borrows checks the capture read through the
/// lifetime-fabricated box. Fails on UB, not values.
#[test]
fn sealed_pinned_open_invokes_a_fat_pointer_continuation() {
    let backing: Rc<Vec<u32>> = Rc::new(vec![7, 8, 9]);
    let sealed: SealedPinned<DynContinuationFamily, Rc<Vec<u32>>> = {
        let captured: &u32 = &backing[2];
        SealedPinned::erase(
            Box::new(move || *captured) as TestContinuation<'_>,
            Rc::clone(&backing),
        )
    };
    let operand: SealedExtern<RefFamily> = SealedExtern::erase(&backing[1]);
    let got: u32 = sealed.open(
        operand,
        &backing,
        |_within, continuation: TestContinuation<'_>, other: &u32| continuation() + *other,
    );
    assert_eq!(got, 17);
    // Re-read the region through a sibling `Rc` after the open to catch a tree-borrows regression.
    let _again: &u32 = &backing[0];
}

/// [`SealedPinned::open`] over a **borrowed** fat-pointer carrier: a `Copy` closure bumped into the
/// region the seal pins is stored as a `&dyn Fn` inside a droppable enum, erased, and **called
/// through the reference inside the brand**. Distinct from the boxed-continuation round trip above:
/// there the fabricated lifetime covers only a heap box the carrier owns, here it covers a referent
/// living in the pinned region, so a tree-borrows regression at the re-anchor shows up as a read of
/// region memory through a forged tag rather than as a heap read. Fails on UB, not values.
#[test]
fn sealed_pinned_open_calls_a_bumped_continuation_by_reference() {
    let host: Rc<RegionHost<TieredProfile>> = RegionHost::fresh(None);
    let sealed: SealedPinned<TieredContinuationFamily, Rc<RegionHost<TieredProfile>>> = {
        let handle = RegionHandle::from_owner(&*host);
        // `value` takes `T: Copy`, so the bumped closure captures only `Copy` data and the region
        // stays Drop-free — the production guard, here as the only door that admits the closure.
        let bumped: &dyn Fn() -> u32 = handle.allocator().value(|| 40u32);
        SealedPinned::erase(TieredContinuation::Bumped(bumped), Rc::clone(&host))
    };
    let operand: SealedExtern<RefFamily> = SealedExtern::erase(&2u32);
    let got: u32 = sealed.open(
        operand,
        &host,
        |_within, continuation: TieredContinuation<'_>, other: &u32| match continuation {
            TieredContinuation::Bumped(call) => call() + *other,
            TieredContinuation::Boxed(call) => call() + *other,
        },
    );
    assert_eq!(got, 42);
}

/// **The owned tier's drop order** — a `SealedPinned` dropped unopened, holding the last `Rc` on
/// the region its value's own drop glue reads. `GlueProbe`'s destructor dereferences a region
/// borrow, so the value's glue must run while the bundled pins still hold that region: field order
/// (`value` before `pins`) is what supplies it. A by-value drop of the seal is also the retag shape
/// the dormant union slot exists for. Fails on UB, not values — the assertion only confirms the
/// glue ran at all.
#[test]
fn sealed_pinned_drop_runs_value_glue_before_pins() {
    let cart = Rc::new(TestCart {
        backing: vec![41, 42],
        outer: None,
    });
    let seen = Rc::new(Cell::new(0u32));
    let alive = Rc::downgrade(&cart);

    let sealed: SealedPinned<GlueProbeFamily, Rc<TestCart>> = SealedPinned::erase(
        GlueProbe {
            last: &cart.backing[1],
            seen: Rc::clone(&seen),
        },
        Rc::clone(&cart),
    );
    // The seal's bundled pin is now the last `Rc` on the region its probe borrows into.
    drop(cart);
    assert!(
        alive.upgrade().is_some(),
        "the seal's pin is the sole holder"
    );

    std::mem::drop(sealed);
    assert_eq!(
        seen.get(),
        42,
        "the value's drop glue read region memory before the bundled pins released it"
    );
    assert!(
        alive.upgrade().is_none(),
        "and the pins freed the region once the glue had run"
    );
}

/// The **step-open shape** under the two-tier split: an owned-tier continuation opens beside its
/// zipped [`SealedExtern`] operands at a **single** brand — the run-loop step's (continuation,
/// contract, region) miniature. The droppable boxed carrier rests on [`SealedPinned`] with its pin
/// co-located; the `DropFree` side — an *optional* present carrier and a plain reference — zips into
/// one `SealedExtern`, and all three read at one `'b`.
#[test]
fn sealed_pinned_opens_beside_a_zipped_extern_operand() {
    let backing: Rc<Vec<u32>> = Rc::new(vec![1, 2, 3]);
    let boxed: SealedPinned<BoxFamily, Rc<Vec<u32>>> =
        SealedPinned::erase(Box::new(&backing[0]), Rc::clone(&backing));
    // The `Some` arm: a present optional opens to `Some(..)` at the brand.
    let contract: SealedExtern<OptionOf<RefFamily>> = seal_option(Some(Erased::erase(&backing[1])));
    let region: SealedExtern<RefFamily> = SealedExtern::seal(Erased::erase(&backing[2]));
    let sum: u32 = boxed.open(
        contract.zip(region),
        &backing,
        |_within, boxed: Box<&u32>, (contract, region): (Option<&u32>, &u32)| {
            **boxed + *contract.expect("present optional opens to Some") + *region
        },
    );
    assert_eq!(sum, 6);
    let _again: &u32 = &backing[0];
}

/// [`seal_option`]'s `None` arm opens to `None` at the brand — the run-loop's frameless / no-contract
/// gate, where the optional operand carries no value but must still ride the combined open.
#[test]
fn seal_option_none_opens_to_none() {
    let backing: Rc<Vec<u32>> = Rc::new(vec![9]);
    let absent: SealedExtern<OptionOf<RefFamily>> = seal_option(None);
    let region: SealedExtern<RefFamily> = SealedExtern::erase(&backing[0]);
    let got: u32 = region
        .zip(absent)
        .open(&backing, |(region, absent): (&u32, Option<&u32>)| {
            assert!(absent.is_none(), "None optional opens to None");
            *region
        });
    assert_eq!(got, 9);
}

/// Library-`Region` frame profile for the [`StepContext`] fold tests — every construction door
/// mints a description into a real library arena, so the plain `[u32]`-region `TestCart` cannot
/// host one.
struct StepProfile;

impl StorageProfile for StepProfile {
    type FrameOwner = StepFrame;
}

struct StepFrame {
    region: Region<StepProfile>,
}

// SAFETY: the owned `Region`'s arena pages stay fixed-address while `self` is held (behind an `Rc`
// at every use site).
unsafe impl RegionOwner for StepFrame {
    type Region = Region<StepProfile>;
    fn region(&self) -> &Region<StepProfile> {
        &self.region
    }
}

// SAFETY: a `StepFrame` has no ancestry — it pins exactly its own region, so identity is the whole
// pins relation.
unsafe impl PinsRegion for StepFrame {
    fn pins_region(&self, region: &Region<StepProfile>) -> bool {
        std::ptr::eq(&self.region, region)
    }

    #[cfg(debug_assertions)]
    fn for_each_pinned_region(&self, visit: &mut dyn FnMut(&Region<StepProfile>)) {
        visit(&self.region);
    }
}

fn step_frame() -> Rc<StepFrame> {
    Rc::new_cyclic(|me| StepFrame {
        region: Region::new(me.clone()),
    })
}

/// [`StepContext::alloc`]: the built value's carrier references a description with **no members** —
/// reach = own region only, which is residence, not reach — hosted in the frame's own region;
/// liveness is the frame the step loop holds.
#[test]
fn step_context_alloc_carrier_names_its_home_and_no_members() {
    static SEVEN: u32 = 7;
    let frame = step_frame();
    let ctx: StepContext<StepFrame> = StepContext::new(Rc::clone(&frame));
    let w: Witnessed<RefFamily, Carrier<StepFrame>> =
        ctx.alloc_in_region::<RefFamily, StepProfile>(|_region| &SEVEN);
    assert_eq!(w.with_pinned(&frame, |r| **r), 7);
    let sealed = Sealed::seal(w, RegionHandle::from_owner(&*frame));
    let opened = sealed.open_at();
    assert!(!opened.has_reach_members(), "reach = own region only");
    assert!(!opened.borrows_home());
    assert!(opened.with_home_region(|region| std::ptr::eq(region, frame.region())));
}

/// [`StepContext::alloc_with`]: the dep run relocates in one act, each dep claiming its own
/// delivery envelope's pins, so the built value's carrier names every dep's home as a minted reach
/// member, and the dep views arrive at `build` in the staging order of `deps`.
#[test]
fn step_context_alloc_with_mints_dep_homes_and_preserves_dep_order() {
    static ONE: u32 = 1;
    static TWO: u32 = 2;
    let own = step_frame();
    let dep_a = step_frame();
    let dep_b = step_frame();
    let delivered_a: Delivered<RefFamily, Carrier<StepFrame>, StepFrame> =
        RegionHandle::<StepProfile>::from_owner(&*dep_a).deliver_resident(&ONE);
    let delivered_b: Delivered<RefFamily, Carrier<StepFrame>, StepFrame> =
        RegionHandle::<StepProfile>::from_owner(&*dep_b).deliver_resident(&TWO);

    let ctx: StepContext<StepFrame> = StepContext::new(Rc::clone(&own));
    let built = ctx.alloc_with_in_region::<RefFamily, RefFamily, StepProfile>(
        &[&delivered_a, &delivered_b],
        |_region, views, _token| {
            assert_eq!(views.iter().map(|v| **v).collect::<Vec<_>>(), vec![1, 2]);
            &ONE
        },
    );
    // Both dep homes composed into the set minted into `own`'s arena — they arrived as ordinary
    // members of each dep envelope's pins. `own` itself is not a member: nothing composed it in
    // (the accumulator is region-pure), so the built value borrows into no region of its own.
    let sealed: Retained<RefFamily, Carrier<StepFrame>> = built.into_cell();
    let pin = StepCoverage::<StepFrame>::of(Rc::clone(&own));
    let opened = sealed.open_at_with(&pin);
    opened.with_reach(|reach| {
        assert!(reach.pins_region(dep_a.region()));
        assert!(reach.pins_region(dep_b.region()));
        assert!(!reach.pins_region(own.region()));
    });
    assert!(!opened.borrows_home());
    assert!(opened.with_home_region(|region| std::ptr::eq(region, own.region())));
}
