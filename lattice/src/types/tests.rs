//! The lattice's own suite: the golden identity pins, the generated-type
//! strategy, the laws, the residue a law cannot express (a family's, the ranking's, the intervals'
//! and lexical variables' worked examples in files of their own), and the heap-allocation bracket.

mod families;
mod generators;
mod golden;
mod heap;
mod intervals;
mod levels;
mod properties;
mod ranking;
mod residue;
mod verdicts;

use crate::bump::BumpAllocator;
use crate::symbols::{BinderSymbol, TypeSymbol};

use super::handle::{Handle, KType, Parametric, wrap};
use super::registry::TypeRegistry;
use super::shape::DispatchTokenElement;

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
        classes: &[u8],
        ret: Handle,
    ) -> (Handle, &'s [usize]);
}

impl RawSchemes for TypeRegistry<'_> {
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
        classes: &[u8],
        ret: Handle,
    ) -> (Handle, &'s [usize]) {
        let elements: Vec<DispatchTokenElement<Parametric>> =
            elements.iter().map(|element| element.view()).collect();
        let minted = self.shape_scheme(scratch, quantifiers, bounds, &elements, classes, wrap(ret));
        (minted.handle.raw(), minted.quantifier_map)
    }
}
