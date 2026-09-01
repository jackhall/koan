//! The **environment copy**: the engine behind the
//! [`Consolidate`](crate::machine::model::RegionEscape::Consolidate) verb. Deep-copying a callable
//! rebuilds the per-call portion of the scope chain it captured at the destination region — data
//! bindings relocated under the severing seam, captured callables recursed, eternal-homed scopes
//! referenced verbatim — so the product reaches no source region and the producer is free to die.
//!
//! **One lifetime throughout.** The engine is entered from inside a relocation fold, where the
//! source callable arrives as the fold's operand view and the destination is the fold's own brand:
//! both are `'b`, so the source chain is walked and the copy built with no re-anchor of its own.
//! That is what lets a copied binding go in through the same severing bind door a `CLOSE OVER`
//! capture takes, which derives each entry's exact reach from the product it rebuilt.
//!
//! **A captured callable is rebuilt in a fold of its own.** A binding's value reaches the engine as
//! a delivery envelope, whose sealed content re-anchors only at the borrow that opens it — never at
//! `'b`. So a bound callable is not read out and rebuilt; it is **relocated**, through a nested
//! [`transfer_into`](crate::witnessed::Delivered::transfer_into) whose destination operand carries
//! the copied scope it attaches under ([`RegionScopeFamily`]). Inside that fold the source callable
//! and the copied scope meet at one brand, and the retention claim is derived from the product: the
//! rebuilt callable borrows its new captured scope and nothing else, so every source region the
//! entry pinned is released.
//!
//! **Nothing here waits.** Readiness ([`Scope::chain_is_copy_ready`]) is a gate, not a barrier: an
//! environment the engine cannot rebuild answers `None` and the caller rides the value verbatim
//! under the pin. There is no park, no edge, and no queue in this module — which is what makes the
//! deadlock two mutually-referencing in-flight environments would create *unconstructible* rather
//! than merely handled.
//!
//! **Cycles and sharing** fall out of the memo. A source scope is entered in it *before* its tables
//! are filled, so a binding whose value is a callable capturing that same scope attaches under the
//! copy already under construction: a recursive closure copies to a closure whose captured scope
//! binds the copy itself, and two closures over one defining scope copy to two closures over one
//! copied scope.

use allocator_api2::alloc::Global;

use super::Scope;
use crate::machine::DeliveredCarried;
use crate::machine::core::bindings::BindingIndex;
use crate::machine::core::carrier_witness::{DeliveredFunction, OverloadSeal};
use crate::machine::core::kfunction::{KFunction, KFunctionFamily};
use crate::machine::core::ref_carriers::RegionScopeFamily;
use crate::machine::core::{FoldingBrand, KoanRegion, KoanStorageProfile, RegionBrand};
use crate::machine::model::{Carried, KObject};
use crate::witnessed::{FoldedPlacement, RegionHandle};

/// One relocation's memo: the address of a source scope beside the copy built for it. Address *is*
/// identity here — a source scope is region-resident and lives for the whole relocation, so nothing
/// is recycled underneath it — and an address is also the only form of a source scope the engine
/// can read back out of a delivery envelope without re-anchoring what the envelope seals.
///
/// A plain `Vec` over the global heap, not an arena: the memo is transient to one relocation and
/// must leave no bump bytes behind in the destination region, and a captured chain is a handful of
/// links, so a linear scan beats a hash.
struct CopiedScopes<'b> {
    entries: Vec<(usize, &'b Scope<'b>)>,
}

impl<'b> CopiedScopes<'b> {
    fn new() -> Self {
        CopiedScopes {
            entries: Vec::new(),
        }
    }

    fn get(&self, address: usize) -> Option<&'b Scope<'b>> {
        self.entries
            .iter()
            .find(|(key, _)| *key == address)
            .map(|(_, copy)| *copy)
    }

    fn insert(&mut self, address: usize, copy: &'b Scope<'b>) {
        self.entries.push((address, copy));
    }
}

/// The address a scope is memoized under.
fn scope_address(scope: &Scope<'_>) -> usize {
    scope as *const Scope<'_> as usize
}

