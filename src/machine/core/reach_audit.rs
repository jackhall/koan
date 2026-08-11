//! The **reach-tightness report**: the over-pinning audit at the fold chokepoint
//! ([`StepAllocator::alloc_carried_with`](super::StepAllocator::alloc_carried_with)). Every
//! residence audit in the engine catches under-pinning — a value naming storage nothing keeps
//! alive. Nothing catches the other direction: a fold that pins an operand's regions when its
//! product embeds nothing of that operand keeps those regions alive for as long as the product
//! lives, and every check passes silently.
//!
//! **The ground truth is address intersection, not stored reach.** Asking a value what it borrows
//! (`still_borrows`, a substrate's reach union) reads back what the folds under audit *declared* —
//! circular. What is not circular is the fold's own moment: inside the brand, collect the addresses
//! reachable from each operand view and from the product, and an operand **contributed** exactly
//! when the two sets intersect. A contributing operand justifies its whole coverage; a
//! non-contributing one justifies none of it, and any description member left unjustified is the
//! over-fold this module flags.
//!
//! Pointer equality suffices for the comparison because a mint folds its sources' *exact* `Rc`
//! members and subsumption only ever drops members — so every member of a minted description is
//! pointer-identical to a member of some operand's coverage, or to the home the engine unioned in.
//! No region is minted and no address table is kept.
//!
//! Compiled only under `cfg(any(test, feature = "region-audit"))`: a release build carries neither
//! the walker, the comparison, nor the log. Its place among the memory model's checks is
//! [memory-model.md § Debug region audits](../../../design/memory-model.md#debug-region-audits);
//! the reach model it reads is
//! [reach.md](../../../workgraph/design/reach.md), whose
//! [§ Debug audits](../../../workgraph/design/reach.md#debug-audits) covers the library-side pin-ring
//! detector this audit's over-pin direction pairs with.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use crate::machine::DeliveredCarried;
use crate::machine::model::{Carried, Held, KObject};
use crate::witnessed::PinsRegion;

use super::arena::FrameStorage;

/// The addresses a value's borrows reach — every region-hosted pointee the walk below finds,
/// identified by address alone. A set rather than a list: the comparison is an intersection test,
/// and a value may reach one substrate through several paths.
type AddressSet = HashSet<usize>;

/// One flagged **over-fold**: a region the product's description pins that no contributing operand
/// justifies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TightnessFlag {
    /// The fold door the flag was raised at.
    pub site: &'static str,
    /// `Rc::as_ptr` of the unjustified frame owner the product's description names.
    pub member: usize,
    /// Indices into the fold's dep list whose operands contributed nothing to the product — the
    /// candidates the unjustified member most likely came in on.
    pub non_contributing: Vec<usize>,
}

thread_local! {
    static TIGHTNESS_FLAGS: RefCell<Vec<TightnessFlag>> = const { RefCell::new(Vec::new()) };
}

/// Every over-fold flagged on this thread since the last reset, oldest first. Cloned out, so a
/// reader cannot narrow the log in place.
pub fn tightness_flags() -> Vec<TightnessFlag> {
    TIGHTNESS_FLAGS.with(|log| log.borrow().clone())
}

/// Empty the flag log for this thread. Callers reset before a measured run so [`tightness_flags`]
/// reads back that run's own flags only.
pub fn reset_tightness_flags() {
    TIGHTNESS_FLAGS.with(|log| log.borrow_mut().clear());
}

