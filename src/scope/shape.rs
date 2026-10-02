//! The **shape**: one per body, built once into program storage and shared by every activation of
//! that body.
//!
//! It holds the body's declared names as one three-channel run — value names first, type names
//! after, and each keyworded definition's registration last, each channel sorted by symbol and
//! each entry at its slot — every mention with its class and coordinate, each bucket declaration's
//! ranking and the ranking each registration carries, each keyworded use's candidate list,
//! the capture layout a callable's closure bindings are born through, the strongly connected
//! components of the body's bindings, the shapes nested in it by site, the form node a callable's
//! body sits in, the callable body each binder births, each `LET` binder's right-hand side, and
//! each type binder's declaration node, and the order its units run in. [`build`] is the one
//! builder every kind goes through.
//!
//! A **registration** is a binder no text names: the slot a definition's function is bound to
//! under its bucket key. A keyworded use reaches it by key, through a [`CandidateList`] fixed where
//! the shape is built — the builtin overloads at the key, then each registration visible to the
//! use — and each registration in the list is resolved as an eager read at the use's statement, so
//! captures, components and units treat it as they treat a name.
//!
//! A quote value's code is a [`ShapeKind::Code`] shape nested at the quote's site, built where the
//! program loads. Its captures are its `$` names and `$(…)` uses' registrations, bound where the
//! quote is written, its open holes and its open `\` marks. A keyworded use in it lists the builtin
//! overloads and the code's own registrations, and — unmarked — its key as a hole a `USING` fills,
//! or — under `\(…)` — its key alone, which the `EVAL` running the code offers; each is a
//! [`Candidate::Spread`], a capture named by the key. The shape records the holes some use selects
//! from alone, carries its type and, when its code is malformed, the error an `EVAL` of it reports.
//! See [README.md § Quotes](README.md#quotes).
//!
//! A shape also carries a write-once cell for each type fact the elaborator's load pass fixes
//! before the program runs — each [`TypeExpression`] it records, each type binder, each
//! registration, a callable body's own type, and a code shape's typing refusal — laid down empty by
//! the builder, since `scope` sits below `elaborate`. One more cell holds the value channel's
//! [`Statics`] — a static type per value expression and binder, and each keyworded use's
//! [`Narrowing`] — which the language's load pass fixes. See [README.md § Load-time
//! types](README.md#load-time-types).
//!
//! **Visibility** is one comparison, [`Position::sees`]: a binding is visible to a reader whose
//! position is strictly greater than the binding's own. A parameter writes at `0`, statement `i` at
//! `i + 1`, and the body's end is one past its last statement. An eager mention reads at its
//! statement's position and a deferred one at the end.
//!
//! See [README.md § Resolution](README.md#resolution).

use std::cell::Cell;
use std::fmt;

use crate::memory::{BumpAllocator, ProgramBrand};
use crate::parse::builtin_shapes::BuiltinShapeId;
use crate::parse::builtin_shapes::role::{DefinitionKind, Heads, Role};
use crate::parse::{ExpressionPart, KExpression, KeyElement, Mark};
use crate::source::SourceRef;
use crate::symbols::{BinderSymbol, KeySymbol, KeywordSymbol, SymbolInterner};
use crate::type_lattice::{
    DeclaredGroup, DeclaredType, Interval, KType, Parametric, TypeRegistry, display_name,
};
use crate::values::Knotted;

use super::builtins::Builtins;
use super::channels::Channels;
use super::groups::GroupFrame;
use super::typed::{
    Elaboration, Narrowing, Static, StaticCallable, StaticRegistered, StaticType, Statics,
};

mod build;

pub(crate) use build::IMPLICIT;

/// An index into an activation's slot run: value slots first, type slots after, registration slots
/// last.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Slot(pub(crate) u32);

/// An index into a callable's closure bindings.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct CaptureSlot(pub(crate) u32);

/// An index into the builtin table: values first, types after, overloads last.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct BuiltinIndex(pub(crate) u32);

/// An index into a shape's components.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ComponentIndex(pub(crate) u32);

impl Slot {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl CaptureSlot {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl BuiltinIndex {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl ComponentIndex {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// A lexical position within one shape: `0` for a parameter, `i + 1` for statement `i`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Position(pub(crate) u32);

impl Position {
    /// Where every parameter writes.
    pub const PARAMETER: Position = Position(0);

    /// Where statement `index` writes and where its eager mentions read.
    pub fn statement(index: usize) -> Position {
        Position(index as u32 + 1)
    }

    /// The statement that writes here, or `None` for a parameter.
    pub fn statement_index(self) -> Option<usize> {
        (self.0 as usize).checked_sub(1)
    }

    /// Whether a binding declared at `declared` is visible to a reader at this position — the one
    /// visibility comparison.
    pub fn sees(self, declared: Position) -> bool {
        declared < self
    }
}

/// Where a resolved read lands in the activation its hops reach.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    Local(Slot),
    Capture(CaptureSlot),
}

/// A resolved read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Coordinate {
    /// Through any activation's header: no hops.
    Builtin(BuiltinIndex),
    /// `hops` enclosing block activations out (`0` for the reader's own), then `target` there.
    Activation { hops: u32, target: Target },
}

impl Coordinate {
    /// This coordinate read from a block nested one activation further in.
    pub(crate) fn through_block(self) -> Coordinate {
        match self {
            Coordinate::Builtin(index) => Coordinate::Builtin(index),
            Coordinate::Activation { hops, target } => Coordinate::Activation {
                hops: hops + 1,
                target,
            },
        }
    }
}

/// Whether a mention needs its value where it is read, or only stores or captures it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MentionClass {
    Eager,
    Deferred,
}

/// The identity of one part in program storage — its address, which is what a reader holding the
/// part can recompute.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Site(usize);

impl Site {
    pub fn of(part: &ExpressionPart<'_>) -> Site {
        Site(std::ptr::from_ref(part) as usize)
    }

    /// The site of a whole node — its parts run's address, which every copy of the node shares.
    pub fn of_node(node: &KExpression<'_>) -> Site {
        Site(node.parts.as_ptr() as usize)
    }

    /// The site of `form`'s body part — where the shape enclosing `form` records the body nested in
    /// it — or `None` for a node whose builtin shape declares no body.
    pub fn of_body(form: &KExpression<'_>) -> Option<Site> {
        let shape = form.cache().builtin_shape()?;
        shape
            .roles()
            .zip(form.parts)
            .find(|(role, _)| matches!(role, Role::Body(_)))
            .map(|(_, part)| Site::of(&part.value))
    }
}

/// One identifier or type-name occurrence a body reads.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Mention {
    pub site: Site,
    pub name: BinderSymbol,
    /// The position the mention reads at: its statement's when eager, the shape's end when
    /// deferred.
    pub at: Position,
    pub class: MentionClass,
    pub coordinate: Coordinate,
}

