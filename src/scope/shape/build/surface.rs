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
//! A signature's bodyless keyworded heads are surfaced too, as records of where each head, its
//! `SIG` and the ascription naming it are written: the block holds each as a registration of its
//! own, which the load types through the ascription's pins.
//!
//! The reader records no mention and pushes no capture: it only reads names. The operand itself is
//! walked as an ordinary eager argument by the mention pass.
//!
//! It also answers how a value name is bound to a quantified function ([`Quantified`]): a module
//! body's member, read unmarked anywhere the static pass instantiates it; a keyworded form's name
//! or a surfaced member, read only at a call's head; or any other binding, the static pass's to
//! instantiate or refuse.
//!
//! See [README.md § Names that arrive at run time](../../README.md#names-that-arrive-at-run-time).

use crate::memory::{BumpAllocator, BumpVec, collect, resident};
use crate::parse::BuiltinShapeId;
use crate::parse::quantifier_entries;
use crate::parse::{BodyKind, DefinitionKind, Role};
use crate::parse::{ExpressionPart, KExpression, KeyElement, Mark};
use crate::parse::{fn_def_binder_bucket, op_def_binder_bucket};
use crate::symbols::BinderSymbol;
use crate::type_lattice::DeclaredGroup;

use super::super::super::groups::{
    BuiltinGroup, Claim, builtin_equal, declared_group, groups_equal,
};
use super::super::{Position, ShapeError, ShapeKind, Site, Slot, SurfacedHead, Which};
use super::{Builder, body_of, quoted_body};
use crate::source::SourceRef;

/// What one operand surfaces: its names, in the order the spine gives them, the operator groups
/// the body may chain under, and its signature's bodyless keyworded heads. The binders pass sorts
/// the names into layout order, so the reader owes no ordering of its own.
pub(super) struct Surfaced<'x, 'graph> {
    pub names: BumpVec<'x, BinderSymbol>,
    pub groups: BumpVec<'x, &'graph DeclaredGroup<'graph>>,
    /// The names among `names` bound to quantified functions, read only at the head of a call.
    pub quantified: BumpVec<'x, BinderSymbol>,
    /// Each surfaced head, under each bucket key a definition with that head registers at.
    pub heads: BumpVec<'x, SurfacedKey<'graph>>,
    /// The draft level the block will be built at, which each head's places count hops from.
    body: usize,
}

/// One bucket key a surfaced head registers the block at: the head, the key, and which of the
/// head's keys it is.
#[derive(Clone, Copy)]
pub(super) struct SurfacedKey<'graph> {
    pub head: &'graph SurfacedHead<'graph>,
    pub elements: &'graph [KeyElement],
    pub which: Which,
}

