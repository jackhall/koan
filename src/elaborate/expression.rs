//! One type expression, part by part: a name read through the activation, a composite built from
//! the handles its parts elaborate to.

use crate::memory::{BumpAllocator, BumpVec};
use crate::parse::builtin_shapes::{BuiltinShapeId, KEYWORDS};
use crate::parse::{ExpressionPart, KExpression};
use crate::scope::{ActivationView, Coordinate, Site, Slot, Target, pair_name};
use crate::symbols::{BinderSymbol, KeywordSymbol, StaticName, TypeSymbol};
use crate::type_lattice::{
    DispatchTokenElement, GroupIntern, KType, TypeRegistry, constructor_param_names,
};
use crate::values::{KnottedFamily, Value};

use super::Elaboration;

/// The connector keywords of the formless composites.
struct Connectors {
    list: StaticName<KeywordSymbol>,
    of: StaticName<KeywordSymbol>,
    map: StaticName<KeywordSymbol>,
    union: StaticName<KeywordSymbol>,
    /// Arity-one constructor application. A connector of the type language, not a table keyword:
    /// the surrounding `:(…)` is what puts it in type context.
    as_: StaticName<KeywordSymbol>,
}

static CONNECTORS: Connectors = Connectors {
    list: crate::static_name!(KeywordSymbol, "LIST"),
    of: crate::static_name!(KeywordSymbol, "OF"),
    map: crate::static_name!(KeywordSymbol, "MAP"),
    union: crate::static_name!(KeywordSymbol, "|"),
    as_: crate::static_name!(KeywordSymbol, "AS"),
};

/// `part` as a type, its names read through `reader`: a name in `quantifiers` is that group's
/// quantifier at its position, and every other name is the mention `reader`'s shape recorded at its
/// site, which must read as a type.
pub fn type_expression<'graph, XF: KnottedFamily<'graph>>(
    part: &ExpressionPart<'graph>,
    reader: &ActivationView<'graph, '_, XF>,
    quantifiers: &[TypeSymbol],
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Result<KType, Elaboration> {
    let groups = Groups {
        names: quantifiers,
        outer: None,
    };
    Elaborator {
        reader,
        types,
        scratch,
        fellows: &[],
        locals: &[],
    }
    .part(part, &groups)
}

/// The `FOR ALL` groups enclosing a part, innermost first.
#[derive(Clone, Copy)]
pub(super) struct Groups<'q> {
    pub(super) names: &'q [TypeSymbol],
    pub(super) outer: Option<&'q Groups<'q>>,
}

/// Where a name sits among the enclosing groups.
enum Quantifier {
    /// The innermost group's quantifier at this position.
    Innermost(usize),
    /// An enclosing group's, read under a nested shape whose own group shadows it.
    Shadowed,
    /// No group's: a mention.
    Free,
}

impl Groups<'_> {
    fn find(&self, name: TypeSymbol) -> Quantifier {
        if let Some(index) = self.names.iter().position(|declared| *declared == name) {
            return Quantifier::Innermost(index);
        }
        let mut outer = self.outer;
        while let Some(group) = outer {
            if group.names.contains(&name) {
                return Quantifier::Shadowed;
            }
            outer = group.outer;
        }
        Quantifier::Free
    }
}

/// What elaborating one expression reads through.
pub(super) struct Elaborator<'e, 'run, 'graph, 'cell, 'x, XF: KnottedFamily<'graph>> {
    pub(super) reader: &'e ActivationView<'graph, 'cell, XF>,
    pub(super) types: &'e TypeRegistry<'run>,
    pub(super) scratch: BumpAllocator<'x>,
    /// Fellow members of the component being declared, each at the relative handle it is named by
    /// until the group seals: a member's own sibling, or a `UNION` binder's union of its variants'
    /// siblings. Empty for an ordinary type expression.
    pub(super) fellows: &'e [(Slot, KType)],
    /// Names declared inside the definition being elaborated and holding no slot of the enclosing
    /// shape — a `SIG` body's abstract and manifest members, and a higher-kinded declarator's
    /// parameters. Empty outside a definition.
    pub(super) locals: &'e [(TypeSymbol, KType)],
}