/// Which of a definition's bucket keys a registration is under: its only one, or for a `UNARY OP`,
/// its unary key `⊕ _` or its binary key `_ ⊕ _`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Which {
    Only,
    Unary,
    Binary,
}

/// One registration a body declares: a keyworded definition's function, bound at `slot`, under one
/// of its bucket keys — or, in a `USING … SCOPE` block, a bodyless head its operand's signature
/// declares.
#[derive(Clone, Copy, Debug)]
pub struct Registration<'graph> {
    pub slot: Slot,
    pub key: KeySymbol,
    /// The key as its keywords and slots.
    pub elements: &'graph [KeyElement],
    /// Each slot's dense priority class, in element order: the ranking of the declaration visible
    /// where the definition is written, an operator's chaining, a surfaced head's own ranking, or
    /// written order.
    pub classes: &'graph [u8],
    pub which: Which,
    /// Where a surfaced head is written; `None` for a definition's registration.
    pub surfaced: Option<&'graph SurfacedHead<'graph>>,
}

/// A bodyless keyworded head a `USING … SCOPE` operand's signature declares, which the block holds
/// as a registration of its own: a parameter of the registration channel, typed where the program
/// loads and bound by nothing at run. Each place is named by how many shapes out from the block it
/// lies, so the load pass reads it off the chain of shapes it keeps.
#[derive(Clone, Copy, Debug)]
pub struct SurfacedHead<'graph> {
    /// The head's statement in the signature's body.
    pub head: &'graph KExpression<'graph>,
    /// The `SIG` declaration's type binder: its shape's hops out, and its slot there.
    pub signature: (u32, Slot),
    /// The ascription naming the signature: its shape's hops out, and its type part's site there.
    pub ascription: (u32, Site),
}

/// A bucket declaration a body holds, `EXPR #(MOVE 2 TO 1)`: the ranking it gives its key, from
/// the position it is written at. It binds nothing.
#[derive(Clone, Copy, Debug)]
pub struct Ranking<'graph> {
    pub key: KeySymbol,
    pub elements: &'graph [KeyElement],
    pub at: Position,
    pub classes: &'graph [u8],
}

/// One callable, or list of callables, a keyworded use may select.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Candidate {
    /// A builtin overload, or a registration read where the use resolves it.
    One(Coordinate),
    /// A capture of a quote's code named by the use's key, whose value is a list of functions: a
    /// keyworded hole a `USING` fills, or a `\(…)` use's key the `EVAL` running the code offers.
    Spread(Coordinate),
}

/// What a keyworded use may select, fixed where its shape is built: the builtin overloads at its
/// key, in table order, then each registration at the key visible to it, the enclosing bodies
/// innermost first, then — in a quote's code — the capture its key names. Every candidate a shape
/// sees carries one ranking, `classes`; a spread's is checked where it is filled.
#[derive(Clone, Copy, Debug)]
pub struct CandidateList<'graph> {
    pub key: KeySymbol,
    pub elements: &'graph [KeyElement],
    pub classes: &'graph [u8],
    pub candidates: &'graph [Candidate],
}

/// The five kinds of body a shape describes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShapeKind {
    /// The top level: no captures and no enclosing activation.
    Program,
    /// A `FN`, `EXPR` or `OP` body: captures, and is a deferring boundary.
    Callable,
    /// A `MODULE` or `GROUP` body: captures, and is an eager context.
    Module,
    /// A `MATCH` or `TRY` arm, or a `USING … SCOPE` body: activated in the enclosing frame beside a
    /// pointer to the enclosing activation.
    Block,
    /// A quote value's code: captures its `$` names where the quote is written, like a callable,
    /// and leaves every other free name an open hole or an open `\` mark. An `EVAL` activates it
    /// with no enclosing activation.
    Code,
}

/// One closure binding a birth fills, and where from. `mark` is the mark the name was read through
/// — `None` for an unmarked name — so a hole `x`, a `$x` and a `\x` read in one body are three
/// captures.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CaptureSpec {
    pub name: BinderSymbol,
    pub mark: Option<Mark>,
    pub source: CaptureSource,
}

/// Where a closure binding comes from at birth.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CaptureSource {
    /// Read this coordinate of the enclosing activation.
    Read(Coordinate),
    /// Member `index` of `component` of the enclosing shape — the component the callable itself
    /// belongs to — held as an edge into the knot that component is born in.
    Member {
        component: ComponentIndex,
        index: u32,
    },
    /// An open hole of a code shape, a name or a keyworded use's key: a `USING` supplies it, or it
    /// stays unbound — a key's then holds no function, unless some use selects from it alone.
    Hole,
    /// An open `\` mark of a code shape: the `EVAL` that runs the code offers it.
    Offered,
}

/// A strongly connected component of a shape's bindings.
#[derive(Clone, Copy, Debug)]
pub struct Component<'graph> {
    /// The members, in slot order.
    pub members: &'graph [Slot],
    /// Whether every mention between members is deferred — the component a knot can tie.
    pub deferred_only: bool,
    /// Whether the component holds more than one member or a member reads itself. A caller ties a
    /// component of value binders when it is cyclic or when every member births a callable or a
    /// module; a non-cyclic data binder is an ordinary value, and a component of type binders is the
    /// elaborator's. A component never mixes the two channels: a schema names types only, so no
    /// mention leaves a type binder for a value binder.
    pub cyclic: bool,
}

/// What one unit of a body performs: a component of its bindings, or a statement that binds
/// nothing, by index into [`BodyShape::body`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnitWork {
    Component(ComponentIndex),
    Statement(u32),
}

/// One unit of a body, in the order its body runs them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Unit {
    pub work: UnitWork,
    /// Whether this unit holds the body's last statement — whose value a called body's is.
    pub last: bool,
}

/// A type expression the shape records, which the load pass types on its own: a `:(…)` or `:{…}`
/// in value position, a type part of an expression shape that births no callable, or a
/// `MATCH … WITH` guard. A type nested in a recorded one is part of it, and a callable's signature
/// or a declaration's definition is typed with its callable or binder instead.
pub struct TypeExpression<'graph> {
    pub site: Site,
    /// The part as written; for a guard, its quote.
    pub part: &'graph ExpressionPart<'graph>,
    /// The statement it is written in, by index into [`BodyShape::body`] — where a refusal about it
    /// is located.
    pub statement: u32,
    /// For a guard: the site of its arm set, and its place among the arms in written order.
    pub guard: Option<(Site, u32)>,
    typed: Cell<StaticType<'graph>>,
}

