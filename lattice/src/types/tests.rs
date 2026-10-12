//! The lattice's own suite: the golden identity pins, the generated-type
//! strategy, the laws, the residue a law cannot express (a family's, the ranking's, the intervals'
//! and lexical variables' worked examples in files of their own), the heap-allocation bracket, and the store's own
//! contract: its footprint and its lookups.

mod families;
mod generators;
mod golden;
mod heap;
mod intervals;
mod levels;
mod properties;
mod ranking;
mod raw;
mod residue;
mod storage;
mod tiers;
mod verdicts;

use crate::bump::BumpAllocator;
use crate::symbols::{BinderSymbol, TypeSymbol};
use crate::types::tiers::{BumpHalf, Root, RootBuilder};

use super::chain::TypeChain;
use super::handle::{Handle, KType, Parametric, wrap};
use super::shape::DispatchTokenElement;
use super::tiers::Habitat;

/// A chain over a branch of a freshly seeded root in `region`: two tiers, so what a test interns
/// lands in the branch over the root's constants. The suite's default fixture.
pub(super) fn tiered(region: BumpAllocator<'_>) -> TypeChain<'_, BumpHalf<'_>> {
    TypeChain::new(Root::seeded(region), BumpHalf::new(region))
}

/// The load's chain over a fresh root builder in `region`: one table, for a test about the root's
/// own table.
pub(super) fn building(region: BumpAllocator<'_>) -> TypeChain<'_, &RootBuilder<'_>> {
    TypeChain::building(RootBuilder::in_region(region))
}

/// The two scheme doors over raw positions, answering the raw handle and the quantifier map — how
/// the suite mints a scheme from parts it holds raw. A position may itself hold a scheme, which no
/// koan door hands one as a [`Parametric`]: the suite exercises the raw walks past what koan builds.
trait RawSchemes {
    fn function_scheme_raw<'s>(
        &self,
        scratch: BumpAllocator<'s>,
        quantifiers: &[TypeSymbol],
        bounds: &[KType],
        params: &[(BinderSymbol, Handle)],
        ret: Handle,
    ) -> (Handle, &'s [usize]);

    fn shape_scheme_raw<'s>(
        &self,
        scratch: BumpAllocator<'s>,
        quantifiers: &[TypeSymbol],
        bounds: &[KType],
        elements: &[DispatchTokenElement],
        classes: &[u16],
        ret: Handle,
    ) -> (Handle, &'s [usize]);
}

impl<'run, H: Habitat<'run>> RawSchemes for TypeChain<'run, H> {
    fn function_scheme_raw<'s>(
        &self,
        scratch: BumpAllocator<'s>,
        quantifiers: &[TypeSymbol],
        bounds: &[KType],
        params: &[(BinderSymbol, Handle)],
        ret: Handle,
    ) -> (Handle, &'s [usize]) {
        let params: Vec<(BinderSymbol, Parametric)> =
            params.iter().map(|(name, kt)| (*name, wrap(*kt))).collect();
        let minted = self.function_scheme(scratch, quantifiers, bounds, &params, wrap(ret));
        (minted.handle.raw(), minted.quantifier_map)
    }

    fn shape_scheme_raw<'s>(
        &self,
        scratch: BumpAllocator<'s>,
        quantifiers: &[TypeSymbol],
        bounds: &[KType],
        elements: &[DispatchTokenElement],
        classes: &[u16],
        ret: Handle,
    ) -> (Handle, &'s [usize]) {
        let elements: Vec<DispatchTokenElement<Parametric>> = elements
            .iter()
            .map(|element| match *element {
                DispatchTokenElement::Keyword(keyword) => DispatchTokenElement::Keyword(keyword),
                DispatchTokenElement::Slot(slot) => DispatchTokenElement::Slot(wrap(slot)),
            })
            .collect();
        let minted = self.shape_scheme(scratch, quantifiers, bounds, &elements, classes, wrap(ret));
        (minted.handle.raw(), minted.quantifier_map)
    }
}
