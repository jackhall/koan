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
//! the statement the parser produced. Every node and part the rewrite builds carries a span of the
//! source it was built from: a chained node spans its operands, a keyword the operator it came from,
//! a hoist its operand, and a synthesized block and its last statement the whole run.
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
use crate::parse::builtin_shapes::binder::bounded_run;
use crate::parse::builtin_shapes::role::{DefinitionKind, Heads, Role};
use crate::parse::builtin_shapes::{BuiltinShapeId, KEYWORDS};
use crate::parse::{DispatchShape, ExpressionPart, KExpression, ProgramNode, Spanned};
use crate::source::{FileId, SourceRef, Span};
use crate::symbols::{KeywordSymbol, ValueSymbol};
use crate::type_lattice::{FoldDirection, ReductionMode};

use super::super::super::groups::{
    Cover, equal_symbol, equality_mode, is_equality, is_unequal, not_symbol,
};
use super::super::super::signature::{pair_name, signature_run};
use super::super::ShapeError;
use super::Builder;

/// A part run under construction, in scratch.
type Run<'x, 'graph> = BumpVec<'x, Spanned<ExpressionPart<'graph>>>;

/// An operator of a run, beside the extent it is written at.
#[derive(Clone, Copy)]
struct Operator {
    symbol: KeywordSymbol,
    span: Span,
}

/// The extent from `first`'s start to `last`'s end, or `fallback` when either is unspanned.
fn cover(first: Option<Span>, last: Option<Span>, fallback: Span) -> Span {
    match (first, last) {
        (Some(first), Some(last)) => Span {
            start: first.start,
            end: last.end,
        },
        _ => fallback,
    }
}

/// A part holding the built `node`, spanned as the node is.
fn holding<'graph>(node: ProgramNode<'graph>) -> Spanned<ExpressionPart<'graph>> {
    Spanned::at(
        ExpressionPart::Expression(node),
        node.reference().source.span,
    )
}