impl<'graph> TypeExpression<'graph> {
    /// What the load pass fixed for this expression; `Unknown` before it runs.
    pub fn typed(&self) -> StaticType<'graph> {
        self.typed.get()
    }

    /// Written once, by the load pass.
    pub fn fix(&self, typed: StaticType<'graph>) {
        debug_assert!(matches!(self.typed.get(), Static::Unknown));
        self.typed.set(typed);
    }
}

/// One body's resolved lexical structure. See the module documentation.
#[derive(Clone, Copy)]
pub struct BodyShape<'graph> {
    kind: ShapeKind,
    /// Each declared name at its slot, beside the position it writes at.
    names: Channels<'graph, Position>,
    /// This body's own statements, rewritten — the run every reader takes them from, and the one
    /// every part address this shape records lies inside.
    body: &'graph [KExpression<'graph>],
    /// The operator groups the shapes enclosing this body hold, this body's own included.
    group_frame: &'graph GroupFrame<'graph>,
    /// The groups this body itself holds: a `GROUP`'s own group, or the groups a `USING … SCOPE`
    /// operand surfaces. Empty for every other body.
    held: &'graph [&'graph DeclaredGroup<'graph>],
    /// The position in the enclosing shape this one is entered at: the statement's for an eager
    /// boundary, the enclosing body's end for a deferred one.
    entered_at: Position,
    component_of: &'graph [ComponentIndex],
    components: &'graph [Component<'graph>],
    mentions: &'graph [Mention],
    captures: &'graph [CaptureSpec],
    nested: &'graph [(Site, &'graph BodyShape<'graph>)],
    /// The `FN`, `EXPR` or `OP` node a callable's body sits in.
    form: Option<&'graph KExpression<'graph>>,
    /// Each binder whose right-hand side births a callable or a module, beside that body, by slot.
    births: &'graph [(Slot, &'graph BodyShape<'graph>)],
    /// Each `LET` binder beside its right-hand side part, by slot.
    rhs: &'graph [(Slot, &'graph ExpressionPart<'graph>)],
    /// Each type binder beside the declaration node that binds it, by slot.
    declarations: &'graph [(Slot, &'graph KExpression<'graph>)],
    /// The body's units in the order they are performed.
    units: &'graph [Unit],
    /// What this body is to the `MATCH` or `TRY` holding it, when it is an arm.
    arm: Option<Arm<'graph>>,
    /// A code shape's carried type: its code kind needing its open `\` marks. `Code` for every
    /// other kind.
    code_type: KType,
    /// Why a code shape's code cannot be built — reported when an `EVAL` runs it. A refused code
    /// shape holds its `$` captures and nothing else.
    refusal: Option<&'graph ShapeError<'graph>>,
    /// Each `EVAL` of a code parameter whose type needs names, by its operand's site, beside each
    /// needed name or key and what the `EVAL` offers for it.
    offers: &'graph [(Site, &'graph [(BinderSymbol, Offer<'graph>)])],
    /// Each registration this body declares, by slot.
    registrations: &'graph [Registration<'graph>],
    /// Each bucket declaration this body holds, in statement order.
    rankings: &'graph [Ranking<'graph>],
    /// Each keyworded use's candidates, by the use's node site.
    candidates: &'graph [(Site, CandidateList<'graph>)],
    /// A code shape's keyworded holes some use selects from alone, sorted.
    required: &'graph [KeySymbol],
    /// Each type expression this body records, by site.
    type_expressions: &'graph [TypeExpression<'graph>],
    /// Each type binder's load-time type, parallel to `declarations`.
    declared: &'graph [Cell<StaticType<'graph>>],
    /// Each registration's load-time bucket entry, parallel to `registrations`.
    registered: &'graph [Cell<StaticRegistered<'graph>>],
    /// A callable body's load-time type; `Unknown` for every other kind.
    callable: &'graph Cell<StaticCallable<'graph>>,
    /// A callable body's own `FOR ALL` group as its body reads it; empty for every other kind.
    group_levels: &'graph Cell<&'graph [Parametric]>,
    /// Why a code shape's code does not type, where the load pass found a refusal in it.
    typing_refusal: &'graph Cell<Option<&'graph ShapeError<'graph>>>,
    /// The value channel's static types and narrowings, fixed by the language's load pass.
    statics: &'graph Cell<Option<Statics<'graph>>>,
}

/// What an `EVAL` of a code parameter offers the code it runs for one name its `NEEDING` list
/// names.
#[derive(Clone, Copy, Debug)]
pub enum Offer<'graph> {
    /// A name, where it resolves at the `EVAL`.
    Name(Coordinate),
    /// A bucket key, as a use at the key written at the `EVAL` resolves it.
    Key(&'graph CandidateList<'graph>),
}

/// What a `MATCH` or `TRY` arm's block is to the expression shape holding it.
#[derive(Clone, Copy, Debug)]
pub struct Arm<'graph> {
    /// The guard's quote as written — `it`'s type or label — or `None` for the `_` default.
    pub guard: Option<&'graph ExpressionPart<'graph>>,
    /// Whether a type (`MATCH … WITH`) or a label (`MATCH … OVER`, `TRY`) is written there.
    pub heads: Heads,
    /// Whether the block's last statement is in tail position.
    pub tail: bool,
}

