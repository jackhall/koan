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

use super::judge;
use super::reads::{Reads, TypeAt};
use crate::memory::{BumpAllocator, BumpVec};
use crate::parse::{BuiltinShapeId, KEYWORDS};
use crate::parse::{ExpressionPart, KExpression};
use crate::parse::{SlotLabel, needed_entry, needing, quantifier_entries};
use crate::scope::{
    Coordinate, Elaboration, PathSpelling, Rule, Site, Slot, SpelledRoot, Spelling, Target, WIDEST,
    Width, applied, declared_member, declared_variant, head_params, pair_label, path_spelling,
    unbuilt, variant_named,
};
use crate::symbols::{BinderSymbol, KeywordSymbol, StaticName, TypeSymbol};
use crate::type_lattice::{
    DeclaredType, DispatchTokenElement, GroupIntern, KType, Parametric, SigOrigin, Step, TypeNode,
    dense_classes, member, tied_members,
};
use crate::values::Chain;

/// The head of a spelling over a head: closed, or a type declared over run-bound names.
enum Head {
    Closed(KType),
    Declared,
}

/// Why a type whose names are all closed is concrete.
const CLOSED: &str = "a type holding no variable is concrete";

/// The connector keywords of the formless composites.
struct Connectors {
    list: StaticName<KeywordSymbol>,
    of: StaticName<KeywordSymbol>,
    map: StaticName<KeywordSymbol>,
    union: StaticName<KeywordSymbol>,
    meet: StaticName<KeywordSymbol>,
}

static CONNECTORS: Connectors = Connectors {
    list: crate::static_name!(KeywordSymbol, "LIST"),
    of: crate::static_name!(KeywordSymbol, "OF"),
    map: crate::static_name!(KeywordSymbol, "MAP"),
    union: crate::static_name!(KeywordSymbol, "|"),
    meet: crate::static_name!(KeywordSymbol, "&"),
};

/// `part` as a type, its names read through `reader`: every name is the mention `reader`'s shape
/// recorded at its site, which must read as a type, save one a `FOR ALL` group inside `part`
/// declares.
pub fn type_expression<'graph, R: Reads<'graph> + ?Sized>(
    part: &ExpressionPart<'graph>,
    reader: &R,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> Result<Parametric, Elaboration> {
    #[cfg(test)]
    super::elaborated();
    let groups = Groups {
        names: &[],
        bounds: &[],
        outer: None,
    };
    Elaborator {
        reader,
        types,
        scratch,
        fellows: Fellows::NONE,
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
/// `Any` for a name written without one — beside each name whose bound a callable's own group
/// writes over an encloser's names, with that bound as written: its laid bound is the bound's
/// upper end at every binding, and the run writes the binding's content in where the callable is
/// born.
pub(super) struct QuantifierGroup<'x> {
    pub(super) names: BumpVec<'x, TypeSymbol>,
    pub(super) bounds: BumpVec<'x, KType>,
    pub(super) related: BumpVec<'x, (TypeSymbol, Parametric)>,
}

impl<'x> QuantifierGroup<'x> {
    /// The group an unquantified callable carries.
    pub(super) fn empty(scratch: BumpAllocator<'x>) -> Self {
        QuantifierGroup {
            names: BumpVec::new_in(scratch),
            bounds: BumpVec::new_in(scratch),
            related: BumpVec::new_in(scratch),
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
#[derive(Clone, Copy)]
pub(super) struct Fellow<'f> {
    pub(super) slot: Slot,
    pub(super) handle: Parametric,
    pub(super) params: &'f [TypeSymbol],
}

/// A component's signature or alias fellow not yet read, by its slot: `Some` with its relative
/// type, read on first mention, or the refusal of a cycle through signatures and aliases alone;
/// `None` for a slot that names no such fellow.
pub(super) type Unread<'e> = &'e dyn Fn(Slot) -> Option<Result<Parametric, Elaboration>>;

/// The fellow members of the component being declared, as a type expression in it reads them.
#[derive(Clone, Copy)]
pub(super) struct Fellows<'e> {
    /// Each fellow at the relative handle it is named by until the group seals: a nominal member's
    /// own sibling, a `UNION` binder's union of its variants' siblings, and a signature's or
    /// alias's relative type once it is read.
    pub(super) read: &'e [Fellow<'e>],
    /// The signature and alias fellows not yet read. `None` once every fellow is read, and outside
    /// a component.
    pub(super) unread: Option<Unread<'e>>,
}

impl Fellows<'_> {
    /// No fellow: an ordinary type expression, or a member declared alone.
    pub(super) const NONE: Fellows<'static> = Fellows {
        read: &[],
        unread: None,
    };
}

