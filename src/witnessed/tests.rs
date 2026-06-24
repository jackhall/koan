//! Miri slate (tree borrows) for the lifetime-erasure carrier. Every test carries a *real* borrow
//! into the erased store and reads it back after the original binding drops, so the erase → reattach
//! → read round-trip pins the lifetime-fabricated read under tree borrows. Names only stand-in
//! families (a covariant `&'r u32`, an invariant `Cell<&'r u32>`, a mutable-scope-plus-pool family),
//! never a koan type. Fails on UB, not values. The escape-can't-compile guards live as
//! `compile_fail` doctests on [`Witnessed::with`] / [`Witnessed::map`].

use std::cell::Cell;
use std::marker::PhantomData;
use std::rc::Rc;

use super::*;

/// Covariant stand-in: a plain shared reference. `At<'r>` is a `&'r u32`, whose lifetime the borrow
/// checker can't track across the `'static` store.
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

// Each stand-in is one type generic only in `'r` with a lifetime-independent layout (a reference, a
// cell of a reference, a struct of both); the shared `reattachable!` macro discharges the obligation.
reattachable! {
    RefFamily => &'r u32,
    InvFamily => Cell<&'r u32>,
    ScopeFamily => ScopeAndPool<'r>,
}

/// The witness-less primitives still routed by the value-carrier path: `Erased` storage and the
/// transient `reattach_value` / `reattach_ref` helpers, exercised over a real borrow.
#[test]
fn erased_roundtrip_and_helpers() {
    let backing = [7u32, 8, 9];
    let erased: Erased<RefFamily> = Erased::erase(&backing[0]);
    // SAFETY: `backing` is held live to the end of the test, pinning the re-anchored borrow.
    let reattached: &u32 = unsafe { erased.reattach() };
    assert_eq!(*reattached, 7);

    // SAFETY: as above; the value genuinely lives for the read.
    let owned: &u32 = unsafe { reattach_value::<RefFamily>(&backing[1]) };
    assert_eq!(*owned, 8);
    let by_ref: &&u32 = &&backing[2];
    // SAFETY: as above.
    let viaref: &&u32 = unsafe { reattach_ref::<RefFamily>(by_ref) };
    assert_eq!(**viaref, 9);

    // Re-read the first borrow to catch a tree-borrows regression from the later helper calls.
    assert_eq!(*reattached, 7);
}

/// `vend_carrier`: re-anchor a stored carrier against a *borrowed* frame witness.
#[test]
fn witness_borrowed_reattach() {
    let frame: Rc<u32> = Rc::new(0);
    let backing = [11u32, 22];
    let a: Erased<RefFamily> = Erased::erase(&backing[0]);
    let via_vend: &u32 = vend_carrier(a, &frame);
    assert_eq!(*via_vend, 11);
}

/// `Witnessed::read`: the carrier escapes the call bounded by the `&self` borrow, read after the
/// original binding drops. The witness pins the pointee for the borrow the returned `&u32` rides.
#[test]
fn read_borrow_bounded_witness_only() {
    let backing: Rc<Vec<u32>> = Rc::new(vec![5, 6, 7]);
    let w: Witnessed<RefFamily, Rc<Vec<u32>>> = {
        let borrow: &u32 = &backing[2];
        Witnessed::new(borrow, Rc::clone(&backing))
    };
    drop(backing); // witness is sole owner.
    let escaped: &u32 = w.read(); // hands the carrier OUT, bounded by `&w`.
    assert_eq!(*escaped, 7);
    // `w` stays borrowed while `escaped` is live, so the witness pin holds.
    assert_eq!(*w.read(), 7);
}

/// `reattach_with` / `reattach_slice_with` / `reattach_ref_with`: re-anchor a live value, a slice,
/// and a reference-to-an-erased-store to a borrowed witness's lifetime — the witness-explicit
/// transient re-anchors. `reattach_ref_with` mirrors the production region-store flow: erase a borrow
/// to the `'static` store, then re-hand a reference to it bounded by the witness pin.
#[test]
fn reattach_with_live_value_and_slice() {
    let frame: Rc<u32> = Rc::new(0);
    let backing = [11u32, 22, 33];
    let one: &u32 = reattach_with::<RefFamily, _>(&backing[0], &frame);
    assert_eq!(*one, 11);
    let elems: &[&u32] = &[&backing[1], &backing[2]];
    let viaslice: &[&u32] = reattach_slice_with::<RefFamily, _>(elems, &frame);
    assert_eq!(viaslice.iter().map(|r| **r).sum::<u32>(), 55);
    // Erase a borrow to the `'static` store, then re-anchor a *reference* to it under the witness —
    // the shape the region's store-side re-anchor and the scope pointer's `reattach_witnessed` route.
    let stored: <RefFamily as Reattachable>::At<'static> =
        erase_to_static::<RefFamily>(&backing[0]);
    let reref: &&u32 = reattach_ref_with::<RefFamily, _>(&stored, &frame);
    assert_eq!(**reref, 11);
}

/// Covariant carrier round-trips after the original borrow drops; the bundled witness keeps it live.
/// The rank-2 closure returns a copied scalar (`'b`-independent), so nothing escapes.
#[test]
fn covariant_roundtrip_witness_only() {
    let backing: Rc<Vec<u32>> = Rc::new(vec![7, 8, 9]);
    let w: Witnessed<RefFamily, Rc<Vec<u32>>> = {
        let borrow: &u32 = &backing[0]; // original binding...
        Witnessed::new(borrow, Rc::clone(&backing))
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
        Witnessed::new(cell, Rc::clone(&backing))
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
        Witnessed::new(carrier, Rc::clone(&backing))
    };
    // At the brand, bind pool[2] into the invariant scope slot — sound only because scope and bound
    // value share the brand — then re-seal.
    let post: Witnessed<ScopeFamily, Rc<Vec<u32>>> = pre.map(|c, _brand: PhantomData<&_>| {
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
        Witnessed::new(Cell::new(&backing[0]), Rc::clone(&backing));
    let got = w.with(|c| {
        let here = c.get();
        c.set(here);
        *c.get()
    });
    assert_eq!(got, 100);
    drop(backing);
    assert_eq!(w.with(|c| *c.get()), 100);
}