const _: () = assert!(!std::mem::needs_drop::<BodyShape<'static>>());

impl<'graph> BodyShape<'graph> {
    /// The shape of a program's top-level statements.
    pub fn of_program<X: Knotted>(
        brand: ProgramBrand<'graph>,
        statements: &[KExpression<'graph>],
        builtins: &Builtins<'_, X>,
        types: &TypeRegistry<'graph>,
        symbols: &SymbolInterner,
        scratch: BumpAllocator<'_>,
    ) -> Result<&'graph BodyShape<'graph>, ShapeError<'graph>> {
        build::program(brand, statements, builtins, types, symbols, scratch)
    }

    pub fn kind(&self) -> ShapeKind {
        self.kind
    }

    /// How many slots an activation of this shape holds.
    pub fn slots(&self) -> usize {
        self.names.len()
    }

    pub fn statements(&self) -> u32 {
        self.body.len() as u32
    }

    /// This body's statements, **rewritten**: every operator run in them is already chained, and
    /// every site this shape records is an address inside this run. A reader takes a body's
    /// statements from here and never from the parse.
    pub fn body(&self) -> &'graph [KExpression<'graph>] {
        self.body
    }

    /// The operator-group frame this body was built under — what decides which groups an operator
    /// run here may chain under.
    pub fn group_frame(&self) -> &'graph GroupFrame<'graph> {
        self.group_frame
    }

    /// The groups this body itself holds. A `GROUP` module's self-signature carries them.
    pub fn held_groups(&self) -> &'graph [&'graph DeclaredGroup<'graph>] {
        self.held
    }

    /// One past the last statement — where a deferred mention reads.
    pub fn end(&self) -> Position {
        Position(self.body.len() as u32 + 1)
    }

    pub fn entered_at(&self) -> Position {
        self.entered_at
    }

    /// The slot and declared position of `name` in its own channel.
    pub fn slot(&self, name: BinderSymbol) -> Option<(Slot, Position)> {
        let index = self.names.find(name)?;
        Some((Slot(index as u32), self.names.get(index)))
    }

    /// The name declared at `slot`.
    pub fn slot_name(&self, slot: Slot) -> BinderSymbol {
        self.names.name(slot.index())
    }

    /// Every mention, sorted by site.
    pub fn mentions(&self) -> &'graph [Mention] {
        self.mentions
    }

    /// The mention recorded for the name part at `site`, if the part is one this shape reads.
    pub fn mention(&self, site: Site) -> Option<&'graph Mention> {
        let index = self
            .mentions
            .binary_search_by_key(&site, |mention| mention.site)
            .ok()?;
        Some(&self.mentions[index])
    }

    /// The capture layout, in closure-slot order. Empty for a program and a block.
    ///
    /// A code shape's captures are its `$` names, each read where the quote is written, its open
    /// holes and its open `\` marks — every free name of its code, so nothing is searched when an
    /// `EVAL` runs it.
    pub fn captures(&self) -> &'graph [CaptureSpec] {
        self.captures
    }

    pub fn components(&self) -> &'graph [Component<'graph>] {
        self.components
    }

    /// The body's units in the order they are performed: each after every unit it reads, two
    /// independent ones as they are written. A unit is a component whose members are not all
    /// parameters, or a statement that binds nothing.
    pub fn units(&self) -> &'graph [Unit] {
        self.units
    }

    /// The component `slot` belongs to, by index into [`components`](Self::components).
    pub fn component_index(&self, slot: Slot) -> ComponentIndex {
        self.component_of[slot.index()]
    }

    pub fn component_of(&self, slot: Slot) -> &'graph Component<'graph> {
        &self.components[self.component_index(slot).index()]
    }

    /// The shape of the body or arm whose part sits at `site`.
    pub fn nested(&self, site: Site) -> Option<&'graph BodyShape<'graph>> {
        let index = self
            .nested
            .binary_search_by_key(&site, |(nested, _)| *nested)
            .ok()?;
        Some(self.nested[index].1)
    }

    /// Every nested shape by site.
    pub fn nested_shapes(&self) -> &'graph [(Site, &'graph BodyShape<'graph>)] {
        self.nested
    }

    /// What this body is to the `MATCH` or `TRY` holding it: `Some` exactly for an arm.
    pub fn arm(&self) -> Option<Arm<'graph>> {
        self.arm
    }

    /// The `FN`, `EXPR` or `OP` node whose body this shape is — where a callable's signature and
    /// return type are read. `None` for every other kind.
    pub fn form(&self) -> Option<&'graph KExpression<'graph>> {
        self.form
    }

    /// The body the binder at `slot` births: `Some` for a binder whose right-hand side is a
    /// callable form at its root (`LET f = FN …`), for both binders of a combined form (`LET f =
    /// FN EXPR …`, `LET f = OP …`) and a bare definition's registration, and for a `MODULE` or
    /// `GROUP` binder, whose body shape is [`ShapeKind::Module`] and carries no
    /// [`form`](Self::form); `None` for a data binder, a parameter, and a body nested under
    /// anything else.
    pub fn births(&self, slot: Slot) -> Option<&'graph BodyShape<'graph>> {
        let index = self
            .births
            .binary_search_by_key(&slot, |(binder, _)| *binder)
            .ok()?;
        Some(self.births[index].1)
    }

    /// Where the body the binder at `slot` births sits in this shape's own node — what a caller
    /// that must ask for that body by site names it by.
    pub fn birth_site(&self, slot: Slot) -> Option<Site> {
        let body = self.births(slot)?;
        self.nested
            .iter()
            .find(|(_, nested)| std::ptr::eq(*nested, body))
            .map(|(site, _)| *site)
    }

    /// The right-hand side part of the `LET` binder at `slot`, the part a knot's data member is built
    /// from. `None` for a parameter, a type declaration and a module binder.
    pub fn rhs(&self, slot: Slot) -> Option<&'graph ExpressionPart<'graph>> {
        let index = self
            .rhs
            .binary_search_by_key(&slot, |(binder, _)| *binder)
            .ok()?;
        Some(self.rhs[index].1)
    }

    /// The declaration node of the type binder at `slot` — a `NEWTYPE`, `UNION`, `SIG` or a `LET`
    /// of a type name, whole, so the door reads which declaration it is and where its
    /// declared part sits off the node's builtin shape, as [`form`](Self::form) is where a
    /// callable's signature is read. `None` for a value binder and for a parameter.
    pub fn declarations(&self, slot: Slot) -> Option<&'graph KExpression<'graph>> {
        let index = self
            .declarations
            .binary_search_by_key(&slot, |(binder, _)| *binder)
            .ok()?;
        Some(self.declarations[index].1)
    }

    /// A code shape's carried type — its code kind needing its open `\` marks — and `Code` for
    /// every other kind.
    pub fn code_type(&self) -> KType {
        self.code_type
    }

    /// Why this code shape's code cannot be built, or — once it is built — does not type, reported
    /// when an `EVAL` runs it.
    pub fn refusal(&self) -> Option<&'graph ShapeError<'graph>> {
        self.refusal.or(self.typing_refusal.get())
    }

    /// Every type expression this body records, sorted by site.
    pub fn type_expressions(&self) -> &'graph [TypeExpression<'graph>] {
        self.type_expressions
    }

    /// What the load pass fixed for the type expression at `site`: `Unknown` where this body
    /// records none there.
    pub fn typed_expression(&self, site: Site) -> StaticType<'graph> {
        self.type_expressions
            .binary_search_by_key(&site, |recorded| recorded.site)
            .map_or(Static::Unknown, |index| {
                self.type_expressions[index].typed()
            })
    }

    /// What the load pass fixed for the type binder at `slot`.
    pub fn declared_type(&self, slot: Slot) -> StaticType<'graph> {
        self.declarations
            .binary_search_by_key(&slot, |(binder, _)| *binder)
            .map_or(Static::Unknown, |index| self.declared[index].get())
    }

    /// What the load pass fixed for the registration at `slot`.
    pub fn registered_type(&self, slot: Slot) -> StaticRegistered<'graph> {
        self.registrations
            .binary_search_by_key(&slot, |registration| registration.slot)
            .map_or(Static::Unknown, |index| self.registered[index].get())
    }

    /// What the load pass fixed for this callable body's type.
    pub fn callable_type(&self) -> StaticCallable<'graph> {
        self.callable.get()
    }

    /// Written once, by the load pass.
    pub fn fix_declared(&self, slot: Slot, typed: StaticType<'graph>) {
        let index = self
            .declarations
            .binary_search_by_key(&slot, |(binder, _)| *binder)
            .expect("a type binder records its declaration node");
        debug_assert!(matches!(self.declared[index].get(), Static::Unknown));
        self.declared[index].set(typed);
    }

    /// Written once, by the load pass.
    pub fn fix_registered(&self, slot: Slot, typed: StaticRegistered<'graph>) {
        let index = self
            .registrations
            .binary_search_by_key(&slot, |registration| registration.slot)
            .expect("a registration's slot is one this body declares");
        debug_assert!(matches!(self.registered[index].get(), Static::Unknown));
        self.registered[index].set(typed);
    }

    /// Written once, by the load pass.
    pub fn fix_callable(&self, typed: StaticCallable<'graph>) {
        debug_assert_eq!(self.kind, ShapeKind::Callable);
        debug_assert!(matches!(self.callable.get(), Static::Unknown));
        self.callable.set(typed);
    }

    /// A callable body's own `FOR ALL` group as its body reads it: the lexical variable each
    /// variable of the group is, in group order. Empty for every other shape, and where the load
    /// did not type the callable.
    pub fn group_levels(&self) -> &'graph [Parametric] {
        self.group_levels.get()
    }

    /// Written once, by the load pass.
    pub fn fix_group_levels(&self, levels: &'graph [Parametric]) {
        debug_assert_eq!(self.kind, ShapeKind::Callable);
        debug_assert!(self.group_levels.get().is_empty());
        self.group_levels.set(levels);
    }

    /// Written once, by the load pass, on a code shape whose code does not type.
    pub fn refuse_typing(&self, refusal: &'graph ShapeError<'graph>) {
        debug_assert_eq!(self.kind, ShapeKind::Code);
        debug_assert!(self.typing_refusal.get().is_none());
        self.typing_refusal.set(Some(refusal));
    }

    /// The value channel's load-time facts, once the language's load pass has fixed them.
    pub fn statics(&self) -> Option<Statics<'graph>> {
        self.statics.get()
    }

    /// The static type of the part at `site`, where the load pass typed one.
    pub fn value_type(&self, site: Site) -> Option<Interval> {
        let parts = self.statics.get()?.parts;
        let index = parts.binary_search_by_key(&site, |(at, _)| *at).ok()?;
        Some(parts[index].1)
    }

    /// The static type of statement `index` of [`body`](Self::body).
    pub fn statement_type(&self, index: usize) -> Option<Interval> {
        self.statics.get()?.statements.get(index).copied()
    }

    /// The static type of the binder at `slot` — a quantified callable's binder its scheme.
    pub fn binder_type(&self, slot: Slot) -> Option<DeclaredType<Interval>> {
        self.statics.get()?.binders.get(slot.index()).copied()
    }

    /// What the load fixed about the candidates of the keyworded use at `site`: `Full` where it
    /// fixed nothing.
    pub fn narrowing(&self, site: Site) -> Narrowing<'graph> {
        let Some(statics) = self.statics.get() else {
            return Narrowing::Full;
        };
        self.candidates
            .binary_search_by_key(&site, |(use_site, _)| *use_site)
            .ok()
            .and_then(|index| statics.narrowings.get(index).copied())
            .unwrap_or(Narrowing::Full)
    }

    /// Whether the load settled the ascription at `site`: its operand's static upper end lies under
    /// its type, so the run checks nothing. `false` where the load fixed nothing.
    pub fn settled(&self, site: Site) -> bool {
        self.statics
            .get()
            .is_some_and(|statics| statics.settled.binary_search(&site).is_ok())
    }

    /// Written once, by the language's load pass.
    pub fn fix_statics(&self, statics: Statics<'graph>) {
        debug_assert!(self.statics.get().is_none());
        debug_assert_eq!(statics.narrowings.len(), self.candidates.len());
        self.statics.set(Some(statics));
    }

    /// The names and keys the `EVAL` whose operand sits at `site` offers the code it runs, each
    /// beside what it offers. Empty for an `EVAL` of anything but a parameter needing names.
    pub fn offers(&self, site: Site) -> &'graph [(BinderSymbol, Offer<'graph>)] {
        self.offers
            .binary_search_by_key(&site, |(offered, _)| *offered)
            .map_or(&[], |index| self.offers[index].1)
    }

    /// Every registration this body declares, by slot.
    pub fn registrations(&self) -> &'graph [Registration<'graph>] {
        self.registrations
    }

    /// The registration bound at `slot`, or `None` for a name's slot.
    pub fn registration(&self, slot: Slot) -> Option<&'graph Registration<'graph>> {
        let index = self
            .registrations
            .binary_search_by_key(&slot, |registration| registration.slot)
            .ok()?;
        Some(&self.registrations[index])
    }

    /// Every bucket declaration this body holds, in statement order.
    pub fn rankings(&self) -> &'graph [Ranking<'graph>] {
        self.rankings
    }

    /// A code shape's keyworded holes that some use of the code has no other candidate for: an
    /// `EVAL` refuses the code while one is unfilled, and binds any other unfilled hole to no
    /// function.
    pub fn required_holes(&self) -> &'graph [KeySymbol] {
        self.required
    }

    /// Whether every keyworded use at `key` that lists a candidate of its own beside a spread —
    /// here, and in each body nested here short of another quote's code — carries the ranking
    /// `classes`: what a `USING` filling this code's hole at `key` must agree with.
    pub fn ranks_alike(&self, key: KeySymbol, classes: &[u8]) -> bool {
        let own = self.candidates.iter().all(|(_, list)| {
            list.key != key
                || list.classes == classes
                || list
                    .candidates
                    .iter()
                    .all(|candidate| matches!(candidate, Candidate::Spread(_)))
        });
        own && self
            .nested
            .iter()
            .all(|(_, body)| body.kind == ShapeKind::Code || body.ranks_alike(key, classes))
    }

    /// Every keyworded use's candidates, by the use's node site, sorted.
    pub fn candidate_lists(&self) -> &'graph [(Site, CandidateList<'graph>)] {
        self.candidates
    }

    /// The candidates of the keyworded use whose node sits at `site` ([`Site::of_node`]).
    pub fn candidates(&self, site: Site) -> Option<&'graph CandidateList<'graph>> {
        let index = self
            .candidates
            .binary_search_by_key(&site, |(use_site, _)| *use_site)
            .ok()?;
        Some(&self.candidates[index].1)
    }
}