/// Consolidate `value` into `brand`'s region — the `Consolidate` verb's act, entered from the
/// relocation fold that owns `brand`.
///
/// `None` declines, and the caller rides the value verbatim: that is the pin the chooser's
/// readiness answer already priced. Declining is always sound — retaining more never dangles — so
/// the engine re-checks readiness itself rather than trusting the chooser's earlier verdict, which
/// is pricing and not authority.
///
/// Only a `KFunction` consolidates. A `Module`'s child scope is `MODULE`-kinded, carrying an
/// announced window and a group record the readiness gate does not model, so a module declines by
/// the same gate every other unmodelled environment does rather than by a special case here.
pub(crate) fn consolidate_object<'b>(
    value: &KObject<'b>,
    door: FoldingBrand<'b>,
) -> Option<KObject<'b>> {
    let KObject::KFunction(source) = value else {
        return None;
    };
    let mut memo = CopiedScopes::new();
    let captured = copy_chain(source.captured_scope(), *door, None, &mut memo)?;
    Some(KObject::KFunction(door.alloc_function_folded(
        KFunction::copy_at_fold(captured, source),
    )))
}

/// Copy the per-call portion of `source`'s chain into `brand`'s region, outermost link first, and
/// hand back the copy of `source` itself. An eternal-homed `source` comes straight back uncopied:
/// its region outlives everything that could retain it, so the copy references it verbatim.
///
/// Each link is **allocated and memoized before it is filled**, which is the whole termination
/// argument: filling a scope can route a bound callable back through this walk, whose memo probe
/// then answers with the copy under construction instead of recursing, so the
/// `scope → function → scope` cycle a recursive `FN` creates closes rather than looping.
fn copy_chain<'b>(
    source: &'b Scope<'b>,
    brand: RegionBrand<'b>,
    anchor: Option<(usize, &'b Scope<'b>)>,
    memo: &mut CopiedScopes<'b>,
) -> Option<&'b Scope<'b>> {
    if !source.chain_is_copy_ready() {
        return None;
    }

    // Where the walk stops, and what the outermost copied link takes as its `outer`. Without an
    // anchor that is the chain's own eternal home, referenced verbatim. With one it is a scope the
    // caller has already copied, standing in for the source scope at that address — which is how a
    // nested rebuild attaches to the copy already built for the chain it shares.
    let (stop, mut outer) = match anchor {
        Some((address, copied)) => {
            if source
                .per_call_chain()
                .all(|link| scope_address(link) != address)
            {
                // The anchor is not on this chain, so there is nothing to attach under.
                return None;
            }
            (address, copied)
        }
        None => {
            let eternal = source.innermost_eternal_home();
            debug_assert!(
                eternal.parent_frame_pin().is_none(),
                "a copied chain's outermost link takes an eternal-homed outer: an unpinned \
                 `&Scope` outer is sound only because its region outlives the run",
            );
            (0, eternal)
        }
    };

    // Outermost-first, so each link's `outer` is one the copy has already built. An anchored walk
    // stops at the anchor: everything above it is the caller's copy already.
    let mut chain: Vec<&'b Scope<'b>> = source
        .per_call_chain()
        .take_while(|link| scope_address(link) != stop)
        .collect();
    chain.reverse();

    for link in chain {
        outer = match memo.get(scope_address(link)) {
            Some(hit) => hit,
            None => {
                let copied = Scope::alloc_copied_child(outer, brand);
                memo.insert(scope_address(link), copied);
                fill_scope(link, copied, memo)?;
                copied.close();
                copied
            }
        };
    }
    Some(outer)
}

