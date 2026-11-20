//! The walk's side of the type half: the walk as the elaborator's [`TypeContext`], and steps two
//! and three of a unit — its declarations, then its type expressions, its guards, its
//! registrations and the callables it births — through the elaborator's entry points
//! ([`crate::elaborate::declarations`] and its siblings).
//!
//! A registration closed and unquantified is checked against the builtin overloads at its key
//! where it is typed ([`overlap`](super::super::check::overlap)), outside a quote's code, whose
//! closing record keeps its first overlap for the `EVAL` running it to report.

use crate::elaborate::judge;
use crate::elaborate::{self, Signatures, Tools, TypeContext};
use crate::memory::{BumpVec, collect};
use crate::scope::{
    Binder, BodyShape, Born, Component, Located, PathName, Position, Registration, Resolved,
    Segment, ShapeError, ShapeKind, Shapes, Site, Slot, StaticCallable, StaticRegistered,
    StaticType, Store, TypeExpression, Unknown,
};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{KType, Parametric};

use super::super::check::overlap;
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

    fn note_open(&self, unknown: Unknown, mark: crate::scope::OpenMark<'static>) {
        Pass::note_open(self, unknown, mark);
    }

    fn marks(&self) -> &dyn crate::elaborate::judge::Marks {
        self
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
            self.lay,
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
    /// set, and each `MATCH … UNDER`'s arms against its union.
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
        elaborate::guards(
            self,
            self.types,
            shape,
            &written,
            self.scratch,
            |expression| self.type_cell(shape, expression.site),
        )?;
        elaborate::arms_under(self.types, self.scratch, shape, &written, |expression| {
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
        // A quote's code keeps its overlap for the `EVAL` running it to report.
        let in_code = (self.chain[..=level].iter()).any(|at| at.shape.kind() == ShapeKind::Code);
        if !in_code {
            overlap(
                &*self,
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
    /// at a constant cost. An untraced body's part is typed first, read where the callable is born.
    pub(super) fn birth(
        &mut self,
        level: usize,
        body: &'graph BodyShape<'graph>,
    ) -> Result<(), ShapeError<'o>> {
        if body.deferred() {
            self.deferred(level, body)?;
        }
        let shape = self.chain[level].shape;
        let typed = elaborate::callable(&*self, self.tools(), self.lay, shape, body)?;
        let levels = elaborate::own_levels(&*self, self.tools(), self.lay, body, typed);
        let born = Born {
            callable: typed,
            levels,
            instance: None,
            digestible: self.digestible(body, typed, levels),
        };
        self.set_born(body.segment(), born);
        Ok(())
    }

    /// The part an untraced `body`, born in the shape at `level`, is written as, typed there: a load
    /// refusal where it can never be code.
    fn deferred(
        &mut self,
        level: usize,
        body: &'graph BodyShape<'graph>,
    ) -> Result<(), ShapeError<'o>> {
        let shape = self.chain[level].shape;
        let form = body.form().expect("a callable's body records its form");
        let site = shape
            .site_of_body(form)
            .expect("a callable's form has a body");
        let Located::Part(part) = shape.located(site) else {
            unreachable!("a body's site locates its part")
        };
        let typed = self.part(level, part)?;
        let (types, scratch) = (self.types, self.scratch);
        match judge::never_satisfies(self, types, scratch, typed, KType::ANY_CODE.into()) {
            Some(refused) => Err(ShapeError::BodyNotCode {
                refused,
                at: self.source(level, site),
            }),
            None => Ok(()),
        }
    }

    /// The parameters of `frame`, typed `typed` with its own group at `levels`, whose arguments
    /// digest at a constant cost, sorted: a type binder, or one declared closed under `Module` or
    /// as a type value. A list's or a record's digest costs its size, so any other parameter keys
    /// nothing.
    fn digestible(
        &self,
        frame: &BodyShape<'graph>,
        typed: StaticCallable<'o>,
        levels: &[Parametric],
    ) -> &'o [Slot] {
        let (types, scratch) = (self.types, self.scratch);
        // The function type as the body reads it, instantiated once for every parameter. Each
        // `FOR ALL` name the binding binds is already closed in `levels`.
        let params = self
            .function_in_body(typed, levels)
            .map(|(params, _)| params);
        let digests = |slot: Slot| {
            let name = frame.slot_name(slot);
            if let BinderSymbol::Type(_) = name {
                return true;
            }
            let declared = params.and_then(|params| params.get(name.symbol()));
            // A parameter over names digests where its declared bound lies under a module or a
            // type value, as every binding of it does: a key's cost is the load's to fix, so it
            // asks nothing a binding answers, and a binding never keys more than the load does.
            let under = |ceiling: KType| {
                declared.is_some_and(|declared| {
                    judge::under_every_binding(types, scratch, declared, ceiling)
                })
            };
            under(KType::EMPTY_SIGNATURE) || under(KType::ANY_TYPE)
        };
        let mut digestible = BumpVec::new_in(scratch);
        for slot in (0..frame.slots() as u32).map(Slot) {
            if frame.position_of(slot) == Position::PARAMETER && digests(slot) {
                digestible.push(slot);
            }
        }
        collect(self.lay, digestible.iter().copied())
    }

    /// What the path at `level` of the chain `root` roots resolves to, where the walk resolved it,
    /// the resolving unit's frontier gathered.
    pub(super) fn resolution_at(&self, root: Segment, level: usize) -> Option<Resolved> {
        let (resolution, tag): (_, Tag) = self.resolved_at(root, PathName::Spelled(level))?;
        self.gather(tag);
        Some(resolution.resolved)
    }
}
