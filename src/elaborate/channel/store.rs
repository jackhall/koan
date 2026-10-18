//! The type channel's **fact store**: the one door every read and write of a channel cell goes
//! through ([the build module's design](../../build/README.md#type-facts-and-the-solver)). The
//! walk reaches a type binder's, a registration's, a type expression's or a callable's cell only
//! through the methods here, which `tests::boundary` holds it to, so the store is the one place a
//! read is recorded.

use crate::scope::{
    BodyShape, Keyed, PathEntry, PathRead, ShapeError, Site, Slot, StaticCallable,
    StaticRegistered, StaticType, Target, TypeExpression,
};
use crate::type_lattice::Parametric;

use super::Pass;

impl<'p, 'graph, X> Pass<'p, 'graph, '_, X> {
    /// The cell of the type binder at `slot` of `shape`.
    pub(super) fn declared_cell(
        &self,
        shape: &BodyShape<'graph>,
        slot: Slot,
    ) -> StaticType<'graph> {
        shape.declared_type(slot)
    }

    /// The cell of the registration at `slot` of `shape`.
    pub(super) fn registered_cell(
        &self,
        shape: &BodyShape<'graph>,
        slot: Slot,
    ) -> StaticRegistered<'graph> {
        shape.registered_type(slot)
    }

    /// The cell of the type expression at `site` of `shape`.
    pub(super) fn type_cell(&self, shape: &BodyShape<'graph>, site: Site) -> StaticType<'graph> {
        shape.typed_expression(site)
    }

    /// The cell of the type expression `expression`.
    pub(super) fn expression_cell(
        &self,
        expression: &TypeExpression<'graph>,
    ) -> StaticType<'graph> {
        expression.typed()
    }

    /// The callable body `body`'s function type as its body reads it, its own group at its group
    /// levels: `None` where the channel did not type it.
    pub(super) fn body_type(&self, body: &BodyShape<'graph>) -> Option<Parametric> {
        body.in_body(self.types, self.scratch)
    }

    /// Whether the quote's code `code` holds a refusal.
    pub(super) fn code_refused(&self, code: &BodyShape<'graph>) -> bool {
        code.refusal().is_some()
    }

    /// Every type expression `shape` records, each cell the walk fixes.
    pub(super) fn type_expressions(
        &self,
        shape: &BodyShape<'graph>,
    ) -> &'graph [TypeExpression<'graph>] {
        shape.type_expressions()
    }

    /// Fix `expression`'s cell.
    pub(super) fn fix_expression(
        &self,
        expression: &TypeExpression<'graph>,
        typed: StaticType<'graph>,
    ) {
        expression.fix(typed);
    }

    /// Fix the cell of the type binder at `slot` of `shape`.
    pub(super) fn fix_declared(
        &self,
        shape: &BodyShape<'graph>,
        slot: Slot,
        typed: StaticType<'graph>,
    ) {
        shape.fix_declared(slot, typed);
    }

    /// Fix the cell of the registration at `slot` of `shape`.
    pub(super) fn fix_registered(
        &self,
        shape: &BodyShape<'graph>,
        slot: Slot,
        typed: StaticRegistered<'graph>,
    ) {
        shape.fix_registered(slot, typed);
    }

    /// Fix the callable body `body`'s cell.
    pub(super) fn fix_callable(&self, body: &BodyShape<'graph>, typed: StaticCallable<'graph>) {
        body.fix_callable(typed);
    }

    /// Fix the callable body `body`'s own group as its body reads it.
    pub(super) fn fix_group_levels(&self, body: &BodyShape<'graph>, levels: &'graph [Parametric]) {
        body.fix_group_levels(levels);
    }

    /// Keep the refusal of the quote's code `code`.
    pub(super) fn refuse_code(
        &self,
        code: &BodyShape<'graph>,
        refusal: &'graph ShapeError<'graph>,
    ) {
        code.refuse_typing(refusal);
    }

    /// Lay the path spellings `shape`'s type expressions read down.
    pub(super) fn fix_path_reads(
        &self,
        shape: &BodyShape<'graph>,
        reads: Keyed<'graph, (Site, PathRead<'graph>)>,
    ) {
        shape.fix_path_reads(reads);
    }

    /// Lay the path entries the channel minted for the chain `root` roots down.
    pub(super) fn fix_channel_paths(
        &self,
        root: &BodyShape<'graph>,
        paths: &'graph [&'graph PathEntry<'graph>],
    ) {
        root.fix_channel_paths(paths);
    }

    /// Lay each lexical variable `shape` declares down, beside where its activation holds it.
    pub(super) fn fix_declared_variables(
        &self,
        shape: &BodyShape<'graph>,
        variables: &'graph [(usize, Target)],
    ) {
        shape.fix_declared_variables(variables);
    }
}