/// Fill `copied` from `source`'s visible bindings. `source` is closed and claim-free (the readiness
/// gate), so no visibility cutoff applies: a scope no live call-site chain names reads as complete,
/// every entry visible to every body that captured it, and copying the table wholesale under a
/// fresh id reproduces exactly what the source answered.
///
/// A binding whose value **is** a callable is rebuilt against the copied scope it captured
/// ([`rebuild_callable`]); that is what makes the recursive-closure and sibling-sharing cases hold.
/// Every other value goes in through the **severing** bind door, whose disposition is `Relocate`
/// unconditionally: the entry is rebuilt at the destination and sealed under the reach derived from
/// that rebuild, so a data-only environment leaves the region it came from free to die. A callable
/// buried as a *cell* inside a container rides verbatim there, and the entry's own composed reach
/// records that pin.
///
/// Dispatch registrations copy the same way, each overload's callable rebuilt and registered fresh,
/// so a keyworded recursive `FN` reaches the copy rather than the source.
fn fill_scope<'b>(
    source: &'b Scope<'b>,
    copied: &'b Scope<'b>,
    memo: &CopiedScopes<'b>,
) -> Option<()> {
    let visible = source.bindings().visible_for_capture(None, Global);

    // The type channel first, and in full: a nominal declared in a per-call scope is reached by
    // name from every body that captured it, so a copy that dropped it would leave the rebuilt
    // closure unable to name a type the source could.
    for (name, kt, site) in source.bindings().copied_types() {
        copied.bindings().insert_copied_type(name, kt, site);
    }

    for (name, cell) in visible.data.iter() {
        let sealed = match callable_anchor(cell, copied, memo) {
            Some(anchor) => copied.store_function_cell(&rebuild_callable(cell, anchor)),
            None => copied
                .adopt_for_capture(cell, |carried| Ok(carried.object()))
                .ok()?,
        };
        copied
            .bindings()
            .insert_copied_value(*name, BindingIndex::value(0), sealed);
    }

    for cell in visible.functions.iter() {
        let anchor = anchor_for(&cell.open(captured_chain_addresses), copied, memo);
        let rebuilt = rebuild_registration(cell, anchor);
        copied.bindings().insert_copied_overload(
            BindingIndex::value(0),
            OverloadSeal::of_delivered(copied, &rebuilt),
        );
    }

    debug_assert!(
        visible.operators.is_empty(),
        "the readiness gate declines a scope holding an operator registry entry",
    );
    debug_assert!(
        visible.claims.is_empty(),
        "the readiness gate declines a scope with a standing claim",
    );
    Some(())
}

/// The rebuild operand for the callable `cell` carries — `None` if `cell` carries something other
/// than a callable, so the caller relocates it as ordinary data instead.
///
/// Everything is read under the envelope's own rank-2 open and comes back as **addresses**, which
/// is exactly why the memo is keyed by one: the sealed callable re-anchors only at the borrow that
/// opened it, never at the destination brand, so an address is the only form of it the engine can
/// carry out here.
fn callable_anchor<'b>(
    cell: &DeliveredCarried,
    fallback: &'b Scope<'b>,
    memo: &CopiedScopes<'b>,
) -> Option<Anchor<'b>> {
    let chain = cell.open(|live| match live {
        Carried::Object(KObject::KFunction(function)) => Some(captured_chain_addresses(function)),
        _ => None,
    })?;
    Some(anchor_for(&chain, fallback, memo))
}

/// The addresses of `function`'s captured per-call chain, innermost first.
fn captured_chain_addresses(function: &KFunction<'_>) -> Vec<usize> {
    function
        .captured_scope()
        .per_call_chain()
        .map(scope_address)
        .collect()
}

/// The rebuild's attach point: the **innermost link of the callable's own captured chain that the
/// memo already holds**, which is what a nested rebuild hangs its copy under, so a callable and the
/// scope it captured land on one copy rather than two.
///
/// A callable capturing a scope the copy has not reached — one nested *inside* a copied link rather
/// than above it — anchors nowhere. `fallback` then stands in so the operand still carries the
/// destination region, and the rebuild copies that chain whole under its own eternal home instead.
fn anchor_for<'b>(chain: &[usize], fallback: &'b Scope<'b>, memo: &CopiedScopes<'b>) -> Anchor<'b> {
    match chain
        .iter()
        .find_map(|address| memo.get(*address).map(|copied| (*address, copied)))
    {
        Some((address, copied)) => Anchor {
            scope: copied,
            source: Some(address),
        },
        None => Anchor {
            scope: fallback,
            source: None,
        },
    }
}

/// A callable rebuild's destination operand before it is delivered: the scope the fold builds
/// beside, and the source scope it stands for when it is a genuine attach point rather than only
/// the destination region's representative.
#[derive(Clone, Copy)]
struct Anchor<'b> {
    scope: &'b Scope<'b>,
    source: Option<usize>,
}

