//! One type expression, part by part: a name read through the activation, a composite built from
//! the handles its parts elaborate to.

use crate::memory::{BumpAllocator, BumpVec};
use crate::parse::builtin_shapes::binder::{bounded_name, quantifier_entries};
use crate::parse::builtin_shapes::{BuiltinShapeId, KEYWORDS};
use crate::parse::{ExpressionPart, KExpression};
use crate::scope::{ActivationView, Coordinate, Site, Slot, Target, pair_name};
use crate::symbols::{BinderSymbol, KeywordSymbol, StaticName, Symbol, TypeSymbol};
use crate::type_lattice::{
    DispatchTokenElement, GroupIntern, KType, NodeSchema, TypeNode, TypeRegistry,
    constructor_param_names, meet,
};
use crate::values::{KnottedFamily, Value};

use super::Elaboration;

/// The connector keywords of the formless composites.
struct Connectors {
    list: StaticName<KeywordSymbol>,
    of: StaticName<KeywordSymbol>,
    map: StaticName<KeywordSymbol>,
    union: StaticName<KeywordSymbol>,
    meet: StaticName<KeywordSymbol>,
    /// Arity-one constructor application. A connector of the type language, not a table keyword:
    /// the surrounding `:(…)` is what puts it in type context.
    as_: StaticName<KeywordSymbol>,
}

static CONNECTORS: Connectors = Connectors {
    list: crate::static_name!(KeywordSymbol, "LIST"),
    of: crate::static_name!(KeywordSymbol, "OF"),
    map: crate::static_name!(KeywordSymbol, "MAP"),
    union: crate::static_name!(KeywordSymbol, "|"),
    meet: crate::static_name!(KeywordSymbol, "&"),
    as_: crate::static_name!(KeywordSymbol, "AS"),
};

