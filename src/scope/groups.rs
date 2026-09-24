//! **Operator groups**: which operators chain together, how a run of them reduces, and where a
//! body may see one.
//!
//! A group is a set of operator symbols under one [`ReductionMode`], and its identity is its
//! content — [`DeclaredGroup`] is the record, borrowed from the lattice so a `SIG`'s operator
//! channel and a body's held group are the same type and compare with `==`. Two declarations of an
//! equal member set under an equal mode are one group.
//!
//! Three things decide how a symbol chains, in order:
//!
//! 1. a [`BuiltinGroup`] covering it — visible everywhere, overridden by nothing;
//! 2. the [`Claim`] the program's `GROUP` and `UNARY OP` statements make over it, collected by a
//!    position-blind pre-scan ([`Claims`]) of the whole code being built;
//! 3. nothing: the symbol chains fold-left, alone.
//!
//! A claim says how a symbol chains, not where an operator run of it may be written. That is the
//! [`GroupFrame`] chain: a `GROUP`'s own body holds its group, a `USING … SCOPE` body holds the
//! groups its operand surfaces, and an operator run over a claimed symbol whose group no enclosing
//! frame holds is refused rather than chained by default.
//!
//! `==` and `!=` belong to no group and are claimed by nothing: they join whichever pairwise group
//! the rest of an operator run chains under, and alone they fold pairwise through `AND`.
//!
//! See [README.md § Operator groups](README.md#operator-groups).

use crate::memory::{BumpAllocator, BumpVec, ProgramBrand, collect, resident};
use crate::parse::builtin_shapes::BuiltinShapeId;
use crate::parse::builtin_shapes::binder::{OpArity, op_declaration_arity, symbol_from_quote_body};
use crate::parse::builtin_shapes::role::{BodyKind, DefinitionKind, Role};
use crate::parse::{ExpressionPart, KExpression};
use crate::symbols::{KeywordSymbol, StaticName};
use crate::type_lattice::{DeclaredGroup, FoldDirection, ReductionMode};

use super::shape::{Position, ShapeError};

/// The operator symbols the builtin groups and the rewrite are spelled with, each declared once and
/// minted once — the same discipline the surface keywords keep.
struct OperatorSymbols {
    less: StaticName<KeywordSymbol>,
    less_equal: StaticName<KeywordSymbol>,
    greater: StaticName<KeywordSymbol>,
    greater_equal: StaticName<KeywordSymbol>,
    plus: StaticName<KeywordSymbol>,
    minus: StaticName<KeywordSymbol>,
    times: StaticName<KeywordSymbol>,
    divide: StaticName<KeywordSymbol>,
    union: StaticName<KeywordSymbol>,
    meet: StaticName<KeywordSymbol>,
    /// The combiner a comparison run and a bare equality run fold their pairs through.
    and: StaticName<KeywordSymbol>,
    /// What the rewrite of `a != b` negates through.
    not: StaticName<KeywordSymbol>,
    equal: StaticName<KeywordSymbol>,
    unequal: StaticName<KeywordSymbol>,
}

static OPERATORS: OperatorSymbols = OperatorSymbols {
    less: crate::static_name!(KeywordSymbol, "<"),
    less_equal: crate::static_name!(KeywordSymbol, "<="),
    greater: crate::static_name!(KeywordSymbol, ">"),
    greater_equal: crate::static_name!(KeywordSymbol, ">="),
    plus: crate::static_name!(KeywordSymbol, "+"),
    minus: crate::static_name!(KeywordSymbol, "-"),
    times: crate::static_name!(KeywordSymbol, "*"),
    divide: crate::static_name!(KeywordSymbol, "/"),
    union: crate::static_name!(KeywordSymbol, "|"),
    meet: crate::static_name!(KeywordSymbol, "&"),
    and: crate::static_name!(KeywordSymbol, "AND"),
    not: crate::static_name!(KeywordSymbol, "NOT"),
    equal: crate::static_name!(KeywordSymbol, "=="),
    unequal: crate::static_name!(KeywordSymbol, "!="),
};