/// What elaborating one expression reads through.
pub(super) struct Elaborator<'e, 'run, 'p, 'x, 'reg, R: ?Sized> {
    pub(super) reader: &'e R,
    pub(super) types: &'e Chain<'run, 'p, 'reg, 'e>,
    pub(super) scratch: BumpAllocator<'x>,
    /// Fellow members of the component being declared, each named by its relative handle until the
    /// group seals. [`Fellows::NONE`] for an ordinary type expression.
    pub(super) fellows: Fellows<'e>,
    /// Names declared inside the definition being elaborated and holding no slot of the enclosing
    /// shape — a `SIG`'s head parameters and manifest members, and a higher-kinded declarator's
    /// parameters. Empty outside a definition.
    pub(super) locals: &'e [(TypeSymbol, Parametric)],
    /// Whether the next composite node may be a quantified function type or expression shape: set
    /// for one signature member's type, and cleared by the first composite the elaborator reaches,
    /// so a quantified type nested inside one is refused.
    pub(super) binder: Cell<bool>,
}

impl<'graph, 'x, R: Reads<'graph> + ?Sized> Elaborator<'_, '_, '_, 'x, '_, R> {
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
                site: self.reader.shape().site(part),
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
        let site = self.reader.shape().site(part);
        match part {
            ExpressionPart::Type(name) => self.name(site, *name, groups).map(DeclaredType::Type),
            // A marked name resolves past the definition's own quantifiers and names, through the
            // mention the shape recorded.
            ExpressionPart::MarkedName(_, BinderSymbol::Type(name)) => {
                self.mention(site, *name).map(DeclaredType::Type)
            }
            ExpressionPart::Expression(node) | ExpressionPart::SigiledTypeExpr(node) => {
                self.node_declared(site, node, groups)
            }
            ExpressionPart::RecordType(node) => {
                self.binder.set(false);
                let mut fields = BumpVec::new_in(self.scratch);
                self.pairs(site, node, groups, |name, ktype| {
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
        // relative handle its still-open window minted, or, for a signature or alias not read yet,
        // by its relative type, read now. A definition part opens no nested shape, so every fellow
        // mention is local to the declaring shape.
        if let Coordinate::Activation {
            hops: 0,
            target: Target::Local(slot),
        } = mention.coordinate
        {
            if let Some(fellow) = self.fellows.read.iter().find(|fellow| fellow.slot == slot) {
                return Ok(fellow.handle);
            }
            if let Some(answer) = self.fellows.unread.and_then(|unread| unread(slot)) {
                return answer;
            }
        }
        match self.reader.type_at(mention.coordinate) {
            TypeAt::Type(handle) => Ok(handle.into()),
            TypeAt::Rigid(handle) => Ok(handle),
            TypeAt::NotAType => Err(Elaboration::NotAType { name, site }),
            TypeAt::Unknown(unwalked) => Err(Elaboration::Unknown { site, unwalked }),
        }
    }

    /// The member read `node`, at `site`, where it is a **path** by its tokens ([`path_spelling`]):
    /// a type member read through a value name holding a module, a chain of value members rooted at
    /// one, or such a chain rooted at an expression written in place — as the reader reads the path.
    /// `None` where `node` is no path, or its root name has no mention, which leaves the projection
    /// to a type's own members.
    pub(super) fn module_member(
        &self,
        site: Site,
        node: &KExpression<'graph>,
    ) -> Option<Result<Parametric, Elaboration>> {
        let PathSpelling { root, chain, label } = path_spelling(node, self.scratch)?;
        match root {
            SpelledRoot::Inline(root) => {
                let read =
                    self.reader
                        .inline_member(site, self.reader.shape().site(root), &chain, label);
                Some(match read {
                    Ok(read) => path_type(site, label, read, KType::EMPTY_SIGNATURE),
                    Err(path) => Err(Elaboration::Unresolved { site, path }),
                })
            }
            SpelledRoot::Name(root) => {
                let mention = self
                    .reader
                    .shape()
                    .mention(self.reader.shape().site(root))?;
                let read =
                    self.reader
                        .member_type(mention.coordinate, site, &chain, label, self.types);
                Some(match read {
                    Ok(read) => {
                        let (read, owner) = read?;
                        path_type(site, label, read, owner)
                    }
                    Err(path) => Err(Elaboration::Unresolved { site, path }),
                })
            }
        }
    }

    /// The head `head` of a spelling over a head, at `site`: closed, or a type declared over
    /// run-bound names, whose declaration licenses the spelling. A run-bound name, a path's
    /// variable among them, is no head at any binding, since the type a value carries is always
    /// applied ([`Spelling::Head`]); `refused` where it names a `FOR ALL` variable or a head
    /// parameter, which no head spelling takes.
    fn head(
        &self,
        site: Site,
        head: Parametric,
        refused: Elaboration,
    ) -> Result<Head, Elaboration> {
        match judge::names_held(self.types, self.scratch, head) {
            judge::Names::Closed => Ok(Head::Closed(
                judge::static_of(self.types, head).expect(CLOSED),
            )),
            judge::Names::RunBound | judge::Names::Path => {
                match judge::declared(self.reader.marks(), self.types, head) {
                    Some(_) => Ok(Head::Declared),
                    None => Err(self.unlicensed(site, head, Spelling::Head)),
                }
            }
            judge::Names::Unbound => Err(refused),
        }
    }

    /// The closed head `head`, refused `refused` where it is a declaration over names: a spelling
    /// no declaration over names takes, a code kind's.
    fn closed_head(
        &self,
        site: Site,
        head: Parametric,
        refused: Elaboration,
    ) -> Result<KType, Elaboration> {
        match self.head(site, head, refused)? {
            Head::Closed(closed) => Ok(closed),
            Head::Declared => Err(refused),
        }
    }

    /// `rule`'s output over `operands`, a type declared over run-bound names among them whose
    /// declaration licenses the spelling: the derived name a binding reads it through. `refused`
    /// where it has none; `Unknown` outside a walk, where no binding is written in.
    fn ruled(
        &self,
        site: Site,
        rule: Rule,
        operands: &[DeclaredType<Parametric>],
        refused: Elaboration,
    ) -> Result<Parametric, Elaboration> {
        match judge::ruled_one(
            self.reader.marks(),
            self.types,
            self.scratch,
            rule,
            operands,
        ) {
            Ok(Some(ruled)) => Ok(ruled),
            Ok(None) => Err(refused),
            Err(unwalked) => Err(Elaboration::Unknown {
                site,
                unwalked: unwalked.into(),
            }),
        }
    }

    /// The record type over `named`, by name: the arguments or pins a spelling's rule reads.
    fn named(&self, named: &[(BinderSymbol, Parametric)]) -> DeclaredType<Parametric> {
        DeclaredType::Type(self.types.record(self.scratch, named))
    }

    /// The refusal of `spelling` over the run-bound `name` at `site`, which its bound does not
    /// license at every binding.
    fn unlicensed(&self, site: Site, name: Parametric, spelling: Spelling) -> Elaboration {
        Elaboration::Unlicensed {
            name,
            written: self.reader.written_bound(name, self.types),
            spelling,
            site,
        }
    }

    /// The member `name` of `owner`, written `Owner.name`, at `site`. A closed owner's is a variant
    /// of the union under it, else a field of the record under it, else, over a union of record
    /// types, the union of their fields. An owner over run-bound names reads a variant its text
    /// shows — a family applied at a name, or a union declared over one, whose declaration
    /// licenses the read — else it is the lattice's projection, where the owner's bound
    /// guarantees the field; elsewhere it is refused, as a variant read off a run-bound name always
    /// is, since a binding under a union may be a lone variant. `refused` where `owner` names a
    /// `FOR ALL` variable or a head parameter, which declares no member.
    fn member(
        &self,
        site: Site,
        owner: Parametric,
        name: BinderSymbol,
        refused: Elaboration,
    ) -> Result<Parametric, Elaboration> {
        let (types, scratch) = (self.types, self.scratch);
        let field = Step::Field(name);
        match judge::names_held(types, scratch, owner) {
            judge::Names::Closed => {
                let closed = judge::static_of(types, owner).expect(CLOSED);
                (declared_member(types, scratch, closed, name.symbol()).map(Parametric::from))
                    .or_else(|| types.projection(scratch, owner, field))
                    .ok_or(refused)
            }
            judge::Names::RunBound | judge::Names::Path => {
                if let Some(variant) = variant_named(types, owner, name.symbol()) {
                    return Ok(variant);
                }
                if let Some(declared) = judge::declared(self.reader.marks(), types, owner) {
                    if !declared.shows_member(types, scratch, name) {
                        return Err(refused);
                    }
                    let operands = [DeclaredType::Type(owner)];
                    return self.ruled(site, Rule::Member(name.symbol()), &operands, refused);
                }
                types.projection(scratch, owner, field).ok_or_else(|| {
                    let bound = judge::named_bound(types, scratch, owner);
                    let spelling = match declared_variant(types, scratch, bound, name.symbol()) {
                        Some(_) => Spelling::Variant(name.symbol()),
                        None => Spelling::Field(name.symbol()),
                    };
                    self.unlicensed(site, owner, spelling)
                })
            }
            judge::Names::Unbound => Err(refused),
        }
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
            let head = self.part(&head.value, groups)?;
            let constructor = self.head(site, head, unsupported)?;
            let mut arguments_typed = BumpVec::with_capacity_in(arguments.len(), self.scratch);
            for (name, argument) in arguments.iter() {
                arguments_typed.push((*name, self.part(argument, groups)?));
            }
            let applied = match constructor {
                Head::Closed(constructor) => self.apply(site, constructor, &arguments_typed)?,
                Head::Declared => {
                    // The declaration names its parameters, each once.
                    let declared = judge::declared(self.reader.marks(), self.types, head);
                    let params = declared.as_ref().and_then(judge::Declared::params);
                    let params = params.ok_or(unsupported)?;
                    let each_once = arguments_typed.len() == params.len()
                        && (arguments_typed.iter()).all(|(name, _)| {
                            params
                                .iter()
                                .any(|param| BinderSymbol::Type(*param) == *name)
                        });
                    if !each_once {
                        return Err(unsupported);
                    }
                    let operands = [head.into(), self.named(&arguments_typed)];
                    self.ruled(site, Rule::Applied { sole: false }, &operands, unsupported)?
                }
            };
            return Ok(DeclaredType::Type(applied));
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
                    let group = self.group(part(3), groups, false)?;
                    Ok(self.function(&group, part(4), part(6), groups)?.handle)
                }
                BuiltinShapeId::ExpressionHead => {
                    let group = QuantifierGroup::empty(self.scratch);
                    self.shape(&group, part(1), part(3), groups)
                }
                BuiltinShapeId::QuantifiedExpressionHead => {
                    let group = self.group(part(3), groups, false)?;
                    self.shape(&group, part(4), part(6), groups)
                }
                BuiltinShapeId::Attribute => {
                    if let Some(member) = self.module_member(site, node) {
                        return member.map(DeclaredType::Type);
                    }
                    let owner = self.part(part(1), groups)?;
                    let name = match part(2) {
                        ExpressionPart::Type(name) => BinderSymbol::Type(*name),
                        ExpressionPart::Identifier(name) => BinderSymbol::Value(*name),
                        _ => return Err(unsupported),
                    };
                    let refused = Elaboration::NoSuchMember {
                        owner,
                        name: name.symbol(),
                        site,
                    };
                    Ok(DeclaredType::Type(self.member(site, owner, name, refused)?))
                }
                _ => Err(unsupported),
            };
        }
        self.composite(site, node, groups).map(DeclaredType::Type)
    }