impl<'graph, 'x, XF: KnottedFamily<'graph>> Elaborator<'_, '_, 'graph, '_, 'x, XF> {
    pub(super) fn part(
        &self,
        part: &ExpressionPart<'graph>,
        groups: &Groups<'_>,
    ) -> Result<KType, Elaboration> {
        let site = Site::of(part);
        match part {
            ExpressionPart::Type(name) => self.name(site, *name, groups),
            ExpressionPart::Expression(node) | ExpressionPart::SigiledTypeExpr(node) => {
                self.node(site, node.reference(), groups)
            }
            ExpressionPart::RecordType(node) => {
                let mut fields = BumpVec::new_in(self.scratch);
                self.pairs(site, node.reference(), groups, |name, ktype| {
                    fields.push((name, ktype));
                    Ok(())
                })?;
                Ok(self.types.record(self.scratch, &fields))
            }
            _ => Err(Elaboration::Unsupported { site }),
        }
    }

    /// A type name: a quantifier of the innermost group, or the type its mention reads.
    fn name(
        &self,
        site: Site,
        name: TypeSymbol,
        groups: &Groups<'_>,
    ) -> Result<KType, Elaboration> {
        match groups.find(name) {
            Quantifier::Innermost(index) => return Ok(self.types.quantified(index, KType::ANY)),
            Quantifier::Shadowed => return Err(Elaboration::Unsupported { site }),
            Quantifier::Free => {}
        }
        // A name the definition declares records no mention, so it is answered before the mention
        // lookup, which would otherwise find nothing to read.
        if let Some((_, handle)) = self.locals.iter().find(|(declared, _)| *declared == name) {
            return Ok(*handle);
        }
        // A definition declares its own names, so the shape records no mention for one. Every
        // other name a type expression reads has one; a definition-local name reaching here has
        // not been declared yet — a forward reference the local table cannot answer.
        let Some(mention) = self.reader.shape().mention(site) else {
            return Err(Elaboration::Unsupported { site });
        };
        // A fellow member of the component being declared is not bound yet: it is named by the
        // relative handle its still-open window minted. A definition part opens no nested shape,
        // so every fellow mention is local to the declaring shape.
        if let Coordinate::Activation {
            hops: 0,
            target: Target::Local(slot),
        } = mention.coordinate
            && let Some((_, handle)) = self.fellows.iter().find(|(fellow, _)| *fellow == slot)
        {
            return Ok(*handle);
        }
        match self.reader.read(mention.coordinate) {
            Value::Type(value) => Ok(value.handle()),
            _ => Err(Elaboration::NotAType { name, site }),
        }
    }