/// The members of each builtin group, as the names they are declared under.
static COMPARISON: &[&StaticName<KeywordSymbol>] = &[
    &OPERATORS.less,
    &OPERATORS.less_equal,
    &OPERATORS.greater,
    &OPERATORS.greater_equal,
];
static ADDITIVE: &[&StaticName<KeywordSymbol>] = &[&OPERATORS.plus, &OPERATORS.minus];
static MULTIPLICATIVE: &[&StaticName<KeywordSymbol>] = &[&OPERATORS.times, &OPERATORS.divide];
static UNION: &[&StaticName<KeywordSymbol>] = &[&OPERATORS.union];
static MEET: &[&StaticName<KeywordSymbol>] = &[&OPERATORS.meet];

/// The negation `a != b` is rewritten through.
pub(crate) fn not_symbol() -> KeywordSymbol {
    OPERATORS.not.symbol()
}

pub(crate) fn equal_symbol() -> KeywordSymbol {
    OPERATORS.equal.symbol()
}

/// Whether `symbol` is `==` or `!=` — the two symbols that belong to no group, take `Any`, and join
/// whichever pairwise group the rest of an operator run chains under.
pub fn is_equality(symbol: KeywordSymbol) -> bool {
    symbol == OPERATORS.equal.symbol() || symbol == OPERATORS.unequal.symbol()
}

/// Whether `symbol` is `==`, the one equality symbol a program declares over its own types. Its
/// result is always `Bool`: `!=` is its negation by construction, so an `==` answering anything
/// else would leave `!=` nothing to negate.
pub fn is_equal(symbol: KeywordSymbol) -> bool {
    symbol == OPERATORS.equal.symbol()
}

/// Whether `symbol` is `!=`, which no declaration may name: the builder rewrites every infix
/// `a != b` as `NOT (a == b)`, so it never reaches dispatch and is opposite by construction.
pub fn is_unequal(symbol: KeywordSymbol) -> bool {
    symbol == OPERATORS.unequal.symbol()
}

/// The five groups the language itself declares. They cover their members everywhere, no
/// declaration overrides one, and a `GROUP` equal to one of them declares nothing new.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BuiltinGroup {
    /// `{< <= > >=}`, pairwise through `AND`, folding left.
    Comparison,
    /// `{+ -}`, folding left.
    Additive,
    /// `{* /}`, folding left.
    Multiplicative,
    /// `{|}`, unary — a union type's operator run.
    Union,
    /// `{&}`, unary — a meet type's operator run.
    Meet,
}

impl BuiltinGroup {
    /// Every builtin group, in declaration order.
    const ALL: [BuiltinGroup; 5] = [
        BuiltinGroup::Comparison,
        BuiltinGroup::Additive,
        BuiltinGroup::Multiplicative,
        BuiltinGroup::Union,
        BuiltinGroup::Meet,
    ];

    /// The builtin group covering `symbol`, if one does.
    pub fn of(symbol: KeywordSymbol) -> Option<BuiltinGroup> {
        BuiltinGroup::ALL
            .into_iter()
            .find(|group| group.names().iter().any(|name| name.symbol() == symbol))
    }

    /// This group's members as the names they are declared under.
    fn names(self) -> &'static [&'static StaticName<KeywordSymbol>] {
        match self {
            BuiltinGroup::Comparison => COMPARISON,
            BuiltinGroup::Additive => ADDITIVE,
            BuiltinGroup::Multiplicative => MULTIPLICATIVE,
            BuiltinGroup::Union => UNION,
            BuiltinGroup::Meet => MEET,
        }
    }

    /// How a run of this group's operators reduces.
    pub fn mode(self) -> ReductionMode {
        match self {
            BuiltinGroup::Comparison => ReductionMode::Pairwise {
                combiner: OPERATORS.and.symbol(),
                direction: FoldDirection::Left,
            },
            BuiltinGroup::Additive | BuiltinGroup::Multiplicative => ReductionMode::FoldLeft,
            BuiltinGroup::Union | BuiltinGroup::Meet => ReductionMode::Unary,
        }
    }

    /// Whether `group` is this builtin group written out — the same member set under the same mode,
    /// which under content identity makes the two one group.
    pub fn equals(self, group: &DeclaredGroup<'_>) -> bool {
        if group.mode != self.mode() {
            return false;
        }
        let names = self.names();
        group.members.len() == names.len()
            && names
                .iter()
                .all(|name| group.members.contains(&name.symbol()))
    }
}

