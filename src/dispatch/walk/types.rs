//! The walk's side of the type half: the walk as the elaborator's [`TypeContext`], and steps two
//! and three of a unit — its declarations, then its type expressions, its guards, its
//! registrations and the callables it births — through the elaborator's entry points
//! ([`crate::elaborate::declarations`] and its siblings).
//!
//! A registration closed and unquantified is checked against the builtin overloads at its key
//! where it is typed ([`overlap`](super::super::check::overlap)), outside a quote's code, whose
//! `EVAL` checks its own.

use crate::elaborate::{self, Signatures, Tools, TypeContext};
use crate::memory::{BumpVec, collect};
use crate::scope::{
    Binder, BodyShape, Born, Component, Registration, Resolved, Segment, ShapeError, ShapeKind,
    Shapes, Site, Slot, StaticCallable, StaticRegistered, StaticType, Store, TypeExpression,
    Unknown,
};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{KType, Parametric, is_subtype_of};

use super::super::check::overlap;
use super::reads::Part;
use super::{Pass, Tag};

impl<'p, 'o: 'p, 'graph: 'o + 'p, 'tier: 'o, S: Store<'o>> TypeContext<'graph, 'o>
    for Pass<'p, 'o, '_, 'graph, 'tier, '_, '_, S>
{
    fn shapes(&self) -> &'graph Shapes<'graph> {
        self.shapes
    }

    fn binding(&self, unknown: Unknown) -> Option<crate::scope::Bound<'o>> {
        self.bound_to(unknown)
    }

    fn declared(&self, binder: Binder) -> StaticType<'o> {
        self.declared_cell(self.shape_at(binder.body), binder.slot)
    }

    fn registered(&self, binder: Binder) -> StaticRegistered<'o> {
        self.registered_cell(self.shape_at(binder.body), binder.slot)
    }

    fn callable(&self, body: Segment) -> StaticCallable<'o> {
        self.callable_cell(self.shape_at(body))
    }

    fn group_levels(&self, body: Segment) -> &'o [Parametric] {
        self.born(body).map_or(&[], |born| born.levels)
    }

    fn expression(&self, segment: Segment, site: Site) -> StaticType<'o> {
        self.type_cell(self.shape_at(segment), site)
    }

    fn resolution(&self, root: Segment, level: usize) -> Option<Resolved> {
        self.resolution_at(root, level)
    }

    fn note(&self, unknowns: &[Unknown]) {
        Pass::note(self, unknowns);
    }
}

