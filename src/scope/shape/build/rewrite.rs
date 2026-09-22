//! The **operator-run rewrite**: one pass over a body's statements, before its binders are laid
//! out, that chains every operator run written in them.
//!
//! An operator run — a slot-led node whose keywords alternate with slots, two or more of them — is
//! rewritten exactly once, here, where the body shape holding it is built. What it chains under is
//! the one chaining every symbol in it shares: a builtin group, a group an enclosing shape holds,
//! or the unary or fold-left chaining of a lone symbol no group claims. Nothing downstream ever
//! meets an operator run.
//!
//! Every node the rewrite builds goes through [`parse`](crate::parse)'s node constructor, so it
//! carries a real structural cache and reads like any other node. A subtree nothing changed is
//! returned as `None` and keeps its own part addresses, so a statement holding no operator run is
//! the statement the parser produced.
//!
//! The four rewrites, over operands `o0 … on` and operators `k1 … kn`:
//!
//! - **fold left** `(((o0 k1 o1) k2 o2) …)`, **fold right** `(o0 k1 (o1 k2 (…)))`;
//! - **unary** `k1 [o0 … on]`, one keyword-first call over a list literal;
//! - **pairwise** the adjacent pairs `o(i-1) ki oi`, folded through the group's combiner written
//!   infix. An operand that is not a name or a literal would be evaluated twice, so it is hoisted
//!   into an anonymous slot of a synthesized block shape, in source order.
//!
//! `a != b` is never built: wherever a pair or a bare infix run spells it, the rewrite emits
//! `NOT (a == b)` instead, so `!=` reaches no bucket and is the opposite of `==` by construction.
//!
//! See [README.md § Operator groups](../../README.md#operator-groups).

use crate::memory::{BumpVec, collect};
use crate::parse::builtin_shapes::KEYWORDS;
use crate::parse::builtin_shapes::role::Role;
use crate::parse::{DispatchShape, ExpressionPart, KExpression, ProgramNode, Spanned};
use crate::symbols::{KeywordSymbol, ValueSymbol};
use crate::type_lattice::{FoldDirection, ReductionMode};

use super::super::super::groups::{
    Cover, equal_symbol, equality_mode, is_equality, is_unequal, not_symbol,
};
use super::super::super::signature::{pair_name, signature_run};
use super::super::{Position, ShapeError};
use super::Builder;

/// A part run under construction, in scratch.
type Run<'x, 'graph> = BumpVec<'x, Spanned<ExpressionPart<'graph>>>;