/// Whether some builtin group is `group` written out, which under content identity makes the two
/// one group: the declaration says nothing the language does not already say.
pub(crate) fn builtin_equal(group: &DeclaredGroup<'_>) -> bool {
    BuiltinGroup::ALL
        .into_iter()
        .any(|builtin| builtin.equals(group))
}

/// What the program's declarations say about one symbol, whatever position they sit at.
#[derive(Clone, Copy)]
pub(crate) enum Claim<'graph> {
    /// A `GROUP` statement names it. The record is the one every equal declaration shares.
    Group(&'graph DeclaredGroup<'graph>),
    /// A `UNARY OP` marks it unary. Not a group: it may repeat, and it holds no fellow members.
    Unary,
}

/// Every claim the code being built makes, collected once before the first draft and read by
/// symbol. A `Claims` of an `EVAL`'s code chains to the program's, so evaluated code is held to the
/// program's declarations.
#[derive(Clone, Copy)]
pub(crate) struct Claims<'graph> {
    /// Sorted by symbol.
    entries: &'graph [(KeywordSymbol, Claim<'graph>)],
    outer: Option<&'graph Claims<'graph>>,
}

impl<'graph> Claims<'graph> {
    /// What `symbol` is claimed as here, else in the code enclosing this one.
    pub(crate) fn get(&self, symbol: KeywordSymbol) -> Option<Claim<'graph>> {
        if let Ok(index) = self
            .entries
            .binary_search_by_key(&symbol, |(held, _)| *held)
        {
            return Some(self.entries[index].1);
        }
        self.outer.and_then(|outer| outer.get(symbol))
    }
}

/// The groups the shapes enclosing a body hold, innermost first — which groups an operator run
/// written in that body may chain under.
///
/// Only two kinds of body hold a group: a `GROUP`'s own body, and a `USING … SCOPE` body over an
/// operand that surfaces one. Both hold it as a parameter is held, so no position is ever compared.
#[derive(Clone, Copy)]
pub struct GroupFrame<'graph> {
    held: &'graph [&'graph DeclaredGroup<'graph>],
    outer: Option<&'graph GroupFrame<'graph>>,
    claims: &'graph Claims<'graph>,
}

const _: () = assert!(!std::mem::needs_drop::<GroupFrame<'static>>());

/// How one symbol of an operator run chains. `==` and `!=` never reach here: they belong to no
/// group, so the caller takes them out of the operator run before asking.
#[derive(Clone, Copy)]
pub(crate) enum Cover<'graph> {
    Builtin(BuiltinGroup),
    Declared(&'graph DeclaredGroup<'graph>),
    /// A `UNARY OP`'d symbol no group claims.
    Unary(KeywordSymbol),
    /// A symbol nothing claims: fold-left, alone.
    Alone(KeywordSymbol),
}

impl Cover<'_> {
    /// How an operator run under this cover reduces.
    pub(crate) fn mode(self) -> ReductionMode {
        match self {
            Cover::Builtin(group) => group.mode(),
            Cover::Declared(group) => group.mode,
            Cover::Unary(_) => ReductionMode::Unary,
            Cover::Alone(_) => ReductionMode::FoldLeft,
        }
    }

    /// Whether two symbols of one operator run agree. A declared group compares by content, so two
    /// instantiations of one functor's `GROUP` agree.
    pub(crate) fn agrees(self, other: Cover<'_>) -> bool {
        match (self, other) {
            (Cover::Builtin(left), Cover::Builtin(right)) => left == right,
            (Cover::Declared(left), Cover::Declared(right)) => left == right,
            (Cover::Unary(left), Cover::Unary(right)) => left == right,
            (Cover::Alone(left), Cover::Alone(right)) => left == right,
            _ => false,
        }
    }
}