impl<'p, 'o: 'p, 'x, 'graph: 'o + 'p, 'tier: 'o, 'pair, 'reg, S: Store<'o>>
    Pass<'p, 'o, 'x, 'graph, 'tier, 'pair, 'reg, S>
{
    /// What the type half's rules are handed.
    fn tools(
        &self,
    ) -> Tools<'_, 'p, 'graph, 'tier, 'pair, 'reg, 'x, crate::knot::Knotted<'graph, 'graph>> {
        Tools {
            builtins: self.builtins,
            types: self.types,
            scratch: self.scratch,
        }
    }

    /// Step two: declare `component`, a component of type binders of the shape at `level`.
    pub(super) fn declare(
        &mut self,
        level: usize,
        component: &Component<'graph>,
    ) -> Result<(), ShapeError<'o>> {
        let shape = self.chain[level].shape;
        let mut fixed = BumpVec::new_in(self.scratch);
        elaborate::declarations(
            &*self,
            self.tools(),
            shape,
            component,
            &mut |slot, typed| {
                fixed.push((slot, typed));
            },
        )?;
        for (slot, typed) in fixed {
            self.set_declared(level, slot, typed);
        }
        Ok(())
    }

    /// Step three's types: the type expressions of the shape at `level` at `expressions`, indices
    /// into its own in program order, then each `MATCH … WITH` guard against the others of its arm
    /// set.
    pub(super) fn type_expressions(
        &mut self,
        level: usize,
        expressions: &[u32],
    ) -> Result<(), ShapeError<'o>> {
        let shape = self.chain[level].shape;
        let all = shape.type_expressions();
        let mut written: BumpVec<'_, &TypeExpression<'graph>> = BumpVec::new_in(self.scratch);
        written.extend(expressions.iter().map(|at| &all[*at as usize]));
        for expression in written.iter() {
            self.type_expression(level, expression)?;
        }
        elaborate::guards(shape, &written, |expression| {
            self.type_cell(shape, expression.site)
        })
    }

    /// Type `expression`, one of the shape at `level`'s.
    pub(super) fn type_expression(
        &mut self,
        level: usize,
        expression: &TypeExpression<'graph>,
    ) -> Result<(), ShapeError<'o>> {
        let shape = self.chain[level].shape;
        self.chain[level].statement = expression.statement as usize;
        let typed = elaborate::expression(&*self, self.tools(), self.lay, shape, expression)?;
        self.set_expression(level, expression.site, typed);
        Ok(())
    }

    /// Step three's registration: the entry of `registration`, one the shape at `level` binds, and
    /// its overlap check.
    pub(super) fn register(
        &mut self,
        level: usize,
        registration: &Registration<'graph>,
    ) -> Result<(), ShapeError<'o>> {
        let shape = self.chain[level].shape;
        let mut signatures: Signatures<'p> = BumpVec::new_in(self.scratch);
        let typed = elaborate::registration(
            &*self,
            self.tools(),
            self.lay,
            shape,
            registration,
            &mut signatures,
        )?;
        self.set_registered(level, registration.slot, typed);
        let root = self.shape_at(self.chain_root_segment(level));
        if root.kind() != ShapeKind::Code {
            overlap(
                shape,
                registration,
                typed,
                self.builtins,
                self.types,
                self.scratch,
            )?;
        }
        Ok(())
    }

    /// Step three's birth: the callable `body`, born in the shape at `level`, typed through its
    /// encloser's names, beside its own group as its body reads it and its parameters that digest
    /// at a constant cost.
    pub(super) fn birth(
        &mut self,
        level: usize,
        body: &'graph BodyShape<'graph>,
    ) -> Result<(), ShapeError<'o>> {
        let shape = self.chain[level].shape;
        let typed = elaborate::callable(&*self, self.tools(), self.lay, shape, body)?;
        let levels = elaborate::own_levels(&*self, self.tools(), self.lay, body, typed);
        let born = Born {
            callable: typed,
            levels,
            instance: None,
            digestible: &[],
        };
        self.set_born(body.segment(), born);
        let mut digestible = BumpVec::new_in(self.scratch);
        for slot in (0..body.slots() as u32).map(Slot) {
            if body.position_of(slot) == crate::scope::Position::PARAMETER
                && self.digests(body, slot)
            {
                digestible.push(slot);
            }
        }
        let digestible = collect(self.lay, digestible.iter().copied());
        self.set_born(body.segment(), Born { digestible, ..born });
        Ok(())
    }

    /// Whether `frame`'s parameter at `slot` digests at a constant cost: a type binder, or one
    /// declared closed under `Module` or as a type value. A list's or a record's digest costs its
    /// size, so any other parameter keys nothing.
    fn digests(&self, frame: &BodyShape<'graph>, slot: Slot) -> bool {
        let name = frame.slot_name(slot);
        if let BinderSymbol::Type(_) = name {
            return true;
        }
        let (types, scratch) = (self.types, self.scratch);
        let declared = (self.body_type(frame, Part::Parameter(name.symbol())))
            .and_then(|declared| types.concrete(declared));
        declared.is_some_and(|declared| {
            is_subtype_of(types, scratch, declared, KType::EMPTY_SIGNATURE)
                || is_subtype_of(types, scratch, declared, KType::ANY_TYPE)
        })
    }

    /// What the path at `level` of the chain `root` roots resolves to, where the walk resolved it,
    /// the resolving unit's frontier gathered.
    pub(super) fn resolution_at(&self, root: Segment, level: usize) -> Option<Resolved> {
        let (resolution, tag): (_, Tag) = self.resolved_at(root, level)?;
        self.gather(tag);
        Some(resolution.resolved)
    }
}
