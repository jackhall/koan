//! Miri slate (tree borrows) for the lifetime-erasure carrier. Every test carries a *real* borrow
//! into the erased store and reads it back after the original binding drops, so the erase → reattach
//! → read round-trip pins the lifetime-fabricated read under tree borrows. Names only stand-in
//! families (a covariant `&'r u32`, an invariant `Cell<&'r u32>`, a mutable-scope-plus-pool family)
//! and a stand-in cart (`TestCart`: a region-backing `Vec` plus an `outer` ancestor chain), never a
//! koan type. Fails on UB, not values. The escape-can't-compile guards live as `compile_fail`
//! doctests on [`Witnessed::with`] / [`Witnessed::map`] / [`Witnessed::yoke`] / [`Witnessed::merge`].

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

/// Non-`Copy` stand-in: a boxed borrow. `At<'r>` is a `Box<&'r u32>`, which (like koan's boxed
/// `NodeContinuation`) is consumed by value, not copied — the case [`SealedExtern::open`] admits and
/// [`Sealed::open`]'s `At<'static>: Copy` bound excludes.
struct BoxFamily;

// Each stand-in is one type generic only in `'r` with a lifetime-independent layout (a reference, a
// cell of a reference, a struct of both, a boxed reference); the shared `reattachable!` macro
// discharges the obligation.
reattachable! {
    RefFamily => &'r u32,
    InvFamily => Cell<&'r u32>,
    ScopeFamily => ScopeAndPool<'r>,
    BoxFamily => Box<&'r u32>,
}

/// Cart stand-in for the witness-with-a-region cases (`yoke` / `merge`): a backing `Vec` (the
/// "region") plus an `outer` link, mirroring `FrameStorage`'s region + ancestor-pin chain without
/// naming a koan type. Held by `Rc`, so the backing's heap address is stable (a `StableDeref`); a
/// descendant's `outer` chain keeps its ancestors' backings alive, exactly the relation `merge`
/// reads.
struct TestCart {
    backing: Vec<u32>,
    outer: Option<Rc<TestCart>>,
}

// `Rc<TestCart>` is already a `Witness` (the blanket `Rc<F>` impl). Its region is the owned backing.
// SAFETY: the backing lives inside the `Rc`-owned `TestCart` at a fixed heap address for the whole
// life of the `Rc`, so a value built from `&backing` is pinned by the witness.
unsafe impl WitnessRegion for Rc<TestCart> {
    type Region = [u32];
    fn region(&self) -> &[u32] {
        &self.backing
    }
}

/// Whether holding `holder` keeps `target`'s backing alive — true when `target` is `holder` itself or
/// any of its `outer` ancestors (each pinned by the chain). The descendant test that `merge` uses.
fn pins(holder: &Rc<TestCart>, target: &Rc<TestCart>) -> bool {
    let mut node: &TestCart = holder;
    loop {
        if std::ptr::eq(node, &**target) {
            return true;
        }
        match &node.outer {
            Some(o) => node = o,
            None => return false,
        }
    }
}

// SAFETY: `Some(Rc::clone(left))` is returned only when `pins(left, right)` — holding `left` keeps
// `right`'s backing alive via the `outer` chain (and symmetrically for `right`); `None` only when
// neither pins the other, the sole safe verdict for unrelated carts. A single-region witness, so the
// union of two related carts collapses to the descendant — there is no multi-member set here.
unsafe impl MergeWitness for Rc<TestCart> {
    fn merge(left: &Self, right: &Self) -> Option<Self> {
        if pins(left, right) {
            Some(Rc::clone(left))
        } else if pins(right, left) {
            Some(Rc::clone(right))
        } else {
            None
        }
    }
}

/// The witness-less primitive still routed by the value-carrier path: `Erased` storage, exercised
/// over a real borrow.
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

/// `Witnessed::read`: the carrier escapes the call bounded by the `&self` borrow, read after the
/// original binding drops. The witness pins the pointee for the borrow the returned `&u32` rides.
#[test]
fn read_borrow_bounded_witness_only() {
    let backing: Rc<Vec<u32>> = Rc::new(vec![5, 6, 7]);
    let w: Witnessed<RefFamily, Rc<Vec<u32>>> = {
        let borrow: &u32 = &backing[2];
        Witnessed::from_erased(Erased::erase(borrow), Rc::clone(&backing))
    };
    drop(backing); // witness is sole owner.
    let escaped: &u32 = w.read(); // hands the carrier OUT, bounded by `&w`.
    assert_eq!(*escaped, 7);
    // `w` stays borrowed while `escaped` is live, so the witness pin holds.
    assert_eq!(*w.read(), 7);
}