/// Every address the value's borrows reach, accumulated into `out`. Recursive through the stored
/// cells of every container, because a product embedding one element of an operand's list is a
/// contribution by that operand as surely as embedding the list itself.
///
/// The match is exhaustive by design — no `_` arm — so a new [`KObject`] variant is a compile error
/// here rather than a silent hole in the audit.
fn collect_addresses(value: &Carried<'_>, out: &mut AddressSet) {
    match value {
        Carried::Object(object) => {
            out.insert(*object as *const KObject<'_> as usize);
            collect_object_addresses(object, out);
        }
        // Both type-channel arms are `Copy` handles — an interned registry index, a borrow of a
        // name already resident where it was parsed — so neither reaches a region the fold pins.
        Carried::Type(_) | Carried::UnresolvedType(_) => {}
    }
}

/// [`collect_addresses`] for a bare object — the recursion's body.
fn collect_object_addresses(object: &KObject<'_>, out: &mut AddressSet) {
    match object {
        // Owned leaves: no borrow, so nothing to record.
        KObject::Number(_) | KObject::Bool(_) | KObject::Null => {}
        KObject::KString(text) => {
            out.insert(text.as_ptr() as usize);
        }
        KObject::KExpression(expression) => {
            out.insert(expression.parts.as_ptr() as usize);
        }
        KObject::KFunction(function) => {
            out.insert(*function as *const _ as usize);
            out.insert(function.captured_scope() as *const _ as usize);
        }
        KObject::Module(module) => {
            out.insert(*module as *const _ as usize);
            out.insert(module.child_scope() as *const _ as usize);
        }
        KObject::List(substrate, _) => collect_substrate_addresses(*substrate, out),
        KObject::Dict(substrate, _) => collect_substrate_addresses(*substrate, out),
        KObject::Record(substrate, _) => collect_substrate_addresses(*substrate, out),
        KObject::Tagged { tag, value, .. } => {
            out.insert(tag.as_ptr() as usize);
            collect_substrate_addresses(*value, out);
        }
        KObject::Wrapped { inner, .. } => collect_substrate_addresses(*inner, out),
    }
}

/// The substrate's own address plus every cell it stores — the shared arm of the four composite
/// carriers, which differ only in their index block.
fn collect_substrate_addresses<C>(
    substrate: &crate::machine::model::ContainerSubstrate<'_, C>,
    out: &mut AddressSet,
) {
    out.insert(substrate as *const _ as usize);
    for cell in substrate.cells() {
        out.insert(*cell as *const Held<'_> as usize);
        match cell {
            Held::Object(object) => collect_object_addresses(object, out),
            Held::Type(_) | Held::UnresolvedType(_) => {}
        }
    }
}

/// The three moments of one audited fold, threaded through the brand: the operand views and the
/// product are only nameable *inside* the closure, so their addresses are collected there and only
/// `usize`es cross back out to the comparison [`Self::finish`] runs afterwards.
pub(crate) struct FoldAudit {
    site: &'static str,
    operands: RefCell<Vec<AddressSet>>,
    product: RefCell<AddressSet>,
}

impl FoldAudit {
    /// Open an audit for the fold about to run at `site`.
    pub(crate) fn begin(site: &'static str) -> Self {
        FoldAudit {
            site,
            operands: RefCell::new(Vec::new()),
            product: RefCell::new(AddressSet::new()),
        }
    }

    /// Record one operand view, in dep order — called per view inside the fold closure.
    pub(crate) fn note_operand(&self, view: &Carried<'_>) {
        let mut addresses = AddressSet::new();
        collect_addresses(view, &mut addresses);
        self.operands.borrow_mut().push(addresses);
    }

    /// Record the product the closure built, before it leaves the brand.
    pub(crate) fn note_product(&self, product: &Carried<'_>) {
        collect_addresses(product, &mut self.product.borrow_mut());
    }

    /// Compare what the fold **pinned** against what its product can actually reference, logging a
    /// [`TightnessFlag`] per unjustified description member.
    ///
    /// Three members are never flagged. The product's **home** is its residence, not a reach it
    /// could have avoided. An **eternal** member is storage that outlives every region, so pinning
    /// it costs nothing and the retention's own eternal rule drops it anyway. And every member of a
    /// **contributing** operand's coverage is justified wholesale — operand granularity is the
    /// decided ground truth: a fold that embeds any part of an operand has no way to disclaim the
    /// rest of what that operand reaches.
    pub(crate) fn finish(&self, deps: &[&DeliveredCarried], product: &DeliveredCarried) {
        let product_addresses = self.product.borrow();
        let operands = self.operands.borrow();

        let contributed = |index: usize| {
            operands
                .get(index)
                .is_some_and(|addresses| !addresses.is_disjoint(&product_addresses))
        };
        let justified: Vec<Rc<FrameStorage>> = deps
            .iter()
            .enumerate()
            .filter(|(index, _)| contributed(*index))
            .flat_map(|(_, dep)| dep.coverage().members().iter().cloned())
            .collect();
        let non_contributing: Vec<usize> = (0..deps.len()).filter(|i| !contributed(*i)).collect();

        // The description is readable only from the **opened** carrier state, whose `'b` is the pin
        // borrow the re-anchor needs; the envelope supplies its own coverage for the open.
        let unjustified = product.open_at().with_reach_for_test(|reach| {
            reach
                .members()
                .into_iter()
                .filter(|member| {
                    // `minted()`, never `region()`: an audit that forced a region into existence
                    // would be measuring its own footprint.
                    let is_home = reach.with_home_region(|home| {
                        member.minted().is_some_and(|own| std::ptr::eq(own, home))
                    });
                    !is_home
                        && !member.needs_no_pin()
                        && !justified.iter().any(|ok| Rc::ptr_eq(ok, member))
                })
                .map(|member| Rc::as_ptr(&member) as usize)
                .collect::<Vec<usize>>()
        });

        TIGHTNESS_FLAGS.with(|log| {
            let mut log = log.borrow_mut();
            for member in unjustified {
                log.push(TightnessFlag {
                    site: self.site,
                    member,
                    non_contributing: non_contributing.clone(),
                });
            }
        });
    }
}

#[cfg(test)]
mod tests;