impl<'graph, 'x> Builder<'graph, 'x, '_> {
    /// One statement with every operator run in it chained, or `None` when it holds none.
    ///
    /// A statement that is itself a hoisting pairwise operator run becomes the one-part wrapper
    /// around its block, so the block is held by an `Expression` part wherever it lands.
    pub(super) fn rewrite_statement(
        &mut self,
        statement: usize,
        node: &KExpression<'graph>,
    ) -> Result<Option<KExpression<'graph>>, ShapeError> {
        let at = Position::statement(statement);
        let Some(rewritten) = self.rewrite_node(at, node)? else {
            return Ok(None);
        };
        if self.is_block(rewritten) {
            let mut run: Run<'x, 'graph> = BumpVec::new_in(self.scratch);
            run.push(Spanned::bare(ExpressionPart::Expression(rewritten)));
            return Ok(Some(*self.brand.nested_node(&run).reference()));
        }
        Ok(Some(*rewritten.reference()))
    }

    /// Whether `node` is a block the pairwise rewrite synthesized.
    fn is_block(&self, node: ProgramNode<'graph>) -> bool {
        self.blocks
            .contains_key(&(node.reference().parts.as_ptr() as usize))
    }

    /// One node with every operator run under it chained. A node with a builtin shape is read by
    /// its form's roles — a body and an arm's body are rewritten by their own draft, under their
    /// own frame — and a formless one part by part, and is chained when it is an operator run.
    fn rewrite_node(
        &mut self,
        at: Position,
        node: &KExpression<'graph>,
    ) -> Result<Option<ProgramNode<'graph>>, ShapeError> {
        let mut run: Run<'x, 'graph> = BumpVec::with_capacity_in(node.parts.len(), self.scratch);
        let mut changed = false;
        match node.cache().builtin_shape() {
            Some(form) => {
                for (role, part) in form.roles().zip(node.parts) {
                    let rewritten = match role {
                        Role::Rhs | Role::Argument | Role::TypeExpression | Role::Definition(_) => {
                            self.rewrite_part(at, &part.value)?
                        }
                        Role::Signature => self.rewrite_signature(at, &part.value)?,
                        Role::Branches(_) => self.rewrite_branches(at, &part.value)?,
                        Role::Keyword
                        | Role::Name
                        | Role::Data
                        | Role::Label
                        | Role::Quantifiers
                        | Role::Body(_)
                        | Role::Unsupported => None,
                    };
                    changed |= rewritten.is_some();
                    run.push(Spanned {
                        value: rewritten.unwrap_or(part.value),
                        span: part.span,
                    });
                }
            }
            None => {
                for part in node.parts {
                    let rewritten = self.rewrite_part(at, &part.value)?;
                    changed |= rewritten.is_some();
                    run.push(Spanned {
                        value: rewritten.unwrap_or(part.value),
                        span: part.span,
                    });
                }
                if node.shape() == DispatchShape::OperatorChain {
                    return self.chain(at, &run).map(Some);
                }
                // `a != b` written alone is the same rewrite one pair of a pairwise run takes.
                if let [left, separator, right] = &run[..]
                    && let ExpressionPart::Keyword(symbol) = separator.value
                    && is_unequal(symbol)
                {
                    return self.infix(at, *left, symbol, *right).map(Some);
                }
            }
        }
        Ok(changed.then(|| self.brand.nested_node(&run)))
    }

    /// One part with every operator run under it chained. A quote is data and is never rewritten.
    fn rewrite_part(
        &mut self,
        at: Position,
        part: &ExpressionPart<'graph>,
    ) -> Result<Option<ExpressionPart<'graph>>, ShapeError> {
        match part {
            ExpressionPart::Expression(node) => Ok(self
                .rewrite_node(at, node.reference())?
                .map(ExpressionPart::Expression)),
            ExpressionPart::SigiledTypeExpr(node) => Ok(self
                .rewrite_node(at, node.reference())?
                .map(ExpressionPart::SigiledTypeExpr)),
            ExpressionPart::RecordType(node) => Ok(self
                .rewrite_fields(at, node.reference())?
                .map(ExpressionPart::RecordType)),
            ExpressionPart::ListLiteral(items) => {
                let mut run = BumpVec::with_capacity_in(items.len(), self.scratch);
                let mut changed = false;
                for item in items.iter() {
                    let rewritten = self.rewrite_part(at, item)?;
                    changed |= rewritten.is_some();
                    run.push(rewritten.unwrap_or(*item));
                }
                Ok(changed.then(|| {
                    ExpressionPart::ListLiteral(collect(self.brand.writer(), run.iter().copied()))
                }))
            }
            ExpressionPart::DictLiteral(pairs) => {
                let mut run = BumpVec::with_capacity_in(pairs.len(), self.scratch);
                let mut changed = false;
                for (key, value) in pairs.iter() {
                    let rewritten = (self.rewrite_part(at, key)?, self.rewrite_part(at, value)?);
                    changed |= rewritten.0.is_some() || rewritten.1.is_some();
                    run.push((rewritten.0.unwrap_or(*key), rewritten.1.unwrap_or(*value)));
                }
                Ok(changed.then(|| {
                    ExpressionPart::DictLiteral(collect(self.brand.writer(), run.iter().copied()))
                }))
            }
            ExpressionPart::RecordLiteral(pairs) => {
                let mut run = BumpVec::with_capacity_in(pairs.len(), self.scratch);
                let mut changed = false;
                for (name, value) in pairs.iter() {
                    let rewritten = self.rewrite_part(at, value)?;
                    changed |= rewritten.is_some();
                    run.push((*name, rewritten.unwrap_or(*value)));
                }
                Ok(changed.then(|| {
                    ExpressionPart::RecordLiteral(collect(self.brand.writer(), run.iter().copied()))
                }))
            }
            ExpressionPart::Type(_)
            | ExpressionPart::Identifier(_)
            | ExpressionPart::Keyword(_)
            | ExpressionPart::Literal(_)
            | ExpressionPart::QuotedExpression(_) => Ok(None),
        }
    }

    /// A signature part: a `:{…}` schema's field list or an `EXPR` head, whose type halves are
    /// rewritten and whose spine — its keywords and declared names — is left alone. Read through
    /// the same pair walk the mention pass reads it through, so a head's keyword run is never taken
    /// for an operator run.
    fn rewrite_signature(
        &mut self,
        at: Position,
        part: &ExpressionPart<'graph>,
    ) -> Result<Option<ExpressionPart<'graph>>, ShapeError> {
        let Some(run) = signature_run(part) else {
            return self.rewrite_part(at, part);
        };
        let Some(rewritten) = self.rewrite_fields(at, run)? else {
            return Ok(None);
        };
        Ok(Some(match part {
            ExpressionPart::RecordType(_) => ExpressionPart::RecordType(rewritten),
            _ => ExpressionPart::Expression(rewritten),
        }))
    }

    /// A field list's type halves, rewritten in place.
    fn rewrite_fields(
        &mut self,
        at: Position,
        run: &KExpression<'graph>,
    ) -> Result<Option<ProgramNode<'graph>>, ShapeError> {
        let mut parts: Run<'x, 'graph> = BumpVec::with_capacity_in(run.parts.len(), self.scratch);
        parts.extend_from_slice(run.parts);
        let mut changed = false;
        let mut index = 0;
        while index < run.parts.len() {
            let typed = pair_name(run, index).is_some();
            let position = if typed { index + 1 } else { index };
            if let Some(rewritten) = self.rewrite_part(at, &run.parts[position].value)? {
                changed = true;
                parts[position].value = rewritten;
            }
            index += if typed { 2 } else { 1 };
        }
        Ok(changed.then(|| self.brand.nested_node(&parts)))
    }

    /// The arm heads of a branches part. An arm's body is a block shape of its own and is rewritten
    /// by its own draft.
    fn rewrite_branches(
        &mut self,
        at: Position,
        part: &ExpressionPart<'graph>,
    ) -> Result<Option<ExpressionPart<'graph>>, ShapeError> {
        let ExpressionPart::Expression(branches) = part else {
            return Ok(None);
        };
        let branches = branches.reference();
        let mut parts: Run<'x, 'graph> =
            BumpVec::with_capacity_in(branches.parts.len(), self.scratch);
        parts.extend_from_slice(branches.parts);
        let mut changed = false;
        for (index, arm) in branches.parts.iter().enumerate() {
            if !index.is_multiple_of(3) {
                continue;
            }
            if let Some(rewritten) = self.rewrite_part(at, &arm.value)? {
                changed = true;
                parts[index].value = rewritten;
            }
        }
        Ok(changed.then(|| ExpressionPart::Expression(self.brand.nested_node(&parts))))
    }
}

