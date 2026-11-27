//! The walk's side of the type half: the walk as the elaborator's [`TypeContext`], and steps two
//! and three of a unit — its declarations, then its type expressions, its guards, its
//! registrations and the callables it births — through the elaborator's entry points
//! ([`crate::elaborate::declarations`] and its siblings).
//!
//! A registration closed and unquantified is checked against the builtin overloads at its key
//! where it is typed ([`overlap`](super::super::check::overlap)), outside a quote's code, whose
//! closing record keeps its first overlap for the `EVAL` running it to report. A binding that makes
//! a deferred body's part no code refuses it in its words ([`SiteRule::NeverCode`]).

use crate::elaborate::judge;
use crate::elaborate::{self, Signatures, Stop, Tools, TypeContext};
use crate::elaborate::{SignatureMember, signature_member};
use crate::memory::{BumpVec, Lays, resident};
use crate::scope::{
    BodyShape, Born, Component, Located, NeverSatisfied, PathName, Payload, Position, Question,
    Registration, Resolved, Segment, ShapeError, ShapeKind, Shapes, Site, SiteRule, Slot,
    StaticCallable, StaticRegistered, StaticType, Store, TypeExpression, Unknown,
};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{DeclaredType, KType, Parametric};

use super::super::check::overlap;
use super::{Pass, Tag};

impl<'p, 'o: 'p, 'graph: 'o + 'p, 'tier: 'o, S: Store<'o>> TypeContext<'graph, 'o>
    for Pass<'p, 'o, '_, 'graph, 'tier, '_, '_, S>
{
    fn shapes(&self) -> &'graph Shapes<'graph> {
        self.shapes
    }

    fn declared(&self, body: Segment, slot: Slot) -> StaticType<'o> {
        self.declared_cell(self.shape_at(body), slot)
    }

    fn registered(&self, body: Segment, slot: Slot) -> StaticRegistered<'o> {
        self.registered_cell(self.shape_at(body), slot)
    }

    fn callable(&self, body: Segment) -> StaticCallable<'o> {
        self.callable_cell(self.shape_at(body))
    }

    fn surfaced(&self, body: Segment, slot: Slot) -> Option<KType> {
        self.surfaced_type(self.level_of(body)?, slot)
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

    fn marks(&self) -> &dyn crate::elaborate::judge::Derives<Unwalked = std::convert::Infallible> {
        self
    }
}

