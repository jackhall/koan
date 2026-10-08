//! The **shape builder**: one recursive walk that every kind of shape goes through.
//!
//! A body is built in four passes over a draft kept in scratch. The binders pass lays out the
//! declared names and refuses a repeated name or a builtin's, then lays out each definition's
//! registrations — and a `USING … SCOPE` block's surfaced heads, as registration parameters — and
//! reads each bucket declaration's ranking, ranking each registration by its chaining, its head or
//! the declaration it sees and refusing two rankings of one key that meet. The
//! mention pass walks each statement from its root, carrying the class a mention met there would
//! take, resolving each mention and each keyworded use's candidates as it is met and building each
//! nested body or arm as a draft of its own on top of the chain of enclosing drafts; a binder or
//! declaration it meets anywhere but at its statement's root is refused. The components pass
//! condenses the bindings' reference graph, with every binder of one statement forced into one
//! component, refuses a component with an eager internal mention, turns a nested callable's capture
//! of a fellow member into a knot edge, and seals the nested drafts into program storage. The units
//! pass orders the body's units — its components and the statements that bind nothing — so each
//! follows every unit it reads and independent ones come out as written; the runner performs them
//! in that order.
//!
//! A `MODULE` or `GROUP` body is held to its **capture contract**: a name it reads from outside —
//! bound neither by the body nor by a top-level statement — is captured only where its `OVER` list
//! names it, a registration by its key, and the first that is not refuses the body. An entry the
//! body never reads is captured all the same, and a top-level one counts by its binding, recorded
//! on the shape, since the run reads it where it lives.
//!
//! A nested draft takes everything it needs from its statement through one [`Entry`] — its
//! parameters, signature, surfaced heads, held groups and capture contract — and is built under
//! [`Builder::nested`], which sets the enclosing walk's state aside and restores it whether or not
//! the draft is built, so nothing one draft leaves is taken by the next.
//!
//! A keyworded node inside a type expression is a type the elaborator reads, not a use, so the walk
//! counts the type expressions it is inside and resolves no candidates there.
//!
//! A callable body records the form node holding it, a binder whose right-hand side is a callable
//! form at its root — or a combined form that is one, or a `MODULE`/`GROUP` binder — records the
//! body it births, and a `LET` value binder records its right-hand side, so a component's tie reads
//! each member's signature, body or data off the shape.
//!
//! A quote value's code is built here too, as a [`ShapeKind::Code`] draft entered where the quote
//! is written ([`Builder::enter_code`]): under the builtin groups and the groups its code holds,
//! with its `$` names and `$(…)` uses resolved outward and every other free name and key left open.
//! A group mark covers the use it wraps and, when that use tops an operator run, every use the
//! rewrite built for that operator run — never one in an operand. A code draft that cannot be built is not an
//! error of the program; its error is kept for the `EVAL` that runs it, and only a `$` mark nothing
//! binds where the quote is written, or one reading a quantified function anywhere but a call's
//! head, refuses the program.
//!
//! See [README.md § Visibility](../README.md#visibility).

use crate::memory::{
    BumpAllocator, BumpBackedMap, BumpVec, ProgramBrand, bump_table, collect, resident,
    strongly_connected_components,
};
use crate::parse::{
    BinderSurface, DeclaredElement, SlotLabel, declared_element, head_run, needed_key, needed_name,
    needing, next_is_type_slot, slot_label,
};
use crate::parse::{BuiltinShape, BuiltinShapeId, ShapeElement, builtin_shape_for};
use crate::parse::{ExpressionPart, KExpression, KeyElement, Mark};
use crate::source::SourceRef;
use crate::symbols::{
    BinderSymbol, KeySymbol, RegistrationSymbol, StaticName, SymbolInterner, TypeSymbol,
    ValueSymbol,
};
use crate::values::{ContentDigest, Knotted, admits_part};

use crate::type_lattice::{DeclaredGroup, KType, ReductionMode, TypeRegistry, dense_classes};

use super::super::builtins::{BuiltinNames, Builtins};
use super::super::channels::Channels;
use super::super::groups::{self, Claim, Claims, GroupFrame};
use super::super::signature::{
    body_of, declare_family_parameters, declare_parameters, declare_quantifiers, pair_label,
    quantifier_bounds, quoted_body, signature_run,
};
use super::super::typed::Static;
use super::{
    Arm, BodyShape, BuiltinIndex, Candidate, CandidateList, CaptureSlot, CaptureSource,
    CaptureSpec, Component, ComponentIndex, Coordinate, Listed, Mention, MentionClass, Offer,
    Position, QuotedPart, Ranking, Registration, ShapeError, ShapeKind, Site, Slot, SurfacedHead,
    Target, TopLevel, TypeExpression, Unit, UnitWork, Which, resolve_here,
};
use crate::parse::{BodyKind, DefinitionKind, Heads, Reading, Role};
use std::cell::Cell;

mod digest;
mod locate;
mod rewrite;
mod surface;

pub(in crate::scope) use locate::source_within;

use rewrite::{Built, BuiltKind, chained};
use surface::{Quantified, Surfaced, SurfacedKey, quantified_value};

/// The names a body binds that no signature writes. The shape builder binds them, and the
/// elaborator reads an operator's function type over them, so the two cannot disagree.
pub(crate) struct ImplicitNames {
    /// An arm's matched value.
    pub(crate) it: StaticName<ValueSymbol>,
    /// A binary operator's operands.
    pub(crate) left: StaticName<ValueSymbol>,
    pub(crate) right: StaticName<ValueSymbol>,
    /// A unary operator's operand run.
    pub(crate) operands: StaticName<ValueSymbol>,
}

pub(crate) static IMPLICIT: ImplicitNames = ImplicitNames {
    it: crate::static_name!(ValueSymbol, "it"),
    left: crate::static_name!(ValueSymbol, "left"),
    right: crate::static_name!(ValueSymbol, "right"),
    operands: crate::static_name!(ValueSymbol, "operands"),
};

/// The shape of a program's top-level statements.
pub(super) fn program<'graph, X: Knotted>(
    brand: ProgramBrand<'graph>,
    statements: &[KExpression<'graph>],
    builtins: &Builtins<'_, X>,
    types: &TypeRegistry<'graph>,
    symbols: &SymbolInterner,
    scratch: BumpAllocator<'_>,
) -> Result<&'graph BodyShape<'graph>, ShapeError<'graph>> {
    // Every claim the program makes over an operator symbol is collected before the first draft, so
    // how a symbol chains never depends on where its declarations sit.
    let claims = groups::claims(brand, scratch, statements.iter(), None)?;
    let frame = resident(brand.writer(), GroupFrame::new(&[], None, claims));
    let mut builder = Builder::new(brand, scratch, builtins, types, symbols, claims, frame);
    let statements = statements
        .iter()
        .enumerate()
        .map(|(index, statement)| (statement, index + 1));
    let draft = builder.draft(
        ShapeKind::Program,
        Entry::PLAIN,
        Position::PARAMETER,
        statements,
    )?;
    Ok(builder.seal(draft, None))
}

/// The parameters a node declares before any of its parts is read: a signature's or an `EXPR`
/// head's names, a `FOR ALL` group's, and a parameterized union's family parameters. One walk,
/// shared by the statement walk and the definition walk.
fn form_parameters(
    form: &BuiltinShape,
    node: &KExpression<'_>,
    into: &mut BumpVec<'_, BinderSymbol>,
) {
    let union = form.id == BuiltinShapeId::Union;
    for (role, part) in form.roles().zip(node.parts) {
        match role {
            Role::Signature | Role::Head => declare_parameters(&part.value, into),
            Role::Quantifiers => declare_quantifiers(&part.value, into),
            Role::Name if union => declare_family_parameters(&part.value, into),
            _ => {}
        }
    }
}

/// The static check of a builtin node, before any part is walked: each part its role reads as
/// written — as a quote, as bare syntax or as a container of quotes — is written as the reading
/// says, and admits one of its slot's types by [`admits_part`], the one admission rule — a type
/// expression's and an in-place operand's type is
/// the value it denotes, so only their spelling is checked. A binder name is a bare name, or a bare
/// declarator group for `UNION` and `NEWTYPE`. Code written where a quote or a container of
/// quotes is wanted is `Unquoted`, a quote where bare syntax is wanted `Malformed`, and a part
/// whose syntax fills no slot type `Inadmissible`. The readers after it assume a well-formed part.
fn written_as_read<'e>(
    form: &'static BuiltinShape,
    node: &KExpression<'_>,
    types: &TypeRegistry<'_>,
) -> Result<(), ShapeError<'e>> {
    let declarator = matches!(
        form.id,
        BuiltinShapeId::Union | BuiltinShapeId::NewTypeDeclaration
    );
    let quoted = |part: &ExpressionPart<'_>| matches!(part, ExpressionPart::QuotedExpression(_));
    for (index, (element, part)) in form.elements.iter().zip(node.parts).enumerate() {
        let ShapeElement::Slot {
            role,
            types: slot_types,
        } = element
        else {
            continue;
        };
        let reading = role.reading();
        let at = part.span.map_or(node.source, |span| SourceRef {
            span,
            file: node.source.file,
        });
        let part = &part.value;
        let written = match reading {
            Reading::Quote => quoted(part),
            Reading::Bare if *role == Role::Name => match part {
                ExpressionPart::Identifier(_) | ExpressionPart::Type(_) => true,
                ExpressionPart::Expression(_) => declarator,
                _ => false,
            },
            Reading::Bare => !quoted(part),
            Reading::Container => match part {
                ExpressionPart::ListLiteral(items) => items.iter().all(quoted),
                ExpressionPart::DictLiteral(pairs) => pairs
                    .iter()
                    .all(|(key, value)| (key.is_wildcard() || quoted(key)) && quoted(value)),
                _ => false,
            },
            Reading::Label | Reading::Evaluated => continue,
        };
        let code = part.code_kind().is_some()
            || matches!(
                part,
                ExpressionPart::ListLiteral(_) | ExpressionPart::DictLiteral(_)
            );
        let inadmissible = ShapeError::Inadmissible {
            form: form.id,
            index,
            slot: slot_types[0],
            at,
        };
        if !written {
            return Err(match reading {
                Reading::Quote | Reading::Container if code => ShapeError::Unquoted {
                    form: form.id,
                    part: QuotedPart::of(*role),
                    at,
                },
                Reading::Quote | Reading::Container => inadmissible,
                _ => ShapeError::Malformed { form: form.id, at },
            });
        }
        // A type expression and an in-place operand are typed by the value they denote, so only
        // their spelling is checked here.
        let typed_by_code = !matches!(role, Role::TypeExpression | Role::InPlace);
        if typed_by_code
            && !slot_types
                .iter()
                .any(|slot| admits_part((*slot).into(), part, types))
        {
            return Err(inadmissible);
        }
    }
    Ok(())
}

/// The type part a signature run pairs with the parameter `name`.
fn parameter_type<'graph>(
    run: &'graph KExpression<'graph>,
    name: BinderSymbol,
) -> Option<&'graph ExpressionPart<'graph>> {
    let mut index = 0;
    while index < run.parts.len() {
        match pair_label(run, index) {
            Some(label) if label.name() == Some(name) => return Some(&run.parts[index + 1].value),
            Some(_) => index += 2,
            None => index += 1,
        }
    }
    None
}

/// What a mark in a quote's code is written over: a name, or the keyworded use a group mark wraps.
#[derive(Clone, Copy)]
enum Marked<'graph> {
    Name(BinderSymbol),
    Use(&'graph KExpression<'graph>),
}

/// Every mark `node` holds outside the quote values nested in it, each beside its site: what a code
/// draft that could not be built would have met. A quote a builtin reads as written is syntax of
/// the code around it, so its marks are the code's. A group mark over an operator run is left out:
/// which uses the operator run becomes is the rewrite's, and the code was not built.
fn code_marks<'graph>(
    node: &KExpression<'graph>,
    out: &mut BumpVec<'_, (Mark, Marked<'graph>, Site)>,
) {
    let form = node.cache().builtin_shape();
    for (index, part) in node.parts.iter().enumerate() {
        let written = form
            .and_then(|form| form.roles().nth(index))
            .is_some_and(|role| role.reading() != Reading::Evaluated);
        part_marks(&part.value, written, out);
    }
}

/// [`code_marks`] for one part; `written` when a builtin reads it as written.
fn part_marks<'graph>(
    part: &ExpressionPart<'graph>,
    written: bool,
    out: &mut BumpVec<'_, (Mark, Marked<'graph>, Site)>,
) {
    match part {
        ExpressionPart::MarkedName(mark, name) => {
            out.push((*mark, Marked::Name(*name), Site::of(part)))
        }
        ExpressionPart::QuotedExpression(node) if written => code_marks(node.reference(), out),
        ExpressionPart::MarkedUse(mark, node) => {
            let node = node.reference();
            let keyed = node
                .parts
                .iter()
                .any(|part| matches!(part.value, ExpressionPart::Keyword(_)));
            if keyed && !chained(node) {
                out.push((*mark, Marked::Use(node), Site::of_node(node)));
            }
            code_marks(node, out)
        }
        ExpressionPart::Expression(node)
        | ExpressionPart::SigiledTypeExpr(node)
        | ExpressionPart::RecordType(node) => code_marks(node.reference(), out),
        ExpressionPart::ListLiteral(items) => {
            for item in items.iter() {
                part_marks(item, written, out);
            }
        }
        ExpressionPart::DictLiteral(pairs) => {
            for (key, value) in pairs.iter() {
                part_marks(key, written, out);
                part_marks(value, written, out);
            }
        }
        ExpressionPart::RecordLiteral(fields) => {
            for (_, value) in fields.iter() {
                part_marks(value, written, out);
            }
        }
        ExpressionPart::QuotedExpression(_)
        | ExpressionPart::Identifier(_)
        | ExpressionPart::Type(_)
        | ExpressionPart::Keyword(_)
        | ExpressionPart::Literal(_) => {}
    }
}

/// What the next part the walk reaches may be beyond what any part may: set right before a call's
/// head is walked, and taken by the first part there that is not a one-part wrapper. A name bound
/// to a quantified function is read anywhere else only where [`Quantified`] allows it.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Admits {
    #[default]
    Nothing,
    /// A call's head: a quantified `FN` written there, or a name bound to one.
    Head,
}

/// The class a mention met in this context takes, before the context's own role applies.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    /// A binding's right-hand side, before any context: a mention here is the root and eager.
    Root,
    /// Under constructor slots and at most one callable-body boundary.
    Deferred,
    /// Under any other context.
    Eager,
}

impl State {
    /// Entering a constructor slot: a list element, a dict value, a record field, a definition, a
    /// nominal construction's payload.
    fn constructor(self) -> State {
        match self {
            State::Root | State::Deferred => State::Deferred,
            State::Eager => State::Eager,
        }
    }

    fn class(self) -> MentionClass {
        match self {
            State::Deferred => MentionClass::Deferred,
            State::Root | State::Eager => MentionClass::Eager,
        }
    }
}

/// The reading end of a resolution: the level and statement a mention sits in, and its class.
#[derive(Clone, Copy)]
struct Reader {
    level: usize,
    statement: u32,
    class: MentionClass,
}

/// A group mark in force over the walk: the mark, and the operator run the node it wraps tops, if
/// it tops one, whose every use the mark covers too.
#[derive(Clone, Copy)]
struct Marking {
    mark: Mark,
    run: Option<u32>,
}