    /// A parenthesized or sigiled type expression.
    pub(super) fn node(
        &self,
        site: Site,
        node: &KExpression<'graph>,
        groups: &Groups<'_>,
    ) -> Result<KType, Elaboration> {
        let unsupported = Elaboration::Unsupported { site };
        let parts = node.parts;
        if let [only] = parts {
            return self.part(&only.value, groups);
        }
        // `Ctor {Param = Type, …}` — a declared type constructor applied to its arguments by
        // member name. It resolves no builtin shape: the head is a type name and the payload a
        // record literal, so the arm is keyed structurally, ahead of the table lookup.
        if let [head, payload] = parts
            && let ExpressionPart::RecordLiteral(arguments) = &payload.value
        {
            let constructor = self.part(&head.value, groups)?;
            let mut applied = BumpVec::with_capacity_in(arguments.len(), self.scratch);
            for (name, argument) in arguments.iter() {
                applied.push((*name, self.part(argument, groups)?));
            }
            return self.apply(site, constructor, &applied);
        }
        if let Some(form) = node.cache().builtin_shape() {
            let part = |index: usize| &parts[index].value;
            return match form.id {
                BuiltinShapeId::LambdaType => {
                    Ok(self.function(&[], part(1), part(3), groups)?.handle)
                }
                BuiltinShapeId::QuantifiedLambdaType => {
                    let names = quantifiers(part(3), self.scratch);
                    Ok(self.function(&names, part(4), part(6), groups)?.handle)
                }
                BuiltinShapeId::ExpressionHead => self.shape(&[], part(1), part(3), groups),
                BuiltinShapeId::QuantifiedExpressionHead => {
                    let names = quantifiers(part(3), self.scratch);
                    self.shape(&names, part(4), part(6), groups)
                }
                BuiltinShapeId::Attribute => {
                    let union = self.part(part(1), groups)?;
                    let tag = match part(2) {
                        ExpressionPart::Type(name) => name.symbol(),
                        ExpressionPart::Identifier(name) => name.symbol(),
                        _ => return Err(unsupported),
                    };
                    self.types
                        .union_member_named(union, tag)
                        .ok_or(Elaboration::NoSuchMember { union, tag })
                }
                _ => Err(unsupported),
            };
        }
        let keyword = |index: usize, expected: &StaticName<KeywordSymbol>| matches!(parts[index].value, ExpressionPart::Keyword(symbol) if symbol == expected.symbol());
        match parts.len() {
            3 if keyword(0, &CONNECTORS.list) && keyword(1, &CONNECTORS.of) => {
                Ok(self.types.list(self.part(&parts[2].value, groups)?))
            }
            // `Type AS Ctor` — the arity-one sugar for the application above.
            3 if keyword(1, &CONNECTORS.as_) => {
                let constructor = self.part(&parts[2].value, groups)?;
                let [param] =
                    constructor_param_names(constructor, self.types).ok_or(unsupported)?
                else {
                    return Err(unsupported);
                };
                let argument = self.part(&parts[0].value, groups)?;
                self.apply(site, constructor, &[(BinderSymbol::Type(*param), argument)])
            }
            4 if keyword(0, &CONNECTORS.map) && keyword(2, &KEYWORDS.arrow) => {
                let key = self.part(&parts[1].value, groups)?;
                let value = self.part(&parts[3].value, groups)?;
                Ok(self.types.dict(key, value))
            }
            // `A | B`. A longer run is an operator run, which the shape builder already chained
            // into the unary call below, so nothing here walks a union part by part.
            3 if keyword(1, &CONNECTORS.union) => {
                let members = [
                    self.part(&parts[0].value, groups)?,
                    self.part(&parts[2].value, groups)?,
                ];
                Ok(self.types.union_of(self.scratch, &members))
            }
            // `| [A B C]` — the chained form of `A | B | C`, under the builtin unary union group.
            2 if keyword(0, &CONNECTORS.union) => {
                let ExpressionPart::ListLiteral(operands) = parts[1].value else {
                    return Err(unsupported);
                };
                let mut members = BumpVec::with_capacity_in(operands.len(), self.scratch);
                for member in operands.iter() {
                    members.push(self.part(member, groups)?);
                }
                Ok(self.types.union_of(self.scratch, &members))
            }
            _ => Err(unsupported),
        }
    }

    /// A declared type constructor applied to `arguments`, keyed by the parameter names the
    /// family declares: every parameter named once, and no name the family does not declare.
    fn apply(
        &self,
        site: Site,
        constructor: KType,
        arguments: &[(BinderSymbol, KType)],
    ) -> Result<KType, Elaboration> {
        let unsupported = Elaboration::Unsupported { site };
        let declared = constructor_param_names(constructor, self.types).ok_or(unsupported)?;
        if arguments.len() != declared.len()
            || !arguments.iter().all(
                |(name, _)| matches!(name, BinderSymbol::Type(name) if declared.contains(name)),
            )
            || arguments
                .iter()
                .enumerate()
                .any(|(index, (name, _))| arguments[..index].iter().any(|(seen, _)| seen == name))
        {
            return Err(unsupported);
        }
        Ok(self
            .types
            .constructor_apply(self.scratch, constructor, arguments))
    }