/// `with_branded_ref`: re-anchor a reference-to-an-erased-store behind the rank-2 brand and copy a
/// scalar out — the witnessed read the deleted free-`'b` reattach is replaced by. Mirrors the
/// production region-store flow: erase a borrow to the `'static` store, then read it back under the
/// brand, the fabricated `'b` confined to the closure (`R` is a copied scalar that cannot name it).
#[test]
fn branded_ref_reads_erased_store() {
    let backing = [11u32, 22, 33];
    // Erase a borrow to the `'static` store, then re-anchor behind the brand — the shape the region's
    // store-side read routes. The pointee (`backing`) is kept live across the call; the brand keeps the
    // view from escaping it.
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

/// `merge` as the function-into-scope composition: a witnessed `ScopeFamily` carrier in the
/// *descendant* cart binds, at the shared brand, a witnessed `&u32` sourced from the *ancestor* cart.
/// The result is sealed under the descendant, whose `outer` chain keeps the ancestor backing alive
/// after both call handles drop. Miri must stay clean reading the bound ancestor ref back.
#[test]
fn merge_binds_ancestor_ref_into_descendant_scope() {
    let ancestor: Rc<TestCart> = Rc::new(TestCart {
        backing: vec![100, 200],
        outer: None,
    });
    let descendant: Rc<TestCart> = Rc::new(TestCart {
        backing: vec![1, 2, 3],
        outer: Some(Rc::clone(&ancestor)),
    });
    // Scope carrier in the descendant: empty slot, pool = the descendant's own region.
    let scope_w: Witnessed<ScopeFamily, Rc<TestCart>> =
        Witnessed::yoke(Rc::clone(&descendant), |region| ScopeAndPool {
            scope: Cell::new(None),
            pool: region,
        });
    // Function stand-in: a reference sourced from the ancestor's region.
    let fn_w: Witnessed<RefFamily, Rc<TestCart>> =
        Witnessed::yoke(Rc::clone(&ancestor), |region| &region[1]);
    // Bind the ancestor ref into the descendant scope at the shared brand, then re-seal.
    let merged: Witnessed<ScopeFamily, Rc<TestCart>> = scope_w
        .merge::<RefFamily, ScopeFamily>(fn_w, |scope, func, _brand: PhantomData<&_>| {
            scope.scope.set(Some(func));
            scope
        })
        .expect("descendant and ancestor carts are related");
    // Drop both call handles. `merged`'s witness is the descendant clone; its `outer` chain still
    // pins the ancestor backing the bound `&200` points into.
    drop(descendant);
    drop(ancestor);
    assert_eq!(merged.with(|c| *c.scope.get().unwrap()), 200);
}

/// `merge` rejects unrelated carts: neither pins the other's backing, so no surviving witness would
/// keep a combined carrier live — [`MergeWitness::merge`] returns `None` and the carrier merge yields
/// `None` before the closure builds an unsound value.
#[test]
fn merge_rejects_unrelated_carts() {
    let a: Rc<TestCart> = Rc::new(TestCart {
        backing: vec![1],
        outer: None,
    });
    let b: Rc<TestCart> = Rc::new(TestCart {
        backing: vec![2],
        outer: None,
    });
    let wa: Witnessed<RefFamily, Rc<TestCart>> = Witnessed::yoke(Rc::clone(&a), |r| &r[0]);
    let wb: Witnessed<RefFamily, Rc<TestCart>> = Witnessed::yoke(Rc::clone(&b), |r| &r[0]);
    let merged = wa.merge::<RefFamily, RefFamily>(wb, |l, _r, _brand: PhantomData<&_>| l);
    assert!(merged.is_none());
}

/// `SealedExtern::open` — the **consuming, externally-witnessed** rank-2 open, distinct from the
/// bundled-witness [`Sealed::open`] (which this slate covers via its own `compile_fail` doctest and
/// the `Witnessed` round-trips). A real borrow is erased into the witness-less `SealedExtern`, opened
/// against a *separately-held* `Rc` witness, and the invariant value read back inside the brand after
/// the original binding drops; the witness pins the pointee for the call, and the `for<'b>` brand
/// confines the read. A sibling mutation after the open catches a tree-borrows regression. Fails on
/// UB, not values.
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
    // Mutate the region through a sibling `Rc` after the open to catch a stacked/tree-borrow regression.
    let _again: &u32 = &backing[2];
}

/// `SealedExtern::open` over a **non-`Copy`** carrier: a `Box<&u32>` is moved (not copied) through the
/// seal and consumed by the open, proving the verb admits the boxed continuation shape
/// [`Sealed::open`]'s `Copy` bound excludes. The boxed borrow is read inside the brand after the
/// source drops; the held witness pins it. Fails on UB, not values.
#[test]
fn sealed_extern_open_consumes_non_copy() {
    let backing: Rc<Vec<u32>> = Rc::new(vec![10, 20]);
    let sealed: SealedExtern<BoxFamily> = {
        let borrow: &u32 = &backing[0];
        SealedExtern::erase(Box::new(borrow))
    };
    let seen: u32 = sealed.open(&backing, |boxed: Box<&u32>| **boxed);
    assert_eq!(seen, 10);
    let _again: &u32 = &backing[1];
}

/// `SealedExtern::zip` + [`seal_option`]: heterogeneous carriers pinned by the same witness open at a
/// **single** brand — the run-loop step's (continuation, contract, region) shape in miniature. A
/// non-`Copy` boxed carrier, an *optional* present carrier, and a plain reference are combined and
/// opened together; each is read at one `'b`, and a sibling mutation after catches a regression.
#[test]
fn sealed_extern_zip_opens_heterogeneous_at_one_brand() {
    let backing: Rc<Vec<u32>> = Rc::new(vec![1, 2, 3]);
    let boxed: SealedExtern<BoxFamily> = SealedExtern::erase(Box::new(&backing[0]));
    // The optional operand is sealed via `seal`-of-`Erased` then folded into an `Option` carrier; the
    // `Some` arm proves a present optional opens to `Some(..)` at the brand.
    let contract: SealedExtern<OptionOf<RefFamily>> = seal_option(Some(Erased::erase(&backing[1])));
    let region: SealedExtern<RefFamily> = SealedExtern::seal(Erased::erase(&backing[2]));
    let sum: u32 = boxed.zip(contract).zip(region).open(
        &backing,
        |((boxed, contract), region): ((Box<&u32>, Option<&u32>), &u32)| {
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
