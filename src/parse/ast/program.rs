//! Program storage's construction doors for the AST: [`ProgramBrand`]'s mints, and
//! [`ProgramExpression`], the `Copy` newtype lowering carries a node in while it is still being
//! placed.
//!
//! A node's parts hold plain `&'a KExpression<'a>` references, so the storage a node lies in is
//! its lifetime: a node at the program's lifetime is eternal by that lifetime alone. These doors
//! take the brand at the node's own lifetime, and the brand is invariant in it, so program
//! storage never holds a node of shorter-lived storage. See
//! [README.md](../README.md#the-eternal-tier-is-the-lifetime).

use std::ops::Deref;

use crate::memory::{ProgramBrand, resident};
use crate::source::{SourceRef, Spanned};

use super::{ExpressionPart, KExpression, RunIter};

/// A node whose parts run — and everything reachable from it — is hosted in eternal program
/// storage, as lowering carries a node before it places it. Minted only by [`ProgramBrand`]'s
/// doors below.
///
/// The field is private to this module, so the only way to obtain one is a door that took a
/// `ProgramBrand` or an accessor on a value that already carries the proof. A node cannot be
/// wrapped after the fact:
///
/// ```compile_fail
/// let storage = koan::memory::program_storage();
/// let program = storage.brand();
/// # use koan::source::{register, SourceFile, SourceRef, Span};
/// # let file = register(SourceFile::new("<doc>", String::new()));
/// # let source = SourceRef { span: Span { start: 0, end: 0 }, file };
/// let node = koan::parse::KExpression::build(program.writer(), &[], source);
/// let _marked = koan::parse::ProgramExpression(node);
/// ```
///
/// The door path is the only one that compiles:
///
/// ```
/// let storage = koan::memory::program_storage();
/// let program = storage.brand();
/// # use koan::source::{register, SourceFile, SourceRef, Span};
/// # let file = register(SourceFile::new("<doc>", String::new()));
/// # let source = SourceRef { span: Span { start: 0, end: 0 }, file };
/// let _marked: koan::parse::ProgramExpression<'_> = program.build_expression(&[], source);
/// ```
#[derive(Clone, Copy, Debug)]
pub struct ProgramExpression<'a>(KExpression<'a>);

impl<'a> ProgramExpression<'a> {
    /// Widening exit to the bare node, mirroring [`ProgramBrand::writer`]: widens, never narrows.
    pub fn node(self) -> KExpression<'a> {
        self.0
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
/// No door carries a prose obligation: the parameter types are the tier. `parts` carries
/// `Spanned<ExpressionPart<'a>>` at the brand's own `'a` — as a borrowed run or as an exact-length
/// iterator, per [`RunIter`] — and the brand is invariant in `'a`,
/// so a caller cannot shorten it to reach a door with shorter-lived parts. Everything reachable
/// from `parts` therefore outlives the program-storage borrow — program-hosted, another
/// eternal-tier region, or `'static`, each of which the eternal rule already prices as reaching
/// nothing.
///
/// A part at a step lifetime cannot reach a door: the brand cannot shorten to `'step`, and the
/// part cannot lengthen to `'program`. The two-lifetime shape below is `'program: 'step`, never
/// the reverse:
///
/// ```compile_fail
/// use koan::memory::ProgramBrand;
/// use koan::parse::{ExpressionPart, ProgramExpression};
/// use koan::source::{SourceRef, Spanned};
/// fn mint_step_part<'program: 'step, 'step>(
///     program: ProgramBrand<'program>,
///     part: Spanned<ExpressionPart<'step>>,
///     source: SourceRef,
/// ) -> ProgramExpression<'step> {
///     program.build_expression(&[part], source)
/// }
/// ```
///
/// The result is taken at `'step` on purpose, so nothing about the *return* can carry the
/// rejection: a covariant brand would shorten to `'step`, accept the part, and mint. Only the
/// brand's invariance refuses it.
impl<'a> ProgramBrand<'a> {
    /// Mint — [`KExpression::build`] with the tier proof attached.
    pub fn build_expression(
        self,
        parts: &[Spanned<ExpressionPart<'a>>],
        source: SourceRef,
    ) -> ProgramExpression<'a> {
        ProgramExpression(KExpression::build(self.writer(), parts, source))
    }

    /// [`build_expression`](Self::build_expression)'s peer for a computed run —
    /// [`KExpression::build_from_iter`] with the tier proof attached.
    pub fn build_expression_from_iter<I>(self, parts: I, source: SourceRef) -> ProgramExpression<'a>
    where
        I: IntoIterator<Item = Spanned<ExpressionPart<'a>>>,
        RunIter<I>: ExactSizeIterator,
    {
        ProgramExpression(KExpression::build_from_iter(self.writer(), parts, source))
    }

    /// Write a marked node into program storage, yielding the reference an arm payload holds.
    pub fn alloc_node(self, expression: ProgramExpression<'a>) -> &'a KExpression<'a> {
        resident(self.writer(), expression.0)
    }

    /// Build and write in one step — the [`ExpressionPart::Expression`] analogue of
    /// [`KExpression::nested`], for the arm constructions that mint a fresh child node.
    pub fn nested_node(
        self,
        parts: &[Spanned<ExpressionPart<'a>>],
        source: SourceRef,
    ) -> &'a KExpression<'a> {
        KExpression::nested(self.writer(), parts, source)
    }

    /// [`nested_node`](Self::nested_node)'s peer for a computed run —
    /// [`KExpression::nested_from_iter`] with the tier proof attached.
    pub fn nested_node_from_iter<I>(self, parts: I, source: SourceRef) -> &'a KExpression<'a>
    where
        I: IntoIterator<Item = Spanned<ExpressionPart<'a>>>,
        RunIter<I>: ExactSizeIterator,
    {
        KExpression::nested_from_iter(self.writer(), parts, source)
    }
}
