//! One type expression, part by part: a name read through the activation, a composite built from
//! the handles its parts elaborate to.
//!
//! A part elaborates to a [`Parametric`] type: a `FOR ALL` name reads as its quantified variable, a
//! signature's head parameter as itself, and a run-bound name at load as its lexical variable. A
//! quantified callable's type is a [`Scheme`](crate::type_lattice::Scheme), written only as the
//! whole type of a signature member ([`Elaborator::part_declared`]). An operand whose value over a
//! variable can differ from substituting first — a meet, a projection's owner, an application's
//! head, a `NEEDING` kind — is read concrete. A binder whose parameters, slots or representation
//! hold a union two of whose members tie ([`tied_members`]) is refused where it is declared: an
//! argument both admit would solve its group by whichever member is stored first.

use std::cell::Cell;

use super::reads::{Reads, TypeAt};
use crate::memory::{BumpAllocator, BumpVec};
use crate::parse::{BuiltinShapeId, KEYWORDS};
use crate::parse::{ExpressionPart, KExpression};
use crate::parse::{SlotLabel, needed_entry, needing, quantifier_entries};
use crate::scope::{Coordinate, Elaboration, Site, Slot, Target, pair_label};
use crate::symbols::{BinderSymbol, KeywordSymbol, StaticName, Symbol, TypeSymbol};
use crate::type_lattice::{
    DeclaredType, DispatchTokenElement, GroupIntern, KType, NodeSchema, Parametric, SigOrigin,
    TypeNode, TypeRegistry, constructor_param_names, dense_classes, meet, member, tied_members,
};

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
pub fn type_expression<'graph, R: Reads<'graph> + ?Sized>(
    part: &ExpressionPart<'graph>,
    reader: &R,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Result<Parametric, Elaboration> {
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
        binder: Cell::new(false),
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

/// A member of the component being declared, named by its relative handle until the group seals,
/// with the parameter names it takes when it is a family — symbol-sorted, empty otherwise.
pub(super) struct Fellow<'f> {
    pub(super) slot: Slot,
    pub(super) handle: KType,
    pub(super) params: &'f [TypeSymbol],
}

/// What elaborating one expression reads through.
pub(super) struct Elaborator<'e, 'run, 'x, R: ?Sized> {
    pub(super) reader: &'e R,
    pub(super) types: &'e TypeRegistry<'run>,
    pub(super) scratch: BumpAllocator<'x>,
    /// Fellow members of the component being declared, each at the relative handle it is named by
    /// until the group seals: a member's own sibling, or a `UNION` binder's union of its variants'
    /// siblings. Empty for an ordinary type expression.
    pub(super) fellows: &'e [Fellow<'e>],
    /// Names declared inside the definition being elaborated and holding no slot of the enclosing
    /// shape — a `SIG`'s head parameters and manifest members, and a higher-kinded declarator's
    /// parameters. Empty outside a definition.
    pub(super) locals: &'e [(TypeSymbol, Parametric)],
    /// Whether the next composite node may be a quantified function type or expression shape: set
    /// for one signature member's type, and cleared by the first composite the elaborator reaches,
    /// so a quantified type nested inside one is refused.
    pub(super) binder: Cell<bool>,
}

impl<'graph, 'x, R: Reads<'graph> + ?Sized> Elaborator<'_, '_, 'x, R> {
    /// `part` as a type. A quantified callable's type is refused here: it is written only as the
    /// whole type of a signature member, which [`part_declared`](Self::part_declared) reads.
    pub(super) fn part(
        &self,
        part: &ExpressionPart<'graph>,
        groups: &Groups<'_>,
    ) -> Result<Parametric, Elaboration> {
        match self.part_declared(part, groups)? {
            DeclaredType::Type(kt) => Ok(kt),
            DeclaredType::Scheme(_) => Err(Elaboration::Quantified {
                site: Site::of(part),
            }),
        }
    }

