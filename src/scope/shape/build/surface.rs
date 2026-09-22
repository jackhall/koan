//! The **surfaced-name reader**: which names — and which operator groups — a
//! `USING <operand> SCOPE <body>` operand surfaces, read where the shape is built.
//!
//! The body is a block whose parameters are exactly those names, so a surfaced name resolves like
//! any other local and no coordinate names a member. That only works if the names are readable
//! statically, so the reader walks the operand's spine back to a declaration that states its
//! members — a `MODULE` or `GROUP` binder's body, the `SIG` an ascription at the site names, a
//! `LET` rooted at either, a value or type alias, and a `WITH` pin — and refuses anything else,
//! naming the ascription the site needs.
//!
//! An operand that surfaces a `GROUP` — a group binder, or a `SIG` whose body holds a bodyless
//! `GROUP` head — surfaces its group too, so the body may write an operator run of its members. A
//! group is content, so the record a binder surfaces is the one its claim already holds and the
//! record a signature surfaces is built here, from the same member scan.
//!
//! The reader records no mention and pushes no capture: it only reads names. The operand itself is
//! walked as an ordinary eager argument by the mention pass.
//!
//! See [README.md § Names that arrive at run time](../../README.md#names-that-arrive-at-run-time).

use crate::memory::{BumpAllocator, BumpVec};
use crate::parse::builtin_shapes::BuiltinShapeId;
use crate::parse::builtin_shapes::role::{BodyKind, Role};
use crate::parse::{ExpressionPart, KExpression};
use crate::symbols::BinderSymbol;
use crate::type_lattice::DeclaredGroup;

use super::super::super::groups::{
    BuiltinGroup, Claim, builtin_equal, declared_group, groups_equal,
};
use super::super::{Position, ShapeError, ShapeKind, Site};
use super::{Builder, body_of};

/// What one operand surfaces: its names, in the order the spine gives them, and the operator groups
/// the body may chain under. The binders pass sorts the names into layout order, so the reader owes
/// no ordering of its own.
pub(super) struct Surfaced<'x, 'graph> {
    pub names: BumpVec<'x, BinderSymbol>,
    pub groups: BumpVec<'x, &'graph DeclaredGroup<'graph>>,
}

impl<'x, 'graph> Surfaced<'x, 'graph> {
    pub(super) fn new(scratch: BumpAllocator<'x>) -> Self {
        Surfaced {
            names: BumpVec::new_in(scratch),
            groups: BumpVec::new_in(scratch),
        }
    }
}

impl<'graph, 'x> Builder<'graph, 'x, '_> {
    /// The names `node`'s operand surfaces — `node` being a whole `USING … SCOPE` form at
    /// `statement` of the draft at `level`.
    pub(super) fn surfaced(
        &self,
        level: usize,
        statement: u32,
        node: &KExpression<'graph>,
        out: &mut Surfaced<'x, 'graph>,
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
        out: &mut Surfaced<'x, 'graph>,
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
                    ) => {
                        self.surfaced_group(statement, out)?;
                        body_binders(statement, &mut out.names)
                    }
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
        out: &mut Surfaced<'x, 'graph>,
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
                        self.sig_members(definition, out)
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

    /// The groups a `USING` body holds, out of the ones its operand surfaced: the ones it does not
    /// already have.
    ///
    /// A group equal to a builtin one, and a group an enclosing frame already holds, say nothing
    /// new and are dropped. Anything else that would give a member a second chaining — a builtin
    /// group covering it, a `UNARY OP` marking it, a visible group that is not this one — is
    /// refused, since a symbol chains one way for the whole build.
    pub(super) fn surfaced_groups(
        &self,
        at: Position,
        surfaced: &Surfaced<'x, 'graph>,
    ) -> Result<BumpVec<'x, &'graph DeclaredGroup<'graph>>, ShapeError> {
        let mut kept = BumpVec::new_in(self.scratch);
        for group in surfaced.groups.iter() {
            if builtin_equal(group) {
                continue;
            }
            let refused = |symbol| ShapeError::RedeclaresGroup { symbol, at };
            let mut already = false;
            for member in group.members {
                if BuiltinGroup::of(*member).is_some()
                    || matches!(self.claims.get(*member), Some(Claim::Unary))
                {
                    return Err(refused(*member));
                }
                match self.frame.visible(*member) {
                    Some(visible) if groups_equal(visible, group) => already = true,
                    Some(_) => return Err(refused(*member)),
                    None => {}
                }
            }
            if !already {
                kept.push(*group);
            }
        }
        Ok(kept)
    }

    /// The group a `GROUP` binder surfaces: the one record its claim already holds, so a group
    /// surfaced twice is surfaced once. A `GROUP` written out equal to a builtin group claims
    /// nothing and surfaces nothing — the language already says what it says.
    fn surfaced_group(
        &self,
        statement: &KExpression<'graph>,
        out: &mut Surfaced<'x, 'graph>,
    ) -> Result<(), ()> {
        let Some(declared) = declared_group(statement, self.scratch)? else {
            return Ok(());
        };
        let first = *declared.members.first().ok_or(())?;
        if let Some(Claim::Group(record)) = self.claims.get(first) {
            out.groups.push(record);
        }
        Ok(())
    }

    /// The members a `SIG` body declares, and the groups its bodyless `GROUP` heads do. A bodyless
    /// `EXPR` or `OP` head declares no member until dispatch gives it a slot; anything else in a
    /// signature body is read by nobody here.
    ///
    /// A signature's group lives in its operator channel, so no claim holds a record for it and one
    /// is built here, in program storage, from the same member scan a `GROUP` statement takes.
    fn sig_members(
        &self,
        definition: &ExpressionPart<'graph>,
        out: &mut Surfaced<'x, 'graph>,
    ) -> Result<(), ()> {
        let body = body_of(definition).ok_or(())?;
        for (line, _) in body.body_statements() {
            match line.cache().builtin_shape().map(|shape| shape.id) {
                Some(BuiltinShapeId::TypeDeclaration | BuiltinShapeId::LetValue) => {
                    out.names.push(
                        line.statement_binder_plan()
                            .and_then(|plan| plan.name)
                            .ok_or(())?,
                    );
                }
                Some(BuiltinShapeId::Val) => {
                    let ExpressionPart::Identifier(name) = line.parts.get(1).ok_or(())?.value
                    else {
                        return Err(());
                    };
                    out.names.push(BinderSymbol::Value(name));
                }
                Some(
                    BuiltinShapeId::GroupHeadFoldLeft
                    | BuiltinShapeId::GroupHeadFoldRight
                    | BuiltinShapeId::GroupHeadPairwiseFoldLeft
                    | BuiltinShapeId::GroupHeadPairwiseFoldRight,
                ) => {
                    let storage = self.brand.allocator();
                    let group = declared_group(line, storage)?.ok_or(())?;
                    out.groups.push(storage.alloc(group));
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
fn body_binders<'graph>(
    statement: &KExpression<'graph>,
    out: &mut BumpVec<'_, BinderSymbol>,
) -> Result<(), ()> {
    let body = body_of(role_part(statement, Role::Body(BodyKind::Module)).ok_or(())?).ok_or(())?;
    for (line, _) in body.body_statements() {
        if let Some(name) = line.statement_binder_plan().and_then(|plan| plan.name) {
            out.push(name);
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
