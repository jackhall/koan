//! The suite's handle-level doors. Inside the lattice every relation and walk runs over nodes; the
//! suite states its laws over raw handles, which reach past what koan's typed doors take — a scheme
//! in any position, a variable in the order. Each door here enters its handles once, runs the
//! relation of the same name, and reads its result back as handles. The modules mirror the
//! lattice's own, so a test imports `raw::order::fits` where it means the order's *fits*.
//!
//! [`RawRegistry`] gives the registry back the readers that moved onto a node: a scheme's split,
//! a declared type and the four probes.

use crate::bump::BumpAllocator;

use crate::types::handle::{DeclaredType, Handle, Parametric, TypeHandle};
use crate::types::node::{Group, TypeNode};
use crate::types::registry::TypeRegistry;
use crate::types::stored::Linked;

/// The registry's handle-level readers, for the suite alone.
pub(super) trait RawRegistry<'run> {
    /// `handle`'s own group and the type under it.
    fn split(&self, handle: Handle) -> (Group<'run>, Handle);
    /// What `handle` is declared as.
    fn declared(&self, handle: Handle) -> DeclaredType<Parametric>;
    fn contains_quantified(&self, handle: Handle) -> bool;
    fn contains_rigid(&self, handle: Handle) -> bool;
    fn is_concrete(&self, handle: Handle) -> bool;
    fn contains_carrier(&self, handle: Handle) -> bool;
}

impl<'run> RawRegistry<'run> for TypeRegistry<'run> {
    fn split(&self, handle: Handle) -> (Group<'run>, Handle) {
        let (group, body) = self.enter(handle).split();
        (group, body.handle())
    }

    fn declared(&self, handle: Handle) -> DeclaredType<Parametric> {
        self.enter(handle).declared()
    }

    fn contains_quantified(&self, handle: Handle) -> bool {
        self.enter(handle).contains_quantified()
    }

    fn contains_rigid(&self, handle: Handle) -> bool {
        self.enter(handle).contains_rigid()
    }

    fn is_concrete(&self, handle: Handle) -> bool {
        self.enter(handle).is_concrete()
    }

    fn contains_carrier(&self, handle: Handle) -> bool {
        self.enter(handle).contains_carrier()
    }
}

/// Each of `handles`' nodes.
fn enter_all<'run>(types: &TypeRegistry<'run>, handles: &[Handle]) -> Vec<Linked<'run>> {
    handles.iter().map(|handle| types.enter(*handle)).collect()
}

pub(super) mod order {
    use super::*;

    pub(in crate::types::tests) fn is_subtype_of(
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        a: Handle,
        b: Handle,
    ) -> bool {
        crate::types::order::is_subtype_of(types, scratch, types.enter(a), types.enter(b))
    }

    pub(in crate::types::tests) fn fits(
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        a: Handle,
        b: Handle,
    ) -> bool {
        crate::types::order::fits(types, scratch, types.enter(a), types.enter(b))
    }

    pub(in crate::types::tests) fn satisfied_by(
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        slot: Handle,
        carried: Handle,
    ) -> bool {
        crate::types::order::satisfied_by(types, scratch, types.enter(slot), types.enter(carried))
    }
}

pub(super) mod lattice {
    use super::*;

    pub(in crate::types::tests) fn join(
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        a: Handle,
        b: Handle,
    ) -> Handle {
        crate::types::lattice::join(types, scratch, types.enter(a), types.enter(b)).handle()
    }

    pub(in crate::types::tests) fn join_iter(
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        iter: impl IntoIterator<Item = Handle>,
    ) -> Handle {
        let nodes = iter.into_iter().map(|handle| types.enter(handle));
        crate::types::lattice::join_iter(types, scratch, nodes).handle()
    }

    pub(in crate::types::tests) fn meet_through_variables(
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        a: Handle,
        b: Handle,
    ) -> Handle {
        crate::types::lattice::meet_through_variables(
            types,
            scratch,
            types.enter(a),
            types.enter(b),
        )
        .handle()
    }
}

pub(super) mod ranking {
    use super::*;
    use crate::bump::BumpVec;
    use crate::types::unify::Interval;

    pub(in crate::types::tests) use crate::types::ranking::{Judged, Verdict};