    /// `part` as a declared type: a type, or — where [`binder`](Self::binder) allows one, as the
    /// whole type of a signature member — a quantified callable's scheme.
    pub(super) fn part_declared(
        &self,
        part: &ExpressionPart<'graph>,
        groups: &Groups<'_>,
    ) -> Result<DeclaredType<Parametric>, Elaboration> {
        let site = Site::of(part);
        match part {
            ExpressionPart::Type(name) => self.name(site, *name, groups).map(DeclaredType::Type),
            // A marked name resolves past the definition's own quantifiers and names, through the
            // mention the shape recorded.
            ExpressionPart::MarkedName(_, BinderSymbol::Type(name)) => {
                self.mention(site, *name).map(DeclaredType::Type)
            }
            ExpressionPart::Expression(node) | ExpressionPart::SigiledTypeExpr(node) => {
                self.node_declared(site, node.reference(), groups)
            }
            ExpressionPart::RecordType(node) => {
                self.binder.set(false);
                let mut fields = BumpVec::new_in(self.scratch);
                self.pairs(site, node.reference(), groups, |name, ktype| {
                    fields.push((name, ktype));
                    Ok(())
                })?;
                Ok(DeclaredType::Type(self.types.record(self.scratch, &fields)))
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
    ) -> Result<Parametric, Elaboration> {
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
        self.mention(site, name)
    }

    /// A type name read through the mention the shape recorded at `site`. A run-bound name reads
    /// as its lexical variable, which no group's binder captures.
    fn mention(&self, site: Site, name: TypeSymbol) -> Result<Parametric, Elaboration> {
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
            && let Some(fellow) = self.fellows.iter().find(|fellow| fellow.slot == slot)
        {
            return Ok(fellow.handle.into());
        }
        match self.reader.type_at(mention.coordinate) {
            TypeAt::Type(handle) => Ok(handle.into()),
            TypeAt::Rigid(handle) => Ok(handle),
            TypeAt::NotAType => Err(Elaboration::NotAType { name, site }),
            TypeAt::Unknown => Err(Elaboration::Unknown { site }),
        }
    }

    /// `operands` elaborated, and refused `Unknown` at `site` when one read a lexical variable: the
    /// operands of a spelling whose value over a variable can differ from substituting first and
    /// elaborating after — a meet, a projection's owner, an application's head, a `NEEDING` kind —
    /// and a bound, which holds no variable.
    fn closed_operands<T>(
        &self,
        site: Site,
        operands: impl FnOnce() -> Result<T, Elaboration>,
    ) -> Result<T, Elaboration> {
        let before = self.reader.rigid_reads();
        let elaborated = operands()?;
        if self.reader.rigid_reads() > before {
            return Err(Elaboration::Unknown { site });
        }
        Ok(elaborated)
    }

    /// The operand `operands` elaborates to, read concrete: refused `Unknown` where it read a
    /// lexical variable, and `Unsupported` where it names a `FOR ALL` variable or a head parameter,
    /// which no spelling that reads its operand concrete takes.
    fn closed_concrete(
        &self,
        site: Site,
        operands: impl FnOnce() -> Result<Parametric, Elaboration>,
    ) -> Result<KType, Elaboration> {
        let operand = self.closed_operands(site, operands)?;
        self.types
            .concrete(operand)
            .ok_or(Elaboration::Unsupported { site })
    }

    /// A parenthesized or sigiled type expression, refusing a scheme as [`part`](Self::part) does.
    pub(super) fn node(
        &self,
        site: Site,
        node: &KExpression<'graph>,
        groups: &Groups<'_>,
    ) -> Result<Parametric, Elaboration> {
        match self.node_declared(site, node, groups)? {
            DeclaredType::Type(kt) => Ok(kt),
            DeclaredType::Scheme(_) => Err(Elaboration::Quantified { site }),
        }
    }

    /// A parenthesized or sigiled type expression as a declared type: a quantified function type
    /// or expression shape is a scheme, where [`binder`](Self::binder) allows one.
    pub(super) fn node_declared(
        &self,
        site: Site,
        node: &KExpression<'graph>,
        groups: &Groups<'_>,
    ) -> Result<DeclaredType<Parametric>, Elaboration> {
        let unsupported = Elaboration::Unsupported { site };
        let parts = node.parts;
        if let [only] = parts {
            return self.part_declared(&only.value, groups);
        }
        let binder = self.binder.take();
        let quantified = Elaboration::Quantified { site };
        // `Ctor {Param = Type, …}` — a declared type constructor applied to its arguments by
        // member name. It resolves no builtin shape: the head is a type name and the payload a
        // record literal, so the arm is keyed structurally, ahead of the table lookup.
        if let [head, payload] = parts
            && let ExpressionPart::RecordLiteral(arguments) = &payload.value
        {
            let constructor = self.closed_concrete(site, || self.part(&head.value, groups))?;
            let mut applied = BumpVec::with_capacity_in(arguments.len(), self.scratch);
            for (name, argument) in arguments.iter() {
                applied.push((*name, self.part(argument, groups)?));
            }
            return self
                .apply(site, constructor, &applied)
                .map(DeclaredType::Type);
        }
        if let Some(form) = node.cache().builtin_shape() {
            let part = |index: usize| &parts[index].value;
            return match form.id {
                BuiltinShapeId::LambdaType => {
                    let group = QuantifierGroup::empty(self.scratch);
                    Ok(self.function(&group, part(1), part(3), groups)?.handle)
                }
                BuiltinShapeId::QuantifiedLambdaType if !binder => Err(quantified),
                BuiltinShapeId::QuantifiedExpressionHead if !binder => Err(quantified),
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
                    let owner = self.closed_operands(site, || self.part(part(1), groups))?;
                    let name = match part(2) {
                        ExpressionPart::Type(name) => name.symbol(),
                        ExpressionPart::Identifier(name) => name.symbol(),
                        _ => return Err(unsupported),
                    };
                    // An owner naming a variable declares no member.
                    self.types
                        .concrete(owner)
                        .and_then(|owner| {
                            self.types
                                .union_member_named(owner, name)
                                .or_else(|| declared_field(self.types, self.scratch, owner, name))
                        })
                        .map(|member| DeclaredType::Type(member.into()))
                        .ok_or(Elaboration::NoSuchMember { owner, name, site })
                }
                _ => Err(unsupported),
            };
        }
        self.composite(site, node, groups).map(DeclaredType::Type)
    }

    /// A composite of the formless connectors: a list, a dict, a union, a meet, an application, a
    /// code kind needing names, or a pinned signature.
    fn composite(
        &self,
        site: Site,
        node: &KExpression<'graph>,
        groups: &Groups<'_>,
    ) -> Result<Parametric, Elaboration> {
        let unsupported = Elaboration::Unsupported { site };
        let parts = node.parts;
        let keyword = |index: usize, expected: &StaticName<KeywordSymbol>| matches!(parts[index].value, ExpressionPart::Keyword(symbol) if symbol == expected.symbol());
        match parts.len() {
            3 if keyword(0, &CONNECTORS.list) && keyword(1, &CONNECTORS.of) => {
                Ok(self.types.list(self.part(&parts[2].value, groups)?))
            }
            // `Type AS Ctor` — the arity-one sugar for the application above.
            3 if keyword(1, &CONNECTORS.as_) => {
                let constructor =
                    self.closed_concrete(site, || self.part(&parts[2].value, groups))?;
                let [param] = self.param_names(constructor).ok_or(unsupported)? else {
                    return Err(unsupported);
                };
                let argument = self.part(&parts[0].value, groups)?;
                self.apply(site, constructor, &[(BinderSymbol::Type(*param), argument)])
            }
            // `Kind NEEDING #[y …]` — a code kind below `Code`, and a list of quotes each of one
            // name or of a bucket key.
            3 if let Some((kind, quotes)) = needing(node) => {
                let kind = self.closed_concrete(site, || self.part(kind, groups))?;
                if kind.code_parent().is_none() {
                    return Err(unsupported);
                }
                let mut names = BumpVec::with_capacity_in(quotes.len(), self.scratch);
                for quote in quotes.iter() {
                    names.push(needed_entry(quote).ok_or(unsupported)?);
                }
                Ok(self.types.code_needing(self.scratch, kind, &names).into())
            }
            // `Sig WITH {Param = Type, …}` — a declared signature with some head parameters pinned.
            3 if keyword(1, &KEYWORDS.with)
                && let ExpressionPart::RecordLiteral(pins) = &parts[2].value =>
            {
                let signature =
                    self.closed_concrete(site, || self.part(&parts[0].value, groups))?;
                self.pin(site, signature, pins, groups)
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
            // that comes out `Never` is a type like any other; only a bound refuses it. An operand
            // naming a `FOR ALL` variable or a head parameter is refused: each call solves the
            // variable, so the meet cannot be taken here.
            3 if keyword(1, &CONNECTORS.meet) => {
                let operands = self.closed_operands(site, || {
                    Ok([
                        self.part(&parts[0].value, groups)?,
                        self.part(&parts[2].value, groups)?,
                    ])
                })?;
                self.meet(Site::of(&parts[1].value), &operands)
            }
            2 if keyword(0, &CONNECTORS.meet) => {
                let ExpressionPart::ListLiteral(written) = parts[1].value else {
                    return Err(unsupported);
                };
                let operands = self.closed_operands(site, || {
                    let mut operands = BumpVec::with_capacity_in(written.len(), self.scratch);
                    for operand in written.iter() {
                        operands.push(self.part(operand, groups)?);
                    }
                    Ok(operands)
                })?;
                self.meet(Site::of(&parts[0].value), &operands)
            }
            _ => Err(unsupported),
        }
    }

    /// The meet of `operands`, refused at `site` — the `&` — when one names a `FOR ALL` variable or
    /// a head parameter: the meet relates concrete types only.
    fn meet(&self, site: Site, operands: &[Parametric]) -> Result<Parametric, Elaboration> {
        let mut met = KType::ANY;
        for operand in operands {
            let operand = self
                .types
                .concrete(*operand)
                .ok_or(Elaboration::MeetOverVariable { site })?;
            met = meet(self.types, self.scratch, met, operand);
        }
        Ok(met.into())
    }

    /// A `FOR ALL` group's names and bounds, in written order: a list of name quotes, or a dict of
    /// name quotes to bound quotes; an entry naming no lone type is unsupported. Every bound is
    /// read under the group with no bounds of its own, so a bound naming one of the group's names
    /// is refused.
    pub(super) fn group(
        &self,
        part: &ExpressionPart<'graph>,
        groups: &Groups<'_>,
    ) -> Result<QuantifierGroup<'x>, Elaboration> {
        let mut group = QuantifierGroup::empty(self.scratch);
        let mut written = BumpVec::new_in(self.scratch);
        for entry in quantifier_entries(part) {
            let name = entry.name.ok_or(Elaboration::Unsupported {
                site: Site::of(entry.written),
            })?;
            group.names.push(name);
            written.push(entry.bound);
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

    /// A bound: a closed, inhabited type. One naming a run-bound name is left for the run; one
    /// naming any other type variable — a `FOR ALL` name or a signature's head parameter — or
    /// that is `Never` is refused at its site.
    pub(super) fn bound(
        &self,
        part: &ExpressionPart<'graph>,
        groups: &Groups<'_>,
    ) -> Result<KType, Elaboration> {
        let refused = Elaboration::Bound {
            site: Site::of(part),
        };
        let bound = self.closed_operands(Site::of(part), || self.part(part, groups))?;
        // An opaque carrier is concrete, but one as a bound waits on modules.
        match self.types.concrete(bound) {
            Some(bound) if bound != KType::NEVER && !self.types.holds_carrier(bound) => Ok(bound),
            _ => Err(refused),
        }
    }

    /// The parameter names a constructor head takes: a fellow family's while its group is open, a
    /// declared family's, or — for a union every member of
    /// which takes one parameter set — that set, since applying the union applies each member.
    fn param_names(&self, constructor: KType) -> Option<&[TypeSymbol]> {
        if let Some(fellow) = self
            .fellows
            .iter()
            .find(|fellow| fellow.handle == constructor && !fellow.params.is_empty())
        {
            return Some(fellow.params);
        }
        if let TypeNode::Union { members } = self.types.node(constructor) {
            let mut members = members.iter();
            let names = constructor_param_names(members.next()?, self.types)?;
            return members
                .all(|member| constructor_param_names(member, self.types) == Some(names))
                .then_some(names);
        }
        constructor_param_names(constructor, self.types)
    }

    /// A declared type constructor applied to `arguments`, keyed by the parameter names the
    /// family declares: every parameter named once, and no name the family does not declare. A
    /// parameterized union's head applies each of its variants at the same arguments.
    fn apply(
        &self,
        site: Site,
        constructor: KType,
        arguments: &[(BinderSymbol, Parametric)],
    ) -> Result<Parametric, Elaboration> {
        let unsupported = Elaboration::Unsupported { site };
        let declared = self.param_names(constructor).ok_or(unsupported)?;
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
        if let TypeNode::Union { members } = self.types.node(constructor) {
            let mut applied = BumpVec::with_capacity_in(members.len(), self.scratch);
            applied.extend(members.iter().map(|member| {
                self.types
                    .constructor_apply(self.scratch, member, arguments)
            }));
            return Ok(self.types.union_of(self.scratch, &applied));
        }
        Ok(self
            .types
            .constructor_apply(self.scratch, constructor, arguments))
    }

    /// `signature` with `pins` fixing head parameters by name: `signature` a declared signature,
    /// and each key one of its parameters. The parser has refused a repeated key.
    fn pin(
        &self,
        site: Site,
        signature: KType,
        pins: &[(BinderSymbol, ExpressionPart<'graph>)],
        groups: &Groups<'_>,
    ) -> Result<Parametric, Elaboration> {
        let unsupported = Elaboration::Unsupported { site };
        let TypeNode::Signature { schema, .. } = self.types.node(signature) else {
            return Err(unsupported);
        };
        if schema.origin != SigOrigin::Declared {
            return Err(unsupported);
        }
        let mut pinned = BumpVec::with_capacity_in(pins.len(), self.scratch);
        for (name, pin) in pins {
            let BinderSymbol::Type(parameter) = name else {
                return Err(unsupported);
            };
            if member(schema.parameters, *parameter).is_none() {
                return Err(unsupported);
            }
            pinned.push((*name, self.part(pin, groups)?));
        }
        Ok(self.types.signature_apply(self.scratch, signature, &pinned))
    }

    /// `FN [FOR ALL <names>] <schema> -> <return>`: a scheme over a non-empty group.
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
        if !group.names.is_empty() {
            let params = params.iter().map(|(_, param)| *param);
            self.untied(Site::of(schema), (&group.names, &group.bounds), params)?;
        }
        let ret = self.part(ret, groups)?;
        Ok(self
            .types
            .function_scheme(self.scratch, &group.names, &group.bounds, &params, ret))
    }

    /// The **function** type an `EXPR` definition's head declares, bare or combined: the head's
    /// `<name> :<Type>` pairs as a params record, its keywords dropped, under a group of its own.
    ///
    /// A call through the function is by name, not by keyword, so this is the type its value holds;
    /// the head's shape goes only to the dispatch bucket. A `_` pair is unsupported here: a
    /// body-bearing definition names its parameters, and a function type has no positional slot
    /// to put a nameless one in.
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
        let ExpressionPart::QuotedExpression(run) = head else {
            return Err(unsupported);
        };
        let run = run.reference();
        let mut params = BumpVec::with_capacity_in(run.parts.len() / 2, self.scratch);
        walk_head(run, unsupported, |element| {
            if let HeadElement::Slot(label, slot) = element {
                params.push((label.name().ok_or(unsupported)?, self.part(slot, groups)?));
            }
            Ok(())
        })?;
        if !group.names.is_empty() {
            let params = params.iter().map(|(_, param)| *param);
            self.untied(Site::of(head), (&group.names, &group.bounds), params)?;
        }
        let ret = self.part(ret, groups)?;
        Ok(self
            .types
            .function_scheme(self.scratch, &group.names, &group.bounds, &params, ret))
    }

    /// `EXPR [FOR ALL <names>] <head> -> <return>`: the head's keywords and typed slots, under a
    /// group of its own, ranked by the integers a signature member writes in its slots' places: a
    /// scheme over a non-empty group. An empty group still shadows every enclosing one, so a name
    /// one declares is refused here, and the shape binds nothing.
    pub(super) fn shape(
        &self,
        group: &QuantifierGroup<'_>,
        head: &ExpressionPart<'graph>,
        ret: &ExpressionPart<'graph>,
        groups: &Groups<'_>,
    ) -> Result<DeclaredType<Parametric>, Elaboration> {
        let own = &Groups {
            names: &group.names,
            bounds: &group.bounds,
            outer: Some(groups),
        };
        let unsupported = Elaboration::Unsupported {
            site: Site::of(head),
        };
        let ExpressionPart::QuotedExpression(run) = head else {
            return Err(unsupported);
        };
        let run = run.reference();
        let mut elements = BumpVec::with_capacity_in(run.parts.len(), self.scratch);
        let mut ranks = BumpVec::with_capacity_in(run.parts.len() / 2, self.scratch);
        walk_head(run, unsupported, |element| {
            elements.push(match element {
                HeadElement::Keyword(symbol) => DispatchTokenElement::Keyword(symbol),
                HeadElement::Slot(label, slot) => {
                    ranks.push(label.rank());
                    DispatchTokenElement::Slot(self.part(slot, own)?)
                }
            });
            Ok(())
        })?;
        let classes = dense_classes(self.scratch, &ranks);
        if !group.names.is_empty() {
            let slots = elements.iter().filter_map(|element| match element {
                DispatchTokenElement::Slot(slot) => Some(*slot),
                DispatchTokenElement::Keyword(_) => None,
            });
            self.untied(Site::of(head), (&group.names, &group.bounds), slots)?;
        }
        let ret = self.part(ret, own)?;
        Ok(self
            .types
            .shape_scheme(
                self.scratch,
                &group.names,
                &group.bounds,
                &elements,
                classes,
                ret,
            )
            .handle)
    }

    /// Refuse a binder at `site` one of whose `positions`, read under its own group of `names`
    /// bounded by `bounds`, holds a union two of whose members tie ([`tied_members`]).
    pub(super) fn untied(
        &self,
        site: Site,
        (names, bounds): (&[TypeSymbol], &[KType]),
        positions: impl IntoIterator<Item = Parametric>,
    ) -> Result<(), Elaboration> {
        for position in positions {
            if let Some(members) = tied_members(self.types, self.scratch, position, names, bounds) {
                return Err(Elaboration::TiedUnion { members, site });
            }
        }
        Ok(())
    }

    /// Each `<name> :<Type>` pair of a field list, its type elaborated; a `_` pair or anything
    /// that is not a pair is unsupported.
    fn pairs(
        &self,
        site: Site,
        run: &KExpression<'graph>,
        groups: &Groups<'_>,
        mut field: impl FnMut(crate::symbols::BinderSymbol, Parametric) -> Result<(), Elaboration>,
    ) -> Result<(), Elaboration> {
        let mut index = 0;
        while index < run.parts.len() {
            let Some(SlotLabel::Named(name)) = pair_label(run, index) else {
                return Err(Elaboration::Unsupported { site });
            };
            field(name, self.part(&run.parts[index + 1].value, groups)?)?;
            index += 2;
        }
        Ok(())
    }
}

/// One element of an `EXPR` head's run.
#[derive(Clone, Copy)]
pub(super) enum HeadElement<'p, 'graph> {
    Keyword(KeywordSymbol),
    /// A `<label> :<Type>` pair: its label — a name, `_`, or a rank — and its type part.
    Slot(SlotLabel, &'p ExpressionPart<'graph>),
}

/// Hand each keyword and `<label> :<Type>` pair of an `EXPR` head's `run` to `each`, in written
/// order, or fail with `malformed` at the first part that is neither. The one walk every reader of
/// a head shares: its shape, its function type, and the shape a definition registers.
pub(super) fn walk_head<'p, 'graph, E>(
    run: &'p KExpression<'graph>,
    malformed: E,
    mut each: impl FnMut(HeadElement<'p, 'graph>) -> Result<(), E>,
) -> Result<(), E> {
    let mut index = 0;
    while index < run.parts.len() {
        match (&run.parts[index].value, pair_label(run, index)) {
            (_, Some(name)) => {
                each(HeadElement::Slot(name, &run.parts[index + 1].value))?;
                index += 2;
            }
            (ExpressionPart::Keyword(symbol), None) => {
                each(HeadElement::Keyword(*symbol))?;
                index += 1;
            }
            _ => return Err(malformed),
        }
    }
    Ok(())
}

/// The type the record under `owner` declares `name` with, read through every newtype layer above
/// it — a `NEWTYPE`'s representation, a union variant's payload. `None` when no record lies under
/// `owner` or it declares no `name`; a ring of newtypes with no record under it, `NEWTYPE Loop =
/// Loop`, is peeled once round and then refused.
pub fn declared_field(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    owner: KType,
    name: Symbol,
) -> Option<KType> {
    let mut peeled = BumpVec::new_in(scratch);
    let mut layer = owner;
    loop {
        match types.node(layer) {
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