impl<'graph, 'x> Builder<'graph, 'x, '_> {
    /// One operator run, already rewritten part by part, chained under the one chaining its symbols
    /// share.
    fn chain(
        &mut self,
        at: Position,
        parts: &[Spanned<ExpressionPart<'graph>>],
    ) -> Result<ProgramNode<'graph>, ShapeError> {
        let mut operators: BumpVec<'x, KeywordSymbol> = BumpVec::new_in(self.scratch);
        operators.extend(
            parts
                .iter()
                .skip(1)
                .step_by(2)
                .map(|part| match part.value {
                    ExpressionPart::Keyword(symbol) => symbol,
                    _ => unreachable!("an operator run's odd positions are keywords"),
                }),
        );
        let mut operands: Run<'x, 'graph> = BumpVec::new_in(self.scratch);
        operands.extend(parts.iter().step_by(2).copied());

        let mode = self.chaining(at, &operators)?;
        match mode {
            ReductionMode::FoldLeft | ReductionMode::FoldRight => {
                self.fold(at, &operands, &operators, mode)
            }
            ReductionMode::Unary => {
                let items = collect(
                    self.brand.writer(),
                    operands.iter().map(|operand| operand.value),
                );
                let run = [
                    Spanned::bare(ExpressionPart::Keyword(operators[0])),
                    Spanned::bare(ExpressionPart::ListLiteral(items)),
                ];
                self.built(at, operators[0], &run)
            }
            ReductionMode::Pairwise {
                combiner,
                direction,
            } => self.pairwise(at, &operands, &operators, combiner, direction),
        }
    }

    /// How an operator run of `operators` reduces where this frame is: every symbol but `==` and
    /// `!=` must agree, and an equality symbol beside them joins only a pairwise group.
    fn chaining(
        &self,
        at: Position,
        operators: &[KeywordSymbol],
    ) -> Result<ReductionMode, ShapeError> {
        let mut chosen: Option<(KeywordSymbol, Cover<'graph>)> = None;
        let mut equality = None;
        for symbol in operators {
            if is_equality(*symbol) {
                equality.get_or_insert(*symbol);
                continue;
            }
            let cover = self
                .frame
                .cover(*symbol)
                .map_err(|()| ShapeError::Unchained {
                    symbol: *symbol,
                    at,
                })?;
            match chosen {
                None => chosen = Some((*symbol, cover)),
                Some((first, held)) if !held.agrees(cover) => {
                    return Err(ShapeError::MixedGroups {
                        first,
                        second: *symbol,
                        at,
                    });
                }
                Some(_) => {}
            }
        }
        let Some((symbol, cover)) = chosen else {
            // Equality alone folds its pairs through `AND`, left.
            return Ok(equality_mode());
        };
        let mode = cover.mode();
        if let Some(second) = equality
            && !matches!(mode, ReductionMode::Pairwise { .. })
        {
            return Err(ShapeError::MixedGroups {
                first: symbol,
                second,
                at,
            });
        }
        Ok(mode)
    }

    /// `operands` folded through `operators` in `mode`'s direction, one nested binary node per
    /// operator.
    fn fold(
        &mut self,
        at: Position,
        operands: &[Spanned<ExpressionPart<'graph>>],
        operators: &[KeywordSymbol],
        mode: ReductionMode,
    ) -> Result<ProgramNode<'graph>, ShapeError> {
        let direction = match mode {
            ReductionMode::FoldRight => FoldDirection::Right,
            _ => FoldDirection::Left,
        };
        self.fold_run(at, operands, operators, direction)
    }

    /// The fold itself: `operands` has one more element than `operators`, and both a fold rewrite
    /// and a pairwise rewrite's combiner pass run through here.
    fn fold_run(
        &mut self,
        at: Position,
        operands: &[Spanned<ExpressionPart<'graph>>],
        operators: &[KeywordSymbol],
        direction: FoldDirection,
    ) -> Result<ProgramNode<'graph>, ShapeError> {
        debug_assert_eq!(operands.len(), operators.len() + 1);
        let mut node = None;
        match direction {
            FoldDirection::Left => {
                let mut left = operands[0];
                for (index, symbol) in operators.iter().enumerate() {
                    let built = self.infix(at, left, *symbol, operands[index + 1])?;
                    left = Spanned::bare(ExpressionPart::Expression(built));
                    node = Some(built);
                }
            }
            FoldDirection::Right => {
                let mut right = operands[operands.len() - 1];
                for (index, symbol) in operators.iter().enumerate().rev() {
                    let built = self.infix(at, operands[index], *symbol, right)?;
                    right = Spanned::bare(ExpressionPart::Expression(built));
                    node = Some(built);
                }
            }
        }
        Ok(node.expect("an operator run names at least one operator"))
    }

    /// The pairwise rewrite: each adjacent pair through its own operator, the pair results folded
    /// through `combiner`. An operand that would be evaluated twice is hoisted, in source order,
    /// into an anonymous slot of a synthesized block.
    fn pairwise(
        &mut self,
        at: Position,
        operands: &[Spanned<ExpressionPart<'graph>>],
        operators: &[KeywordSymbol],
        combiner: KeywordSymbol,
        direction: FoldDirection,
    ) -> Result<ProgramNode<'graph>, ShapeError> {
        let mut hoisted: Run<'x, 'graph> = BumpVec::new_in(self.scratch);
        let mut named: Run<'x, 'graph> = BumpVec::with_capacity_in(operands.len(), self.scratch);
        for (index, operand) in operands.iter().enumerate() {
            if evaluates_once(operand.value) {
                named.push(*operand);
                continue;
            }
            let name = anonymous_operand(index);
            let binding = [
                Spanned::bare(ExpressionPart::Keyword(KEYWORDS.let_.symbol())),
                Spanned::bare(ExpressionPart::Identifier(name)),
                Spanned::bare(ExpressionPart::Keyword(KEYWORDS.equals.symbol())),
                *operand,
            ];
            let binding = self.brand.nested_node(&binding);
            hoisted.push(Spanned::bare(ExpressionPart::Expression(binding)));
            named.push(Spanned::bare(ExpressionPart::Identifier(name)));
        }

        let mut pairs: Run<'x, 'graph> = BumpVec::with_capacity_in(operators.len(), self.scratch);
        let mut combiners: BumpVec<'x, KeywordSymbol> =
            BumpVec::with_capacity_in(operators.len(), self.scratch);
        for (index, symbol) in operators.iter().enumerate() {
            let pair = self.infix(at, named[index], *symbol, named[index + 1])?;
            pairs.push(Spanned::bare(ExpressionPart::Expression(pair)));
            if index > 0 {
                combiners.push(combiner);
            }
        }
        let folded = if let [only] = &pairs[..] {
            let ExpressionPart::Expression(node) = only.value else {
                unreachable!("a pair is an expression part");
            };
            node
        } else {
            self.fold_run(at, &pairs, &combiners, direction)?
        };
        if hoisted.is_empty() {
            return Ok(folded);
        }
        hoisted.push(Spanned::bare(ExpressionPart::Expression(folded)));
        let block = self.brand.nested_node(&hoisted);
        self.blocks
            .insert(block.reference().parts.as_ptr() as usize, ());
        Ok(block)
    }

    /// One infix node `left <symbol> right`. `a != b` is never built: it becomes `NOT (a == b)`, so
    /// `!=` reaches no bucket.
    fn infix(
        &mut self,
        at: Position,
        left: Spanned<ExpressionPart<'graph>>,
        symbol: KeywordSymbol,
        right: Spanned<ExpressionPart<'graph>>,
    ) -> Result<ProgramNode<'graph>, ShapeError> {
        if is_unequal(symbol) {
            let equal = [
                left,
                Spanned::bare(ExpressionPart::Keyword(equal_symbol())),
                right,
            ];
            let equal = self.built(at, symbol, &equal)?;
            let negated = [
                Spanned::bare(ExpressionPart::Keyword(not_symbol())),
                Spanned::bare(ExpressionPart::Expression(equal)),
            ];
            return self.built(at, symbol, &negated);
        }
        let run = [left, Spanned::bare(ExpressionPart::Keyword(symbol)), right];
        self.built(at, symbol, &run)
    }

    /// One node of the rewrite, refused when its run spells a builtin form — an operator symbol
    /// whose chained node a later reader would walk as a form rather than as a call.
    fn built(
        &self,
        at: Position,
        symbol: KeywordSymbol,
        run: &[Spanned<ExpressionPart<'graph>>],
    ) -> Result<ProgramNode<'graph>, ShapeError> {
        let node = self.brand.nested_node(run);
        if node.reference().cache().builtin_shape().is_some() {
            return Err(ShapeError::SpellsForm { symbol, at });
        }
        Ok(node)
    }
}

