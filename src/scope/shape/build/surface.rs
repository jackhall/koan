//! The **surfaced-name reader**: which names a `USING <operand> SCOPE <body>` operand surfaces,
//! read where the shape is built.
//!
//! The body is a block whose parameters are exactly those names, so a surfaced name resolves like
//! any other local and no coordinate names a member. That only works if the names are readable
//! statically, so the reader walks the operand's spine back to a declaration that states its
//! members — a `MODULE` or `GROUP` binder's body, the `SIG` an ascription at the site names, a
//! `LET` rooted at either, a value or type alias, and a `WITH` pin — and refuses anything else,
//! naming the ascription the site needs.
//!
//! The reader records no mention and pushes no capture: it only reads names. The operand itself is
//! walked as an ordinary eager argument by the mention pass.
//!
//! See [README.md § Names that arrive at run time](../../README.md#names-that-arrive-at-run-time).

use crate::memory::BumpVec;
use crate::parse::builtin_shapes::BuiltinShapeId;
use crate::parse::builtin_shapes::role::{BodyKind, Role};
use crate::parse::{BinderSymbol, ExpressionPart, KExpression};

use super::super::{Position, ShapeError, ShapeKind, Site};
use super::{Builder, body_of};

/// The names read out of one operand, in the order the spine gives them. The binders pass sorts
/// them into layout order, so the reader owes no ordering of its own.
pub(super) type Names<'x> = BumpVec<'x, BinderSymbol>;

impl<'graph, 'x> Builder<'graph, 'x, '_> {
    /// The names `node`'s operand surfaces — `node` being a whole `USING … SCOPE` form at
    /// `statement` of the draft at `level`.
    pub(super) fn surfaced(
        &self,
        level: usize,
        statement: u32,
        node: &KExpression<'graph>,
        out: &mut Names<'x>,
    ) -> Result<(), ShapeError> {
        let operand = &node
            .parts
            .get(1)
            .ok_or(ShapeError::Malformed {
                form: BuiltinShapeId::UsingScope,
                at: Position::statement(statement as usize),
            })?
            .value;
        // One rule applied per unit of fuel, so an alias that names itself terminates.
        let mut fuel = self
            .chain
            .iter()
            .map(|draft| draft.nodes.len())
            .sum::<usize>()
            + 1;
        let at = Position::statement(statement as usize);
        self.value_names(level, at, operand, out, &mut fuel)
            .map_err(|()| ShapeError::Unsurfaced {
                at,
                site: Site::of(operand),
            })
    }

    /// The names the value `part` denotes, read in the draft at `level` at position `at`.
    fn value_names(
        &self,
        level: usize,
        at: Position,
        part: &ExpressionPart<'graph>,
        out: &mut Names<'x>,
        fuel: &mut usize,
    ) -> Result<(), ()> {
        spend(fuel)?;
        match part {
            ExpressionPart::Expression(node) => {
                let node = node.reference();
                match node.cache().builtin_shape().map(|shape| shape.id) {
                    Some(BuiltinShapeId::AscribeOpaque | BuiltinShapeId::AscribeTransparent) => {
                        let signature = &node.parts.get(2).ok_or(())?.value;
                        self.type_names(level, at, signature, out, fuel)
                    }
                    Some(_) => Err(()),
                    // A parenthesized operand wrapping one part is that part.
                    None => match node.parts {
                        [only] => self.value_names(level, at, &only.value, out, fuel),
                        _ => Err(()),
                    },
                }
            }
            ExpressionPart::Identifier(name) => {
                let name = BinderSymbol::Value(*name);
                let (level, at, statement) = self.declaring(level, name, at).ok_or(())?;
                match statement.cache().builtin_shape().map(|shape| shape.id) {
                    Some(
                        BuiltinShapeId::Module
                        | BuiltinShapeId::GroupFoldLeft
                        | BuiltinShapeId::GroupFoldRight
                        | BuiltinShapeId::GroupPairwiseFoldLeft
                        | BuiltinShapeId::GroupPairwiseFoldRight,
                    ) => body_binders(statement, out),
                    Some(BuiltinShapeId::LetValue) => {
                        let rhs = role_part(statement, Role::Rhs).ok_or(())?;
                        self.value_names(level, at, rhs, out, fuel)
                    }
                    _ => Err(()),
                }
            }
            _ => Err(()),
        }
    }

    /// The members the type `part` declares, read in the draft at `level` at position `at`.
    fn type_names(
        &self,
        level: usize,
        at: Position,
        part: &ExpressionPart<'graph>,
        out: &mut Names<'x>,
        fuel: &mut usize,
    ) -> Result<(), ()> {
        spend(fuel)?;
        match part {
            ExpressionPart::Expression(node) | ExpressionPart::SigiledTypeExpr(node) => {
                let node = node.reference();
                // `<Sig> WITH {…}`: a pin changes no name.
                if let [pinned, separator, _] = node.parts
                    && matches!(separator.value, ExpressionPart::Keyword(_))
                {
                    return self.type_names(level, at, &pinned.value, out, fuel);
                }
                match node.parts {
                    [only] => self.type_names(level, at, &only.value, out, fuel),
                    _ => Err(()),
                }
            }
            ExpressionPart::Type(name) if self.skips(name) => Err(()),
            ExpressionPart::Type(name) => {
                let name = BinderSymbol::Type(*name);
                let (level, at, statement) = self.declaring(level, name, at).ok_or(())?;
                match statement.cache().builtin_shape().map(|shape| shape.id) {
                    Some(BuiltinShapeId::Sig) => {
                        let definition = role_part(
                            statement,
                            Role::Definition(
                                crate::parse::builtin_shapes::role::DefinitionKind::Plain,
                            ),
                        )
                        .ok_or(())?;
                        sig_members(definition, out)
                    }
                    Some(BuiltinShapeId::LetValue) => {
                        let rhs = role_part(statement, Role::Rhs).ok_or(())?;
                        self.type_names(level, at, rhs, out, fuel)
                    }
                    _ => Err(()),
                }
            }
            _ => Err(()),
        }
    }

    /// The draft level, the position a read there takes, and the statement declaring `name` —
    /// [`Builder::resolve`]'s walk with nothing recorded. `None` for a builtin, a name reached only
    /// through an `EVAL`'s enclosing activation, a parameter (which declares no statement), and a
    /// name with no binding at all.
    fn declaring(
        &self,
        level: usize,
        name: BinderSymbol,
        at: Position,
    ) -> Option<(usize, Position, &KExpression<'graph>)> {
        let (mut level, mut at) = (level, at);
        loop {
            let draft = &self.chain[level];
            let names = draft.channels();
            if let Some(index) = names.find(name)
                && at.sees(names.get(index))
            {
                let declared = names.get(index);
                let statement = declared.0.checked_sub(1)? as usize;
                return Some((level, declared, &draft.nodes[statement]));
            }
            if draft.kind == ShapeKind::Program {
                return None;
            }
            level = level.checked_sub(1)?;
            at = self.chain[level].boundary();
        }
    }
}