/// The bare-equality mode: an operator run of `==` and `!=` alone folds its pairs through `AND`,
/// left.
pub(crate) fn equality_mode() -> ReductionMode {
    ReductionMode::Pairwise {
        combiner: OPERATORS.and.symbol(),
        direction: FoldDirection::Left,
    }
}

impl<'graph> GroupFrame<'graph> {
    /// The frame of a body holding `held`, inside `outer`.
    pub(crate) fn new(
        held: &'graph [&'graph DeclaredGroup<'graph>],
        outer: Option<&'graph GroupFrame<'graph>>,
        claims: &'graph Claims<'graph>,
    ) -> GroupFrame<'graph> {
        GroupFrame {
            held,
            outer,
            claims,
        }
    }

    pub(crate) fn claims(&self) -> &'graph Claims<'graph> {
        self.claims
    }

    /// The held group covering `symbol`, walking outward. `None` when no enclosing body holds one.
    pub fn visible(&self, symbol: KeywordSymbol) -> Option<&'graph DeclaredGroup<'graph>> {
        let mut frame = Some(self);
        while let Some(current) = frame {
            if let Some(group) = current
                .held
                .iter()
                .find(|group| group.members.contains(&symbol))
            {
                return Some(group);
            }
            frame = current.outer;
        }
        None
    }

    /// How `symbol` chains where this frame is. `Err` when the symbol's group exists but no
    /// enclosing body holds it — an operator run of it is unchained here.
    pub(crate) fn cover(&self, symbol: KeywordSymbol) -> Result<Cover<'graph>, ()> {
        debug_assert!(
            !is_equality(symbol),
            "equality belongs to no group and is taken out of the operator run first"
        );
        if let Some(group) = BuiltinGroup::of(symbol) {
            return Ok(Cover::Builtin(group));
        }
        if let Some(group) = self.visible(symbol) {
            return Ok(Cover::Declared(group));
        }
        match self.claims.get(symbol) {
            Some(Claim::Group(_)) => Err(()),
            Some(Claim::Unary) => Ok(Cover::Unary(symbol)),
            None => Ok(Cover::Alone(symbol)),
        }
    }

    /// Whether `symbol` chains pairwise — the condition a binary `OP` declaring a result type of
    /// its own is admitted under.
    ///
    /// Read off the symbol's chaining wherever its group is declared, since a declaration sits
    /// outside the body holding its group as often as inside it: a `GROUP` statement's claim covers
    /// its members program-wide, and a group held by an enclosing frame covers its members here. A
    /// signature's bodyless `GROUP` claims nothing and reaches a body only as a held group, so a
    /// declaration under a `USING` that surfaces one is admitted by the frame alone.
    pub fn pairwise(&self, symbol: KeywordSymbol) -> bool {
        if is_equality(symbol) {
            return true;
        }
        let mode = match BuiltinGroup::of(symbol) {
            Some(group) => group.mode(),
            None => match self.visible(symbol) {
                Some(group) => group.mode,
                None => match self.claims.get(symbol) {
                    Some(Claim::Group(group)) => group.mode,
                    Some(Claim::Unary) | None => return false,
                },
            },
        };
        matches!(mode, ReductionMode::Pairwise { .. })
    }
}