    /// `FN [FOR ALL <names>] <schema> -> <return>`.
    ///
    /// A non-empty `names` opens a group of its own, which the fields and the return read under;
    /// an empty one reads them under `groups` unchanged, because an unquantified `FN` type written
    /// inside a quantified head keeps reading that head's variables.
    pub(super) fn function(
        &self,
        names: &[TypeSymbol],
        schema: &ExpressionPart<'graph>,
        ret: &ExpressionPart<'graph>,
        groups: &Groups<'_>,
    ) -> Result<GroupIntern<'x>, Elaboration> {
        let own = Groups {
            names,
            outer: Some(groups),
        };
        let groups = if names.is_empty() { groups } else { &own };
        let ExpressionPart::RecordType(fields) = schema else {
            return Err(Elaboration::Unsupported {
                site: Site::of(schema),
            });
        };
        let mut params = BumpVec::new_in(self.scratch);
        self.pairs(
            Site::of(schema),
            fields.reference(),
            groups,
            |name, ktype| {
                params.push((name, ktype));
                Ok(())
            },
        )?;
        let ret = self.part(ret, groups)?;
        Ok(self.types.function_type(self.scratch, names, &params, ret))
    }

    /// The **function** type a combined form's head declares: the head's `<name> :<Type>` pairs as
    /// a params record, its keywords dropped, under a group of its own.
    ///
    /// A call through the `LET` name the combined form binds is by name, not by keyword, so this
    /// is the type the name holds; the head's shape goes only to the dispatch bucket. A `_` pair
    /// is unsupported here: a body-bearing definition names its parameters, and a function type
    /// has no positional slot to put a nameless one in.
    pub(super) fn head_function(
        &self,
        names: &[TypeSymbol],
        head: &ExpressionPart<'graph>,
        ret: &ExpressionPart<'graph>,
        groups: &Groups<'_>,
    ) -> Result<GroupIntern<'x>, Elaboration> {
        let own = Groups {
            names,
            outer: Some(groups),
        };
        let groups = if names.is_empty() { groups } else { &own };
        let unsupported = Elaboration::Unsupported {
            site: Site::of(head),
        };
        let ExpressionPart::Expression(run) = head else {
            return Err(unsupported);
        };
        let run = run.reference();
        let mut params = BumpVec::with_capacity_in(run.parts.len() / 2, self.scratch);
        let mut index = 0;
        while index < run.parts.len() {
            match (run.parts[index].value, pair_name(run, index)) {
                (_, Some(Some(name))) => {
                    params.push((name, self.part(&run.parts[index + 1].value, groups)?));
                    index += 2;
                }
                (ExpressionPart::Keyword(_), None) => index += 1,
                _ => return Err(unsupported),
            }
        }
        let ret = self.part(ret, groups)?;
        Ok(self.types.function_type(self.scratch, names, &params, ret))
    }

    /// `EXPR [FOR ALL <names>] <head> -> <return>`: the head's keywords and typed slots, under a
    /// group of its own.
    pub(super) fn shape(
        &self,
        names: &[TypeSymbol],
        head: &ExpressionPart<'graph>,
        ret: &ExpressionPart<'graph>,
        groups: &Groups<'_>,
    ) -> Result<KType, Elaboration> {
        let own = Groups {
            names,
            outer: Some(groups),
        };
        let unsupported = Elaboration::Unsupported {
            site: Site::of(head),
        };
        let ExpressionPart::Expression(run) = head else {
            return Err(unsupported);
        };
        let run = run.reference();
        let mut elements = BumpVec::with_capacity_in(run.parts.len(), self.scratch);
        let mut index = 0;
        while index < run.parts.len() {
            match (run.parts[index].value, pair_name(run, index)) {
                (_, Some(_)) => {
                    let slot = self.part(&run.parts[index + 1].value, &own)?;
                    elements.push(DispatchTokenElement::Slot(slot));
                    index += 2;
                }
                (ExpressionPart::Keyword(symbol), None) => {
                    elements.push(DispatchTokenElement::Keyword(symbol));
                    index += 1;
                }
                _ => return Err(unsupported),
            }
        }
        let ret = self.part(ret, &own)?;
        Ok(self
            .types
            .shape_type(self.scratch, names, &elements, ret)
            .handle)
    }

    /// Each `<name> :<Type>` pair of a field list, its type elaborated; a `_` pair or anything
    /// that is not a pair is unsupported.
    fn pairs(
        &self,
        site: Site,
        run: &KExpression<'graph>,
        groups: &Groups<'_>,
        mut field: impl FnMut(crate::symbols::BinderSymbol, KType) -> Result<(), Elaboration>,
    ) -> Result<(), Elaboration> {
        let mut index = 0;
        while index < run.parts.len() {
            let Some(Some(name)) = pair_name(run, index) else {
                return Err(Elaboration::Unsupported { site });
            };
            field(name, self.part(&run.parts[index + 1].value, groups)?)?;
            index += 2;
        }
        Ok(())
    }
}

/// The names a `FOR ALL` group declares, in written order.
pub(super) fn quantifiers<'x>(
    group: &ExpressionPart<'_>,
    scratch: BumpAllocator<'x>,
) -> BumpVec<'x, TypeSymbol> {
    let mut names = BumpVec::new_in(scratch);
    if let ExpressionPart::Expression(group) = group {
        names.extend(group.parts.iter().filter_map(|part| match part.value {
            ExpressionPart::Type(name) => Some(name),
            _ => None,
        }));
    }
    names
}