impl<'x, 'graph> Surfaced<'x, 'graph> {
    pub(super) fn new(scratch: BumpAllocator<'x>) -> Self {
        Surfaced {
            names: BumpVec::new_in(scratch),
            groups: BumpVec::new_in(scratch),
            quantified: BumpVec::new_in(scratch),
            heads: BumpVec::new_in(scratch),
            body: 0,
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
    ) -> Result<(), ShapeError<'graph>> {
        let operand = &node
            .parts
            .get(1)
            .ok_or(ShapeError::Malformed {
                form: BuiltinShapeId::UsingScope,
                at: node.source,
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
        out.body = level + 1;
        self.value_names(level, at, operand, out, &mut fuel)
            .map_err(|()| ShapeError::Unsurfaced {
                at: self.part_source(level, statement, Site::of(operand)),
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
                        let ascription = (level, Site::of(signature));
                        self.type_names(level, at, signature, ascription, out, fuel)
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
                let (level, at, _, statement) = self.declaring(level, name, at).ok_or(())?;
                match statement.cache().builtin_shape().map(|shape| shape.id) {
                    Some(
                        BuiltinShapeId::Module
                        | BuiltinShapeId::GroupFoldLeft
                        | BuiltinShapeId::GroupFoldRight
                        | BuiltinShapeId::GroupPairwiseFoldLeft
                        | BuiltinShapeId::GroupPairwiseFoldRight,
                    ) => {
                        self.surfaced_group(statement, out)?;
                        body_binders(statement, out)
                    }
                    Some(BuiltinShapeId::LetValue) => {
                        let rhs = role_part(statement, Role::Rhs).ok_or(())?;
                        self.value_names(level, at, rhs, out, fuel)
                    }
                    // An annotated binder holds its value to its type, as `:!` does.
                    Some(BuiltinShapeId::LetAnnotated) => {
                        let annotation = role_part(statement, Role::TypeExpression).ok_or(())?;
                        let ascription = (level, Site::of(annotation));
                        self.type_names(level, at, annotation, ascription, out, fuel)
                    }
                    _ => Err(()),
                }
            }
            _ => Err(()),
        }
    }

    /// The members the type `part` declares, read in the draft at `level` at position `at`, beside
    /// the level and site of the ascription that wrote it.
    fn type_names(
        &self,
        level: usize,
        at: Position,
        part: &ExpressionPart<'graph>,
        ascription: (usize, Site),
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
                    return self.type_names(level, at, &pinned.value, ascription, out, fuel);
                }
                match node.parts {
                    [only] => self.type_names(level, at, &only.value, ascription, out, fuel),
                    _ => Err(()),
                }
            }
            ExpressionPart::Type(name) if self.skips(name) => Err(()),
            ExpressionPart::Type(name) => {
                let name = BinderSymbol::Type(*name);
                let (level, at, slot, statement) = self.declaring(level, name, at).ok_or(())?;
                match statement.cache().builtin_shape().map(|shape| shape.id) {
                    Some(BuiltinShapeId::Sig | BuiltinShapeId::QuantifiedSig) => {
                        let hops = |declared: usize| (out.body - declared) as u32;
                        let places = ((hops(level), slot), (hops(ascription.0), ascription.1));
                        self.sig_members(statement, places, out)
                    }
                    Some(BuiltinShapeId::LetValue) => {
                        let rhs = role_part(statement, Role::Rhs).ok_or(())?;
                        self.type_names(level, at, rhs, ascription, out, fuel)
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
        at: SourceRef,
        surfaced: &Surfaced<'x, 'graph>,
    ) -> Result<BumpVec<'x, &'graph DeclaredGroup<'graph>>, ShapeError<'graph>> {
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

    /// The names a `SIG` declares — its head parameters and its body's members — the groups its
    /// bodyless `GROUP` heads do, and its bodyless `EXPR` and `OP` heads, each under the keys a
    /// definition with that head registers at, beside `places`: where the `SIG`'s binder and the
    /// ascription naming it lie, in hops from the block. Anything else in a signature body is read
    /// by nobody here.
    ///
    /// A signature's group lives in its operator channel, so no claim holds a record for it and one
    /// is built here, in program storage, from the same member scan a `GROUP` statement takes.
    fn sig_members(
        &self,
        statement: &KExpression<'graph>,
        (signature, ascription): ((u32, Slot), (u32, Site)),
        out: &mut Surfaced<'x, 'graph>,
    ) -> Result<(), ()> {
        if let Some(group) = role_part(statement, Role::Quantifiers) {
            for entry in quantifier_entries(group) {
                out.names.push(BinderSymbol::Type(entry.name.ok_or(())?));
            }
        }
        let definition =
            role_part(statement, Role::Definition(DefinitionKind::Members)).ok_or(())?;
        let ExpressionPart::ListLiteral(members) = definition else {
            return Err(());
        };
        for member in members.iter() {
            let line = quoted_body(member).ok_or(())?.statement_spine();
            match line.cache().builtin_shape().map(|shape| shape.id) {
                Some(BuiltinShapeId::LetValue) => {
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
                    if role_part(line, Role::TypeExpression).is_some_and(quantified_type) {
                        out.quantified.push(BinderSymbol::Value(name));
                    }
                }
                Some(
                    BuiltinShapeId::GroupHeadFoldLeft
                    | BuiltinShapeId::GroupHeadFoldRight
                    | BuiltinShapeId::GroupHeadPairwiseFoldLeft
                    | BuiltinShapeId::GroupHeadPairwiseFoldRight,
                ) => {
                    // Scanned into scratch, then re-homed: the record a held group names is read
                    // by every activation, so it rests in program storage.
                    let group = declared_group(line, self.scratch)?.ok_or(())?;
                    let writer = self.brand.writer();
                    out.groups.push(resident(
                        writer,
                        DeclaredGroup {
                            members: collect(writer, group.members.iter().copied()),
                            mode: group.mode,
                        },
                    ));
                }
                Some(
                    id @ (BuiltinShapeId::ExpressionHead
                    | BuiltinShapeId::QuantifiedExpressionHead
                    | BuiltinShapeId::OperatorHead
                    | BuiltinShapeId::OperatorHeadReturning
                    | BuiltinShapeId::UnaryOperatorHeadReturning),
                ) => {
                    let writer = self.brand.writer();
                    let keys = match id {
                        BuiltinShapeId::ExpressionHead
                        | BuiltinShapeId::QuantifiedExpressionHead => {
                            fn_def_binder_bucket(writer, line)
                        }
                        _ => op_def_binder_bucket(writer, line),
                    }
                    .ok_or(())?;
                    let head = resident(
                        writer,
                        SurfacedHead {
                            head: line,
                            signature,
                            ascription,
                        },
                    );
                    let unary = keys.count() == 2;
                    for (which, elements) in keys.iter().enumerate() {
                        out.heads.push(SurfacedKey {
                            head,
                            elements,
                            which: match (unary, which) {
                                (false, _) => Which::Only,
                                (true, 0) => Which::Unary,
                                (true, _) => Which::Binary,
                            },
                        });
                    }
                }
                // A result-less `UNARY OP` head is refused where the signature is typed.
                Some(BuiltinShapeId::UnaryOperatorHead) => {}
                _ => return Err(()),
            }
        }
        Ok(())
    }

    /// How the value `name`, read through `mark` in the draft at `level` at `at`, is bound to a
    /// quantified function: see [`Quantified`]. [`Builder::resolve`]'s walk, with nothing recorded:
    /// a `$` name skips each local up to its code and reads outward from there, unmarked, and any
    /// other name stops at its code, since a `\` name is what an `EVAL` offers, checked there.
    pub(super) fn quantified(
        &self,
        level: usize,
        name: BinderSymbol,
        at: Position,
        mark: Option<Mark>,
    ) -> Quantified {
        if !matches!(name, BinderSymbol::Value(_)) {
            return Quantified::No;
        }
        let (mut level, mut at, mut mark) = (level, at, mark);
        loop {
            let draft = &self.chain[level];
            let names = draft.channels();
            if mark != Some(Mark::Written)
                && let Some(index) = names.find(name)
                && at.sees(names.get(index))
            {
                return match names.get(index).0.checked_sub(1) {
                    Some(statement) => {
                        let statement = &draft.nodes[statement as usize];
                        match statement.cache().builtin_shape().map(|shape| shape.id) {
                            Some(BuiltinShapeId::CombinedQuantifiedExpression) => {
                                Quantified::CallOnly
                            }
                            _ if draft.kind == ShapeKind::Module
                                && quantified_statement(statement) =>
                            {
                                Quantified::Member
                            }
                            _ => Quantified::No,
                        }
                    }
                    None if draft.quantified.contains(&name) => Quantified::CallOnly,
                    None => Quantified::No,
                };
            }
            match (draft.kind, mark) {
                (ShapeKind::Code, Some(Mark::Written)) => mark = None,
                (ShapeKind::Program | ShapeKind::Code, _) => return Quantified::No,
                _ => {}
            }
            let Some(parent) = level.checked_sub(1) else {
                return Quantified::No;
            };
            level = parent;
            at = self.chain[level].boundary();
        }
    }

    /// The draft level, the position a read there takes, the slot `name` binds and the statement
    /// declaring it — [`Builder::resolve`]'s walk with nothing recorded. `None` for a builtin, a
    /// parameter (which declares no statement), a hole of a quote's code, and a name with no
    /// binding at all.
    fn declaring(
        &self,
        level: usize,
        name: BinderSymbol,
        at: Position,
    ) -> Option<(usize, Position, Slot, &KExpression<'graph>)> {
        let (mut level, mut at) = (level, at);
        loop {
            let draft = &self.chain[level];
            let names = draft.channels();
            if let Some(index) = names.find(name)
                && at.sees(names.get(index))
            {
                let declared = names.get(index);
                let statement = declared.0.checked_sub(1)? as usize;
                return Some((level, declared, Slot(index as u32), &draft.nodes[statement]));
            }
            if matches!(draft.kind, ShapeKind::Program | ShapeKind::Code) {
                return None;
            }
            level = level.checked_sub(1)?;
            at = self.chain[level].boundary();
        }
    }
}

/// The names the body of a `MODULE` or `GROUP` binder binds, read through the very call the binders
/// pass makes, so the two cannot drift, and which of them are bound to quantified functions.
fn body_binders<'graph>(
    statement: &KExpression<'graph>,
    out: &mut Surfaced<'_, 'graph>,
) -> Result<(), ()> {
    let body = body_of(role_part(statement, Role::Body(BodyKind::Module)).ok_or(())?).ok_or(())?;
    for (line, _) in body.body_statements() {
        if let Some(name) = line.statement_binder_plan().and_then(|plan| plan.name) {
            out.names.push(name);
            if quantified_statement(line) {
                out.quantified.push(name);
            }
        }
    }
    Ok(())
}

/// How a value name is bound to a quantified function, which decides where it may be read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Quantified {
    /// Not quantified, or a plain `LET` of a quantified `FN` outside a module body: the static pass
    /// instantiates that binding or refuses it.
    No,
    /// A module body's plain `LET` of a quantified `FN`: read at the head of a call, or unmarked
    /// where the static pass instantiates it.
    Member,
    /// A keyworded form's name, or a quantified member a `USING … SCOPE` body surfaces: read only at
    /// the head of a call.
    CallOnly,
}

/// Whether `statement`'s value is a quantified function a keyworded form binds: a
/// `LET … = FN EXPR FOR ALL …`, or a bare `EXPR FOR ALL …` definition.
pub(super) fn quantified_value(statement: &KExpression<'_>) -> bool {
    matches!(
        statement.cache().builtin_shape().map(|shape| shape.id),
        Some(
            BuiltinShapeId::CombinedQuantifiedExpression
                | BuiltinShapeId::QuantifiedExpressionDefinition
        )
    )
}

/// Whether `statement` binds a quantified function: a `LET` of a quantified `FN`, or a
/// `LET … = FN EXPR FOR ALL …`.
fn quantified_statement(statement: &KExpression<'_>) -> bool {
    match statement.cache().builtin_shape().map(|shape| shape.id) {
        Some(BuiltinShapeId::CombinedQuantifiedExpression) => true,
        Some(BuiltinShapeId::LetValue) => role_part(statement, Role::Rhs)
            .is_some_and(|rhs| written_as(rhs, BuiltinShapeId::QuantifiedLambda)),
        _ => false,
    }
}

/// Whether a signature member's type part is a quantified function type or expression shape.
fn quantified_type(part: &ExpressionPart<'_>) -> bool {
    written_as(part, BuiltinShapeId::QuantifiedLambdaType)
        || written_as(part, BuiltinShapeId::QuantifiedExpressionHead)
}

/// Whether `part`, through one-part wrappers and a sigil, is a node of the form `id`.
fn written_as(part: &ExpressionPart<'_>, id: BuiltinShapeId) -> bool {
    let (ExpressionPart::Expression(node) | ExpressionPart::SigiledTypeExpr(node)) = part else {
        return false;
    };
    let node = node.reference();
    match (node.cache().builtin_shape(), node.parts) {
        (Some(form), _) => form.id == id,
        (None, [only]) => written_as(&only.value, id),
        _ => false,
    }
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