    /// A composite of the formless connectors: a list, a dict, a union, a meet, an application, a
    /// code kind needing names or unbuilt, or a pinned signature.
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
            // `Type -> Ctor` — the arity-one sugar for the application above. A function type
            // starts with `FN`, so a bare `->` here is always an application.
            3 if keyword(1, &KEYWORDS.arrow) => {
                let head = self.part(&parts[2].value, groups)?;
                let constructor = match self.head(site, head, unsupported)? {
                    Head::Closed(constructor) => constructor,
                    Head::Declared => {
                        let declared = judge::declared(self.reader.marks(), self.types, head);
                        let params = declared.as_ref().and_then(judge::Declared::params);
                        let [_] = params.ok_or(unsupported)? else {
                            return Err(unsupported);
                        };
                        let argument = self.part(&parts[0].value, groups)?;
                        let operands = [head.into(), argument.into()];
                        let rule = Rule::Applied { sole: true };
                        return self.ruled(site, rule, &operands, unsupported);
                    }
                };
                let [param] = self.param_names(constructor).ok_or(unsupported)? else {
                    return Err(unsupported);
                };
                let argument = self.part(&parts[0].value, groups)?;
                self.apply(site, constructor, &[(BinderSymbol::Type(*param), argument)])
            }
            // `Kind NEEDING #[y …]` — a code kind below `Code`, and a list of quotes each of one
            // name or of a bucket key.
            3 if let Some((kind, quotes)) = needing(node) => {
                let head = self.part(kind, groups)?;
                let kind = self.closed_head(site, head, unsupported)?;
                if kind.code_parent().is_none() {
                    return Err(unsupported);
                }
                let mut names = BumpVec::with_capacity_in(quotes.len(), self.scratch);
                for quote in quotes.iter() {
                    names.push(
                        needed_entry(quote.as_part().ok_or(unsupported)?).ok_or(unsupported)?,
                    );
                }
                Ok(
                    crate::scope::needing(self.types, self.scratch, kind, &names)
                        .ok_or(unsupported)?
                        .into(),
                )
            }
            // `UNBUILT Kind` — `Code` or a code kind below it, whatever its code needs.
            2 if keyword(0, &KEYWORDS.unbuilt) => {
                let head = self.part(&parts[1].value, groups)?;
                let kind = self.closed_head(site, head, unsupported)?;
                Ok(unbuilt(self.types, kind).ok_or(unsupported)?.into())
            }
            // `Sig WITH {Param = Type, …}` — a declared signature with some head parameters pinned.
            3 if keyword(1, &KEYWORDS.with)
                && let ExpressionPart::RecordLiteral(pins) = &parts[2].value =>
            {
                let head = self.part(&parts[0].value, groups)?;
                let signature = match self.head(site, head, unsupported)? {
                    Head::Closed(signature) => signature,
                    Head::Declared => {
                        let declared = judge::declared(self.reader.marks(), self.types, head);
                        let Some(judge::Declared::Signature(signed)) = declared else {
                            return Err(unsupported);
                        };
                        let mut pinned = BumpVec::with_capacity_in(pins.len(), self.scratch);
                        for (name, pin) in pins.iter() {
                            let BinderSymbol::Type(parameter) = name else {
                                return Err(unsupported);
                            };
                            if !signed.declares_parameter(*parameter) {
                                return Err(unsupported);
                            }
                            pinned.push((*name, self.part(pin, groups)?));
                        }
                        let operands = [head.into(), self.named(&pinned)];
                        return self.ruled(site, Rule::Pinned, &operands, unsupported);
                    }
                };
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
                    members.push(self.part(member.as_part().ok_or(unsupported)?, groups)?);
                }
                Ok(self.types.union_of(self.scratch, &members))
            }
            // `A & B`, and `& [A B C]`, its chained form — the meet, as a union is the join. A meet
            // that comes out `Never` is a type like any other; only a bound refuses it. An operand
            // naming a `FOR ALL` variable or a head parameter is refused: each call solves the
            // variable, so the meet cannot be taken here.
            3 if keyword(1, &CONNECTORS.meet) => {
                let operands = [
                    self.part(&parts[0].value, groups)?,
                    self.part(&parts[2].value, groups)?,
                ];
                self.meet(self.reader.shape().site(&parts[1].value), &operands)
            }
            2 if keyword(0, &CONNECTORS.meet) => {
                let ExpressionPart::ListLiteral(written) = parts[1].value else {
                    return Err(unsupported);
                };
                let mut operands = BumpVec::with_capacity_in(written.len(), self.scratch);
                for operand in written.iter() {
                    let operand = operand.as_part().ok_or(unsupported)?;
                    operands.push(self.part(operand, groups)?);
                }
                self.meet(self.reader.shape().site(&parts[0].value), &operands)
            }
            _ => Err(unsupported),
        }
    }

    /// The meet of `operands`, refused at `site` — the `&` — when one names a `FOR ALL` variable or
    /// a head parameter: the meet relates concrete types only, and each call solves the variable.
    /// Each operand past the first is met through the rule ([`Rule::Meet`](crate::scope::Rule::Meet)):
    /// closed operands meet, and over a run-bound name the meet is a derived name whose term meets
    /// what the binding gives each: `Elt & Number` is `Number` where `Elt` is `Number`, and a path's
    /// variable is met so, a binding closing it by module substitution. One over names outside a
    /// walk, where no binding is written in, is left for the run.
    fn meet(&self, site: Site, operands: &[Parametric]) -> Result<Parametric, Elaboration> {
        use judge::Names;
        let mut met: Parametric = KType::ANY.into();
        for operand in operands {
            match judge::names_held(self.types, self.scratch, *operand) {
                Names::Unbound => return Err(Elaboration::MeetOverVariable { site }),
                Names::Closed | Names::RunBound | Names::Path => {}
            }
            met = judge::met(self.reader.marks(), self.types, self.scratch, met, *operand)
                .map_err(|unwalked| Elaboration::Unknown {
                    site,
                    unwalked: unwalked.into(),
                })?;
        }
        Ok(met)
    }

    /// A `FOR ALL` group's names and bounds, in written order: a list of name quotes, or a dict of
    /// name quotes to bound quotes; an entry naming no lone type is unsupported. Every bound is
    /// read under the group with no bounds of its own, so a bound naming one of the group's names
    /// is refused. A callable's own group (`own`) may bound a name over an encloser's names
    /// ([`bound`](Self::bound)); a type's may not.
    pub(super) fn group(
        &self,
        part: &ExpressionPart<'graph>,
        groups: &Groups<'_>,
        own: bool,
    ) -> Result<QuantifierGroup<'x>, Elaboration> {
        let mut group = QuantifierGroup::empty(self.scratch);
        let mut written = BumpVec::new_in(self.scratch);
        for entry in quantifier_entries(part) {
            let name = entry.name.ok_or(Elaboration::Unsupported {
                site: self.reader.shape().site(entry.written),
            })?;
            group.names.push(name);
            written.push(entry.bound);
        }
        let unbounded = Groups {
            names: &group.names,
            bounds: &[],
            outer: Some(groups),
        };
        for (name, bound) in group.names.iter().zip(written) {
            group.bounds.push(match bound {
                Some(part) => {
                    let (laid, over_names) = self.bound(part, &unbounded, own)?;
                    if let Some(written) = over_names {
                        group.related.push((*name, written));
                    }
                    laid
                }
                None => KType::ANY,
            });
        }
        Ok(group)
    }

    /// A bound: a closed, inhabited type, beside the bound as written where it reads an encloser's
    /// run-bound names. A callable's own group (`own`) lays such a bound at its upper end at every
    /// binding (`bound_above`, never read as the names' bounds, which a contravariant position
    /// would put below an instance), and the run writes the binding's content in where the
    /// callable is born; inside a type it is refused
    /// ([`BoundOverRunBound`](Elaboration::BoundOverRunBound)), since no type written over names
    /// holds a scheme whose bound a binding decides. One naming any other type variable — a `FOR
    /// ALL` name or a signature's head parameter — or that is `Never` is refused at its site.
    pub(super) fn bound(
        &self,
        part: &ExpressionPart<'graph>,
        groups: &Groups<'_>,
        own: bool,
    ) -> Result<(KType, Option<Parametric>), Elaboration> {
        let site = self.reader.shape().site(part);
        let refused = Elaboration::Bound { site };
        let before = self.reader.rigid_reads();
        let written = self.part(part, groups)?;
        let over_names = self.reader.rigid_reads() > before;
        if over_names && !own {
            return Err(Elaboration::BoundOverRunBound { site });
        }
        let laid = match over_names {
            true if judge::names_held(self.types, self.scratch, written)
                == judge::Names::Unbound =>
            {
                return Err(refused);
            }
            true => judge::upper_end(self.types, self.scratch, written).into(),
            false => written,
        };
        // An opaque carrier is concrete, but a variable's bound holds none.
        match judge::static_of(self.types, laid) {
            Some(bound) if bound != KType::NEVER && !self.types.holds_carrier(bound) => {
                Ok((bound, over_names.then_some(written)))
            }
            _ => Err(refused),
        }
    }

    /// The parameter names a constructor head takes: a fellow family's while its group is open, a
    /// declared family's, or — for a union every member of
    /// which takes one parameter set — that set, since applying the union applies each member.
    fn param_names(&self, constructor: KType) -> Option<&[TypeSymbol]> {
        if let Some(fellow) = self
            .fellows
            .read
            .iter()
            .find(|fellow| fellow.handle == constructor.into() && !fellow.params.is_empty())
        {
            return Some(fellow.params);
        }
        head_params(self.types, constructor)
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
        applied(self.types, self.scratch, constructor, declared, arguments).ok_or(unsupported)
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
        let TypeNode::Signature { schema, .. } = judge::closed_type_node(self.types, signature)
        else {
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
        crate::scope::pinned(self.types, self.scratch, signature, &pinned).ok_or(unsupported)
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
                site: self.reader.shape().site(schema),
            });
        };
        let mut params = BumpVec::new_in(self.scratch);
        self.pairs(
            self.reader.shape().site(schema),
            fields,
            groups,
            |name, ktype| {
                params.push((name, ktype));
                Ok(())
            },
        )?;
        if !group.names.is_empty() {
            let site = self.reader.shape().site(schema);
            storable(site, group.names.len(), (Width::Parameters, params.len()))?;
            let params = params.iter().map(|(_, param)| *param);
            self.untied(site, (&group.names, &group.bounds), params)?;
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
            site: self.reader.shape().site(head),
        };
        let ExpressionPart::QuotedExpression(run) = head else {
            return Err(unsupported);
        };
        let mut params = BumpVec::with_capacity_in(run.parts.len() / 2, self.scratch);
        walk_head(run, unsupported, |element| {
            if let HeadElement::Slot(label, slot) = element {
                params.push((label.name().ok_or(unsupported)?, self.part(slot, groups)?));
            }
            Ok(())
        })?;
        if !group.names.is_empty() {
            let site = self.reader.shape().site(head);
            // Counted as the head's slots, as its author wrote them.
            storable(site, group.names.len(), (Width::Slots, params.len()))?;
            let params = params.iter().map(|(_, param)| *param);
            self.untied(site, (&group.names, &group.bounds), params)?;
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
            site: self.reader.shape().site(head),
        };
        let ExpressionPart::QuotedExpression(run) = head else {
            return Err(unsupported);
        };
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
            let site = self.reader.shape().site(head);
            storable(site, group.names.len(), (Width::Slots, ranks.len()))?;
            let slots = elements.iter().filter_map(|element| match element {
                DispatchTokenElement::Slot(slot) => Some(*slot),
                DispatchTokenElement::Keyword(_) => None,
            });
            self.untied(site, (&group.names, &group.bounds), slots)?;
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