/// The names the body of a `MODULE` or `GROUP` binder binds, read through the very call the binders
/// pass makes, so the two cannot drift.
fn body_binders<'graph>(statement: &KExpression<'graph>, out: &mut Names<'_>) -> Result<(), ()> {
    let body = body_of(role_part(statement, Role::Body(BodyKind::Module)).ok_or(())?).ok_or(())?;
    for (line, _) in body.body_statements() {
        if let Some(name) = line.statement_binder_plan().and_then(|plan| plan.name) {
            out.push(name);
        }
    }
    Ok(())
}

/// The members a `SIG` body declares. A bodyless `EXPR` or `OP` head declares no member until
/// dispatch gives it a slot; anything else in a signature body is read by nobody here.
fn sig_members<'graph>(definition: &ExpressionPart<'graph>, out: &mut Names<'_>) -> Result<(), ()> {
    let body = body_of(definition).ok_or(())?;
    for (line, _) in body.body_statements() {
        match line.cache().builtin_shape().map(|shape| shape.id) {
            Some(BuiltinShapeId::TypeDeclaration | BuiltinShapeId::LetValue) => out.push(
                line.statement_binder_plan()
                    .and_then(|plan| plan.name)
                    .ok_or(())?,
            ),
            Some(BuiltinShapeId::Val) => {
                let ExpressionPart::Identifier(name) = line.parts.get(1).ok_or(())?.value else {
                    return Err(());
                };
                out.push(BinderSymbol::Value(name));
            }
            Some(
                BuiltinShapeId::ExpressionHead
                | BuiltinShapeId::QuantifiedExpressionHead
                | BuiltinShapeId::OperatorHead
                | BuiltinShapeId::OperatorHeadReturning
                | BuiltinShapeId::UnaryOperatorHead
                | BuiltinShapeId::UnaryOperatorHeadReturning,
            ) => {}
            _ => return Err(()),
        }
    }
    Ok(())
}

/// The one part of `node` its form gives `role`.
fn role_part<'n, 'graph>(
    node: &'n KExpression<'graph>,
    role: Role,
) -> Option<&'n ExpressionPart<'graph>> {
    let form = node.cache().builtin_shape()?;
    form.roles()
        .zip(node.parts)
        .find(|(held, _)| *held == role)
        .map(|(_, part)| &part.value)
}

/// One rule's worth of fuel.
fn spend(fuel: &mut usize) -> Result<(), ()> {
    *fuel = fuel.checked_sub(1).ok_or(())?;
    Ok(())
}