impl<'p, 'o: 'p, 'x, 'graph: 'o + 'p, 'tier: 'o, 'pair, 'reg, S: Store<'o>>
    Pass<'p, 'o, 'x, 'graph, 'tier, 'pair, 'reg, S>
{
    /// The type the surfaced type name at `slot` of the `USING … SCOPE` body at `level` is, where
    /// the module its `USING` reads declares the member exactly at its static type: a manifest
    /// member, or a head parameter the type pins, closed. A member the type leaves abstract, or
    /// declares over a name a run binds, is the run's to bind.
    pub(super) fn surfaced_type(&self, level: usize, slot: Slot) -> Option<KType> {
        let shape = self.chain[level].shape;
        let name @ BinderSymbol::Type(_) = shape.slot_name(slot) else {
            return None;
        };
        if shape.position_of(slot) != Position::PARAMETER {
            return None;
        }
        let super::Entering::Surfaced(surfacing, tag) =
            self.entering_of(level.checked_sub(1)?, shape)
        else {
            return None;
        };
        self.gather(tag);
        match signature_member(self, self.types, self.scratch, surfacing.upper, name)? {
            SignatureMember {
                declared: DeclaredType::Type(declared),
                unpinned: None,
            } => judge::static_of(self.types, declared),
            _ => None,
        }
    }

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

    /// `attempt`, typed again after each path it asks for is resolved ([`Stop::Unresolved`]): what
    /// an asking attempt gathered is dropped, so what it types is gathered once. It ends, since
    /// each request resolves one more of the chain's paths or refuses.
    fn typing<T>(
        &mut self,
        mut attempt: impl FnMut(&Self) -> Result<T, Stop<'o>>,
    ) -> Result<T, ShapeError<'o>> {
        loop {
            let gathered = self.gathered();
            match attempt(self) {
                Ok(typed) => return Ok(typed),
                Err(Stop::Refused(refusal)) => return Err(refusal),
                Err(Stop::Unresolved(asked)) => {
                    self.ungather(gathered);
                    self.resolve_asked(asked)?;
                }
            }
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
        self.typing(|pass| {
            fixed.clear();
            elaborate::declarations(
                pass,
                pass.tools(),
                pass.lay,
                shape,
                component,
                &mut |slot, typed| fixed.push((slot, typed)),
            )
        })?;
        for (slot, typed) in fixed {
            self.set_declared(level, slot, typed);
        }
        Ok(())
    }

    /// Step three's types: the type expressions of the shape at `level` at `expressions`, indices
    /// into its own in program order, then each `MATCH … WITH` guard against the others of its arm
    /// set, what a binding may make alike laid at the first expression's site
    /// ([`SiteRule::RepeatedGuard`]), and each `MATCH … UNDER`'s arms against its union.
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
        let repeats = elaborate::guards(
            self,
            self.types,
            shape,
            &written,
            self.scratch,
            |expression| self.type_cell(shape, expression.site),
        )?;
        // A binding making two guards alike refuses the first repeat in its words.
        if let Some(first) = written.first() {
            let mut asks = BumpVec::with_capacity_in(repeats.len(), self.scratch);
            asks.extend(repeats.iter().map(|(asked, _)| *asked));
            let mut refusals = BumpVec::with_capacity_in(repeats.len(), self.scratch);
            refusals.extend(repeats.iter().map(|(_, refusal)| *refusal));
            self.lay_refusals(level, first.site, SiteRule::RepeatedGuard, &asks, &refusals);
        }
        let bound = |clause| {
            let level = judge::lexical_level(self.types, clause)?;
            self.shapes.written_bound(shape.segment(), level)
        };
        elaborate::arms_under(
            self.scratch,
            shape,
            &written,
            |expression| self.type_cell(shape, expression.site),
            |clause| self.shown_variants(clause),
            bound,
        )
    }

    /// Type `expression`, one of the shape at `level`'s.
    pub(super) fn type_expression(
        &mut self,
        level: usize,
        expression: &TypeExpression<'graph>,
    ) -> Result<(), ShapeError<'o>> {
        let shape = self.chain[level].shape;
        self.chain[level].statement = expression.statement as usize;
        let typed = self.typing(|pass| {
            elaborate::expression(pass, pass.tools(), pass.lay, shape, expression)
        })?;
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
        let typed = self.typing(|pass| {
            let mut signatures: Signatures<'p> = BumpVec::new_in(pass.scratch);
            elaborate::registration(
                pass,
                pass.tools(),
                pass.lay,
                shape,
                registration,
                &mut signatures,
            )
        })?;
        self.set_registered(level, registration.slot, typed);
        let asked = overlap(
            &*self,
            shape,
            registration,
            typed,
            self.builtins,
            self.types,
            self.scratch,
        );
        // A quote's code keeps an overlap at every binding for the `EVAL` running it to report
        // (its closing record's); one a binding makes is the instance's at the `EVAL`'s entry.
        let in_code = (self.chain[..=level].iter()).any(|at| at.shape.kind() == ShapeKind::Code);
        let asked = match asked {
            Err(_) if in_code => None,
            asked => asked?,
        };
        // A binding making it overlap refuses it in that overload's words.
        if let (Some((question, refusals)), Some(site)) =
            (asked, shape.birth_site(registration.slot))
        {
            self.lay_decided(level, site, SiteRule::Overlap, &[question], |pass| {
                pass.lay.fill(refusals.len(), |at| {
                    Payload::Refusal(resident(pass.lay, refusals[at]))
                })
            });
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
        let registration = (shape.registrations().iter()).find(|registration| {
            (shape.births(registration.slot)).is_some_and(|born| born.segment() == body.segment())
        });
        let typed = self.typing(|pass| {
            elaborate::callable(pass, pass.tools(), pass.lay, shape, body, registration)
        })?;
        let levels = elaborate::own_levels(&*self, self.tools(), self.lay, body, typed);
        let born = Born {
            callable: typed,
            levels,
            instance: None,
        };
        self.set_born(body.segment(), born);
        Ok(())
    }

    /// The part an untraced `body`, born in the shape at `level`, is written as, typed there: a load
    /// refusal where it can never be code, laid as a binding's ([`SiteRule::NeverCode`]).
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
        let wanted = KType::ANY_CODE.into();
        let at = self.source(level, site);
        if let Some(refused) = judge::never_satisfies(self, types, scratch, typed, wanted) {
            return Err(ShapeError::BodyNotCode { refused, at });
        }
        // A binding that puts the part outside all code refuses it in these words.
        let refused = NeverSatisfied {
            value: typed,
            wanted,
        };
        let asked = Question::NeverSatisfies {
            value: typed,
            wanted,
        };
        let refusal = ShapeError::BodyNotCode { refused, at };
        self.lay_refusals(level, site, SiteRule::NeverCode, &[asked], &[refusal]);
        Ok(())
    }

    /// What the path at `level` of the chain `root` roots resolves to, where the walk resolved it,
    /// the resolving unit's frontier gathered.
    pub(super) fn resolution_at(&self, root: Segment, level: usize) -> Option<Resolved> {
        let (resolution, tag): (_, Tag) = self.resolved_at(root, PathName::Spelled(level))?;
        self.gather(tag);
        Some(resolution.resolved)
    }
}