/// What a path spelled at `site` reads, as a type: `NotAType` is a member `owner`, the module's
/// signature, does not declare.
fn path_type(
    site: Site,
    name: TypeSymbol,
    read: TypeAt,
    owner: KType,
) -> Result<Parametric, Elaboration> {
    match read {
        TypeAt::Type(held) => Ok(held.into()),
        TypeAt::Rigid(held) => Ok(held),
        TypeAt::Unknown(unwalked) => Err(Elaboration::Unknown { site, unwalked }),
        TypeAt::NotAType => Err(Elaboration::NoSuchMember {
            owner: owner.into(),
            name: name.symbol(),
            site,
        }),
    }
}

/// Refuse a quantified function type or expression shape at `site` over `names` `FOR ALL` names
/// and `positions` parameters or slots where either is past [`WIDEST`]: a stored solve names each
/// by a bit, and a call by name reaches the same stored solve as a keyworded use. The limit is the
/// representation's, not the language's.
fn storable(
    site: Site,
    names: usize,
    (width, positions): (Width, usize),
) -> Result<(), Elaboration> {
    let (width, count) = match names > WIDEST {
        true => (Width::Names, names),
        false => (width, positions),
    };
    match count > WIDEST {
        true => Err(Elaboration::TooWide { width, count, site }),
        false => Ok(()),
    }
}