    pub(in crate::types::tests) fn admit_by_class<'s>(
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'s>,
        declared: Handle,
        arguments: &[Handle],
    ) -> Option<&'s [Handle]> {
        let arguments = enter_all(types, arguments);
        let solution = crate::types::ranking::admit_by_class(
            types,
            scratch,
            types.enter(declared),
            &arguments,
        )?;
        Some(scratch.alloc_slice_fill_iter(solution.iter().map(|solved| solved.handle())))
    }

    pub(in crate::types::tests) fn judge_by_class<'s>(
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'s>,
        declared: Handle,
        arguments: &[Interval<Handle>],
    ) -> Judged<'s> {
        let arguments: Vec<_> = arguments
            .iter()
            .map(|argument| Interval {
                lower: types.enter(argument.lower),
                upper: types.enter(argument.upper),
            })
            .collect();
        crate::types::ranking::judge_by_class(types, scratch, types.enter(declared), &arguments)
    }

    pub(in crate::types::tests) fn solving_slots<'s>(
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'s>,
        declared: Handle,
    ) -> &'s [bool] {
        crate::types::ranking::solving_slots(types, scratch, types.enter(declared))
    }

    pub(in crate::types::tests) fn select_by_class<'s>(
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'s>,
        shapes: &[Handle],
    ) -> BumpVec<'s, usize> {
        crate::types::ranking::select_by_class(types, scratch, &enter_all(types, shapes))
    }
}

pub(super) mod shape {
    use super::*;
    use crate::types::shape::{Classes, Shape};

    pub(in crate::types::tests) fn shape_of<'run>(
        types: &TypeRegistry<'run>,
        kt: Handle,
    ) -> Option<Shape<'run>> {
        Shape::of(types.enter(kt))
    }

    pub(in crate::types::tests) fn keys_equal(
        left: Handle,
        right: Handle,
        types: &TypeRegistry<'_>,
    ) -> bool {
        crate::types::shape::keys_equal(types.enter(left), types.enter(right))
    }

    pub(in crate::types::tests) fn shape_classes<'run>(
        kt: Handle,
        types: &TypeRegistry<'run>,
    ) -> Classes<'run> {
        crate::types::shape::shape_classes(types.enter(kt))
    }
}

pub(super) mod sig_relations {
    use super::*;
    use crate::types::shape::Specificity;
    use crate::types::sig_relations::FitsFailure;

    pub(in crate::types::tests) fn admits_shape(
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        declared: Handle,
        candidate: Handle,
    ) -> bool {
        crate::types::sig_relations::admits_shape(
            types,
            scratch,
            types.enter(declared),
            types.enter(candidate),
        )
    }

    pub(in crate::types::tests) fn shape_specificity(
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        a: Handle,
        b: Handle,
    ) -> Specificity {
        crate::types::sig_relations::shape_specificity(
            types,
            scratch,
            types.enter(a),
            types.enter(b),
        )
    }

    pub(in crate::types::tests) fn sig_fits<'run: 's, 's>(
        types: &TypeRegistry<'run>,
        scratch: BumpAllocator<'s>,
        offered: Handle,
        asked: Handle,
    ) -> Result<(), FitsFailure<'run, 's>> {
        crate::types::sig_relations::sig_fits(
            types,
            scratch,
            types.enter(offered),
            types.enter(asked),
        )
    }
}

pub(super) mod signatures {
    use super::*;
    use crate::bump::BumpVec;
    use crate::types::signatures::Application;

    pub(in crate::types::tests) use crate::types::signatures::applications_under;

    pub(in crate::types::tests) fn applications<'run, 's>(
        types: &TypeRegistry<'run>,
        scratch: BumpAllocator<'s>,
        kt: Handle,
    ) -> Option<BumpVec<'s, Application<'run, 'run>>> {
        crate::types::signatures::applications(scratch, types.enter(kt))
    }

    pub(in crate::types::tests) fn is_signature_type(types: &TypeRegistry<'_>, kt: Handle) -> bool {
        crate::types::signatures::is_signature_type(types.enter(kt))
    }
}

pub(super) mod substitute {
    use super::*;
    use crate::symbols::TypeSymbol;
    use crate::types::schema::Members;
    use crate::types::unify::Interval;

    pub(in crate::types::tests) use crate::types::substitute::Side;

    pub(in crate::types::tests) fn substitute_quantified<B: TypeHandle>(
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        kt: Handle,
        bindings: &[B],
    ) -> Handle {
        let bindings: Vec<_> = bindings
            .iter()
            .map(|binding| types.enter(binding.raw()))
            .collect();
        crate::types::substitute::substitute_quantified(types, scratch, types.enter(kt), &bindings)
            .handle()
    }