/// Rebuild the callable `cell` carries at `attach` — one nested relocation whose destination
/// operand pairs the destination region's handle with the copied scope the rebuilt callable
/// captures. Both meet at the fold's own brand, so the rebuild borrows the copy and nothing of the
/// source.
///
/// The retention claim is **derived from the product**, like every other relocation's: the rebuilt
/// callable's one borrow is its captured scope, so a region is still borrowed exactly when it is
/// that scope's. Every source region the entry pinned answers `false` and is released — which is
/// the consolidation, stated as a checked property of the bytes that exist rather than as an
/// assertion.
fn rebuild_callable<'b>(cell: &DeliveredCarried, anchor: Anchor<'b>) -> DeliveredFunction {
    let anchored = anchor.source;
    cell.transfer_into::<RegionScopeFamily, KFunctionFamily, KoanStorageProfile>(
        destination_operand(anchor.scope),
        rebuilt_still_borrows,
        move |value, (handle, scope), placement| {
            let Carried::Object(KObject::KFunction(source)) = value else {
                unreachable!("the callable probe ran over this envelope");
            };
            rebuild_in_fold(source, handle, scope, anchored, placement)
        },
    )
}

/// [`rebuild_callable`] for a dispatch registration, whose envelope already carries the callable
/// itself rather than a value wrapping it.
fn rebuild_registration<'b>(cell: &DeliveredFunction, anchor: Anchor<'b>) -> DeliveredFunction {
    let anchored = anchor.source;
    cell.transfer_into::<RegionScopeFamily, KFunctionFamily, KoanStorageProfile>(
        destination_operand(anchor.scope),
        rebuilt_still_borrows,
        move |source, (handle, scope), placement| {
            rebuild_in_fold(source, handle, scope, anchored, placement)
        },
    )
}

/// The rebuild both callable relocations run, inside their own fold: copy whatever of `source`'s
/// captured chain the caller has not already built — attaching under `scope` when it stands for a
/// link of that chain (`anchored`), or copying the chain whole under its own eternal home when it
/// does not — and assemble the callable there.
///
/// **An environment this cannot rebuild rides verbatim**, as the source callable itself. Sound with
/// no special handling: the retention claim is derived from whichever product comes back
/// ([`rebuilt_still_borrows`]), so a verbatim ride keeps the region it captured and a rebuilt copy
/// releases it, out of one predicate. What a copied scope's *own* entries still reach is held by
/// the destination region's union bundle, which the binding doors mint into as they fill it.
fn rebuild_in_fold<'c>(
    source: &'c KFunction<'c>,
    handle: RegionHandle<'c, KoanStorageProfile>,
    scope: &'c Scope<'c>,
    anchored: Option<usize>,
    placement: FoldedPlacement<'c, KoanStorageProfile>,
) -> &'c KFunction<'c> {
    let mut memo = CopiedScopes::new();
    let anchor = anchored.map(|address| {
        memo.insert(address, scope);
        (address, scope)
    });
    match copy_chain(
        source.captured_scope(),
        RegionBrand(handle),
        anchor,
        &mut memo,
    ) {
        Some(captured) => FoldingBrand::in_fold_closure(placement)
            .alloc_function_folded(KFunction::copy_at_fold(captured, source)),
        None => source,
    }
}

/// The destination operand a callable rebuild folds into: `attach`'s own region handle beside
/// `attach` itself. The pair is what carries both facts one fold needs — the region to build in and
/// the scope to capture — through a combinator that takes exactly one destination.
fn destination_operand<'b>(
    attach: &'b Scope<'b>,
) -> crate::witnessed::Delivered<
    RegionScopeFamily,
    crate::machine::CarrierWitness,
    crate::machine::core::FrameStorage,
> {
    attach.deliver_resident::<RegionScopeFamily>((attach.brand().handle(), attach))
}

/// The retention claim shared by both callable rebuilds: a `KFunction`'s one region borrow is its
/// captured scope, so it still borrows a region exactly when that scope lives there.
fn rebuilt_still_borrows(product: &&KFunction<'_>, region: &KoanRegion) -> bool {
    std::ptr::eq(product.captured_scope().region(), region)
}