/// Where [`Builder::listed`] lists a key's registrations from: the draft at `site` as a use at
/// `at` sees them, each resolved from the draft at `level` through `mark` for `reader`; a closed
/// listing holds the builtin overloads alone.
#[derive(Clone, Copy)]
struct Listing {
    level: usize,
    reader: Reader,
    site: usize,
    at: Position,
    mark: Option<Mark>,
    closed: bool,
}

/// Everything a nested body takes from the statement it is entered from: whether its last
/// statement is in tail position, for a `MATCH` or `TRY` arm the arm's facts, its parameters (each
/// beside where the node declaring it is written), a callable body's signature run, a `USING …
/// SCOPE` body's quantified names and surfaced heads, and the groups it holds.
#[derive(Clone, Copy)]
struct Entry<'e, 'graph> {
    tail: bool,
    arm: Option<Arm<'graph>>,
    parameters: &'e [(BinderSymbol, SourceRef)],
    signature: Option<&'graph KExpression<'graph>>,
    surfacing: Option<Surfacing<'e, 'graph>>,
    held: &'e [&'graph DeclaredGroup<'graph>],
    /// A `MODULE` or `GROUP` body's capture contract.
    over: Option<Over<'graph>>,
}

/// A `MODULE` or `GROUP` body's capture contract: its `OVER` list, `None` where it writes none
/// and so captures nothing from outside, beside where the list and the binder are written.
#[derive(Clone, Copy)]
struct Over<'graph> {
    listed: Option<&'graph [Listed<'graph>]>,
    site: Site,
    at: SourceRef,
}

/// What a `USING … SCOPE` body takes from its operand beyond its parameters: the names bound to
/// quantified functions, and each surfaced head under each key it registers at.
#[derive(Clone, Copy)]
struct Surfacing<'e, 'graph> {
    quantified: &'e [BinderSymbol],
    heads: &'e [SurfacedKey<'graph>],
}

impl Entry<'_, '_> {
    /// A body that is no tail, no arm, binds nothing and holds no group: the program, a
    /// synthesized block.
    const PLAIN: Entry<'static, 'static> = Entry {
        tail: false,
        arm: None,
        parameters: &[],
        signature: None,
        surfacing: None,
        held: &[],
        over: None,
    };
}

/// The slots one statement binds: its name's first, then each registration's — at most three, a
/// combined `UNARY OP`'s name and two keys.
#[derive(Clone, Copy)]
struct Binders {
    slots: [Slot; 3],
    len: u8,
}

impl Binders {
    const NONE: Binders = Binders {
        slots: [Slot(0); 3],
        len: 0,
    };

    fn push(&mut self, slot: Slot) {
        self.slots[self.len as usize] = slot;
        self.len += 1;
    }

    fn iter(&self) -> impl Iterator<Item = Slot> + '_ {
        self.slots[..self.len as usize].iter().copied()
    }

    /// The statement's first binder: its name's slot when it has one. Every binder of a statement
    /// is one component, so this one stands for all of them.
    fn first(&self) -> Option<Slot> {
        self.iter().next()
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// One registration a draft declares, before its slot is known.
#[derive(Clone, Copy)]
struct Registered<'graph> {
    symbol: RegistrationSymbol,
    at: Position,
    key: KeySymbol,
    elements: &'graph [KeyElement],
    classes: &'graph [u8],
    which: Which,
    surfaced: Option<&'graph [SurfacedHead<'graph>]>,
}

/// A ranking some statement gives a key, as a statement or a use at the same key sees it.
#[derive(Clone, Copy)]
enum Seen<'a, 'graph> {
    Declaration(&'a Ranking<'graph>),
    Registration(&'a Registered<'graph>),
}

impl<'graph> Seen<'_, 'graph> {
    fn classes(self) -> &'graph [u8] {
        match self {
            Seen::Declaration(ranking) => ranking.classes,
            Seen::Registration(registered) => registered.classes,
        }
    }
}

/// A draft's kind and keyed entries so far, beside the position a statement of it reads at.
type Own<'a, 'graph> = (
    ShapeKind,
    &'a [Registered<'graph>],
    &'a [Ranking<'graph>],
    Position,
);

/// How many slots `key` holds.
fn slots(key: &[KeyElement]) -> usize {
    key.iter()
        .filter(|element| matches!(element, KeyElement::Slot))
        .count()
}

/// Whether `classes` rank their slots in written order, as every builtin overload's do.
fn written_order(classes: &[u8]) -> bool {
    classes
        .iter()
        .enumerate()
        .all(|(index, class)| *class as usize == index)
}

/// Whether a definition's head writes a rank anywhere, which only a bucket declaration may.
fn ranked_head(node: &KExpression<'_>) -> bool {
    head_run(node).is_some_and(|run| {
        run.parts
            .iter()
            .any(|part| matches!(slot_label(&part.value), Some(SlotLabel::Ranked(_))))
    })
}

/// One body under construction, in scratch.
struct Draft<'graph, 'x> {
    kind: ShapeKind,
    entered_at: Position,
    /// The statement of the enclosing draft this body sits in.
    parent_statement: u32,
    statements: u32,
    values: BumpVec<'x, (ValueSymbol, Position)>,
    types: BumpVec<'x, (TypeSymbol, Position)>,
    registrations: BumpVec<'x, (RegistrationSymbol, Position)>,
    /// Each registration, in statement order.
    registered: BumpVec<'x, Registered<'graph>>,
    /// Each bucket declaration, in statement order.
    rankings: BumpVec<'x, Ranking<'graph>>,
    /// The slots each statement binds.
    statement_binders: BumpVec<'x, Binders>,
    /// Each keyworded use's candidates, by the use's node site.
    candidates: BumpVec<'x, (Site, CandidateList<'graph>)>,
    /// This body's statement spines, **rewritten**, copied into scratch so the surfaced-name reader
    /// can reach a binder's own statement. Each spine's parts run stays where it is in program
    /// storage.
    nodes: BumpVec<'x, KExpression<'graph>>,
    /// The group frame this body was built under, and the groups it holds itself.
    frame: &'graph GroupFrame<'graph>,
    held: &'graph [&'graph DeclaredGroup<'graph>],
    mentions: BumpVec<'x, Mention>,
    captures: BumpVec<'x, CaptureSpec>,
    /// `(binder, bound, class)`: the binder's statement reads the bound slot.
    edges: BumpVec<'x, (Slot, Slot, MentionClass)>,
    /// `(statement, bound)`: the statement reads the bound slot, whether or not it binds — what a
    /// unit waits on.
    reads: BumpVec<'x, (u32, Slot)>,
    /// The body's units in the order they are performed, once the components pass has run.
    units: BumpVec<'x, Unit>,
    /// Finished nested drafts, waiting on this draft's components to settle their captures.
    children: BumpVec<'x, (Site, Draft<'graph, 'x>)>,
    /// `(binder, body)`: the binder's right-hand side births the callable whose body sits at `body`.
    births: BumpVec<'x, (Slot, Site)>,
    /// `(binder, rhs)`: a `LET` value binder's right-hand side part sits at `rhs`.
    rhs: BumpVec<'x, (Slot, Site)>,
    /// `(binder, node)`: a type binder's whole declaration node, in program storage.
    declarations: BumpVec<'x, (Slot, &'graph KExpression<'graph>)>,
    /// Each type expression the load pass types on its own, as met.
    type_expressions: BumpVec<'x, Recorded<'graph>>,
    component_of: BumpVec<'x, ComponentIndex>,
    /// Every component's members, one run after another, each run sorted.
    members: BumpVec<'x, Slot>,
    /// Each component's run in `members`, whether every mention between its members is deferred,
    /// and whether it is cyclic.
    components: BumpVec<'x, DraftComponent>,
    /// A callable body's signature run, which an `EVAL` of one of its parameters reads that
    /// parameter's type off.
    signature: Option<&'graph KExpression<'graph>>,
    /// Each `EVAL` of a parameter needing names, by its operand's site, beside what it offers.
    offers: BumpVec<'x, (Site, &'graph [(BinderSymbol, Offer<'graph>)])>,
    /// A code draft's keyworded holes some use selects from alone.
    required: BumpVec<'x, KeySymbol>,
    /// A code draft's carried type, and why its code cannot be built; see [`BodyShape::code_type`].
    code_type: KType,
    refusal: Option<&'graph ShapeError<'graph>>,
    /// Whether this body's last statement is in tail position.
    tail: bool,
    /// Whether this is a `USING … SCOPE` body, whose operand's registrations the builder does not
    /// read: a use under one may select one, so none is refused for want of a candidate.
    surfaced: bool,
    /// The parameters of a `USING … SCOPE` body bound to quantified functions, each read only at
    /// the head of a call.
    quantified: BumpVec<'x, BinderSymbol>,
    /// What this body is to the `MATCH` or `TRY` holding it, when it is an arm.
    arm: Option<Arm<'graph>>,
    /// The statement being walked when a nested draft was entered, and the class that path takes
    /// at this level.
    current: (u32, MentionClass),
    /// A `MODULE` or `GROUP` body's capture contract.
    over: Option<Over<'graph>>,
    /// The top-level bindings its `OVER` list names.
    listed_top: BumpVec<'x, TopLevel>,
    /// The first outer name it reads that its `OVER` list leaves out.
    unlisted: Option<BinderSymbol>,
    /// The body's code digest, once it is built.
    code: ContentDigest,
    /// Each top-level binding its code names, nested bodies' included, sorted.
    named_top: BumpVec<'x, TopLevel>,
    /// Each capture's top-level slot, where it reads one.
    top_captures: BumpVec<'x, Option<Slot>>,
}

/// A type expression a draft records: see [`TypeExpression`](super::TypeExpression).
#[derive(Clone, Copy)]
struct Recorded<'graph> {
    part: &'graph ExpressionPart<'graph>,
    statement: u32,
    guard: Option<(Site, u32)>,
}

/// A component under construction: its run in [`Draft::members`].
#[derive(Clone, Copy)]
struct DraftComponent {
    start: u32,
    len: u32,
    deferred_only: bool,
    cyclic: bool,
}

impl DraftComponent {
    /// This component's run within every component's `members`.
    fn run(self, members: &[Slot]) -> &[Slot] {
        let start = self.start as usize;
        &members[start..start + self.len as usize]
    }
}

impl Draft<'_, '_> {
    fn members_of(&self, component: ComponentIndex) -> &[Slot] {
        self.components[component.index()].run(&self.members)
    }

    fn end(&self) -> Position {
        Position(self.statements + 1)
    }

    /// Whether `slot` is in the type channel, read through the one owner of the index split.
    fn is_type_slot(&self, slot: Slot) -> bool {
        matches!(self.channels().name(slot.index()), BinderSymbol::Type(_))
    }

    /// The declared names, once the binders pass has sorted them.
    fn channels(&self) -> Channels<'_, Position> {
        Channels::new(&self.values, &self.types, &self.registrations)
    }

    /// Where the path into the nested draft being walked reads at this level.
    fn boundary(&self) -> Position {
        match self.current.1 {
            MentionClass::Deferred => self.end(),
            MentionClass::Eager => Position::statement(self.current.0 as usize),
        }
    }
}

struct Builder<'graph, 'x, 'e> {
    brand: ProgramBrand<'graph>,
    scratch: BumpAllocator<'x>,
    builtins: &'e dyn BuiltinNames,
    /// The program's type registry, which the static check admits written parts through and a
    /// quote's carried type is interned in.
    types: &'e TypeRegistry<'graph>,
    /// The program's interner, which records the spelling of each key a quote's code leaves open,
    /// so its carried type renders the key as written.
    symbols: &'e SymbolInterner,
    /// Every claim the code being built makes over an operator symbol, collected once up front.
    claims: &'graph Claims<'graph>,
    /// The frame of the draft being built: which groups an operator run written there sees.
    frame: &'graph GroupFrame<'graph>,
    /// Every node the operator-run rewrite built, by its parts address: a block the mention pass
    /// enters as a block rather than walking it as a call, and each use a group mark over its
    /// operator run covers.
    built: BumpBackedMap<'x, usize, Built>,
    /// How many operator runs the rewrite has chained, which numbers each run.
    runs: u32,
    /// The group mark over the node being walked, if any.
    marking: Option<Marking>,
    chain: BumpVec<'x, Draft<'graph, 'x>>,
    /// Each callable body's site beside the form node holding it, in program storage.
    forms: BumpBackedMap<'x, Site, &'graph KExpression<'graph>>,
    /// Each recorded right-hand side's site beside the part, in program storage.
    parts: BumpBackedMap<'x, Site, &'graph ExpressionPart<'graph>>,
    /// Type parameters of the forms enclosing the current walk path: never mentions.
    skip: BumpVec<'x, TypeSymbol>,
    /// Where the current draft's own entries in `skip` start; a nested body declares its enclosing
    /// form's type parameters as parameters and reads them as mentions.
    skip_floor: usize,
    /// How many type expressions the walk is inside: a keyworded node there is a type the
    /// elaborator reads, not a use dispatch selects for.
    in_type: u32,
    /// What the next part may be; see [`Admits`].
    admits: Admits,
}

impl<'graph, 'x, 'e> Builder<'graph, 'x, 'e> {
    fn new(
        brand: ProgramBrand<'graph>,
        scratch: BumpAllocator<'x>,
        builtins: &'e dyn BuiltinNames,
        types: &'e TypeRegistry<'graph>,
        symbols: &'e SymbolInterner,
        claims: &'graph Claims<'graph>,
        frame: &'graph GroupFrame<'graph>,
    ) -> Self {
        Builder {
            brand,
            scratch,
            builtins,
            types,
            symbols,
            claims,
            frame,
            built: bump_table(scratch),
            runs: 0,
            marking: None,
            chain: BumpVec::new_in(scratch),
            forms: bump_table(scratch),
            parts: bump_table(scratch),
            skip: BumpVec::new_in(scratch),
            skip_floor: 0,
            in_type: 0,
            admits: Admits::Nothing,
        }
    }