/// The group a `GROUP` statement or a `SIG` body's bodyless `GROUP` head declares: its members,
/// sorted and deduped so `==` is content equality, under the mode its form id names.
///
/// `Ok(None)` when `node` is no `GROUP` form at all. `Err` when it is one whose body is not a run
/// of binary operator declarations — a `UNARY OP` among them, an empty body, or a statement whose
/// symbol will not read.
pub(crate) fn declared_group<'x>(
    node: &KExpression<'_>,
    scratch: BumpAllocator<'x>,
) -> Result<Option<DeclaredGroup<'x>>, ()> {
    let Some(form) = node.cache().builtin_shape() else {
        return Ok(None);
    };
    let definition = matches!(
        form.id,
        BuiltinShapeId::GroupHeadFoldLeft
            | BuiltinShapeId::GroupHeadFoldRight
            | BuiltinShapeId::GroupHeadPairwiseFoldLeft
            | BuiltinShapeId::GroupHeadPairwiseFoldRight
    );
    let statement = matches!(
        form.id,
        BuiltinShapeId::GroupFoldLeft
            | BuiltinShapeId::GroupFoldRight
            | BuiltinShapeId::GroupPairwiseFoldLeft
            | BuiltinShapeId::GroupPairwiseFoldRight
    );
    if !definition && !statement {
        return Ok(None);
    }
    let body_role = if definition {
        Role::Definition(DefinitionKind::Plain)
    } else {
        Role::Body(BodyKind::Module)
    };
    let mut body = None;
    let mut combiner = None;
    for (role, part) in form.roles().zip(node.parts) {
        match role {
            role if role == body_role => body = Some(&part.value),
            Role::Argument => combiner = Some(&part.value),
            _ => {}
        }
    }
    let mode = match form.id {
        BuiltinShapeId::GroupFoldLeft | BuiltinShapeId::GroupHeadFoldLeft => {
            ReductionMode::FoldLeft
        }
        BuiltinShapeId::GroupFoldRight | BuiltinShapeId::GroupHeadFoldRight => {
            ReductionMode::FoldRight
        }
        _ => ReductionMode::Pairwise {
            combiner: quoted_symbol(combiner.ok_or(())?)?,
            direction: match form.id {
                BuiltinShapeId::GroupPairwiseFoldLeft
                | BuiltinShapeId::GroupHeadPairwiseFoldLeft => FoldDirection::Left,
                _ => FoldDirection::Right,
            },
        },
    };
    let ExpressionPart::Expression(body) = body.ok_or(())? else {
        return Err(());
    };
    let members = scan_members(body.reference(), scratch)?;
    Ok(Some(DeclaredGroup {
        members: members.leak(),
        mode,
    }))
}

/// The member symbols of a `GROUP` body: the symbol of every top-level binary operator declaration,
/// sorted and deduped. A `UNARY OP` among them, or a body declaring no operator at all, is refused.
pub(crate) fn scan_members<'x>(
    body: &KExpression<'_>,
    scratch: BumpAllocator<'x>,
) -> Result<BumpVec<'x, KeywordSymbol>, ()> {
    let mut members: BumpVec<'x, KeywordSymbol> = BumpVec::new_in(scratch);
    for (statement, _) in body.body_statements() {
        let node = statement.statement_spine();
        match op_declaration_arity(node) {
            Some(OpArity::Binary) => {}
            Some(OpArity::Unary) => return Err(()),
            None => continue,
        }
        members.push(declaration_symbol(node)?);
    }
    if members.is_empty() {
        return Err(());
    }
    members.sort_unstable();
    members.dedup();
    Ok(members)
}

/// The symbol an operator declaration names: its `Role::Data` part, a `#(…)` quote of one keyword.
pub(crate) fn declaration_symbol(node: &KExpression<'_>) -> Result<KeywordSymbol, ()> {
    let form = node.cache().builtin_shape().ok_or(())?;
    let data = form
        .roles()
        .zip(node.parts)
        .find(|(role, _)| *role == Role::Data)
        .map(|(_, part)| &part.value)
        .ok_or(())?;
    quoted_symbol(data)
}