    pub(in crate::types::tests) fn substitute_levels<B: TypeHandle>(
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        kt: Handle,
        bindings: &[B],
    ) -> Handle {
        let bindings: Vec<_> = bindings
            .iter()
            .map(|binding| types.enter(binding.raw()))
            .collect();
        crate::types::substitute::substitute_levels(types, scratch, types.enter(kt), &bindings)
            .handle()
    }

    pub(in crate::types::tests) fn bound_above(
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        kt: Handle,
    ) -> Handle {
        crate::types::substitute::bound_above(types, scratch, types.enter(kt)).handle()
    }

    pub(in crate::types::tests) fn substitute_parameters<B: TypeHandle>(
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        kt: Handle,
        bindings: Members<'_, TypeSymbol, B>,
    ) -> Handle {
        let bindings = Members::from_pairs(
            scratch,
            bindings
                .iter()
                .map(|(name, binding)| (*name, types.enter(binding.raw()))),
        );
        crate::types::substitute::substitute_parameters(types, scratch, types.enter(kt), bindings)
            .handle()
    }

    /// `interval` is handed each variable's handle and its node read through a handle.
    pub(in crate::types::tests) fn read_through<'run>(
        types: &TypeRegistry<'run>,
        scratch: BumpAllocator<'_>,
        kt: Handle,
        side: Side,
        interval: &mut impl FnMut(Handle, &TypeNode<'run, Handle>) -> Option<Interval<Handle>>,
    ) -> Handle {
        crate::types::substitute::read_through(
            types,
            scratch,
            types.enter(kt),
            side,
            &mut |at, _| {
                let ends = interval(at.handle(), &at.view_as())?;
                Some(Interval {
                    lower: types.enter(ends.lower),
                    upper: types.enter(ends.upper),
                })
            },
        )
        .handle()
    }
}

pub(super) mod unify {
    use super::*;
    use crate::bump::BumpVec;
    use crate::types::unify::{Collector, UnifyFailure};
    use crate::types::walk::Variance;

    pub(in crate::types::tests) fn admits<'run, T>(
        types: &TypeRegistry<'run>,
        scratch: BumpAllocator<'_>,
        declared: Handle,
        carried: Handle,
        variance: Variance,
        collector: &mut Collector<'_, 'run, T>,
    ) -> Result<(), UnifyFailure<Handle>> {
        crate::types::unify::admits(
            types,
            scratch,
            types.enter(declared),
            types.enter(carried),
            variance,
            collector,
        )
        .map_err(UnifyFailure::typed)
    }

    pub(in crate::types::tests) fn most_determined_first<'s>(
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'s>,
        declared: &[Handle],
        carried: Handle,
    ) -> BumpVec<'s, Handle> {
        let order = crate::types::unify::most_determined_first(
            types,
            scratch,
            &enter_all(types, declared),
            types.enter(carried),
        );
        let mut handles = BumpVec::with_capacity_in(order.len(), scratch);
        handles.extend(order.iter().map(|member| member.handle()));
        handles
    }

    pub(in crate::types::tests) fn ties(
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        a: Handle,
        b: Handle,
    ) -> bool {
        crate::types::unify::ties(types, scratch, types.enter(a), types.enter(b))
    }
}

pub(super) mod walk {
    use super::*;
    use crate::types::walk::Variance;
    use crate::types::walk::unary::{Context, Visit};

    /// `at` is handed each node's handle and the node read through a handle.
    pub(in crate::types::tests) fn visit<'run>(
        types: &TypeRegistry<'run>,
        scratch: BumpAllocator<'_>,
        root: Handle,
        at: &mut impl FnMut(Handle, &TypeNode<'run, Handle>, &Context) -> Visit,
    ) -> bool {
        crate::types::walk::unary::visit(scratch, types.enter(root), &mut |node, _, context| {
            at(node.handle(), &node.view_as(), context)
        })
    }

    pub(in crate::types::tests) fn visit_free_quantified(
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
        root: Handle,
        variance: Variance,
        at: &mut impl FnMut(usize, &Context) -> Visit,
    ) -> bool {
        crate::types::walk::unary::visit_free_quantified(scratch, types.enter(root), variance, at)
    }
}