    /// Build one body: its binders, its mentions (and every nested body, depth first), and its
    /// components. The draft comes back unsealed, since whether a capture is a knot edge is
    /// settled by the enclosing draft's components.
    fn draft<'n>(
        &mut self,
        kind: ShapeKind,
        entry: Entry<'_, 'graph>,
        entered_at: Position,
        statements: impl Iterator<Item = (&'n KExpression<'graph>, usize)>,
    ) -> Result<Draft<'graph, 'x>, ShapeError<'graph>>
    where
        'graph: 'n,
    {
        // A body holding a group is built under a frame of its own, which every operator run inside
        // it — and every body nested in it — chains against.
        let writer = self.brand.writer();
        let held: &'graph [&'graph DeclaredGroup<'graph>] =
            collect(writer, entry.held.iter().copied());
        if !held.is_empty() {
            self.frame = resident(writer, GroupFrame::new(held, Some(self.frame), self.claims));
        }
        self.body(kind, entry, held, entered_at, statements)
    }

    /// [`draft`](Self::draft) with this body's frame already standing: rewrite each statement's
    /// operator runs, lay out the binders over the rewritten statements, walk them, and condense.
    fn body<'n>(
        &mut self,
        kind: ShapeKind,
        entry: Entry<'_, 'graph>,
        held: &'graph [&'graph DeclaredGroup<'graph>],
        entered_at: Position,
        statements: impl Iterator<Item = (&'n KExpression<'graph>, usize)>,
    ) -> Result<Draft<'graph, 'x>, ShapeError<'graph>>
    where
        'graph: 'n,
    {
        let writer = self.brand.writer();
        let mut nodes: BumpVec<'x, &'n KExpression<'graph>> = BumpVec::new_in(self.scratch);
        nodes.extend(statements.map(|(node, _)| node));
        for index in 0..nodes.len() {
            if let Some(rewritten) = self.rewrite_statement(nodes[index])? {
                nodes[index] = resident(writer, rewritten);
            }
        }
        // A body whose value is read takes its last statement's: one binding a quantified function
        // would hand on a value no type solves.
        if matches!(
            kind,
            ShapeKind::Callable | ShapeKind::Block | ShapeKind::Code
        ) && let Some(last) = nodes.last()
            && quantified_value(last)
        {
            return Err(ShapeError::QuantifiedValue { at: last.source });
        }
        let parent_statement = self
            .chain
            .last()
            .map_or(u32::MAX, |parent| parent.current.0);
        let heads = entry.surfacing.map_or(&[][..], |surfacing| surfacing.heads);
        let mut draft = self.binders(
            kind,
            entered_at,
            parent_statement,
            (entry.parameters, heads),
            &nodes,
        )?;
        draft.signature = entry.signature;
        if let Some(surfacing) = entry.surfacing {
            draft.quantified.extend_from_slice(surfacing.quantified);
        }
        draft.frame = self.frame;
        draft.held = held;
        draft.tail = entry.tail;
        draft.arm = entry.arm;
        draft.surfaced = entry.surfacing.is_some();
        draft.over = entry.over;
        draft.nodes.extend(nodes.iter().map(|node| **node));
        self.chain.push(draft);
        let level = self.chain.len() - 1;
        for (statement, node) in nodes.iter().enumerate() {
            if let Err(error) = self.walk_node(level, statement as u32, node, State::Root) {
                self.chain.pop();
                return Err(error);
            }
        }
        if let Err(error) = self.contract(level) {
            self.chain.pop();
            return Err(error);
        }
        let statements = collect(self.brand.writer(), self.chain[level].nodes.iter().copied());
        self.digest_draft(level, statements);
        let mut draft = self.chain.pop().expect("this draft was pushed above");
        self.components(&mut draft)?;
        self.units(&mut draft);
        Ok(draft)
    }

    /// The binders pass: every parameter at `0` and every statement's name at its position, a
    /// repeated name and a builtin's name refused in that order; then each surfaced head's
    /// registration and each statement's registrations and bucket declarations, each ranked and
    /// checked against every ranking of its key it sees. Each parameter comes paired with where the
    /// node declaring it is written, which an error about it points at.
    fn binders(
        &self,
        kind: ShapeKind,
        entered_at: Position,
        parent_statement: u32,
        (parameters, heads): (&[(BinderSymbol, SourceRef)], &[SurfacedKey<'graph>]),
        nodes: &[&KExpression<'graph>],
    ) -> Result<Draft<'graph, 'x>, ShapeError<'graph>> {
        let scratch = self.scratch;
        let declared = parameters
            .iter()
            .map(|(name, source)| (Some(*name), Position::PARAMETER, *source))
            .chain(nodes.iter().enumerate().map(|(index, node)| {
                let name = node.statement_binder_plan().and_then(|plan| plan.name);
                (name, Position::statement(index), node.source)
            }));
        let mut seen: BumpBackedMap<BinderSymbol, SourceRef> = bump_table(scratch);
        let mut values = BumpVec::new_in(scratch);
        let mut types = BumpVec::new_in(scratch);
        for (name, position, source) in declared {
            let Some(name) = name else { continue };
            if let Some(first) = seen.insert(name, source) {
                return Err(ShapeError::Rebind {
                    name,
                    first,
                    second: source,
                });
            }
            if self.builtins.lookup(name).is_some() {
                return Err(ShapeError::ShadowsBuiltin { name, at: source });
            }
            match name {
                BinderSymbol::Value(name) => values.push((name, position)),
                BinderSymbol::Type(name) => types.push((name, position)),
                BinderSymbol::Registration(_) | BinderSymbol::Key(_) => {
                    unreachable!("a plan's name is a written name")
                }
            }
        }
        values.sort_unstable_by_key(|(name, _)| *name);
        types.sort_unstable_by_key(|(name, _)| *name);
        let (registered, rankings) = self.keyed(kind, heads, nodes)?;
        let mut registrations = BumpVec::with_capacity_in(registered.len(), scratch);
        registrations.extend(registered.iter().map(|entry| (entry.symbol, entry.at)));
        registrations.sort_unstable_by_key(|(symbol, _)| *symbol);
        let mut statement_binders = BumpVec::with_capacity_in(nodes.len(), scratch);
        statement_binders.resize(nodes.len(), Binders::NONE);
        let entries = values
            .iter()
            .map(|(_, position)| *position)
            .chain(types.iter().map(|(_, position)| *position))
            .chain(registrations.iter().map(|(_, position)| *position));
        for (slot, position) in entries.enumerate() {
            // A parameter writes at `0` and binds no statement.
            if let Some(statement) = position.0.checked_sub(1) {
                statement_binders[statement as usize].push(Slot(slot as u32));
            }
        }
        Ok(Draft {
            kind,
            entered_at,
            parent_statement,
            statements: nodes.len() as u32,
            values,
            types,
            registrations,
            registered,
            rankings,
            statement_binders,
            candidates: BumpVec::new_in(scratch),
            mentions: BumpVec::new_in(scratch),
            captures: BumpVec::new_in(scratch),
            edges: BumpVec::new_in(scratch),
            reads: BumpVec::new_in(scratch),
            units: BumpVec::new_in(scratch),
            children: BumpVec::new_in(scratch),
            births: BumpVec::new_in(scratch),
            rhs: BumpVec::new_in(scratch),
            declarations: BumpVec::new_in(scratch),
            type_expressions: BumpVec::new_in(scratch),
            nodes: BumpVec::new_in(scratch),
            frame: self.frame,
            held: &[],
            component_of: BumpVec::new_in(scratch),
            members: BumpVec::new_in(scratch),
            components: BumpVec::new_in(scratch),
            signature: None,
            offers: BumpVec::new_in(scratch),
            required: BumpVec::new_in(scratch),
            code_type: KType::ANY_CODE,
            refusal: None,
            tail: false,
            surfaced: false,
            quantified: BumpVec::new_in(scratch),
            arm: None,
            current: (0, MentionClass::Eager),
            over: None,
            listed_top: BumpVec::new_in(scratch),
            unlisted: None,
            code: ContentDigest::NONE,
            named_top: BumpVec::new_in(scratch),
            top_captures: BumpVec::new_in(scratch),
        })
    }

    /// The keyed half of the binders pass: each surfaced head's registration, a parameter ranked as
    /// its head writes, then statement by statement each definition's registration under each of
    /// its keys, ranked by its chaining when it is an operator and otherwise by the declaration it
    /// sees, and each bucket declaration's ranking. Each is refused when it sees another ranking of
    /// its key, or a builtin overload there, that differs from its own.
    #[allow(clippy::type_complexity)]
    fn keyed(
        &self,
        kind: ShapeKind,
        heads: &[SurfacedKey<'graph>],
        nodes: &[&KExpression<'graph>],
    ) -> Result<
        (
            BumpVec<'x, Registered<'graph>>,
            BumpVec<'x, Ranking<'graph>>,
        ),
        ShapeError<'graph>,
    > {
        let depth = self.chain.len() as u32;
        let mut registered: BumpVec<'x, Registered<'graph>> = BumpVec::new_in(self.scratch);
        let mut rankings: BumpVec<'x, Ranking<'graph>> = BumpVec::new_in(self.scratch);
        // A surfaced key writes where a parameter does, under an index no statement takes: one
        // registration per key, over every head the operand declares there.
        let mut head_keys: BumpVec<'x, KeySymbol> =
            BumpVec::with_capacity_in(heads.len(), self.scratch);
        head_keys.extend(
            heads
                .iter()
                .map(|entry| KeyElement::key(entry.elements.iter().copied())),
        );
        let mut keys: BumpVec<'x, KeySymbol> = BumpVec::new_in(self.scratch);
        for key in head_keys.iter().copied() {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        for (index, key) in keys.iter().copied().enumerate() {
            let at = Position::PARAMETER;
            let at_key = || {
                heads
                    .iter()
                    .zip(head_keys.iter())
                    .filter(move |(_, head_key)| **head_key == key)
                    .map(|(entry, _)| entry)
            };
            let mut others = at_key();
            let first = *others.next().expect("a key has a head");
            let source = first.head.source();
            self.open_key(first.elements, source)?;
            let own = (kind, &registered[..], &rankings[..], at);
            let classes = self.surfaced_classes(own, key, first);
            if others.any(|entry| self.surfaced_classes(own, key, *entry) != classes) {
                return Err(ShapeError::RankingDisagrees {
                    key: first.elements,
                    at: source,
                });
            }
            self.agrees(own, key, first.elements, classes, source)?;
            let surfaced = at_key().map(|entry| entry.head);
            let surfaced = {
                let mut staged = BumpVec::new_in(self.scratch);
                staged.extend(surfaced);
                collect(self.brand.writer(), staged.iter().copied())
            };
            registered.push(Registered {
                symbol: RegistrationSymbol::of(key, depth, (nodes.len() + index) as u32, 0),
                at,
                key,
                elements: first.elements,
                classes,
                which: first.which,
                surfaced: Some(surfaced),
            });
        }
        for (index, node) in nodes.iter().enumerate() {
            let spine = node.statement_spine();
            let at = Position::statement(index);
            let source = node.source;
            let form = spine.cache().builtin_shape();
            if form.is_some_and(|form| form.id == BuiltinShapeId::BucketDeclaration) {
                let ranking = self.declared(spine, at)?;
                let own = (kind, &registered[..], &rankings[..], at);
                self.agrees(own, ranking.key, ranking.elements, ranking.classes, source)?;
                rankings.push(ranking);
                continue;
            }
            let Some(buckets) = spine.binder_plan().and_then(|plan| plan.buckets) else {
                continue;
            };
            let operator = form
                .and_then(|form| form.binder)
                .is_some_and(|binder| binder.surface == BinderSurface::OperatorDef);
            if !operator && ranked_head(spine) {
                return Err(ShapeError::RankedDefinition { at: source });
            }
            for (which, elements) in buckets.iter().enumerate() {
                self.open_key(elements, source)?;
                let key = KeyElement::key(elements.iter().copied());
                let own = (kind, &registered[..], &rankings[..], at);
                let classes = if operator {
                    self.operator_classes(elements)
                } else {
                    self.adopted(own, key, slots(elements))
                };
                self.agrees(own, key, elements, classes, source)?;
                registered.push(Registered {
                    symbol: RegistrationSymbol::of(key, depth, index as u32, which as u8),
                    at,
                    key,
                    elements,
                    classes,
                    which: Which::of(buckets.count(), which),
                    surfaced: None,
                });
            }
        }
        Ok((registered, rankings))
    }

    /// The ranking a bucket declaration at `at` gives its key: keywords, and an integer or `_` per
    /// slot. A name or a type in its head is `Malformed`.
    fn declared(
        &self,
        node: &KExpression<'graph>,
        at: Position,
    ) -> Result<Ranking<'graph>, ShapeError<'graph>> {
        let malformed = ShapeError::Malformed {
            form: BuiltinShapeId::BucketDeclaration,
            at: node.source,
        };
        let run = head_run(node).ok_or(malformed)?;
        let mut written = BumpVec::with_capacity_in(run.parts.len(), self.scratch);
        for part in run.parts {
            written.push(declared_element(&part.value).ok_or(malformed)?);
        }
        let writer = self.brand.writer();
        let elements = collect(writer, written.iter().map(|element| element.key()));
        self.open_key(elements, node.source)?;
        let mut raw = BumpVec::with_capacity_in(written.len(), self.scratch);
        raw.extend(written.iter().filter_map(|element| match element {
            DeclaredElement::Slot(rank) => Some(*rank),
            DeclaredElement::Keyword(_) => None,
        }));
        let classes = dense_classes(self.scratch, &raw);
        Ok(Ranking {
            key: KeyElement::key(elements.iter().copied()),
            elements,
            at,
            classes: collect(writer, classes.iter().copied()),
        })
    }

    /// Refuse a key a definition or declaration may not name: one spelling no keyword, and one a
    /// builtin expression shape closes.
    fn open_key(
        &self,
        elements: &'graph [KeyElement],
        at: SourceRef,
    ) -> Result<(), ShapeError<'graph>> {
        if !elements
            .iter()
            .any(|element| matches!(element, KeyElement::Keyword(_)))
        {
            return Err(ShapeError::NoKeyword { at });
        }
        if builtin_shape_for(elements.iter().copied()).is_some() {
            return Err(ShapeError::ClosedBucket { key: elements, at });
        }
        Ok(())
    }

    /// An operator registration's ranking, from how a run of its symbol chains: a fold-right
    /// operand first on the right, every other binary one left to right, and a unary key's one slot.
    fn operator_classes(&self, elements: &[KeyElement]) -> &'graph [u8] {
        let classes: &[u8] = match elements {
            [
                KeyElement::Slot,
                KeyElement::Keyword(symbol),
                KeyElement::Slot,
            ] => match self.frame.mode(*symbol) {
                Some(ReductionMode::FoldRight) => &[1, 0],
                _ => &[0, 1],
            },
            _ => &[0],
        };
        collect(self.brand.writer(), classes.iter().copied())
    }

    /// The ranking a surfaced head gives `key`: a signature's head the ranking its run writes, an
    /// operator its chaining, and a body's definition the ranking a definition at the block would
    /// adopt.
    fn surfaced_classes(
        &self,
        own: Own<'_, 'graph>,
        key: KeySymbol,
        entry: SurfacedKey<'graph>,
    ) -> &'graph [u8] {
        match entry.head {
            SurfacedHead::Signature { head, .. } => match head_run(head) {
                Some(run) => self.head_classes(run),
                None => self.operator_classes(entry.elements),
            },
            SurfacedHead::Body { .. } if entry.operator => self.operator_classes(entry.elements),
            SurfacedHead::Body { .. } => self.adopted(own, key, slots(entry.elements)),
        }
    }

    /// The ranking a signature member's head `run` writes: an integer per slot, or written order
    /// where it writes none.
    fn head_classes(&self, run: &KExpression<'graph>) -> &'graph [u8] {
        let parts = run.parts;
        let mut raw = BumpVec::with_capacity_in(parts.len() / 2, self.scratch);
        let mut index = 0;
        while index < parts.len() {
            if let Some(label) = slot_label(&parts[index].value)
                && next_is_type_slot(parts, index + 1)
            {
                raw.push(label.rank());
                index += 2;
                continue;
            }
            index += 1;
        }
        collect(
            self.brand.writer(),
            dense_classes(self.scratch, &raw).iter().copied(),
        )
    }

    /// A definition's ranking: the classes of the nearest bucket declaration of `key` it sees, or
    /// written order over its `slots`.
    fn adopted(&self, own: Own<'_, 'graph>, key: KeySymbol, slots: usize) -> &'graph [u8] {
        let declared = self.visible(own, self.chain.len(), key, |seen| match seen {
            Seen::Declaration(ranking) => Some(ranking.classes),
            Seen::Registration(_) => None,
        });
        declared
            .unwrap_or_else(|| collect(self.brand.writer(), (0..slots).map(|class| class as u8)))
    }

    /// Refuse `classes` for `key` where it meets another ranking of the key: a builtin overload's,
    /// which is written order, or one a statement it sees gives.
    fn agrees(
        &self,
        own: Own<'_, 'graph>,
        key: KeySymbol,
        elements: &'graph [KeyElement],
        classes: &[u8],
        at: SourceRef,
    ) -> Result<(), ShapeError<'graph>> {
        let disagrees = ShapeError::RankingDisagrees { key: elements, at };
        if !self.builtins.overloads(key).is_empty() && !written_order(classes) {
            return Err(disagrees);
        }
        let differs = self.visible(own, self.chain.len(), key, |seen| {
            (seen.classes() != classes).then_some(())
        });
        match differs {
            Some(()) => Err(disagrees),
            None => Ok(()),
        }
    }

    /// Visit each ranking of `key` a statement at `own`'s position sees, innermost first and the
    /// latest first within a body: `own`'s entries so far, then the drafts below `from` on the
    /// chain as a name read there reaches them, stopping past a program or a code draft. The walk
    /// ends at the first `Some` `visit` returns.
    fn visible<'a, R>(
        &'a self,
        (kind, registered, rankings, at): Own<'a, 'graph>,
        from: usize,
        key: KeySymbol,
        mut visit: impl FnMut(Seen<'a, 'graph>) -> Option<R>,
    ) -> Option<R> {
        let mut here = |registered: &'a [Registered<'graph>],
                        rankings: &'a [Ranking<'graph>],
                        at: Position| {
            let declarations = rankings
                .iter()
                .rev()
                .filter(|ranking| ranking.key == key && at.sees(ranking.at))
                .map(Seen::Declaration);
            let definitions = registered
                .iter()
                .rev()
                .filter(|entry| entry.key == key && at.sees(entry.at))
                .map(Seen::Registration);
            declarations.chain(definitions).find_map(&mut visit)
        };
        if let Some(found) = here(registered, rankings, at) {
            return Some(found);
        }
        if matches!(kind, ShapeKind::Program | ShapeKind::Code) {
            return None;
        }
        for draft in self.chain[..from].iter().rev() {
            if let Some(found) = here(&draft.registered, &draft.rankings, draft.boundary()) {
                return Some(found);
            }
            if matches!(draft.kind, ShapeKind::Program | ShapeKind::Code) {
                break;
            }
        }
        None
    }

    /// Walk a node's parts under `state`, by its form's roles. A formless node is a call, a grouping
    /// or a construction: a single-part group is transparent; a two-part node headed by a type name
    /// is a nominal construction, its head eager and its payload a constructor slot — which also
    /// classifies a type-constructor application written in value position; every part of a call
    /// is eager, and a call spelling a keyword is a keyworded use, whose candidates are resolved
    /// here. A binder or a bucket declaration anywhere but at its statement's root is refused.
    ///
    /// A node the rewrite built for the operator run a group mark wraps stays under the mark; any
    /// other node leaves it, so the mark covers no use in an operand.
    fn walk_node(
        &mut self,
        level: usize,
        statement: u32,
        node: &KExpression<'graph>,
        state: State,
    ) -> Result<(), ShapeError<'graph>> {
        let run = self.built_as(node).map(|built| built.run);
        let marking = self
            .marking
            .filter(|marking| marking.run.is_some() && marking.run == run);
        self.walk_marked(level, statement, node, state, marking)
    }

    /// [`walk_node`](Self::walk_node) with `marking` over `node`.
    fn walk_marked(
        &mut self,
        level: usize,
        statement: u32,
        node: &KExpression<'graph>,
        state: State,
        marking: Option<Marking>,
    ) -> Result<(), ShapeError<'graph>> {
        let outer = std::mem::replace(&mut self.marking, marking);
        let walked = self.visit_node(level, statement, node, state);
        self.marking = outer;
        walked
    }

    /// The walk of one node, under whatever mark [`walk_marked`](Self::walk_marked) set.
    fn visit_node(
        &mut self,
        level: usize,
        statement: u32,
        node: &KExpression<'graph>,
        state: State,
    ) -> Result<(), ShapeError<'graph>> {
        let wrapper = node.cache().builtin_shape().is_none()
            && matches!(node.parts, [only] if !matches!(only.value, ExpressionPart::Keyword(_)));
        if !wrapper {
            self.admits = Admits::Nothing;
        }
        let declares = node.cache().binder_plan().is_some()
            || node
                .cache()
                .builtin_shape()
                .is_some_and(|form| form.id == BuiltinShapeId::BucketDeclaration);
        let root = self.chain[level].nodes[statement as usize].statement_spine();
        if declares && !std::ptr::eq(node.parts, root.parts) {
            return Err(ShapeError::NestedBinder { at: node.source });
        }
        let Some(form) = node.cache().builtin_shape() else {
            // A one-part node is its part — save a lone keyword, which is a use of its key.
            if let [only] = node.parts
                && !matches!(only.value, ExpressionPart::Keyword(_))
            {
                return self.walk_part(level, statement, &only.value, state);
            }
            // A `NEEDING` list names what code needs; its quotes are syntax, not quote values.
            if let Some((kind, _)) = needing(node) {
                return self
                    .typed(|builder| builder.walk_part(level, statement, kind, State::Eager));
            }
            if let [head, payload] = node.parts
                && let ExpressionPart::Type(name) = &head.value
                && !self.skips(name)
            {
                self.walk_part(level, statement, &head.value, State::Eager)?;
                return self.walk_part(level, statement, &payload.value, state.constructor());
            }
            if self.in_type == 0
                && node
                    .parts
                    .iter()
                    .any(|part| matches!(part.value, ExpressionPart::Keyword(_)))
            {
                // The `NOT` a `!=` became is always the builtin's.
                let negation = self
                    .built_as(node)
                    .is_some_and(|built| built.kind == BuiltKind::Negation);
                self.candidates(level, statement, node, negation)?;
            }
            // `(f {…})`: a call by name, whose head may be a quantified `FN` or a name bound to one.
            let call = node.parts.len() == 2
                && !node
                    .parts
                    .iter()
                    .any(|part| matches!(part.value, ExpressionPart::Keyword(_)));
            for (index, part) in node.parts.iter().enumerate() {
                if call && index == 0 {
                    self.admits = Admits::Head;
                }
                self.walk_part(level, statement, &part.value, State::Eager)?;
            }
            return Ok(());
        };
        if !form.supported() {
            return Err(ShapeError::Unsupported {
                form: form.id,
                at: node.source,
            });
        }
        debug_assert_eq!(
            form.elements.len(),
            node.parts.len(),
            "a builtin shape's parts match its run"
        );
        written_as_read(form, node, self.types)?;
        // A builtin shape whose slots dispatch evaluates selects among its own builtin overloads.
        if form.dispatched() && self.in_type == 0 {
            self.candidates(level, statement, node, true)?;
        }
        // A binary operator declaring a result type of its own is admitted only where its symbol
        // chains pairwise: a fold hands its own result back as the next operand.
        if matches!(
            form.id,
            BuiltinShapeId::OperatorDefinitionReturning | BuiltinShapeId::CombinedOperatorReturning
        ) && let Ok(symbol) = groups::declaration_symbol(node)
            && !self.frame.pairwise(symbol)
        {
            return Err(ShapeError::ResultOutsidePairwise {
                symbol,
                at: node.source,
            });
        }
        if form.id == BuiltinShapeId::Eval {
            self.offer(level, statement, node)?;
        }
        // A type binder records its whole declaration node: the door reads which declaration it
        // is, and where its declared part sits, off the node's own builtin shape. The statement's
        // own spine is reached first, so a declarator nested under it records nothing.
        let draft = &mut self.chain[level];
        if state == State::Root
            && let Some(binder) = draft.statement_binders[statement as usize].first()
            && draft.is_type_slot(binder)
            && !draft.declarations.iter().any(|(held, _)| *held == binder)
        {
            draft
                .declarations
                .push((binder, resident(self.brand.writer(), *node)));
        }

        // A callable's parameters are declared before any part is read, so a type parameter a
        // signature names is never taken for a mention of the enclosing shape. A parameterized
        // union's declarator declares the parameters its variants' payloads read.
        let mut parameters = BumpVec::new_in(self.scratch);
        form_parameters(form, node, &mut parameters);
        let mark = self.skip.len();
        self.skip
            .extend(parameters.iter().filter_map(|name| match name {
                BinderSymbol::Type(name) => Some(*name),
                _ => None,
            }));
        let walked = self.walk_parts(level, statement, node, form, &parameters, state);
        self.skip.truncate(mark);
        walked
    }

    /// Walk each of a form's parts by its role, with the form's type parameters on the skip stack.
    #[allow(clippy::too_many_arguments)]
    fn walk_parts(
        &mut self,
        level: usize,
        statement: u32,
        node: &KExpression<'graph>,
        form: &'static BuiltinShape,
        parameters: &[BinderSymbol],
        state: State,
    ) -> Result<(), ShapeError<'graph>> {
        for (role, part) in form.roles().zip(node.parts) {
            let part = &part.value;
            match role {
                // An `OVER` list names captures, read where the module body's shape is built.
                Role::Keyword | Role::Name | Role::Data | Role::Captures => {}
                // A bare name is the label itself; any other label is evaluated.
                Role::Field => {
                    if role.label_reads(part).is_none() {
                        self.walk_part(level, statement, part, State::Eager)?
                    }
                }
                // A group's names are the body's; its bounds are read where the form runs, as the
                // signature's types are. The group's own names are already skipped, so a bound
                // naming one records nothing and the elaborator refuses it.
                Role::Quantifiers => {
                    for bound in quantifier_bounds(part) {
                        self.typed(|builder| {
                            builder.walk_part(level, statement, bound, State::Eager)
                        })?;
                    }
                }
                Role::Rhs => {
                    let draft = &mut self.chain[level];
                    let mut declares_type = false;
                    if state == State::Root
                        && let Some(binder) = draft.statement_binders[statement as usize].first()
                    {
                        draft.rhs.push((binder, Site::of(part)));
                        self.parts.insert(Site::of(part), part);
                        declares_type = draft.is_type_slot(binder);
                    }
                    // A type `LET`'s right-hand side is its declaration's definition, typed with
                    // the binder rather than as a type expression of its own.
                    if declares_type {
                        self.typed(|builder| builder.walk_part(level, statement, part, state))?
                    } else {
                        self.walk_part(level, statement, part, state)?
                    }
                }
                Role::Argument | Role::InPlace => {
                    self.walk_part(level, statement, part, State::Eager)?
                }
                Role::TypeExpression => {
                    if self.in_type == 0 && !types_with_its_binder(form) {
                        self.record_type(level, statement, part, None);
                    }
                    self.typed(|builder| builder.walk_part(level, statement, part, State::Eager))?
                }
                Role::Signature | Role::Head => {
                    self.typed(|builder| builder.walk_signature(level, statement, part))?
                }
                Role::Body(kind) => {
                    self.enter_body(level, statement, node, part, kind, parameters, state)?
                }
                Role::Branches(heads) => {
                    self.enter_arms(level, statement, node, part, heads, state)?
                }
                Role::Definition(kind) => self.typed(|builder| {
                    builder.walk_definition(level, statement, part, kind, state.constructor())
                })?,
                Role::Unsupported => unreachable!("an unsupported form returned above"),
            }
        }
        Ok(())
    }

    fn walk_part(
        &mut self,
        level: usize,
        statement: u32,
        part: &'graph ExpressionPart<'graph>,
        state: State,
    ) -> Result<(), ShapeError<'graph>> {
        // A node passes the allowance on to its own visit; every other part takes it here.
        let admits = if matches!(part, ExpressionPart::Expression(_)) {
            self.admits
        } else {
            std::mem::take(&mut self.admits)
        };
        match part {
            ExpressionPart::Identifier(name) => self.mention_through(
                level,
                statement,
                part,
                BinderSymbol::Value(*name),
                None,
                state,
                admits == Admits::Head,
            ),
            ExpressionPart::Type(name) if !self.skips(name) => {
                self.mention(level, statement, part, BinderSymbol::Type(*name), state)
            }
            ExpressionPart::Expression(node) => {
                // A block the pairwise rewrite synthesized runs as a block: its hoists are its
                // binders, and its value is its last statement's.
                if self
                    .built_as(node.reference())
                    .is_some_and(|built| built.kind == BuiltKind::Block)
                {
                    self.admits = Admits::Nothing;
                    return self.enter_child(
                        level,
                        statement,
                        MentionClass::Eager,
                        Site::of(part),
                        ShapeKind::Block,
                        Entry::PLAIN,
                        node.reference().body_statements(),
                    );
                }
                self.walk_node(level, statement, node.reference(), state)
            }
            ExpressionPart::SigiledTypeExpr(node) => {
                if self.in_type == 0 {
                    self.record_type(level, statement, part, None);
                }
                self.typed(|builder| {
                    builder.walk_node(level, statement, node.reference(), State::Eager)
                })
            }
            ExpressionPart::RecordType(node) => {
                if self.in_type == 0 {
                    self.record_type(level, statement, part, None);
                }
                self.typed(|builder| {
                    builder.walk_fields(level, statement, node.reference(), State::Eager)
                })
            }
            ExpressionPart::ListLiteral(items) => {
                for item in items.iter() {
                    self.walk_part(level, statement, item, state.constructor())?;
                }
                Ok(())
            }
            ExpressionPart::DictLiteral(pairs) => {
                // A value dict's `_` default has no reading yet; an arm set's `_` never reaches
                // here, since arms are read as a container.
                if pairs.iter().any(|(key, _)| key.is_wildcard()) {
                    return Err(ShapeError::DictDefault {
                        site: Site::of(part),
                        at: self.part_source(level, statement, Site::of(part)),
                    });
                }
                for (key, value) in pairs.iter() {
                    self.walk_part(level, statement, key, State::Eager)?;
                    self.walk_part(level, statement, value, state.constructor())?;
                }
                Ok(())
            }
            ExpressionPart::RecordLiteral(pairs) => {
                for (_, value) in pairs.iter() {
                    self.walk_part(level, statement, value, state.constructor())?;
                }
                Ok(())
            }
            ExpressionPart::QuotedExpression(node) => {
                self.enter_code(level, statement, part, node.reference(), state)
            }
            ExpressionPart::MarkedName(mark, name) => self.mention_through(
                level,
                statement,
                part,
                *name,
                Some(*mark),
                state,
                admits == Admits::Head,
            ),
            ExpressionPart::MarkedUse(..) if !self.in_quote(level) => {
                Err(ShapeError::MarkOutsideQuote {
                    at: self.part_source(level, statement, Site::of(part)),
                })
            }
            // A group mark says where the keyworded use it wraps resolves — and each use of the
            // operator run that node tops; the names inside it are read as any others are.
            ExpressionPart::MarkedUse(mark, node) => {
                let node = node.reference();
                let marking = Marking {
                    mark: *mark,
                    run: self.built_as(node).map(|built| built.run),
                };
                self.walk_marked(level, statement, node, State::Eager, Some(marking))
            }
            ExpressionPart::Type(_) | ExpressionPart::Keyword(_) | ExpressionPart::Literal(_) => {
                Ok(())
            }
        }
    }

    /// A field list or a signature run: each pair's name is a label, its type is read under
    /// `state`.
    fn walk_fields(
        &mut self,
        level: usize,
        statement: u32,
        run: &KExpression<'graph>,
        state: State,
    ) -> Result<(), ShapeError<'graph>> {
        let mut index = 0;
        while index < run.parts.len() {
            if pair_label(run, index).is_some() {
                self.walk_part(level, statement, &run.parts[index + 1].value, state)?;
                index += 2;
            } else {
                self.walk_part(level, statement, &run.parts[index].value, state)?;
                index += 1;
            }
        }
        Ok(())
    }

    fn walk_signature(
        &mut self,
        level: usize,
        statement: u32,
        part: &'graph ExpressionPart<'graph>,
    ) -> Result<(), ShapeError<'graph>> {
        match signature_run(part) {
            Some(run) => self.walk_fields(level, statement, run, State::Eager),
            None => self.walk_part(level, statement, part, State::Eager),
        }
    }

    /// A type declaration's definition, under the constructor state: symbols and every name the
    /// definition itself declares are not mentions, and every other type name is. A union's
    /// variants are a dict of tag quotes to payload quotes, and a signature's members a list of
    /// member quotes; each quote is read where it is written.
    fn walk_definition(
        &mut self,
        level: usize,
        statement: u32,
        part: &'graph ExpressionPart<'graph>,
        kind: DefinitionKind,
        state: State,
    ) -> Result<(), ShapeError<'graph>> {
        match (kind, part) {
            // A tag names a variant and is no mention; a payload is a type expression.
            (DefinitionKind::Union, ExpressionPart::DictLiteral(variants)) => {
                for (_, payload) in variants.iter() {
                    if let Some(payload) = quoted_body(payload) {
                        for inner in payload.parts {
                            self.walk_definition_part(level, statement, &inner.value, state)?;
                        }
                    }
                }
                Ok(())
            }
            (DefinitionKind::Members, ExpressionPart::ListLiteral(members)) => {
                // A signature declares its own names — its manifest `LET` members, beside the head
                // parameters its group declares — so a later member naming one is no mention of the
                // enclosing shape. The door resolves them against the definition it is elaborating.
                let mut own = BumpVec::new_in(self.scratch);
                own.extend(members.iter().filter_map(quoted_body).filter_map(|member| {
                    match member.statement_binder_plan()?.name? {
                        BinderSymbol::Type(name) => Some(name),
                        _ => None,
                    }
                }));
                let mark = self.skip.len();
                self.skip.extend_from_slice(&own);
                let mut walked = Ok(());
                for member in members.iter().filter_map(quoted_body) {
                    let member = member.statement_spine();
                    if let Some(form) = member.cache().builtin_shape() {
                        walked =
                            self.walk_definition_statement(level, statement, member, form, state);
                        if walked.is_err() {
                            break;
                        }
                    }
                }
                self.skip.truncate(mark);
                walked
            }
            _ => self.walk_definition_part(level, statement, part, state),
        }
    }

    /// One statement of a definition, by its own builtin shape's roles — the same authority the
    /// top-level walk reads a form's parts through. Its name, symbols, quoted data and `FOR ALL`
    /// names are the statement's own; everything else is read under the definition's state, so a
    /// `SIG` body's `VAL` type is as deferred as the definition holding it.
    fn walk_definition_statement(
        &mut self,
        level: usize,
        statement: u32,
        node: &KExpression<'graph>,
        form: &'static BuiltinShape,
        state: State,
    ) -> Result<(), ShapeError<'graph>> {
        if !form.supported() {
            return Err(ShapeError::Unsupported {
                form: form.id,
                at: node.source,
            });
        }
        written_as_read(form, node, self.types)?;
        let mut parameters = BumpVec::new_in(self.scratch);
        form_parameters(form, node, &mut parameters);
        let mark = self.skip.len();
        self.skip
            .extend(parameters.iter().filter_map(|name| match name {
                BinderSymbol::Type(name) => Some(*name),
                _ => None,
            }));
        let walked = self.walk_definition_roles(level, statement, node, form, state);
        self.skip.truncate(mark);
        walked
    }

    fn walk_definition_roles(
        &mut self,
        level: usize,
        statement: u32,
        node: &KExpression<'graph>,
        form: &'static BuiltinShape,
        state: State,
    ) -> Result<(), ShapeError<'graph>> {
        for (role, part) in form.roles().zip(node.parts) {
            let part = &part.value;
            match role {
                Role::Keyword | Role::Data | Role::Captures => {}
                Role::Quantifiers => {
                    for bound in quantifier_bounds(part) {
                        self.walk_definition_part(level, statement, bound, state)?;
                    }
                }
                Role::Name => {}
                Role::Definition(inner) => {
                    self.walk_definition(level, statement, part, inner, state)?
                }
                Role::Body(_) | Role::Branches(_) | Role::Unsupported => {
                    return Err(ShapeError::Unsupported {
                        form: form.id,
                        at: node.source,
                    });
                }
                // A bare name is the label itself; any other label is evaluated.
                Role::Field if role.label_reads(part).is_some() => {}
                Role::Rhs
                | Role::Argument
                | Role::InPlace
                | Role::Field
                | Role::TypeExpression
                | Role::Signature
                | Role::Head => self.walk_definition_part(level, statement, part, state)?,
            }
        }
        Ok(())
    }

    fn walk_definition_part(
        &mut self,
        level: usize,
        statement: u32,
        part: &'graph ExpressionPart<'graph>,
        state: State,
    ) -> Result<(), ShapeError<'graph>> {
        let run = match part {
            ExpressionPart::Type(name) if !self.skips(name) => {
                return self.mention(level, statement, part, BinderSymbol::Type(*name), state);
            }
            // A marked name never binds to a name the definition declares, so it skips nothing.
            ExpressionPart::MarkedName(mark, name) => {
                return self.mention_through(
                    level,
                    statement,
                    part,
                    *name,
                    Some(*mark),
                    state,
                    false,
                );
            }
            ExpressionPart::MarkedUse(..) if !self.in_quote(level) => {
                return Err(ShapeError::MarkOutsideQuote {
                    at: self.part_source(level, statement, Site::of(part)),
                });
            }
            // A type expression written inside a definition, and a quoted head, are nodes with
            // builtin shapes of their own: read their parts by their roles rather than descending
            // into them blind.
            ExpressionPart::Expression(run) | ExpressionPart::SigiledTypeExpr(run)
                if let Some(form) = run.statement_spine().cache().builtin_shape() =>
            {
                return self.walk_definition_statement(
                    level,
                    statement,
                    run.statement_spine(),
                    form,
                    state,
                );
            }
            ExpressionPart::Expression(run)
            | ExpressionPart::SigiledTypeExpr(run)
            | ExpressionPart::RecordType(run)
            | ExpressionPart::QuotedExpression(run)
            | ExpressionPart::MarkedUse(_, run) => run.reference(),
            ExpressionPart::ListLiteral(_)
            | ExpressionPart::DictLiteral(_)
            | ExpressionPart::RecordLiteral(_) => {
                return self.walk_part(level, statement, part, state);
            }
            ExpressionPart::Type(_)
            | ExpressionPart::Identifier(_)
            | ExpressionPart::Keyword(_)
            | ExpressionPart::Literal(_) => return Ok(()),
        };
        for inner in run.parts {
            self.walk_definition_part(level, statement, &inner.value, state)?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn enter_body(
        &mut self,
        level: usize,
        statement: u32,
        node: &KExpression<'graph>,
        part: &ExpressionPart<'graph>,
        kind: BodyKind,
        signature_names: &[BinderSymbol],
        state: State,
    ) -> Result<(), ShapeError<'graph>> {
        // A callable's body is a written quote; an in-place body is a bare group. The static check
        // has already refused any other spelling.
        let written = match Role::Body(kind).reading() {
            Reading::Quote => quoted_body(part),
            _ => body_of(part),
        };
        let Some(body) = written else {
            return Err(ShapeError::Malformed {
                form: node
                    .cache()
                    .builtin_shape()
                    .expect("a body role is a form's")
                    .id,
                at: node.source,
            });
        };
        // A module body has no callable type, so it records no form; it is still what its binder
        // births. A `USING` body is a block of the enclosing shape and births nothing.
        let site = Site::of(part);
        if kind != BodyKind::Surfaced {
            if kind != BodyKind::Module {
                self.forms
                    .insert(site, resident(self.brand.writer(), *node));
            }
            // Every binder of the statement births the one body: a combined form's name and each
            // registration alike.
            let draft = &mut self.chain[level];
            if state == State::Root {
                let binders = draft.statement_binders[statement as usize];
                draft
                    .births
                    .extend(binders.iter().map(|binder| (binder, site)));
            }
        }
        let (shape_kind, class) = if kind.is_callable() {
            (ShapeKind::Callable, state.constructor().class())
        } else if kind == BodyKind::Module {
            (ShapeKind::Module, MentionClass::Eager)
        } else {
            (ShapeKind::Block, MentionClass::Eager)
        };
        let operator = [
            BinderSymbol::Value(IMPLICIT.left.symbol()),
            BinderSymbol::Value(IMPLICIT.right.symbol()),
        ];
        let unary = [BinderSymbol::Value(IMPLICIT.operands.symbol())];
        let at = node.source;
        let mut surfaced = Surfaced::new(self.scratch);
        let kept;
        let mut held: &[&'graph DeclaredGroup<'graph>] = &[];
        if kind == BodyKind::Surfaced {
            self.surfaced(level, statement, node, &mut surfaced)?;
            kept = self.surfaced_groups(at, &surfaced)?;
            held = &kept;
        }
        // A `GROUP`'s own body holds its group — the one record every equal declaration shares —
        // so an operator run of its members may be written inside it and nowhere else.
        let own;
        if kind == BodyKind::Module
            && let Some(declared) =
                groups::declared_group(node, self.scratch).map_err(|()| ShapeError::Malformed {
                    form: node
                        .cache()
                        .builtin_shape()
                        .expect("a body role is a form's")
                        .id,
                    at,
                })?
            && let Some(Claim::Group(record)) = self.claims.get(declared.members[0])
        {
            for member in record.members {
                if let Some(visible) = self.frame.visible(*member)
                    && !groups::groups_equal(visible, record)
                {
                    return Err(ShapeError::RedeclaresGroup {
                        symbol: *member,
                        at,
                    });
                }
            }
            own = [record];
            held = &own;
        }
        let signature = if kind == BodyKind::Lambda {
            let form = node
                .cache()
                .builtin_shape()
                .expect("a body role is a form's");
            form.roles()
                .zip(node.parts)
                .find(|(role, _)| matches!(role, Role::Signature | Role::Head))
                .and_then(|(_, part)| signature_run(&part.value))
        } else {
            None
        };
        let parameters: &[BinderSymbol] = match kind {
            BodyKind::Lambda => signature_names,
            BodyKind::Operator => &operator,
            BodyKind::UnaryOperator => &unary,
            BodyKind::Module => &[],
            BodyKind::Surfaced => &surfaced.names,
        };
        let mut paired = BumpVec::with_capacity_in(parameters.len(), self.scratch);
        paired.extend(parameters.iter().map(|name| (*name, at)));
        let over = match kind {
            BodyKind::Module => Some(self.over(node)?),
            _ => None,
        };
        let entry = Entry {
            tail: shape_kind == ShapeKind::Callable,
            arm: None,
            parameters: &paired,
            signature,
            surfacing: (kind == BodyKind::Surfaced).then_some(Surfacing {
                quantified: &surfaced.quantified,
                heads: &surfaced.heads,
            }),
            held,
            over,
        };
        self.enter_child(
            level,
            statement,
            class,
            Site::of(part),
            shape_kind,
            entry,
            body.body_statements(),
        )
    }

    /// An arm set: a dict of guard quotes to arm quotes, each arm a block binding `it`. A guard
    /// written under `MATCH … WITH` is a type read where the match runs; a label names a variant
    /// or an error kind and is no mention; `_` is the default arm. An arm's last statement is in
    /// tail position exactly where the `MATCH` or `TRY` is: at the root of a body's last statement
    /// that binds nothing, in a tail body.
    fn enter_arms(
        &mut self,
        level: usize,
        statement: u32,
        node: &KExpression<'graph>,
        part: &ExpressionPart<'graph>,
        heads: Heads,
        state: State,
    ) -> Result<(), ShapeError<'graph>> {
        let ExpressionPart::DictLiteral(arms) = part else {
            unreachable!("the static check admits only a dict of quotes")
        };
        let draft = &self.chain[level];
        let tail = state == State::Root
            && draft.tail
            && draft.statement_binders[statement as usize].is_empty()
            && statement + 1 == draft.statements;
        for (index, (guard, body_part)) in arms.iter().enumerate() {
            let guard = (!guard.is_wildcard()).then_some(guard);
            if heads == Heads::Types
                && let Some(guard) = guard
            {
                self.record_type(
                    level,
                    statement,
                    guard,
                    Some((Site::of(part), index as u32)),
                );
            }
            if heads == Heads::Types
                && let Some(written) = guard.and_then(quoted_body)
            {
                for inner in written.parts {
                    self.typed(|builder| {
                        builder.walk_part(level, statement, &inner.value, State::Eager)
                    })?;
                }
            }
            let body = quoted_body(body_part).expect("the static check admits only quotes");
            let it = [(BinderSymbol::Value(IMPLICIT.it.symbol()), node.source)];
            let entry = Entry {
                tail,
                arm: Some(Arm { guard, heads, tail }),
                parameters: &it,
                ..Entry::PLAIN
            };
            self.enter_child(
                level,
                statement,
                MentionClass::Eager,
                Site::of(body_part),
                ShapeKind::Block,
                entry,
                body.body_statements(),
            )?;
        }
        Ok(())
    }

    /// A quote value: its code, drafted as a code shape nested at the quote's site and entered where
    /// the quote is written, as a callable body is — so a `$` name reads at the statement, or at
    /// the body's end under a constructor slot. The code chains its operator runs under the builtin
    /// groups and the groups it holds, never a frame around it, and a type parameter of a form
    /// around it is no parameter of its own. Code that cannot be built keeps its error as the
    /// shape's refusal; only a `$` name or `$(…)` use nothing binds where the quote is written
    /// refuses the program. The code's carried type needs each `\` name and `\(…)` key it leaves
    /// open.
    fn enter_code(
        &mut self,
        level: usize,
        statement: u32,
        part: &ExpressionPart<'graph>,
        code: &KExpression<'graph>,
        state: State,
    ) -> Result<(), ShapeError<'graph>> {
        let parent = &mut self.chain[level];
        parent.current = (statement, state.constructor().class());
        let entered_at = parent.boundary();
        let built = self.nested(|builder| builder.code_draft(entered_at, code));
        let child = match built {
            Ok(mut draft) => {
                let mut offered = BumpVec::new_in(self.scratch);
                offered.extend(
                    draft
                        .captures
                        .iter()
                        .filter(|capture| capture.source == CaptureSource::Offered)
                        .map(|capture| capture.name),
                );
                draft.code_type = self
                    .types
                    .code_needing(self.scratch, code.code_kind(), &offered);
                draft
            }
            Err(refusal) => self.refused_code(level, statement, entered_at, code, refusal)?,
        };
        self.chain[level].children.push((Site::of(part), child));
        Ok(())
    }

    /// [`enter_code`](Self::enter_code)'s draft, under the code's own claims and a frame holding
    /// nothing.
    fn code_draft(
        &mut self,
        entered_at: Position,
        code: &KExpression<'graph>,
    ) -> Result<Draft<'graph, 'x>, ShapeError<'graph>> {
        let statements = code.body_statements().map(|(statement, _)| statement);
        self.claims = groups::claims(self.brand, self.scratch, statements, Some(self.claims))?;
        self.frame = resident(self.brand.writer(), GroupFrame::new(&[], None, self.claims));
        let entry = Entry {
            tail: true,
            ..Entry::PLAIN
        };
        self.draft(ShapeKind::Code, entry, entered_at, code.body_statements())
    }

    /// The code draft of a quote whose code could not be built: no statements, its `$` names and
    /// `$(…)` uses captured where the quote is written — each as the built draft would have
    /// captured it — and `refusal` kept. Its carried type needs every `\` name and `\(…)` key its
    /// code writes outside the quote values nested in it, since which of them its own binders fill
    /// is unknown.
    fn refused_code(
        &mut self,
        level: usize,
        statement: u32,
        entered_at: Position,
        code: &KExpression<'graph>,
        refusal: ShapeError<'graph>,
    ) -> Result<Draft<'graph, 'x>, ShapeError<'graph>> {
        let mut marks = BumpVec::new_in(self.scratch);
        code_marks(code, &mut marks);
        // A `$` name reads where the quote is written, so a quantified function read through one
        // anywhere but a call's head is the program's error.
        if let ShapeError::QuantifiedRead { site, .. } = refusal
            && marks.iter().any(|(mark, marked, at)| {
                *mark == Mark::Written && matches!(marked, Marked::Name(_)) && *at == site
            })
        {
            return Err(refusal);
        }
        let draft = self.binders(ShapeKind::Code, entered_at, statement, (&[], &[]), &[])?;
        self.chain.push(draft);
        let inner = self.chain.len() - 1;
        let reader = Reader {
            level: inner,
            statement: 0,
            class: MentionClass::Eager,
        };
        let resolved = self.written_marks(level, statement, inner, reader, &marks);
        // Code that cannot be built digests as written.
        let mut statements = BumpVec::new_in(self.scratch);
        statements.extend(code.body_statements().map(|(statement, _)| *statement));
        self.digest_draft(inner, &statements);
        let mut draft = self.chain.pop().expect("this draft was pushed above");
        resolved?;
        let mut built = BumpVec::new_in(self.scratch);
        built.extend(marks.iter().filter_map(|(mark, marked, _)| match marked {
            _ if *mark != Mark::Built => None,
            Marked::Name(name) => self.builtins.lookup(*name).is_none().then_some(*name),
            Marked::Use(node) => Some(BinderSymbol::Key(self.spelled(node.cache().stored_key()))),
        }));
        draft.code_type = self
            .types
            .code_needing(self.scratch, code.code_kind(), &built);
        draft.refusal = Some(resident(self.brand.writer(), refusal));
        Ok(draft)
    }

    /// Resolve each `$` mark of a refused code draft at `inner` where its quote is written, at
    /// `level`: a name is `Unbound`, and a use `NoCandidate`, when nothing there binds it.
    fn written_marks(
        &mut self,
        level: usize,
        statement: u32,
        inner: usize,
        reader: Reader,
        marks: &[(Mark, Marked<'graph>, Site)],
    ) -> Result<(), ShapeError<'graph>> {
        let written = Some(Mark::Written);
        for (mark, marked, site) in marks.iter().copied() {
            if mark != Mark::Written {
                continue;
            }
            match marked {
                Marked::Name(name) if self.builtins.lookup(name).is_some() => {}
                Marked::Name(name) => {
                    if self
                        .resolve(inner, name, written, Position::PARAMETER, reader)
                        .is_none()
                    {
                        let at = self.part_source(level, statement, site);
                        return Err(ShapeError::Unbound { name, site, at });
                    }
                }
                Marked::Use(node) => {
                    let listing = Listing {
                        level: inner,
                        reader,
                        site: level,
                        at: self.chain[level].boundary(),
                        mark: written,
                        closed: false,
                    };
                    let elements = node.cache().stored_key();
                    let mut candidates = BumpVec::new_in(self.scratch);
                    self.listed(listing, elements, node.source, &mut candidates)?;
                    let surfaced = self.chain[..=level].iter().any(|draft| draft.surfaced);
                    if candidates.is_empty() && !surfaced {
                        return Err(ShapeError::NoCandidate {
                            key: elements,
                            at: node.source,
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// An `EVAL` of a callable's parameter whose type needs names offers each of them to the code
    /// it runs: a name resolves here as an eager read at the `EVAL`'s statement would, recorded as
    /// one for the units pass, and is `Unbound` when nothing binds it here and `QuantifiedRead`
    /// when it is bound to a quantified function; a key is listed as a use
    /// at the key written at the `EVAL` would be, and is `NoCandidate` when that lists nothing.
    fn offer(
        &mut self,
        level: usize,
        statement: u32,
        node: &KExpression<'graph>,
    ) -> Result<(), ShapeError<'graph>> {
        let operand = &node.parts[1].value;
        let ExpressionPart::Identifier(name) = operand else {
            return Ok(());
        };
        let at = Position::statement(statement as usize);
        let Some(needed) = self.needed_by(level, BinderSymbol::Value(*name), at) else {
            return Ok(());
        };
        let reader = Reader {
            level,
            statement,
            class: MentionClass::Eager,
        };
        let writer = self.brand.writer();
        let mut offered = BumpVec::with_capacity_in(needed.len(), self.scratch);
        for entry in needed.iter() {
            if let Some(name) = needed_name(entry) {
                // An offer passes the name's value into the code, which a quantified function's
                // name never is.
                if self.quantified(level, name, at, None) != Quantified::No {
                    return Err(ShapeError::QuantifiedRead {
                        name,
                        site: Site::of(operand),
                        at: node.source,
                    });
                }
                let coordinate = match self.builtins.lookup(name) {
                    Some(index) => Coordinate::Builtin(index),
                    None => {
                        self.resolve(level, name, None, at, reader)
                            .ok_or(ShapeError::Unbound {
                                name,
                                site: Site::of(operand),
                                at: node.source,
                            })?
                    }
                };
                offered.push((name, Offer::Name(coordinate)));
                continue;
            }
            // A malformed entry is the elaborator's to refuse.
            let Some(run) = needed_key(entry) else {
                continue;
            };
            let mut staged = BumpVec::new_in(self.scratch);
            staged.extend(run);
            let elements = collect(writer, staged.iter().copied());
            let key = KeyElement::key(elements.iter().copied());
            let listing = Listing {
                level,
                reader,
                site: level,
                at,
                mark: None,
                closed: false,
            };
            let mut candidates = BumpVec::new_in(self.scratch);
            let classes = self.listed(listing, elements, node.source, &mut candidates)?;
            let surfaced = self.chain[..=level].iter().any(|draft| draft.surfaced);
            if candidates.is_empty() && !surfaced {
                return Err(ShapeError::NoCandidate {
                    key: elements,
                    at: node.source,
                });
            }
            let list = CandidateList {
                key,
                elements,
                classes,
                candidates: collect(writer, candidates.iter().copied()),
            };
            offered.push((BinderSymbol::Key(key), Offer::Key(resident(writer, list))));
        }
        let offered = collect(writer, offered.iter().copied());
        self.chain[level].offers.push((Site::of(operand), offered));
        Ok(())
    }

    /// The `NEEDING` list of the parameter `name` names, when `name` read at `at` in the draft at
    /// `level` is a callable's parameter typed `:(<kind> NEEDING #[…])`. A name a code draft does
    /// not bind is a hole of it, so the search stops there.
    fn needed_by(
        &self,
        mut level: usize,
        name: BinderSymbol,
        mut at: Position,
    ) -> Option<&'graph [ExpressionPart<'graph>]> {
        loop {
            let draft = &self.chain[level];
            let channels = draft.channels();
            if let Some(index) = channels.find(name)
                && at.sees(channels.get(index))
            {
                if draft.kind != ShapeKind::Callable || channels.get(index) != Position::PARAMETER {
                    return None;
                }
                let ExpressionPart::SigiledTypeExpr(written) =
                    parameter_type(draft.signature?, name)?
                else {
                    return None;
                };
                return needing(written.reference()).map(|(_, names)| names);
            }
            if matches!(draft.kind, ShapeKind::Program | ShapeKind::Code) {
                return None;
            }
            level = level.checked_sub(1)?;
            at = self.chain[level].boundary();
        }
    }

    /// Build a nested draft from `entry` under the statement being walked at `level`, entered with
    /// `class`, and hold it under `site` until this draft's components settle its captures.
    #[allow(clippy::too_many_arguments)]
    fn enter_child<'n>(
        &mut self,
        level: usize,
        statement: u32,
        class: MentionClass,
        site: Site,
        kind: ShapeKind,
        entry: Entry<'_, 'graph>,
        statements: impl Iterator<Item = (&'n KExpression<'graph>, usize)>,
    ) -> Result<(), ShapeError<'graph>>
    where
        'graph: 'n,
    {
        let parent = &mut self.chain[level];
        parent.current = (statement, class);
        let entered_at = parent.boundary();
        let child = self.nested(|builder| builder.draft(kind, entry, entered_at, statements));
        self.chain[level].children.push((site, child?));
        Ok(())
    }

    /// Run `build` — one nested draft — with the walk state its enclosing draft's walk holds set
    /// aside: the claims, the group frame, the skip stack and its floor, the type depth, the group
    /// mark, what the next part admits, and the chain's height. Each is restored after `build`,
    /// whether or not it succeeded, so a refused code leaves nothing for the next draft. The nested
    /// draft starts with its own skip floor and outside any type expression.
    fn nested<R>(&mut self, build: impl FnOnce(&mut Self) -> R) -> R {
        let saved = (self.claims, self.frame, self.marking, self.admits);
        let (skip, floor, height) = (self.skip.len(), self.skip_floor, self.chain.len());
        let in_type = std::mem::replace(&mut self.in_type, 0);
        self.skip_floor = skip;
        let built = build(self);
        (self.claims, self.frame, self.marking, self.admits) = saved;
        self.skip.truncate(skip);
        self.skip_floor = floor;
        self.chain.truncate(height);
        self.in_type = in_type;
        built
    }

    /// Whether `name` is a type parameter of a form enclosing the walk within the current draft.
    fn skips(&self, name: &TypeSymbol) -> bool {
        self.skip[self.skip_floor..].contains(name)
    }

    /// Record `part`, written in `statement` of the draft at `level`, as a type expression the load
    /// pass types on its own — for a guard, beside its arm set's site and its place there.
    fn record_type(
        &mut self,
        level: usize,
        statement: u32,
        part: &'graph ExpressionPart<'graph>,
        guard: Option<(Site, u32)>,
    ) {
        self.chain[level].type_expressions.push(Recorded {
            part,
            statement,
            guard,
        });
    }

    /// Run `walk` inside a type expression.
    fn typed<R>(&mut self, walk: impl FnOnce(&mut Self) -> R) -> R {
        self.in_type += 1;
        let walked = walk(self);
        self.in_type -= 1;
        walked
    }

    /// Resolve the keyworded use `node`, read at its statement: the builtin overloads at its key,
    /// then — unless `closed`, a builtin shape's own bucket or a `!=`'s `NOT` — each registration
    /// at the key visible to it, resolved as an eager read there, so each is a mention of the use's
    /// statement. In a quote's code the use's mark decides: unmarked, the registrations it sees
    /// within the code and then its key as a hole a `USING` fills; `$(…)`, what a use written
    /// where the quote is sees, captured; `\(…)`, the registrations it sees within the code and
    /// then its key as a `\` capture, which the `EVAL` running the code offers. Every registration
    /// must carry one ranking, and a use with no candidate is refused — except under a
    /// `USING … SCOPE` body, whose operand's registrations are candidates this builder does not
    /// read.
    fn candidates(
        &mut self,
        level: usize,
        statement: u32,
        node: &KExpression<'graph>,
        closed: bool,
    ) -> Result<(), ShapeError<'graph>> {
        let elements = node.cache().stored_key();
        let key = KeyElement::key(elements.iter().copied());
        let at = Position::statement(statement as usize);
        let reader = Reader {
            level,
            statement,
            class: MentionClass::Eager,
        };
        let code = self.chain[..=level]
            .iter()
            .rposition(|draft| draft.kind == ShapeKind::Code);
        let mark = self.marking.filter(|_| !closed).map(|marking| marking.mark);
        let mut candidates = BumpVec::new_in(self.scratch);
        let classes = match (mark, code) {
            (Some(Mark::Built), _) => {
                // The listing an unmarked use makes, which stops at the code root, less the
                // builtins: a `\(…)` lists none.
                let listing = Listing {
                    level,
                    reader,
                    site: level,
                    at,
                    mark: None,
                    closed: false,
                };
                let classes =
                    self.registered(listing, elements, node.source, &mut candidates, None)?;
                let open = BinderSymbol::Key(self.spelled(elements));
                let offered = self
                    .resolve(level, open, Some(Mark::Built), at, reader)
                    .expect("a quote's code leaves a `\\` key open");
                candidates.push(Candidate::Spread(offered));
                classes
            }
            (Some(Mark::Written), Some(code)) => {
                let site = code - 1;
                let written_at = self.chain[site].boundary();
                let listing = Listing {
                    level,
                    reader,
                    site,
                    at: written_at,
                    mark: Some(Mark::Written),
                    closed: false,
                };
                self.listed(listing, elements, node.source, &mut candidates)?
            }
            (Some(_), None) => unreachable!("a mark outside a quote is refused"),
            (None, _) => {
                let listing = Listing {
                    level,
                    reader,
                    site: level,
                    at,
                    mark: None,
                    closed,
                };
                let classes = self.listed(listing, elements, node.source, &mut candidates)?;
                if let Some(code) = code
                    && !closed
                {
                    let required = &mut self.chain[code].required;
                    if candidates.is_empty() && !required.contains(&key) {
                        required.push(key);
                    }
                    let open = BinderSymbol::Key(self.spelled(elements));
                    let hole = self
                        .resolve(level, open, None, at, reader)
                        .expect("a quote's code leaves its holes open");
                    candidates.push(Candidate::Spread(hole));
                }
                classes
            }
        };
        let surfaced = self.chain[..=level].iter().any(|draft| draft.surfaced);
        if candidates.is_empty() && !closed && !surfaced {
            return Err(ShapeError::NoCandidate {
                key: elements,
                at: node.source,
            });
        }
        let list = CandidateList {
            key,
            elements,
            classes,
            candidates: collect(self.brand.writer(), candidates.iter().copied()),
        };
        self.chain[level]
            .candidates
            .push((Site::of_node(node), list));
        Ok(())
    }

    /// The builtin overloads at the key `elements` spell, then — unless the listing is closed —
    /// each registration at the key a use at the listing's site sees, each resolved from its
    /// level through its mark: pushed onto `candidates`, and the ranking they share returned. Two
    /// rankings among them are refused at `source`.
    fn listed(
        &mut self,
        listing: Listing,
        elements: &'graph [KeyElement],
        source: SourceRef,
        candidates: &mut BumpVec<'x, Candidate>,
    ) -> Result<&'graph [u8], ShapeError<'graph>> {
        let key = KeyElement::key(elements.iter().copied());
        let before = candidates.len();
        candidates.extend(
            self.builtins
                .overloads(key)
                .map(|index| Candidate::One(Coordinate::Builtin(BuiltinIndex(index)))),
        );
        let classes = (candidates.len() > before).then(|| self.written(elements));
        self.registered(listing, elements, source, candidates, classes)
    }

    /// [`listed`](Self::listed)'s registrations: unless the listing is closed, each registration
    /// at the key a use at the listing's site sees, resolved from its level through its mark and
    /// pushed onto `candidates`. Returns the ranking they share with `classes`, the ranking of the
    /// candidates listed before them, or written order when none ranks the key. Two rankings among
    /// them are refused at `source`.
    fn registered(
        &mut self,
        listing: Listing,
        elements: &'graph [KeyElement],
        source: SourceRef,
        candidates: &mut BumpVec<'x, Candidate>,
        mut classes: Option<&'graph [u8]>,
    ) -> Result<&'graph [u8], ShapeError<'graph>> {
        let key = KeyElement::key(elements.iter().copied());
        let mut found = BumpVec::new_in(self.scratch);
        if !listing.closed {
            let draft = &self.chain[listing.site];
            let own = (
                draft.kind,
                &draft.registered[..],
                &draft.rankings[..],
                listing.at,
            );
            self.visible(own, listing.site, key, |seen| {
                if let Seen::Registration(entry) = seen {
                    found.push((entry.symbol, entry.classes, entry.surfaced.is_some()));
                }
                None::<()>
            });
        }
        for (symbol, theirs, surfaced) in found.iter().copied() {
            if classes.is_some_and(|classes| classes != theirs) {
                return Err(ShapeError::RankingDisagrees {
                    key: elements,
                    at: source,
                });
            }
            classes = Some(theirs);
            let name = BinderSymbol::Registration(symbol);
            let coordinate = self
                .resolve(
                    listing.level,
                    name,
                    listing.mark,
                    listing.at,
                    listing.reader,
                )
                .expect("a visible registration resolves");
            // A surfaced key holds the list of the functions the module offers there.
            candidates.push(match surfaced {
                true => Candidate::Spread(coordinate),
                false => Candidate::One(coordinate),
            });
        }
        Ok(classes.unwrap_or_else(|| self.written(elements)))
    }

    /// The key `elements` spell, its spelling recorded: a key a quote's code leaves open is named
    /// by the code's carried type, which renders it as written.
    fn spelled(&self, elements: &[KeyElement]) -> KeySymbol {
        self.symbols
            .record_key(elements.iter().map(|element| element.keyword()))
    }

    /// Written-order classes over the slots of `elements`.
    fn written(&self, elements: &[KeyElement]) -> &'graph [u8] {
        collect(
            self.brand.writer(),
            (0..slots(elements)).map(|class| class as u8),
        )
    }

    /// Resolve and record the mention of `name` at `part`.
    fn mention(
        &mut self,
        level: usize,
        statement: u32,
        part: &ExpressionPart<'graph>,
        name: BinderSymbol,
        state: State,
    ) -> Result<(), ShapeError<'graph>> {
        self.mention_through(level, statement, part, name, None, state, false)
    }

    /// [`mention`](Self::mention) of a name read through `mark`, which only a quote value's code
    /// may hold. A name bound to a quantified function is refused unless the read is a call's
    /// `head`.
    #[allow(clippy::too_many_arguments)]
    fn mention_through(
        &mut self,
        level: usize,
        statement: u32,
        part: &ExpressionPart<'graph>,
        name: BinderSymbol,
        mark: Option<Mark>,
        state: State,
        head: bool,
    ) -> Result<(), ShapeError<'graph>> {
        if mark.is_some() && !self.in_quote(level) {
            return Err(ShapeError::MarkOutsideQuote {
                at: self.part_source(level, statement, Site::of(part)),
            });
        }
        let mut class = state.class();
        let mut at = match class {
            MentionClass::Eager => Position::statement(statement as usize),
            MentionClass::Deferred => self.chain[level].end(),
        };
        let site = Site::of(part);
        if !head {
            match self.quantified(level, name, at, mark) {
                Quantified::No => {}
                // A module member read where it is instantiated needs its value then.
                Quantified::Member if mark.is_none() => {
                    class = MentionClass::Eager;
                    at = Position::statement(statement as usize);
                }
                Quantified::Member | Quantified::CallOnly => {
                    return Err(ShapeError::QuantifiedRead {
                        name,
                        site,
                        at: self.part_source(level, statement, site),
                    });
                }
            }
        }
        let reader = Reader {
            level,
            statement,
            class,
        };
        let coordinate = match self.builtins.lookup(name) {
            Some(index) => Coordinate::Builtin(index),
            None => {
                self.resolve(level, name, mark, at, reader)
                    .ok_or_else(|| ShapeError::Unbound {
                        name,
                        site,
                        at: self.part_source(level, statement, site),
                    })?
            }
        };
        self.chain[level].mentions.push(Mention {
            site,
            name,
            at,
            class,
            coordinate,
        });
        Ok(())
    }

    /// `name` read through `mark` at `at` in the draft at `level`: a visible local, else outward —
    /// a capturing draft captures what it finds there, a block steps one activation out. A code
    /// draft is where a mark is spent: a `$` name resolves outward from it, unmarked, and every
    /// other name it does not bind stays an open hole or an open `\` mark of the code.
    fn resolve(
        &mut self,
        level: usize,
        name: BinderSymbol,
        mark: Option<Mark>,
        at: Position,
        reader: Reader,
    ) -> Option<Coordinate> {
        let draft = &self.chain[level];
        let kind = draft.kind;
        if let Some(target) = resolve_here(draft.channels(), &draft.captures, name, mark, at) {
            if let Target::Local(slot) = target {
                self.edge(level, slot, reader);
            }
            return Some(Coordinate::Activation { hops: 0, target });
        }
        let outer = |builder: &mut Self, mark| {
            let parent = level.checked_sub(1)?;
            let at = builder.chain[parent].boundary();
            builder.resolve(parent, name, mark, at, reader)
        };
        let source = match (kind, mark) {
            (ShapeKind::Program, _) => return None,
            (ShapeKind::Block, _) => return Some(outer(self, mark)?.through_block()),
            (ShapeKind::Module, _) => {
                let read = outer(self, mark)?;
                if let Some(unlisted) = self.unlisted(level, name, read) {
                    self.chain[level].unlisted.get_or_insert(unlisted);
                }
                CaptureSource::Read(read)
            }
            (ShapeKind::Callable, _) => CaptureSource::Read(outer(self, mark)?),
            (ShapeKind::Code, Some(Mark::Written)) => CaptureSource::Read(outer(self, None)?),
            (ShapeKind::Code, Some(Mark::Built)) => CaptureSource::Offered,
            (ShapeKind::Code, None) => CaptureSource::Hole,
        };
        let captures = &mut self.chain[level].captures;
        captures.push(CaptureSpec { name, mark, source });
        Some(Coordinate::Activation {
            hops: 0,
            target: Target::Capture(CaptureSlot(captures.len() as u32 - 1)),
        })
    }

    /// The capture contract of `node`, a `MODULE` or `GROUP` binder: its `OVER` list read entry by
    /// entry — a name, or a key written with `_` in each slot — or `None` where it writes none.
    fn over(&self, node: &KExpression<'graph>) -> Result<Over<'graph>, ShapeError<'graph>> {
        let form = node
            .cache()
            .builtin_shape()
            .expect("a body role is a form's");
        let malformed = ShapeError::Malformed {
            form: form.id,
            at: node.source,
        };
        let Some((_, part)) = form
            .roles()
            .zip(node.parts)
            .find(|(role, _)| *role == Role::Captures)
        else {
            return Ok(Over {
                listed: None,
                site: Site::of_node(node),
                at: node.source,
            });
        };
        let ExpressionPart::ListLiteral(items) = &part.value else {
            return Err(malformed);
        };
        let writer = self.brand.writer();
        let mut listed = BumpVec::with_capacity_in(items.len(), self.scratch);
        for item in items.iter() {
            let entry = match (needed_name(item), needed_key(item)) {
                (Some(name), _) => Listed::Name(name),
                (None, Some(run)) => {
                    let mut staged = BumpVec::new_in(self.scratch);
                    staged.extend(run);
                    Listed::Key(collect(writer, staged.iter().copied()))
                }
                (None, None) => return Err(malformed),
            };
            listed.push(entry);
        }
        Ok(Over {
            listed: Some(collect(writer, listed.iter().copied())),
            site: Site::of(&part.value),
            at: node.source,
        })
    }

    /// `name` as a diagnostic spells it — a registration by its key — where the module body at
    /// `level` may not capture it, read at `read` in the shape outside it. A body may capture a
    /// binding of the program's top level, read where it lives; an open hole or `\` mark of a
    /// quote's code it sits in, which the code's own filling binds before it runs, as a builtin
    /// fills a keyworded hole; or one its `OVER` list names.
    fn unlisted(&self, level: usize, name: BinderSymbol, read: Coordinate) -> Option<BinderSymbol> {
        let landed = self.landing(level - 1, read);
        if landed.is_some_and(|(level, _)| self.is_top_level(level)) {
            return None;
        }
        if self.opens(level - 1, read) {
            return None;
        }
        let listed = self.chain[level]
            .over
            .and_then(|over| over.listed)
            .unwrap_or(&[]);
        let lists_key = |key: KeySymbol| {
            listed.iter().any(|listed| {
                matches!(listed, Listed::Key(elements) if KeyElement::key(elements.iter().copied()) == key)
            })
        };
        match name {
            BinderSymbol::Registration(symbol) => {
                let entry = landed.and_then(|(at, _)| {
                    (self.chain[at].registered.iter()).find(|entry| entry.symbol == symbol)
                });
                match entry {
                    Some(entry) if lists_key(entry.key) => None,
                    Some(entry) => Some(BinderSymbol::Key(self.spelled(entry.elements))),
                    None => Some(name),
                }
            }
            // A quote's code's keyworded hole, which its filler binds.
            BinderSymbol::Key(key) => (!lists_key(key)).then_some(name),
            name => (!listed.contains(&Listed::Name(name))).then_some(name),
        }
    }

    /// Whether the draft at `level` is the program's own top level, where a binding is bound once
    /// per program.
    fn is_top_level(&self, level: usize) -> bool {
        level == 0 && self.chain[0].kind == ShapeKind::Program
    }

    /// The draft and target a coordinate read in the draft at `level` lands at, each capture
    /// followed to its source; `None` for a builtin, a hole, a name offered, or a read past the
    /// chain's first draft.
    fn landing(&self, mut level: usize, mut coordinate: Coordinate) -> Option<(usize, Target)> {
        loop {
            let Coordinate::Activation { hops, target } = coordinate else {
                return None;
            };
            level = level.checked_sub(hops as usize)?;
            let Target::Capture(capture) = target else {
                return Some((level, target));
            };
            let CaptureSource::Read(inner) = self.chain[level].captures[capture.index()].source
            else {
                return None;
            };
            level = level.checked_sub(1)?;
            coordinate = inner;
        }
    }

    /// Whether `coordinate`, read in the draft at `level`, reaches an open hole or `\` mark of a
    /// quote's code: a capture chain ending at one the code leaves for its filling.
    fn opens(&self, mut level: usize, mut coordinate: Coordinate) -> bool {
        loop {
            let Coordinate::Activation { hops, target } = coordinate else {
                return false;
            };
            let Some(at) = level.checked_sub(hops as usize) else {
                return false;
            };
            let Target::Capture(capture) = target else {
                return false;
            };
            match self.chain[at].captures[capture.index()].source {
                CaptureSource::Hole | CaptureSource::Offered => return true,
                CaptureSource::Read(inner) => match at.checked_sub(1) {
                    Some(parent) => (level, coordinate) = (parent, inner),
                    None => return false,
                },
                CaptureSource::Member { .. } => return false,
            }
        }
    }

    /// The draft and slot `name` is bound at, read from the module body at `level` as its first
    /// statement would read it — `None` where nothing binds it, or the walk meets a quote's code,
    /// whose free names are holes.
    fn bound_outside(&self, level: usize, name: BinderSymbol) -> Option<(usize, Slot)> {
        let mut at = level;
        while let Some(parent) = at.checked_sub(1) {
            at = parent;
            let draft = &self.chain[at];
            let names = draft.channels();
            if let Some(index) = names.find(name)
                && draft.boundary().sees(names.get(index))
            {
                return Some((at, Slot(index as u32)));
            }
            if matches!(draft.kind, ShapeKind::Program | ShapeKind::Code) {
                return None;
            }
        }
        None
    }

    /// Hold the module body at `level` to its capture contract once every statement is walked:
    /// capture each `OVER` entry the body did not read — a top-level one recorded by its binding,
    /// since the run reads it where it lives — and refuse the first outer name read but not listed.
    fn contract(&mut self, level: usize) -> Result<(), ShapeError<'graph>> {
        let Some(over) = self.chain[level].over else {
            return Ok(());
        };
        let unbound = |name| ShapeError::Unbound {
            name,
            site: over.site,
            at: over.at,
        };
        let reader = Reader {
            level,
            statement: 0,
            class: MentionClass::Eager,
        };
        for entry in over.listed.unwrap_or(&[]) {
            match *entry {
                // One the body reads is captured already, under whatever mark it reads it through.
                Listed::Name(name)
                    if self.chain[level]
                        .captures
                        .iter()
                        .any(|capture| capture.name == name) => {}
                Listed::Name(name) => {
                    if let Some(index) = self.builtins.lookup(name) {
                        self.chain[level].listed_top.push(TopLevel::Builtin(index));
                        continue;
                    }
                    match self.bound_outside(level, name) {
                        Some((at, slot)) if self.is_top_level(at) => {
                            self.chain[level].listed_top.push(TopLevel::Root(slot));
                        }
                        Some(_) => {
                            self.resolve(level, name, None, Position::PARAMETER, reader);
                        }
                        None => return Err(unbound(name)),
                    }
                }
                Listed::Key(elements) => {
                    let key = KeyElement::key(elements.iter().copied());
                    let builtin = self.builtins.overloads(key);
                    for index in builtin.clone() {
                        self.chain[level]
                            .listed_top
                            .push(TopLevel::Builtin(BuiltinIndex(index)));
                    }
                    let parent = level - 1;
                    let draft = &self.chain[parent];
                    let own = (
                        draft.kind,
                        &draft.registered[..],
                        &draft.rankings[..],
                        draft.boundary(),
                    );
                    let mut found = BumpVec::new_in(self.scratch);
                    self.visible(own, parent, key, |seen| {
                        if let Seen::Registration(entry) = seen {
                            found.push(BinderSymbol::Registration(entry.symbol));
                        }
                        None::<()>
                    });
                    if found.is_empty() && builtin.is_empty() {
                        return Err(unbound(BinderSymbol::Key(key)));
                    }
                    for name in found.iter().copied() {
                        match self.bound_outside(level, name) {
                            Some((at, slot)) if self.is_top_level(at) => {
                                self.chain[level].listed_top.push(TopLevel::Root(slot));
                            }
                            _ => {
                                self.resolve(level, name, None, Position::PARAMETER, reader);
                            }
                        }
                    }
                }
            }
        }
        let draft = &mut self.chain[level];
        draft.listed_top.sort_unstable();
        draft.listed_top.dedup();
        match draft.unlisted {
            Some(name) => Err(ShapeError::Unlisted { name, at: over.at }),
            None => Ok(()),
        }
    }

    /// Whether the draft at `level` is a quote value's code or lies inside one — where a mark may
    /// be written.
    fn in_quote(&self, level: usize) -> bool {
        self.chain[..=level]
            .iter()
            .any(|draft| draft.kind == ShapeKind::Code)
    }

    /// Record that the binder of the reading path's statement at `level` reads `slot`.
    fn edge(&mut self, level: usize, slot: Slot, reader: Reader) {
        let draft = &mut self.chain[level];
        let (statement, class) = if level == reader.level {
            (reader.statement, reader.class)
        } else {
            draft.current
        };
        draft.reads.push((statement, slot));
        if let Some(binder) = draft.statement_binders[statement as usize].first() {
            draft.edges.push((binder, slot, class));
        }
    }

    /// The components pass: condense, refuse an eager cycle, settle each nested draft's captures
    /// of a fellow member as edges, and seal the nested drafts.
    fn components(&mut self, draft: &mut Draft<'graph, 'x>) -> Result<(), ShapeError<'graph>> {
        let scratch = self.scratch;
        let count = draft.channels().len();
        // A statement's binders are one component: each reads the next and the last the first, as
        // deferred edges, since none of them is a mention.
        for binders in draft.statement_binders.iter() {
            let slots = &binders.slots[..binders.len as usize];
            if slots.len() > 1 {
                for (index, binder) in slots.iter().enumerate() {
                    let next = slots[(index + 1) % slots.len()];
                    draft.edges.push((*binder, next, MentionClass::Deferred));
                }
            }
        }
        // Compressed rows: `offsets[i]..offsets[i + 1]` of `targets` are the slots binder `i` reads.
        let mut offsets: BumpVec<'x, usize> = BumpVec::with_capacity_in(count + 1, scratch);
        offsets.resize(count + 1, 0);
        for (binder, _, _) in draft.edges.iter() {
            offsets[binder.index() + 1] += 1;
        }
        for index in 0..count {
            offsets[index + 1] += offsets[index];
        }
        let mut fill = BumpVec::with_capacity_in(count, scratch);
        fill.extend_from_slice(&offsets[..count]);
        let mut targets: BumpVec<'x, usize> = BumpVec::with_capacity_in(draft.edges.len(), scratch);
        targets.resize(draft.edges.len(), 0);
        for (binder, bound, _) in draft.edges.iter() {
            let at = &mut fill[binder.index()];
            targets[*at] = bound.index();
            *at += 1;
        }
        let mut borrowed: BumpVec<'x, &[usize]> = BumpVec::with_capacity_in(count, scratch);
        borrowed.extend((0..count).map(|index| &targets[offsets[index]..offsets[index + 1]]));
        let condensed = strongly_connected_components(scratch, &borrowed);

        draft.component_of.resize(count, ComponentIndex(0));
        for (index, mut members) in condensed.into_iter().enumerate() {
            members.sort_unstable();
            let start = draft.members.len() as u32;
            for member in members.iter() {
                draft.component_of[*member] = ComponentIndex(index as u32);
                draft.members.push(Slot(*member as u32));
            }
            draft.components.push(DraftComponent {
                start,
                len: members.len() as u32,
                deferred_only: true,
                cyclic: false,
            });
        }
        for (binder, bound, class) in draft.edges.iter() {
            let component = &mut draft.components[draft.component_of[binder.index()].index()];
            if draft.component_of[binder.index()] != draft.component_of[bound.index()] {
                continue;
            }
            component.cyclic |= binder == bound || component.len > 1;
            if *class == MentionClass::Eager {
                component.deferred_only = false;
            }
        }
        let refused = (0..draft.components.len())
            .map(|index| ComponentIndex(index as u32))
            .filter(|component| {
                let component = draft.components[component.index()];
                component.cyclic && !component.deferred_only
            })
            .min_by_key(|component| {
                draft
                    .members_of(*component)
                    .iter()
                    .map(|slot| draft.channels().get(slot.index()))
                    .min()
            });
        if let Some(component) = refused {
            let first = draft
                .members_of(component)
                .iter()
                .map(|slot| draft.channels().get(slot.index()))
                .min()
                .expect("a component has members");
            // A parameter reads nothing, so it is never on a cycle.
            let statement = first
                .0
                .checked_sub(1)
                .expect("a cyclic member binds a statement");
            // The member names and keys are written to program storage, where the shape would have
            // gone.
            let mut members = BumpVec::new_in(scratch);
            let mut definitions = BumpVec::new_in(scratch);
            for slot in draft.members_of(component) {
                match draft.channels().name(slot.index()) {
                    BinderSymbol::Registration(symbol) => definitions.extend(
                        draft
                            .registered
                            .iter()
                            .filter(|entry| entry.symbol == symbol)
                            .map(|entry| entry.elements),
                    ),
                    name => members.push(name),
                }
            }
            let writer = self.brand.writer();
            return Err(ShapeError::EagerCycle {
                members: collect(writer, members.iter().copied()),
                definitions: collect(writer, definitions.iter().copied()),
                at: draft.nodes[statement as usize].source,
            });
        }

        for (_, child) in draft.children.iter_mut() {
            let binder = draft.statement_binders[child.parent_statement as usize].first();
            for capture in child.captures.iter_mut() {
                let CaptureSource::Read(Coordinate::Activation {
                    hops: 0,
                    target: Target::Local(bound),
                }) = capture.source
                else {
                    continue;
                };
                let component = draft.component_of[bound.index()];
                if binder.is_some_and(|binder| draft.component_of[binder.index()] == component) {
                    let members = draft.components[component.index()].run(&draft.members);
                    let index = members
                        .binary_search(&bound)
                        .expect("a slot sits in its own component");
                    capture.source = CaptureSource::Member {
                        component,
                        index: index as u32,
                    };
                }
            }
        }
        Ok(())
    }

    /// The units pass: one unit per component whose members are not all parameters, keyed by its
    /// lowest statement, and one per statement that binds nothing, keyed by that statement; each
    /// unit waits on the units binding a slot it reads. The units are emitted in the smallest-key
    /// order that respects every wait, so independent units come out as they are written. Every
    /// read, wait and count is scratch: the shape keeps only the order.
    fn units(&self, draft: &mut Draft<'graph, 'x>) {
        let scratch = self.scratch;
        let statements = draft.statements as usize;
        let channels = draft.channels();
        // The unit each statement belongs to, and the key-ordered works: a statement belongs to
        // its binder's component's unit, or to a unit of its own.
        let mut component_key: BumpVec<'x, Option<u32>> =
            BumpVec::with_capacity_in(draft.components.len(), scratch);
        component_key.resize(draft.components.len(), None);
        for (slot, component) in draft.component_of.iter().enumerate() {
            if let Some(statement) = channels.get(slot).0.checked_sub(1) {
                let key = &mut component_key[component.index()];
                *key = Some(key.map_or(statement, |key| key.min(statement)));
            }
        }
        // A unit's key is a statement, and no two units share one, so a run indexed by statement
        // orders them.
        let mut by_key: BumpVec<'x, Option<UnitWork>> =
            BumpVec::with_capacity_in(statements, scratch);
        by_key.resize(statements, None);
        for (component, key) in component_key.iter().enumerate() {
            if let Some(key) = key {
                by_key[*key as usize] = Some(UnitWork::Component(ComponentIndex(component as u32)));
            }
        }
        for (statement, binders) in draft.statement_binders.iter().enumerate() {
            if binders.is_empty() {
                by_key[statement] = Some(UnitWork::Statement(statement as u32));
            }
        }
        let mut works: BumpVec<'x, UnitWork> = BumpVec::with_capacity_in(statements, scratch);
        let mut unit_of_key: BumpVec<'x, u32> = BumpVec::with_capacity_in(statements, scratch);
        unit_of_key.resize(statements, u32::MAX);
        for (key, work) in by_key.iter().enumerate() {
            if let Some(work) = work {
                unit_of_key[key] = works.len() as u32;
                works.push(*work);
            }
        }
        let unit_of_statement = |statement: u32| match draft.statement_binders[statement as usize]
            .first()
        {
            Some(binder) => {
                let component = draft.component_of[binder.index()];
                let key = component_key[component.index()].expect("a binder's component has a key");
                unit_of_key[key as usize]
            }
            None => unit_of_key[statement as usize],
        };
        let unit_of_slot = |slot: Slot| {
            let component = draft.component_of[slot.index()];
            component_key[component.index()].map(|key| unit_of_key[key as usize])
        };
        // `(waited on, waiter)`, one per wait — duplicates only raise a count they also lower. The
        // reads are acyclic: a cycle among bindings is one component.
        let mut waits: BumpVec<'x, (u32, u32)> = BumpVec::new_in(scratch);
        for (statement, slot) in draft.reads.iter() {
            let waiter = unit_of_statement(*statement);
            if let Some(bound) = unit_of_slot(*slot)
                && bound != waiter
            {
                waits.push((bound, waiter));
            }
        }
        waits.sort_unstable();
        // Per unit: the waits outstanding.
        let mut pending: BumpVec<'x, u32> = BumpVec::with_capacity_in(works.len(), scratch);
        pending.resize(works.len(), 0);
        for (_, waiter) in waits.iter() {
            pending[*waiter as usize] += 1;
        }
        let mut emitted: BumpVec<'x, bool> = BumpVec::with_capacity_in(works.len(), scratch);
        emitted.resize(works.len(), false);
        let last = statements
            .checked_sub(1)
            .map(|last| unit_of_statement(last as u32));
        let mut cursor = 0;
        while draft.units.len() < works.len() {
            let unit = (cursor..works.len())
                .find(|unit| !emitted[*unit] && pending[*unit] == 0)
                .expect("the reads are acyclic, so some unit is always ready");
            emitted[unit] = true;
            draft.units.push(Unit {
                work: works[unit],
                last: last == Some(unit as u32),
            });
            cursor = unit + 1;
            let first = waits.partition_point(|(bound, _)| (*bound as usize) < unit);
            for (bound, waiter) in waits[first..].iter() {
                if *bound as usize != unit {
                    break;
                }
                pending[*waiter as usize] -= 1;
                if pending[*waiter as usize] == 0 {
                    cursor = cursor.min(*waiter as usize);
                }
            }
        }
    }

    /// Lay a finished draft down in program storage, its nested drafts first; `form` is the node
    /// holding a callable draft's body.
    fn seal(
        &self,
        draft: Draft<'graph, 'x>,
        form: Option<&'graph KExpression<'graph>>,
    ) -> &'graph BodyShape<'graph> {
        let writer = self.brand.writer();
        let mut nested = BumpVec::with_capacity_in(draft.children.len(), self.scratch);
        for (site, child) in draft.children {
            let form = self.forms.get(&site).copied();
            nested.push((site, self.seal(child, form)));
        }
        nested.sort_unstable_by_key(|(site, _)| *site);
        let mut births = BumpVec::with_capacity_in(draft.births.len(), self.scratch);
        births.extend(draft.births.iter().map(|(binder, site)| {
            let index = nested
                .binary_search_by_key(site, |(nested, _)| *nested)
                .expect("a birth's body is a nested shape");
            (*binder, nested[index].1)
        }));
        births.sort_unstable_by_key(|(binder, _)| *binder);
        let mut rhs = BumpVec::with_capacity_in(draft.rhs.len(), self.scratch);
        rhs.extend(draft.rhs.iter().map(|(binder, site)| {
            let part = self
                .parts
                .get(site)
                .copied()
                .expect("a recorded right-hand side is kept by site");
            (*binder, part)
        }));
        rhs.sort_unstable_by_key(|(binder, _)| *binder);
        let mut declarations = draft.declarations;
        declarations.sort_unstable_by_key(|(binder, _)| *binder);
        let mut mentions = draft.mentions;
        mentions.sort_unstable_by_key(|mention| mention.site);
        let mut offers = draft.offers;
        offers.sort_unstable_by_key(|(site, _)| *site);
        let channels = Channels::new(&draft.values, &draft.types, &draft.registrations);
        let mut registrations = BumpVec::with_capacity_in(draft.registered.len(), self.scratch);
        registrations.extend(draft.registered.iter().map(|entry| {
            let index = channels
                .find(BinderSymbol::Registration(entry.symbol))
                .expect("a registration has its slot");
            Registration {
                slot: Slot(index as u32),
                key: entry.key,
                elements: entry.elements,
                classes: entry.classes,
                which: entry.which,
                surfaced: entry.surfaced,
            }
        }));
        registrations.sort_unstable_by_key(|registration| registration.slot);
        let mut candidates = draft.candidates;
        candidates.sort_unstable_by_key(|(site, _)| *site);
        let mut required = draft.required;
        required.sort_unstable();
        let mut recorded = draft.type_expressions;
        recorded.sort_unstable_by_key(|recorded| Site::of(recorded.part));
        let type_expressions = writer.fill(recorded.len(), |index| {
            let recorded = recorded[index];
            TypeExpression {
                site: Site::of(recorded.part),
                part: recorded.part,
                statement: recorded.statement,
                guard: recorded.guard,
                typed: Cell::new(Static::Unknown),
            }
        });
        let members = collect(writer, draft.members.iter().copied());
        let mut components = BumpVec::with_capacity_in(draft.components.len(), self.scratch);
        components.extend(draft.components.iter().map(|component| Component {
            members: component.run(members),
            deferred_only: component.deferred_only,
            cyclic: component.cyclic,
        }));
        resident(
            writer,
            BodyShape {
                kind: draft.kind,
                names: Channels::new(
                    collect(writer, draft.values.iter().copied()),
                    collect(writer, draft.types.iter().copied()),
                    collect(writer, draft.registrations.iter().copied()),
                ),
                body: collect(writer, draft.nodes.iter().copied()),
                group_frame: draft.frame,
                held: draft.held,
                entered_at: draft.entered_at,
                component_of: collect(writer, draft.component_of.iter().copied()),
                components: collect(writer, components.iter().copied()),
                mentions: collect(writer, mentions.iter().copied()),
                captures: collect(writer, draft.captures.iter().copied()),
                over: draft.over.and_then(|over| over.listed),
                listed_top: collect(writer, draft.listed_top.iter().copied()),
                code: draft.code,
                top_captures: collect(writer, draft.top_captures.iter().copied()),
                nested: collect(writer, nested.iter().copied()),
                form,
                births: collect(writer, births.iter().copied()),
                rhs: collect(writer, rhs.iter().copied()),
                declarations: collect(writer, declarations.iter().copied()),
                declared: writer.fill(declarations.len(), |_| Cell::new(Static::Unknown)),
                units: collect(writer, draft.units.iter().copied()),
                arm: draft.arm,
                code_type: draft.code_type,
                refusal: draft.refusal,
                offers: collect(writer, offers.iter().copied()),
                registrations: collect(writer, registrations.iter().copied()),
                rankings: collect(writer, draft.rankings.iter().copied()),
                candidates: collect(writer, candidates.iter().copied()),
                required: collect(writer, required.iter().copied()),
                type_expressions,
                registered: writer.fill(registrations.len(), |_| Cell::new(Static::Unknown)),
                callable: resident_cell(writer, Static::Unknown),
                group_levels: resident_cell(writer, &[][..]),
                born_instance: resident_cell(writer, Static::Unknown),
                declared_variables: resident_cell(writer, &[][..]),
                type_captures: resident_cell(writer, &[][..]),
                typing_refusal: resident_cell(writer, None),
                statics: resident_cell(writer, None),
            },
        )
    }
}

/// Whether an expression shape's type parts are typed with the callable it births or the binder it
/// declares, rather than each as a type expression of its own: a signature, a head, a `FOR ALL`
/// group, a declared name or definition, or a callable body. An annotated `LET`'s type is the one
/// its value is held to, a type expression of its own.
fn types_with_its_binder(form: &BuiltinShape) -> bool {
    form.id != BuiltinShapeId::LetAnnotated
        && form.roles().any(|role| {
            matches!(
                role,
                Role::Signature | Role::Head | Role::Quantifiers | Role::Name | Role::Definition(_)
            ) || role.is_callable()
        })
}

/// One write-once cell laid down in program storage.
fn resident_cell<T>(writer: crate::memory::Writer<'_>, value: T) -> &Cell<T> {
    let mut value = Some(value);
    &writer.fill(1, |_| Cell::new(value.take().expect("filled once")))[0]
}