/// Where the part at `site` within `node` is written: its own span, else the nearest spanned part's
/// or node's around it. `None` when `node` does not hold the part.
pub fn source_of(node: &KExpression<'_>, site: Site) -> Option<SourceRef> {
    build::source_within(node, site)
}

/// `name` read through `mark` at `at` over one body's declared names and captures: a local visible
/// at `at`, else a capture of that name through the same mark. A `$` name never binds to a local,
/// since it resolves where the quote holding it is written.
fn resolve_here(
    names: Channels<'_, Position>,
    captures: &[CaptureSpec],
    name: BinderSymbol,
    mark: Option<Mark>,
    at: Position,
) -> Option<Target> {
    if mark != Some(Mark::Written)
        && let Some(index) = names.find(name)
        && at.sees(names.get(index))
    {
        return Some(Target::Local(Slot(index as u32)));
    }
    captures
        .iter()
        .position(|capture| capture.name == name && capture.mark == mark)
        .map(|index| Target::Capture(CaptureSlot(index as u32)))
}

/// Why a shape could not be built. Every error carries where it was found, as a [`SourceRef`]:
/// the part it is about when that part carries a span, else the nearest spanned part or node
/// around it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeError<'graph> {
    /// A name declared twice in one shape, at where each declaration is written.
    Rebind {
        name: BinderSymbol,
        first: SourceRef,
        second: SourceRef,
    },
    /// A binding under a builtin's name, declared at `at`.
    ShadowsBuiltin { name: BinderSymbol, at: SourceRef },
    /// A name read at `at`, where no binding of it is visible.
    Unbound {
        name: BinderSymbol,
        site: Site,
        at: SourceRef,
    },
    /// A quantified `FN` written at `at`, anywhere but a binder's right-hand side or the head of a
    /// call.
    QuantifiedLambda { at: SourceRef },
    /// The last statement of a body whose value is read — a callable's, an arm's, a quote's —
    /// binding a quantified function at `at`: the statement's value is the body's, and nothing
    /// solves the function's group to give it a concrete type.
    QuantifiedValue { at: SourceRef },
    /// A name bound to a quantified function, read at `at` anywhere but the head of a call.
    QuantifiedRead {
        name: BinderSymbol,
        site: Site,
        at: SourceRef,
    },
    /// A component containing an eager mention of one of its own members, found at the statement
    /// of the member declared first: its named members, and the key of each registration in it.
    EagerCycle {
        members: &'graph [BinderSymbol],
        definitions: &'graph [&'graph [KeyElement]],
        at: SourceRef,
    },
    /// A `$` or `\` mark at `at` that no quote value holds.
    MarkOutsideQuote { at: SourceRef },
    /// A form the shape builder does not resolve.
    Unsupported { form: BuiltinShapeId, at: SourceRef },
    /// A form whose body or branches are not the shape it declares.
    Malformed { form: BuiltinShapeId, at: SourceRef },
    /// A `USING` whose operand, at `at`, does not say where the shape is built which names it
    /// surfaces.
    Unsurfaced { at: SourceRef, site: Site },
    /// An operator run naming a symbol, at `at`, whose group no enclosing body holds.
    Unchained {
        symbol: KeywordSymbol,
        at: SourceRef,
    },
    /// An operator run whose symbols chain under two different groups; `at` is the second's.
    MixedGroups {
        first: KeywordSymbol,
        second: KeywordSymbol,
        at: SourceRef,
    },
    /// A `GROUP` — or a `USING` surfacing one — over a symbol another group already covers, or
    /// over one a `UNARY OP` has marked unary.
    RedeclaresGroup {
        symbol: KeywordSymbol,
        at: SourceRef,
    },
    /// A binary `OP` declaring a result type of its own whose symbol does not chain pairwise.
    ResultOutsidePairwise {
        symbol: KeywordSymbol,
        at: SourceRef,
    },
    /// An operator run whose chained node would spell a builtin form; `at` is the operator's.
    SpellsForm {
        symbol: KeywordSymbol,
        at: SourceRef,
    },
    /// A declaration naming `!=`, which is always the opposite of `==` and is declared by nobody.
    Derived {
        symbol: KeywordSymbol,
        at: SourceRef,
    },
    /// A part read as a quote, or as a container of quotes, written some other way.
    Unquoted {
        form: BuiltinShapeId,
        part: QuotedPart,
        at: SourceRef,
    },
    /// A part read as written whose syntax admits none of its slot's types; `index` counts the
    /// form's elements from zero, and `slot` is the first overload's type there.
    Inadmissible {
        form: BuiltinShapeId,
        index: usize,
        slot: KType,
        at: SourceRef,
    },
    /// A value dict written with a `_` key, whose default has no reading yet.
    DictDefault { site: Site, at: SourceRef },
    /// A definition or bucket declaration at the key of a builtin expression shape, which is
    /// closed.
    ClosedBucket {
        key: &'graph [KeyElement],
        at: SourceRef,
    },
    /// A definition or bucket declaration whose key spells no keyword.
    NoKeyword { at: SourceRef },
    /// A definition whose head ranks a slot, which only a bucket declaration may.
    RankedDefinition { at: SourceRef },
    /// A binder or bucket declaration that is not its statement's own expression.
    NestedBinder { at: SourceRef },
    /// Two rankings of one key that meet: a declaration or definition seeing another, or a
    /// keyworded use seeing both.
    RankingDisagrees {
        key: &'graph [KeyElement],
        at: SourceRef,
    },
    /// A keyworded use at a key with no builtin overload and no registration visible to it.
    NoCandidate {
        key: &'graph [KeyElement],
        at: SourceRef,
    },
    /// A registration whose closed operand types meet those of the builtin overload `builtin` at
    /// its key, whose operands are not all `Any`.
    Overlaps {
        key: &'graph [KeyElement],
        builtin: KType,
        at: SourceRef,
    },
    /// A keyworded use none of whose candidates can admit its arguments' static types.
    NoAdmittingCandidate {
        key: &'graph [KeyElement],
        arguments: &'graph [Parametric],
        at: SourceRef,
    },
    /// A keyworded use whose last candidate a builtin's type rule dropped, the argument's static
    /// type `of` naming no field `field` the rule needs.
    NoField {
        of: KType,
        field: BinderSymbol,
        at: SourceRef,
    },
    /// A keyworded use every candidate of which always admits its arguments' static types, none of
    /// which ranks first, and no builtin among them.
    Ambiguous {
        key: &'graph [KeyElement],
        arguments: &'graph [Parametric],
        count: usize,
        at: SourceRef,
    },
    /// A callable body whose static type can never satisfy its declared return.
    ReturnNeverSatisfied {
        body: KType,
        returns: KType,
        at: SourceRef,
    },
    /// An ascription whose operand's static type can never satisfy its type.
    AscriptionNeverSatisfied {
        value: KType,
        ascribed: KType,
        at: SourceRef,
    },
    /// An `EVAL` whose operand's static type can never be code.
    NotCode { value: KType, at: SourceRef },
    /// An `EVAL` of traced code whose static type can never satisfy the type the `EVAL` declares.
    EvalNeverSatisfied {
        code: KType,
        returns: KType,
        at: SourceRef,
    },
    /// A closed type that does not elaborate.
    Type { error: Elaboration, at: SourceRef },
    /// Two guards of one `MATCH … WITH` arm set that type to one handle, `guard`; `at` is the
    /// second's.
    RepeatedGuard {
        guard: Parametric,
        first: SourceRef,
        at: SourceRef,
    },
}