/// The one keyword a `#(…)` part quotes.
fn quoted_symbol(part: &ExpressionPart<'_>) -> Result<KeywordSymbol, ()> {
    let ExpressionPart::QuotedExpression(quoted) = part else {
        return Err(());
    };
    symbol_from_quote_body(quoted.reference()).map_err(|_| ())
}

/// Whether two group records are the same group: content identity, over any two lifetimes.
pub(crate) fn groups_equal(left: &DeclaredGroup<'_>, right: &DeclaredGroup<'_>) -> bool {
    left.mode == right.mode && left.members == right.members
}

/// Whether `form` is a `UNARY OP` — a definition, its combined spelling, or a `SIG` body's head.
fn is_unary_declaration(form: BuiltinShapeId) -> bool {
    matches!(
        form,
        BuiltinShapeId::UnaryOperatorDefinitionReturning
            | BuiltinShapeId::CombinedUnaryOperatorReturning
            | BuiltinShapeId::UnaryOperatorHeadReturning
    )
}

/// The pre-scan: every claim the code being built makes over an operator symbol, collected before
/// the first draft and blind to position, so how a symbol chains never depends on where its
/// declarations sit.
///
/// `outer` is the enclosing code's claims — the program's, for the code an `EVAL` runs — so
/// evaluated code is held to the program's declarations. A `GROUP` inside a quote is data and is
/// not walked.
pub(crate) fn claims<'graph, 'n, 'x>(
    brand: ProgramBrand<'graph>,
    scratch: BumpAllocator<'x>,
    statements: impl Iterator<Item = &'n KExpression<'graph>>,
    outer: Option<&'graph Claims<'graph>>,
) -> Result<&'graph Claims<'graph>, ShapeError>
where
    'graph: 'n,
{
    let mut scan = Scan {
        brand,
        scratch,
        outer,
        entries: BumpVec::new_in(scratch),
    };
    for statement in statements {
        scan.node(statement)?;
    }
    let mut entries = scan.entries;
    entries.sort_unstable_by_key(|(symbol, _)| *symbol);
    let writer = brand.writer();
    Ok(resident(
        writer,
        Claims {
            entries: collect(writer, entries.iter().copied()),
            outer,
        },
    ))
}

/// The pre-scan's own state: the claims made so far, beside the storage a new group record lands in.
struct Scan<'graph, 'x> {
    brand: ProgramBrand<'graph>,
    scratch: BumpAllocator<'x>,
    outer: Option<&'graph Claims<'graph>>,
    entries: BumpVec<'x, (KeywordSymbol, Claim<'graph>)>,
}

