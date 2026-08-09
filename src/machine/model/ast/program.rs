//! The eternal-tier marker on the AST's value channel: [`ProgramExpression`] and [`ProgramNode`],
//! the two `Copy` newtypes whose private fields make "this node's parts run is hosted in program
//! storage" a type rather than a discipline.
//!
//! The claim is about the **parts slice**, not the node struct. [`KExpression`] is `Copy` and rides
//! by value in a [`KObject::KExpression`](crate::machine::model::KObject) cell, so what a holder can
//! outlive is the run of parts the node borrows — and, transitively, everything reachable from it.
//! That is why the marker sits on the *references inside* the four expression-holding
//! [`ExpressionPart`](super::ExpressionPart) arms and on the value-channel cell, and why re-homing
//! the node struct itself at any brand ([`ProgramExpression::rehost`]) is sound.
//!
//! The marker is consumed only where the claim is used. The dispatch channel — `sub_dispatches`,
//! [`WorkingExpression`](super::WorkingExpression), the classifier, the structural cache — keeps
//! carrying bare [`KExpression`], so nothing here goes viral and there is no erase point to audit.

use std::ops::Deref;

use crate::machine::core::{ProgramBrand, RegionBrand};
use crate::source::{FileId, Span, Spanned};

use super::{ExpressionPart, KExpression};

/// A node whose parts run — and everything reachable from it — is hosted in eternal program
/// storage. Minted only by [`ProgramBrand`]'s doors below; holding one **is** the proof the value
/// channel's verdicts cite (`object_cell_reach` calling an expression cell `Owned`, `retains_home`
/// answering `false`, and [`RegionBrand::alloc_expression`] sealing with no member).
///
/// The field is private to this module, so the only way to obtain one is a door that took a
/// `ProgramBrand` or an accessor on a value that already carries the proof. A node built at a
/// per-call brand cannot enter the value channel:
///
/// ```compile_fail
/// let storage = koan::machine::program_storage();
/// let program = storage.brand();
/// // A bare `KExpression`, whatever brand built it, is not a `ProgramExpression`.
/// let node = koan::machine::model::ast::KExpression::new(program.region(), Vec::new());
/// let _cell = koan::machine::model::KObject::KExpression(node);
/// ```
///
/// Nor can one be wrapped after the fact — the field is private:
///
/// ```compile_fail
/// let storage = koan::machine::program_storage();
/// let program = storage.brand();
/// let node = koan::machine::model::ast::KExpression::new(program.region(), Vec::new());
/// let _marked = koan::machine::model::ast::program::ProgramExpression(node);
/// ```
///
/// The door path is the only one that compiles:
///
/// ```
/// let storage = koan::machine::program_storage();
/// let program = storage.brand();
/// let marked = program.new_expression(Vec::new());
/// let _cell = koan::machine::model::KObject::KExpression(marked);
/// ```
#[derive(Clone, Copy, Debug)]
pub struct ProgramExpression<'a>(KExpression<'a>);

/// A marked `&'a KExpression<'a>` — the payload of the four expression-holding
/// [`ExpressionPart`](super::ExpressionPart) arms (`Expression`, `SigiledTypeExpr`, `RecordType`,
/// `QuotedExpression`), which are the only conduits from the AST into the value channel. Matching
/// one out of an arm is what lets a value-channel door compile its proof out of the match.
#[derive(Clone, Copy, Debug)]
pub struct ProgramNode<'a>(&'a KExpression<'a>);

impl<'a> ProgramNode<'a> {
    /// The marked node by value — the copy-out the value-channel doors take. Copying the struct
    /// preserves the claim: the parts run it borrows does not move.
    pub fn expression(self) -> ProgramExpression<'a> {
        ProgramExpression(*self.0)
    }

    /// Widening exit to the bare reference, for the read and dispatch sites that carry no claim.
    pub fn reference(self) -> &'a KExpression<'a> {
        self.0
    }
}

impl<'a> Deref for ProgramNode<'a> {
    type Target = KExpression<'a>;
    fn deref(&self) -> &KExpression<'a> {
        self.0
    }
}

impl<'a> ProgramExpression<'a> {
    /// Widening exit to the bare node, mirroring [`ProgramBrand::region`]: widens, never narrows.
    pub fn node(self) -> KExpression<'a> {
        self.0
    }

    /// Re-home the node **struct** into `brand`'s region and hand back the marked reference.
    ///
    /// Sound at any brand because the marker's claim is about the parts run, which this does not
    /// touch: the bump copies the `Copy` struct (its `parts`, structural cache and binder cache all
    /// stay the program-storage borrows they were) into wherever `brand` allocates. What the
    /// resulting `ProgramNode` promises is unchanged — only the address of the node header moves.
    pub fn rehost(self, brand: RegionBrand<'a>) -> ProgramNode<'a> {
        ProgramNode(brand.allocator().value(self.0))
    }
}

impl<'a> Deref for ProgramExpression<'a> {
    type Target = KExpression<'a>;
    fn deref(&self) -> &KExpression<'a> {
        &self.0
    }
}

/// The mint doors. Every one takes a [`ProgramBrand`], so the storage tier is checked at the call
/// site rather than argued about afterwards.
///
/// The obligation each door carries: every string and slice reachable from `parts` must already be
/// allocated under this brand's region. The parser satisfies it structurally — its whole pipeline
/// runs under a `ProgramBrand` — and the one runtime builder that mints marked arms (`op_def`'s
/// bridge body) allocates its texts and operand nodes through `self.region()` for the same reason.
impl<'a> ProgramBrand<'a> {
    /// Spanless mint — [`KExpression::new`] with the tier proof attached.
    pub fn new_expression(self, parts: Vec<Spanned<ExpressionPart<'a>>>) -> ProgramExpression<'a> {
        ProgramExpression(KExpression::new(self.region(), parts))
    }

    /// Full mint — [`KExpression::build`] with the tier proof attached.
    pub fn build_expression(
        self,
        parts: Vec<Spanned<ExpressionPart<'a>>>,
        span: Option<Span>,
        file: Option<FileId>,
    ) -> ProgramExpression<'a> {
        ProgramExpression(KExpression::build(self.region(), parts, span, file))
    }

    /// Bump a marked node into program storage, yielding the reference an arm payload holds.
    pub fn alloc_node(self, expression: ProgramExpression<'a>) -> ProgramNode<'a> {
        ProgramNode(self.region().allocator().value(expression.0))
    }

    /// Build and bump in one step — the [`ExpressionPart::Expression`] analogue of
    /// [`KExpression::nested`], for the arm constructions that mint a fresh child node.
    pub fn nested_node(self, parts: Vec<Spanned<ExpressionPart<'a>>>) -> ProgramNode<'a> {
        ProgramNode(KExpression::nested(self.region(), parts))
    }
}