/// Which part of a form a quote, or a container of quotes, was wanted for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum QuotedPart {
    Body,
    Head,
    Symbol,
    Arms,
    Variants,
    Quantifiers,
    Members,
}

impl QuotedPart {
    /// The part a slot under `role` holds, for a role read as a quote or a container.
    pub(crate) fn of(role: Role) -> QuotedPart {
        match role {
            Role::Head => QuotedPart::Head,
            Role::Data => QuotedPart::Symbol,
            Role::Branches(_) => QuotedPart::Arms,
            Role::Definition(DefinitionKind::Union) => QuotedPart::Variants,
            Role::Definition(DefinitionKind::Members) => QuotedPart::Members,
            Role::Quantifiers => QuotedPart::Quantifiers,
            _ => QuotedPart::Body,
        }
    }

    /// How the part is written, as a diagnostic tells the writer to write it.
    fn spelling(self) -> &'static str {
        match self {
            QuotedPart::Body => "body as a quote: write #(…)",
            QuotedPart::Head => "head as a quote: write #(…)",
            QuotedPart::Symbol => "symbol as a quote: write #(…)",
            QuotedPart::Arms => "arms as a dict of quotes: write #{…}",
            QuotedPart::Variants => "variants as a dict of quotes: write #{…}",
            QuotedPart::Quantifiers => {
                "quantifiers as a list or dict of quotes: write #[…] or #{…}"
            }
            QuotedPart::Members => "members as a list of quotes: write #[…]",
        }
    }
}