/// Whether an operand evaluates once wherever it is written, so a pairwise rewrite may name it
/// twice without hoisting it.
fn evaluates_once(part: ExpressionPart<'_>) -> bool {
    matches!(
        part,
        ExpressionPart::Identifier(_)
            | ExpressionPart::Type(_)
            | ExpressionPart::Literal(_)
            | ExpressionPart::QuotedExpression(_)
    )
}

/// The name the operand at `index` is hoisted under. The space makes it unspellable in source, so
/// a hoist can shadow nothing and be read by nothing but the pairs the rewrite built.
fn anonymous_operand(index: usize) -> ValueSymbol {
    let mut buffer = [0u8; 32];
    const HEAD: &[u8] = b"operand ";
    buffer[..HEAD.len()].copy_from_slice(HEAD);
    let mut digits = [0u8; 20];
    let mut rest = index;
    let mut len = 0;
    loop {
        digits[len] = b'0' + (rest % 10) as u8;
        rest /= 10;
        len += 1;
        if rest == 0 {
            break;
        }
    }
    for (offset, digit) in digits[..len].iter().rev().enumerate() {
        buffer[HEAD.len() + offset] = *digit;
    }
    let text = std::str::from_utf8(&buffer[..HEAD.len() + len]).expect("the run is ASCII");
    ValueSymbol::classify(text).expect("`operand <index>` is a value token")
}