impl<'graph> Scan<'graph, '_> {
    /// What `symbol` is claimed as so far, here or in the enclosing code.
    fn claimed(&self, symbol: KeywordSymbol) -> Option<Claim<'graph>> {
        self.entries
            .iter()
            .find(|(held, _)| *held == symbol)
            .map(|(_, claim)| *claim)
            .or_else(|| self.outer.and_then(|outer| outer.get(symbol)))
    }

    /// One node: what it declares, then everything its parts reach.
    fn node(&mut self, node: &KExpression<'graph>) -> Result<(), ShapeError> {
        if let Some(form) = node.cache().builtin_shape() {
            if let Some(group) =
                declared_group(node, self.scratch).map_err(|()| ShapeError::Malformed {
                    form: form.id,
                    at: Position::PARAMETER,
                })?
            {
                // A `SIG` body's bodyless `GROUP` declares in the signature's operator channel and
                // claims nothing: it reaches a body only by being surfaced through `USING`.
                if !matches!(
                    form.id,
                    BuiltinShapeId::GroupHeadFoldLeft
                        | BuiltinShapeId::GroupHeadFoldRight
                        | BuiltinShapeId::GroupHeadPairwiseFoldLeft
                        | BuiltinShapeId::GroupHeadPairwiseFoldRight
                ) {
                    self.claim_group(&group)?;
                }
            }
            if form
                .binder
                .is_some_and(|binder| binder.surface == crate::parse::BinderSurface::OperatorDef)
                && let Ok(symbol) = declaration_symbol(node)
            {
                self.claim_declaration(form.id, symbol)?;
            }
        }
        for part in node.parts {
            self.part(&part.value)?;
        }
        Ok(())
    }

    fn part(&mut self, part: &ExpressionPart<'graph>) -> Result<(), ShapeError> {
        match part {
            ExpressionPart::Expression(node)
            | ExpressionPart::SigiledTypeExpr(node)
            | ExpressionPart::RecordType(node) => self.node(node.reference()),
            ExpressionPart::ListLiteral(items) => {
                for item in items.iter() {
                    self.part(item)?;
                }
                Ok(())
            }
            ExpressionPart::DictLiteral(pairs) => {
                for (key, value) in pairs.iter() {
                    self.part(key)?;
                    self.part(value)?;
                }
                Ok(())
            }
            ExpressionPart::RecordLiteral(pairs) => {
                for (_, value) in pairs.iter() {
                    self.part(value)?;
                }
                Ok(())
            }
            // A quote is data: the code inside it is rewritten where an `EVAL` of it is built,
            // under that site's own claims.
            ExpressionPart::QuotedExpression(_)
            | ExpressionPart::Keyword(_)
            | ExpressionPart::Identifier(_)
            | ExpressionPart::Type(_)
            | ExpressionPart::Literal(_) => Ok(()),
        }
    }

    /// Record a `GROUP` statement's claim over each of its members.
    fn claim_group(&mut self, group: &DeclaredGroup<'_>) -> Result<(), ShapeError> {
        let refused = |symbol| ShapeError::RedeclaresGroup {
            symbol,
            at: Position::PARAMETER,
        };
        if let Some(symbol) = group.members.iter().find(|symbol| is_equality(**symbol)) {
            return Err(refused(*symbol));
        }
        // A `GROUP` written out equal to a builtin group is that group, and claims nothing new.
        if builtin_equal(group) {
            return Ok(());
        }
        let mut record = None;
        for symbol in group.members {
            if BuiltinGroup::of(*symbol).is_some() {
                return Err(refused(*symbol));
            }
            match self.claimed(*symbol) {
                Some(Claim::Unary) => return Err(refused(*symbol)),
                Some(Claim::Group(held)) if groups_equal(held, group) => record = Some(held),
                Some(Claim::Group(_)) => return Err(refused(*symbol)),
                None => {}
            }
        }
        let writer = self.brand.writer();
        let record = record.unwrap_or_else(|| {
            resident(
                writer,
                DeclaredGroup {
                    members: collect(writer, group.members.iter().copied()),
                    mode: group.mode,
                },
            )
        });
        for symbol in group.members {
            if self.claimed(*symbol).is_none() {
                self.entries.push((*symbol, Claim::Group(record)));
            }
        }
        Ok(())
    }

    /// Record what one operator declaration says about its symbol: `!=` is nobody's to declare, a
    /// `UNARY OP` marks its symbol unary, and a bare `OP` declares an overload and nothing else.
    fn claim_declaration(
        &mut self,
        form: BuiltinShapeId,
        symbol: KeywordSymbol,
    ) -> Result<(), ShapeError> {
        if is_unequal(symbol) {
            return Err(ShapeError::Derived {
                symbol,
                at: Position::PARAMETER,
            });
        }
        if !is_unary_declaration(form) {
            return Ok(());
        }
        let refused = Err(ShapeError::RedeclaresGroup {
            symbol,
            at: Position::PARAMETER,
        });
        if BuiltinGroup::of(symbol).is_some_and(|group| group.mode() != ReductionMode::Unary) {
            return refused;
        }
        match self.claimed(symbol) {
            Some(Claim::Group(_)) => refused,
            Some(Claim::Unary) => Ok(()),
            None => {
                self.entries.push((symbol, Claim::Unary));
                Ok(())
            }
        }
    }
}