impl ShapeError<'_> {
    /// Where the error was found: a rebind's second declaration, every other error's `at`.
    pub fn at(&self) -> SourceRef {
        match self {
            ShapeError::Rebind { second, .. } => *second,
            ShapeError::ShadowsBuiltin { at, .. }
            | ShapeError::Unbound { at, .. }
            | ShapeError::QuantifiedLambda { at }
            | ShapeError::QuantifiedValue { at }
            | ShapeError::QuantifiedRead { at, .. }
            | ShapeError::EagerCycle { at, .. }
            | ShapeError::MarkOutsideQuote { at }
            | ShapeError::Unsupported { at, .. }
            | ShapeError::Malformed { at, .. }
            | ShapeError::Unsurfaced { at, .. }
            | ShapeError::Unchained { at, .. }
            | ShapeError::MixedGroups { at, .. }
            | ShapeError::RedeclaresGroup { at, .. }
            | ShapeError::ResultOutsidePairwise { at, .. }
            | ShapeError::SpellsForm { at, .. }
            | ShapeError::Derived { at, .. }
            | ShapeError::Unquoted { at, .. }
            | ShapeError::Inadmissible { at, .. }
            | ShapeError::DictDefault { at, .. }
            | ShapeError::ClosedBucket { at, .. }
            | ShapeError::NoKeyword { at }
            | ShapeError::RankedDefinition { at }
            | ShapeError::NestedBinder { at }
            | ShapeError::RankingDisagrees { at, .. }
            | ShapeError::NoCandidate { at, .. }
            | ShapeError::Overlaps { at, .. }
            | ShapeError::NoAdmittingCandidate { at, .. }
            | ShapeError::NoField { at, .. }
            | ShapeError::Ambiguous { at, .. }
            | ShapeError::ReturnNeverSatisfied { at, .. }
            | ShapeError::AscriptionNeverSatisfied { at, .. }
            | ShapeError::NotCode { at, .. }
            | ShapeError::EvalNeverSatisfied { at, .. }
            | ShapeError::Type { at, .. }
            | ShapeError::RepeatedGuard { at, .. } => *at,
        }
    }

    /// The error rendered, led by where it was found (`path:line:col: `), with its names spelled
    /// through `symbols` and its types through `types`.
    pub fn display<'x, 'run>(
        &'x self,
        symbols: &'x SymbolInterner,
        types: &'x TypeRegistry<'run>,
    ) -> ShapeErrorDisplay<'x, 'run> {
        ShapeErrorDisplay {
            error: self,
            symbols,
            types,
        }
    }
}

/// A [`ShapeError`] beside the interner its names render through and the registry its types
/// render through.
pub struct ShapeErrorDisplay<'x, 'run> {
    error: &'x ShapeError<'x>,
    symbols: &'x SymbolInterner,
    types: &'x TypeRegistry<'run>,
}