impl<'graph, 'x> Builder<'graph, 'x, '_> {
    /// One statement with every operator run in it chained, or `None` when it holds none.
    ///
    /// A statement that is itself a hoisting pairwise operator run becomes the one-part wrapper
    /// around its block, so the block is held by an `Expression` part wherever it lands.
    pub(super) fn rewrite_statement(
        &mut self,
        node: &KExpression<'graph>,
    ) -> Result<Option<KExpression<'graph>>, ShapeError<'graph>> {
        let Some(rewritten) = self.rewrite_node(node)? else {
            return Ok(None);
        };
        if self.is_block(rewritten) {
            let mut run: Run<'x, 'graph> = BumpVec::new_in(self.scratch);
            run.push(Spanned::at(
                ExpressionPart::Expression(rewritten),
                node.source.span,
            ));
            return Ok(Some(*self.brand.nested_node(&run, node.source).reference()));
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
        node: &KExpression<'graph>,
    ) -> Result<Option<ProgramNode<'graph>>, ShapeError<'graph>> {
        let mut run: Run<'x, 'graph> = BumpVec::with_capacity_in(node.parts.len(), self.scratch);
        let mut changed = false;
        match node.cache().builtin_shape() {
            Some(form) => {
                for (role, part) in form.roles().zip(node.parts) {
                    let rewritten = match role {
                        Role::Rhs
                        | Role::Argument
                        | Role::InPlace
                        | Role::TypeExpression
                        | Role::Definition(DefinitionKind::Plain) => {
                            self.rewrite_part(&part.value)?
                        }
                        // A bare label is the label itself; any other is evaluated.
                        Role::Field => match part.value {
                            ExpressionPart::Identifier(_) | ExpressionPart::Type(_) => None,
                            _ => self.rewrite_part(&part.value)?,
                        },
                        Role::Signature | Role::Head => self.rewrite_signature(&part.value)?,
                        Role::Branches(Heads::Types) => {
                            self.rewrite_quotes(&part.value, Quotes::Keys)?
                        }
                        Role::Definition(DefinitionKind::Union) | Role::Quantifiers => {
                            self.rewrite_quotes(&part.value, Quotes::Values)?
                        }
                        Role::Definition(DefinitionKind::Members) => {
                            self.rewrite_quotes(&part.value, Quotes::Items)?
                        }
                        Role::Name if form.id == BuiltinShapeId::TypeDeclaration => {
                            self.rewrite_bounds(&part.value)?
                        }
                        Role::Keyword
                        | Role::Name
                        | Role::Data
                        | Role::Branches(Heads::Labels)
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
                    let rewritten = self.rewrite_part(&part.value)?;
                    changed |= rewritten.is_some();
                    run.push(Spanned {
                        value: rewritten.unwrap_or(part.value),
                        span: part.span,
                    });
                }
                if node.shape() == DispatchShape::OperatorChain {
                    return self.chain(&run, node.source).map(Some);
                }
                // `a != b` written alone is the same rewrite one pair of a pairwise run takes.
                if let [left, separator, right] = &run[..]
                    && let ExpressionPart::Keyword(symbol) = separator.value
                    && is_unequal(symbol)
                {
                    let op = Operator {
                        symbol,
                        span: separator.span.unwrap_or(node.source.span),
                    };
                    return self.infix(node.source.file, *left, op, *right).map(Some);
                }
            }
        }
        Ok(changed.then(|| self.brand.nested_node(&run, node.source)))
    }

    /// One part with every operator run under it chained. A quote is data and is never rewritten.
    fn rewrite_part(
        &mut self,
        part: &ExpressionPart<'graph>,
    ) -> Result<Option<ExpressionPart<'graph>>, ShapeError<'graph>> {
        match part {
            ExpressionPart::Expression(node) => Ok(self
                .rewrite_node(node.reference())?
                .map(ExpressionPart::Expression)),
            ExpressionPart::SigiledTypeExpr(node) => Ok(self
                .rewrite_node(node.reference())?
                .map(ExpressionPart::SigiledTypeExpr)),
            ExpressionPart::RecordType(node) => Ok(self
                .rewrite_fields(node.reference())?
                .map(ExpressionPart::RecordType)),
            ExpressionPart::ListLiteral(items) => {
                let mut run = BumpVec::with_capacity_in(items.len(), self.scratch);
                let mut changed = false;
                for item in items.iter() {
                    let rewritten = self.rewrite_part(item)?;
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
                    let rewritten = (self.rewrite_part(key)?, self.rewrite_part(value)?);
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
                    let rewritten = self.rewrite_part(value)?;
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

    /// A signature part: a `:{…}` schema's field list or a quoted `EXPR` head, whose type halves
    /// are rewritten and whose spine — its keywords and declared names — is left alone. Read through
    /// the same pair walk the mention pass reads it through, so a head's keyword run is never taken
    /// for an operator run.
    fn rewrite_signature(
        &mut self,
        part: &ExpressionPart<'graph>,
    ) -> Result<Option<ExpressionPart<'graph>>, ShapeError<'graph>> {
        let Some(run) = signature_run(part) else {
            return self.rewrite_part(part);
        };
        let Some(rewritten) = self.rewrite_fields(run)? else {
            return Ok(None);
        };
        Ok(Some(match part {
            ExpressionPart::RecordType(_) => ExpressionPart::RecordType(rewritten),
            _ => ExpressionPart::QuotedExpression(rewritten),
        }))
    }

    /// A field list's type halves, rewritten in place.
    fn rewrite_fields(
        &mut self,
        run: &KExpression<'graph>,
    ) -> Result<Option<ProgramNode<'graph>>, ShapeError<'graph>> {
        let mut parts: Run<'x, 'graph> = BumpVec::with_capacity_in(run.parts.len(), self.scratch);
        parts.extend_from_slice(run.parts);
        let mut changed = false;
        let mut index = 0;
        while index < run.parts.len() {
            let typed = pair_name(run, index).is_some();
            let position = if typed { index + 1 } else { index };
            if let Some(rewritten) = self.rewrite_part(&run.parts[position].value)? {
                changed = true;
                parts[position].value = rewritten;
            }
            index += if typed { 2 } else { 1 };
        }
        Ok(changed.then(|| self.brand.nested_node(&parts, run.source)))
    }

    /// A `TYPE` declarator with its bound's operator runs chained: the third part of a
    /// `<Name> UNDER <bound>` run.
    fn rewrite_bounds(
        &mut self,
        part: &ExpressionPart<'graph>,
    ) -> Result<Option<ExpressionPart<'graph>>, ShapeError<'graph>> {
        let ExpressionPart::Expression(node) = part else {
            return Ok(None);
        };
        let run = node.reference();
        if bounded_run(run).is_none() {
            return Ok(None);
        }
        let Some(rewritten) = self.rewrite_part(&run.parts[2].value)? else {
            return Ok(None);
        };
        let mut parts: Run<'x, 'graph> = BumpVec::with_capacity_in(run.parts.len(), self.scratch);
        parts.extend_from_slice(run.parts);
        parts[2].value = rewritten;
        Ok(Some(ExpressionPart::Expression(
            self.brand.nested_node(&parts, run.source),
        )))
    }

    /// The quotes of a container the shape builder reads where it is written — a list's items, or
    /// a dict's keys or values — each with its operator runs chained. Every other quote is data. An
    /// arm's block is a shape of its own and is rewritten by its own draft.
    fn rewrite_quotes(
        &mut self,
        part: &ExpressionPart<'graph>,
        quotes: Quotes,
    ) -> Result<Option<ExpressionPart<'graph>>, ShapeError<'graph>> {
        let writer = self.brand.writer();
        match (quotes, part) {
            (Quotes::Items, ExpressionPart::ListLiteral(items)) => {
                let mut run = BumpVec::with_capacity_in(items.len(), self.scratch);
                let mut changed = false;
                for item in items.iter() {
                    let rewritten = self.rewrite_quote(item)?;
                    changed |= rewritten.is_some();
                    run.push(rewritten.unwrap_or(*item));
                }
                Ok(changed
                    .then(|| ExpressionPart::ListLiteral(collect(writer, run.iter().copied()))))
            }
            (Quotes::Keys | Quotes::Values, ExpressionPart::DictLiteral(pairs)) => {
                let mut run = BumpVec::with_capacity_in(pairs.len(), self.scratch);
                let mut changed = false;
                for (key, value) in pairs.iter() {
                    let (new_key, new_value) = match quotes {
                        Quotes::Keys => (self.rewrite_quote(key)?, None),
                        _ => (None, self.rewrite_quote(value)?),
                    };
                    changed |= new_key.is_some() || new_value.is_some();
                    run.push((new_key.unwrap_or(*key), new_value.unwrap_or(*value)));
                }
                Ok(changed
                    .then(|| ExpressionPart::DictLiteral(collect(writer, run.iter().copied()))))
            }
            _ => Ok(None),
        }
    }

    /// One quote the builder reads, its body's operator runs chained.
    fn rewrite_quote(
        &mut self,
        part: &ExpressionPart<'graph>,
    ) -> Result<Option<ExpressionPart<'graph>>, ShapeError<'graph>> {
        let ExpressionPart::QuotedExpression(node) = part else {
            return Ok(None);
        };
        Ok(self
            .rewrite_node(node.reference())?
            .map(ExpressionPart::QuotedExpression))
    }
}

/// Which quotes of a container [`Builder::rewrite_quotes`] enters.
#[derive(Clone, Copy)]
enum Quotes {
    /// A list's items: a signature's members.
    Items,
    /// A dict's keys: an arm set's type guards.
    Keys,
    /// A dict's values: a union's payloads, a `FOR ALL` group's bounds.
    Values,
}

impl<'graph, 'x> Builder<'graph, 'x, '_> {
    /// One operator run, already rewritten part by part, chained under the one chaining its symbols
    /// share. `source` is the run's own node's.
    fn chain(
        &mut self,
        parts: &[Spanned<ExpressionPart<'graph>>],
        source: SourceRef,
    ) -> Result<ProgramNode<'graph>, ShapeError<'graph>> {
        let mut operators: BumpVec<'x, Operator> = BumpVec::new_in(self.scratch);
        operators.extend(
            parts
                .iter()
                .skip(1)
                .step_by(2)
                .map(|part| match part.value {
                    ExpressionPart::Keyword(symbol) => Operator {
                        symbol,
                        span: part.span.unwrap_or(source.span),
                    },
                    _ => unreachable!("an operator run's odd positions are keywords"),
                }),
        );
        let mut operands: Run<'x, 'graph> = BumpVec::new_in(self.scratch);
        operands.extend(parts.iter().step_by(2).copied());

        let file = source.file;
        let mode = self.chaining(file, &operators)?;
        match mode {
            ReductionMode::FoldLeft | ReductionMode::FoldRight => {
                self.fold(file, &operands, &operators, mode)
            }
            ReductionMode::Unary => {
                let items = collect(
                    self.brand.writer(),
                    operands.iter().map(|operand| operand.value),
                );
                let last = operands.last().expect("an operator run has operands");
                let run = [
                    Spanned::at(
                        ExpressionPart::Keyword(operators[0].symbol),
                        operators[0].span,
                    ),
                    Spanned::at(
                        ExpressionPart::ListLiteral(items),
                        cover(operands[0].span, last.span, source.span),
                    ),
                ];
                self.built(file, operators[0], &run, source.span)
            }
            ReductionMode::Pairwise {
                combiner,
                direction,
            } => self.pairwise(source, &operands, &operators, combiner, direction),
        }
    }

    /// How an operator run of `operators`, written in `file`, reduces where this frame is: every
    /// symbol but `==` and `!=` must agree, and an equality symbol beside them joins only a pairwise
    /// group. A refusal points at the operator that breaks the run.
    fn chaining(
        &self,
        file: FileId,
        operators: &[Operator],
    ) -> Result<ReductionMode, ShapeError<'graph>> {
        let at = |op: &Operator| SourceRef {
            span: op.span,
            file,
        };
        let mut chosen: Option<(KeywordSymbol, Cover<'graph>)> = None;
        let mut equality: Option<Operator> = None;
        for op in operators {
            let symbol = op.symbol;
            if is_equality(symbol) {
                equality.get_or_insert(*op);
                continue;
            }
            let cover = self
                .frame
                .cover(symbol)
                .map_err(|()| ShapeError::Unchained { symbol, at: at(op) })?;
            match chosen {
                None => chosen = Some((symbol, cover)),
                Some((first, held)) if !held.agrees(cover) => {
                    return Err(ShapeError::MixedGroups {
                        first,
                        second: symbol,
                        at: at(op),
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
                second: second.symbol,
                at: at(&second),
            });
        }
        Ok(mode)
    }

    /// `operands` folded through `operators` in `mode`'s direction, one nested binary node per
    /// operator.
    fn fold(
        &mut self,
        file: FileId,
        operands: &[Spanned<ExpressionPart<'graph>>],
        operators: &[Operator],
        mode: ReductionMode,
    ) -> Result<ProgramNode<'graph>, ShapeError<'graph>> {
        let direction = match mode {
            ReductionMode::FoldRight => FoldDirection::Right,
            _ => FoldDirection::Left,
        };
        self.fold_run(file, operands, operators, direction)
    }

    /// The fold itself: `operands` has one more element than `operators`, and both a fold rewrite
    /// and a pairwise rewrite's combiner pass run through here.
    fn fold_run(
        &mut self,
        file: FileId,
        operands: &[Spanned<ExpressionPart<'graph>>],
        operators: &[Operator],
        direction: FoldDirection,
    ) -> Result<ProgramNode<'graph>, ShapeError<'graph>> {
        debug_assert_eq!(operands.len(), operators.len() + 1);
        let mut node = None;
        match direction {
            FoldDirection::Left => {
                let mut left = operands[0];
                for (index, op) in operators.iter().enumerate() {
                    let built = self.infix(file, left, *op, operands[index + 1])?;
                    left = holding(built);
                    node = Some(built);
                }
            }
            FoldDirection::Right => {
                let mut right = operands[operands.len() - 1];
                for (index, op) in operators.iter().enumerate().rev() {
                    let built = self.infix(file, operands[index], *op, right)?;
                    right = holding(built);
                    node = Some(built);
                }
            }
        }
        Ok(node.expect("an operator run names at least one operator"))
    }

    /// The pairwise rewrite: each adjacent pair through its own operator, the pair results folded
    /// through `combiner`. An operand that would be evaluated twice is hoisted, in source order,
    /// into an anonymous slot of a synthesized block, which is sourced at the whole run, `source`.
    fn pairwise(
        &mut self,
        source: SourceRef,
        operands: &[Spanned<ExpressionPart<'graph>>],
        operators: &[Operator],
        combiner: KeywordSymbol,
        direction: FoldDirection,
    ) -> Result<ProgramNode<'graph>, ShapeError<'graph>> {
        let file = source.file;
        let mut hoisted: Run<'x, 'graph> = BumpVec::new_in(self.scratch);
        let mut named: Run<'x, 'graph> = BumpVec::with_capacity_in(operands.len(), self.scratch);
        for (index, operand) in operands.iter().enumerate() {
            if evaluates_once(operand.value) {
                named.push(*operand);
                continue;
            }
            // A hoist and every name it binds are written where the operand is.
            let written = operand.span.unwrap_or(source.span);
            let name = anonymous_operand(index);
            let binding = [
                Spanned::at(ExpressionPart::Keyword(KEYWORDS.let_.symbol()), written),
                Spanned::at(ExpressionPart::Identifier(name), written),
                Spanned::at(ExpressionPart::Keyword(KEYWORDS.equals.symbol()), written),
                *operand,
            ];
            let binding = self.brand.nested_node(
                &binding,
                SourceRef {
                    span: written,
                    file,
                },
            );
            hoisted.push(holding(binding));
            named.push(Spanned::at(ExpressionPart::Identifier(name), written));
        }

        let mut pairs: Run<'x, 'graph> = BumpVec::with_capacity_in(operators.len(), self.scratch);
        let mut combiners: BumpVec<'x, Operator> =
            BumpVec::with_capacity_in(operators.len(), self.scratch);
        for (index, op) in operators.iter().enumerate() {
            let pair = self.infix(file, named[index], *op, named[index + 1])?;
            pairs.push(holding(pair));
            if index > 0 {
                combiners.push(Operator {
                    symbol: combiner,
                    span: op.span,
                });
            }
        }
        let folded = if let [only] = &pairs[..] {
            let ExpressionPart::Expression(node) = only.value else {
                unreachable!("a pair is an expression part");
            };
            node
        } else {
            self.fold_run(file, &pairs, &combiners, direction)?
        };
        if hoisted.is_empty() {
            return Ok(folded);
        }
        hoisted.push(Spanned::at(ExpressionPart::Expression(folded), source.span));
        let block = self.brand.nested_node(&hoisted, source);
        self.blocks
            .insert(block.reference().parts.as_ptr() as usize, ());
        Ok(block)
    }

    /// One infix node `left <symbol> right`. `a != b` is never built: it becomes `NOT (a == b)`, so
    /// `!=` reaches no bucket.
    /// The node spans its operands, and each keyword the operator it came from.
    fn infix(
        &mut self,
        file: FileId,
        left: Spanned<ExpressionPart<'graph>>,
        op: Operator,
        right: Spanned<ExpressionPart<'graph>>,
    ) -> Result<ProgramNode<'graph>, ShapeError<'graph>> {
        let span = cover(left.span, right.span, op.span);
        if is_unequal(op.symbol) {
            let equal = [
                left,
                Spanned::at(ExpressionPart::Keyword(equal_symbol()), op.span),
                right,
            ];
            let equal = self.built(file, op, &equal, span)?;
            let negated = [
                Spanned::at(ExpressionPart::Keyword(not_symbol()), op.span),
                Spanned::at(ExpressionPart::Expression(equal), span),
            ];
            return self.built(file, op, &negated, span);
        }
        let run = [
            left,
            Spanned::at(ExpressionPart::Keyword(op.symbol), op.span),
            right,
        ];
        self.built(file, op, &run, span)
    }

    /// One node of the rewrite, refused when its run spells a builtin form — an operator symbol
    /// whose chained node a later reader would walk as a form rather than as a call.
    fn built(
        &self,
        file: FileId,
        op: Operator,
        run: &[Spanned<ExpressionPart<'graph>>],
        span: Span,
    ) -> Result<ProgramNode<'graph>, ShapeError<'graph>> {
        let node = self.brand.nested_node(run, SourceRef { span, file });
        if node.reference().cache().builtin_shape().is_some() {
            return Err(ShapeError::SpellsForm {
                symbol: op.symbol,
                at: SourceRef {
                    span: op.span,
                    file,
                },
            });
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