/// `part` as a type, its names read through `reader`: every name is the mention `reader`'s shape
/// recorded at its site, which must read as a type, save one a `FOR ALL` group inside `part`
/// declares.
pub fn type_expression<'graph, XF: KnottedFamily<'graph>>(
    part: &ExpressionPart<'graph>,
    reader: &ActivationView<'graph, '_, XF>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Result<KType, Elaboration> {
    let groups = Groups {
        names: &[],
        bounds: &[],
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
    /// Each name's bound, parallel to `names`. Empty while a group's own bounds are elaborated:
    /// every name then reads bounded by `Any`, and a bound holding one is refused.
    pub(super) bounds: &'q [KType],
    pub(super) outer: Option<&'q Groups<'q>>,
}

/// A `FOR ALL` group as written: its names in written order, each with its elaborated bound —
/// `Any` for a name written without one.
pub(super) struct QuantifierGroup<'x> {
    pub(super) names: BumpVec<'x, TypeSymbol>,
    pub(super) bounds: BumpVec<'x, KType>,
}

impl<'x> QuantifierGroup<'x> {
    /// The group an unquantified callable carries.
    pub(super) fn empty(scratch: BumpAllocator<'x>) -> Self {
        QuantifierGroup {
            names: BumpVec::new_in(scratch),
            bounds: BumpVec::new_in(scratch),
        }
    }
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
            Quantifier::Innermost(index) => {
                let bound = groups.bounds.get(index).copied().unwrap_or(KType::ANY);
                return Ok(self.types.quantified(index, bound));
            }
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
                    let group = QuantifierGroup::empty(self.scratch);
                    Ok(self.function(&group, part(1), part(3), groups)?.handle)
                }
                BuiltinShapeId::QuantifiedLambdaType => {
                    let group = self.group(part(3), groups)?;
                    Ok(self.function(&group, part(4), part(6), groups)?.handle)
                }
                BuiltinShapeId::ExpressionHead => {
                    let group = QuantifierGroup::empty(self.scratch);
                    self.shape(&group, part(1), part(3), groups)
                }
                BuiltinShapeId::QuantifiedExpressionHead => {
                    let group = self.group(part(3), groups)?;
                    self.shape(&group, part(4), part(6), groups)
                }
                BuiltinShapeId::Attribute => {
                    let owner = self.part(part(1), groups)?;
                    let name = match part(2) {
                        ExpressionPart::Type(name) => name.symbol(),
                        ExpressionPart::Identifier(name) => name.symbol(),
                        _ => return Err(unsupported),
                    };
                    self.types
                        .union_member_named(owner, name)
                        .or_else(|| self.field(owner, name))
                        .ok_or(Elaboration::NoSuchMember { owner, name })
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
            // `A & B`, and `& [A B C]`, its chained form — the meet, as a union is the join. A meet
            // that comes out `Never` is a type like any other; only a bound refuses it.
            3 if keyword(1, &CONNECTORS.meet) => Ok(meet(
                self.types,
                self.scratch,
                self.part(&parts[0].value, groups)?,
                self.part(&parts[2].value, groups)?,
            )),
            2 if keyword(0, &CONNECTORS.meet) => {
                let ExpressionPart::ListLiteral(operands) = parts[1].value else {
                    return Err(unsupported);
                };
                let mut met = KType::ANY;
                for operand in operands.iter() {
                    met = meet(self.types, self.scratch, met, self.part(operand, groups)?);
                }
                Ok(met)
            }
            _ => Err(unsupported),
        }
    }

    /// The type the record under `owner` declares `name` with, read through every newtype layer
    /// above it — a `NEWTYPE`'s representation, a union variant's payload. `None` when no record
    /// lies under `owner` or it declares no `name`; a ring of newtypes with no record under it,
    /// `NEWTYPE Loop = Loop`, is peeled once round and then refused.
    fn field(&self, owner: KType, name: Symbol) -> Option<KType> {
        let mut peeled = BumpVec::new_in(self.scratch);
        let mut layer = owner;
        loop {
            match self.types.node(layer) {
                TypeNode::Record { fields } => return fields.get(name),
                TypeNode::SetMember {
                    schema: NodeSchema::NewType(repr),
                    ..
                } if !peeled.contains(&repr) => {
                    peeled.push(layer);
                    layer = repr;
                }
                _ => return None,
            }
        }
    }

    /// A `FOR ALL` group's names and bounds, in written order. Each entry is a bare name or
    /// `(<Name> UNDER <bound>)`; anything else is unsupported. Every bound is read under the group
    /// with no bounds of its own, so a bound naming one of the group's names is refused.
    pub(super) fn group(
        &self,
        part: &ExpressionPart<'graph>,
        groups: &Groups<'_>,
    ) -> Result<QuantifierGroup<'x>, Elaboration> {
        let mut group = QuantifierGroup::empty(self.scratch);
        let mut written = BumpVec::new_in(self.scratch);
        for entry in quantifier_entries(part) {
            let (name, bound) = bounded_name(entry).ok_or(Elaboration::Unsupported {
                site: Site::of(entry),
            })?;
            group.names.push(name);
            written.push(bound);
        }
        let unbounded = Groups {
            names: &group.names,
            bounds: &[],
            outer: Some(groups),
        };
        for bound in written {
            group.bounds.push(match bound {
                Some(part) => self.bound(part, &unbounded)?,
                None => KType::ANY,
            });
        }
        Ok(group)
    }

    /// A bound: a closed, inhabited type. One naming a type variable — a `FOR ALL` name or a
    /// signature's abstract member — or that is `Never` is refused at its site.
    pub(super) fn bound(
        &self,
        part: &ExpressionPart<'graph>,
        groups: &Groups<'_>,
    ) -> Result<KType, Elaboration> {
        let bound = self.part(part, groups)?;
        if bound == KType::NEVER || self.types.contains_rigid(bound) {
            return Err(Elaboration::Bound {
                site: Site::of(part),
            });
        }
        Ok(bound)
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
    /// A non-empty `group` opens a group of its own, which the fields and the return read under;
    /// an empty one reads them under `groups` unchanged, because an unquantified `FN` type written
    /// inside a quantified head keeps reading that head's variables.
    pub(super) fn function(
        &self,
        group: &QuantifierGroup<'_>,
        schema: &ExpressionPart<'graph>,
        ret: &ExpressionPart<'graph>,
        groups: &Groups<'_>,
    ) -> Result<GroupIntern<'x>, Elaboration> {
        let own = Groups {
            names: &group.names,
            bounds: &group.bounds,
            outer: Some(groups),
        };
        let groups = if group.names.is_empty() { groups } else { &own };
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
        Ok(self
            .types
            .function_type(self.scratch, &group.names, &params, ret))
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
        group: &QuantifierGroup<'_>,
        head: &ExpressionPart<'graph>,
        ret: &ExpressionPart<'graph>,
        groups: &Groups<'_>,
    ) -> Result<GroupIntern<'x>, Elaboration> {
        let own = Groups {
            names: &group.names,
            bounds: &group.bounds,
            outer: Some(groups),
        };
        let groups = if group.names.is_empty() { groups } else { &own };
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
        Ok(self
            .types
            .function_type(self.scratch, &group.names, &params, ret))
    }

    /// `EXPR [FOR ALL <names>] <head> -> <return>`: the head's keywords and typed slots, under a
    /// group of its own.
    pub(super) fn shape(
        &self,
        group: &QuantifierGroup<'_>,
        head: &ExpressionPart<'graph>,
        ret: &ExpressionPart<'graph>,
        groups: &Groups<'_>,
    ) -> Result<KType, Elaboration> {
        let own = Groups {
            names: &group.names,
            bounds: &group.bounds,
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
            .shape_type(self.scratch, &group.names, &elements, ret)
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