impl fmt::Display for ShapeErrorDisplay<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = |name: &BinderSymbol| self.symbols.display(name.symbol());
        let operator = |symbol: &KeywordSymbol| self.symbols.display(symbol.symbol());
        write!(f, "{}: ", self.error.at())?;
        match self.error {
            ShapeError::Rebind {
                name: bound, first, ..
            } => write!(f, "`{}` is bound twice; first at {first}", name(bound)),
            ShapeError::ShadowsBuiltin { name: bound, .. } => write!(
                f,
                "`{}` names a builtin, which cannot be rebound",
                name(bound)
            ),
            ShapeError::Unbound { name: read, .. } => {
                write!(f, "`{}` names no binding visible here", name(read))
            }
            ShapeError::QuantifiedLambda { .. } => f.write_str(
                "a quantified `FN` is written only as a binder's right-hand side or the head of a \
                 call",
            ),
            ShapeError::QuantifiedValue { .. } => f.write_str(
                "this binds a quantified function as the body's value, and nothing solves its \
                 group to a concrete type; end the body with another statement",
            ),
            ShapeError::QuantifiedRead { name: read, .. } => write!(
                f,
                "`{}` is quantified, so it is read only at the head of a call; wrap it in an \
                 unquantified `FN` to pass it",
                name(read)
            ),
            ShapeError::EagerCycle {
                members,
                definitions,
                ..
            } => {
                f.write_str("these bindings need each other's values before any of them exists:")?;
                for member in members.iter() {
                    write!(f, " `{}`", name(member))?;
                }
                for key in definitions.iter() {
                    write!(f, " `{}`", self.key(key))?;
                }
                Ok(())
            }
            ShapeError::MarkOutsideQuote { .. } => f.write_str(
                "a `$` or `\\` mark belongs to a quote value, and no quote value holds this one",
            ),
            ShapeError::Unsupported { form, .. } => {
                write!(f, "`{form:?}` is not supported here yet")
            }
            ShapeError::Malformed { form, .. } => {
                write!(f, "`{form:?}` is not the shape it declares")
            }
            ShapeError::Unsurfaced { .. } => f.write_str(
                "`USING` cannot tell which names this module surfaces; \
                 ascribe it here: `USING (m :! Sig) SCOPE (…)`",
            ),
            ShapeError::Unchained { symbol, .. } => write!(
                f,
                "`{}` chains under a group no body around it holds; \
                 surface it here: `USING <group> SCOPE (…)`",
                operator(symbol)
            ),
            ShapeError::MixedGroups { first, second, .. } => write!(
                f,
                "`{}` and `{}` chain under different groups, so this run has no one shape; \
                 parenthesize it",
                operator(first),
                operator(second)
            ),
            ShapeError::RedeclaresGroup { symbol, .. } => write!(
                f,
                "`{}` already chains another way; a symbol chains one way in one program",
                operator(symbol)
            ),
            ShapeError::ResultOutsidePairwise { symbol, .. } => write!(
                f,
                "`{}` declares a result of its own, which only a pairwise operator may do",
                operator(symbol)
            ),
            ShapeError::SpellsForm { symbol, .. } => write!(
                f,
                "a run of `{}` chains into a node spelling a builtin form",
                operator(symbol)
            ),
            ShapeError::Derived { symbol, .. } => write!(
                f,
                "`{}` is always the opposite of `==` and is declared by nobody",
                operator(symbol)
            ),
            ShapeError::Unquoted { form, part, .. } => {
                write!(f, "`{form:?}` takes its {}", part.spelling())
            }
            ShapeError::Inadmissible {
                form, index, slot, ..
            } => write!(
                f,
                "`{form:?}` takes {} as its part {index}",
                display_name(*slot, self.types, self.symbols)
            ),
            ShapeError::DictDefault { .. } => {
                f.write_str("a dict's `_` default is not supported yet")
            }
            ShapeError::ClosedBucket { key, .. } => write!(
                f,
                "`{}` is a builtin expression shape, which no definition adds to",
                self.key(key)
            ),
            ShapeError::NoKeyword { .. } => {
                f.write_str("a definition's head must spell at least one keyword")
            }
            ShapeError::RankedDefinition { .. } => f.write_str(
                "a definition's head ranks no slot; declare the ranking on its own: `EXPR #(…)`",
            ),
            ShapeError::NestedBinder { .. } => {
                f.write_str("a binding must be its statement's own expression, not a part of one")
            }
            ShapeError::RankingDisagrees { key, .. } => {
                write!(f, "`{}` is ranked two ways here", self.key(key))
            }
            ShapeError::NoCandidate { key, .. } => {
                write!(f, "`{}` has no overload visible here", self.key(key))
            }
            ShapeError::Overlaps { key, builtin, .. } => write!(
                f,
                "this overload of `{}` takes operands the builtin {} already takes",
                self.key(key),
                display_name(*builtin, self.types, self.symbols)
            ),
            ShapeError::NoAdmittingCandidate { key, arguments, .. } => {
                write!(f, "no overload of `{}` admits ", self.key(key))?;
                self.arguments(f, arguments)
            }
            ShapeError::NoField { of, field, .. } => write!(
                f,
                "{} has no field {}",
                display_name(*of, self.types, self.symbols),
                name(field)
            ),
            ShapeError::Ambiguous {
                key,
                arguments,
                count,
                ..
            } => {
                write!(
                    f,
                    "ambiguous call of {}: {count} overloads admit ",
                    self.key(key)
                )?;
                self.arguments(f, arguments)?;
                f.write_str(" and none ranks first")
            }
            ShapeError::ReturnNeverSatisfied { body, returns, .. } => write!(
                f,
                "this body returns {}, which can never satisfy its declared return {}",
                display_name(*body, self.types, self.symbols),
                display_name(*returns, self.types, self.symbols)
            ),
            ShapeError::AscriptionNeverSatisfied {
                value, ascribed, ..
            } => write!(
                f,
                "this value is {}, which can never satisfy its ascription {}",
                display_name(*value, self.types, self.symbols),
                display_name(*ascribed, self.types, self.symbols)
            ),
            ShapeError::NotCode { value, .. } => write!(
                f,
                "this value is {}, which can never be code for `EVAL` to run",
                display_name(*value, self.types, self.symbols)
            ),
            ShapeError::EvalNeverSatisfied { code, returns, .. } => write!(
                f,
                "this `EVAL`'s code returns {}, which can never satisfy its declared return {}",
                display_name(*code, self.types, self.symbols),
                display_name(*returns, self.types, self.symbols)
            ),
            ShapeError::Type { error, .. } => {
                write!(f, "{}", error.display(self.symbols, self.types))
            }
            ShapeError::RepeatedGuard { guard, first, .. } => write!(
                f,
                "`{}` guards two arms; first at {first}",
                display_name(*guard, self.types, self.symbols)
            ),
        }
    }
}

impl ShapeErrorDisplay<'_, '_> {
    fn key<'k>(&'k self, key: &'k [KeyElement]) -> KeyDisplay<'k> {
        spelled(key, self.symbols)
    }

    /// `arguments` as a parenthesized, comma-separated list of types.
    fn arguments(&self, f: &mut fmt::Formatter<'_>, arguments: &[Parametric]) -> fmt::Result {
        f.write_str("(")?;
        for (index, argument) in arguments.iter().enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{}", display_name(*argument, self.types, self.symbols))?;
        }
        f.write_str(")")
    }
}

/// `key` spelled as it is written: its keywords, `_` at each slot.
pub fn spelled<'k>(key: &'k [KeyElement], symbols: &'k SymbolInterner) -> KeyDisplay<'k> {
    KeyDisplay { key, symbols }
}

/// A bucket key beside the interner its keywords render through.
pub struct KeyDisplay<'k> {
    key: &'k [KeyElement],
    symbols: &'k SymbolInterner,
}

impl fmt::Display for KeyDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, element) in self.key.iter().enumerate() {
            if index > 0 {
                f.write_str(" ")?;
            }
            match element {
                KeyElement::Keyword(keyword) => {
                    write!(f, "{}", self.symbols.display(keyword.symbol()))?
                }
                KeyElement::Slot => f.write_str("_")?,
            }
        }
        Ok(())
    }
}
