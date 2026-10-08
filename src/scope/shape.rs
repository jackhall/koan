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
//! [`Narrowing`] — which the language's load pass fixes. Two more record where a run reads a
//! lexical variable: each one the body declares, beside its slot or capture, and each **type
//! capture** the static pass adds past the builder's captures, so a callable can read a type name
//! its body never writes but a call in it solves from. See [README.md § Load-time
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
use crate::parse::BuiltinShapeId;
use crate::parse::{DefinitionKind, Heads, Role};
use crate::parse::{ExpressionPart, KExpression, KeyElement, Mark};
use crate::source::SourceRef;
use crate::symbols::{BinderSymbol, KeySymbol, KeywordSymbol, SymbolInterner, TypeSymbol};
use crate::type_lattice::{
    DeclaredGroup, DeclaredType, Interval, KType, Parametric, Scheme, TypeRegistry, display_name,
};
use crate::values::{ContentDigest, Knotted};

use super::builtins::Builtins;
use super::channels::Channels;
use super::groups::GroupFrame;
use super::typed::{
    Elaboration, Narrowing, Static, StaticCallable, StaticRegistered, StaticSolution, StaticType,
    Statics,
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
    /// In a `USING … SCOPE` block, every head its operand declares at the key; `None` for a
    /// definition's registration.
    pub surfaced: Option<&'graph [SurfacedHead<'graph>]>,
}

/// A keyworded head a `USING … SCOPE` operand declares, which the block holds under its key as one
/// registration parameter per key: typed where the program loads, and bound where the block runs
/// to the list of the functions the module offers at the key. Each place is named by how many
/// shapes out from the block it lies, so the load pass reads it off the chain of shapes it keeps.
#[derive(Clone, Copy, Debug)]
pub enum SurfacedHead<'graph> {
    /// A bodyless head the operand's signature declares.
    Signature {
        /// The head's statement in the signature's body.
        head: &'graph KExpression<'graph>,
        /// The `SIG` declaration's type binder: its shape's hops out, and its slot there.
        signature: (u32, Slot),
        /// The ascription naming the signature: its shape's hops out, and its type part's site
        /// there.
        ascription: (u32, Site),
    },
    /// A definition the operand's `MODULE` or `GROUP` body holds.
    Body {
        /// The definition's statement in the body.
        definition: &'graph KExpression<'graph>,
        /// The module's binder: its shape's hops out, and its slot there.
        module: (u32, Slot),
    },
}

impl SurfacedHead<'_> {
    /// Where the head is written.
    pub fn source(&self) -> SourceRef {
        match self {
            SurfacedHead::Signature { head, .. } => head.source,
            SurfacedHead::Body { definition, .. } => definition.source,
        }
    }
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
    /// A `MODULE` or `GROUP` body's `OVER` list, where it writes one; `None` for every other body,
    /// and for a module body written with none, which captures nothing from outside.
    over: Option<&'graph [Listed<'graph>]>,
    /// The top-level bindings a module body's `OVER` list names, sorted: part of its content
    /// whether or not the body reads them.
    listed_top: &'graph [TopLevel],
    /// The body's code as resolved, digested where it was built.
    code: ContentDigest,
    /// Each capture's top-level slot, where it reads the program's top level, parallel to
    /// `captures`.
    top_captures: &'graph [Option<Slot>],
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
    /// The solution a callable body's `FOR ALL` is born instantiated at, wherever it is born;
    /// `Unknown` where it is born quantified, and for every other kind.
    born_instance: &'graph Cell<StaticSolution<'graph>>,
    /// Each lexical variable this body declares, by level, beside where its activation holds the
    /// type a run binds it to. Written once, by the type channel.
    declared_variables: &'graph Cell<&'graph [(usize, Target)]>,
    /// Each type capture the static pass added past `captures`, as the coordinate of the enclosing
    /// activation its birth reads: closure slot `captures.len() + i`. Empty for every kind but a
    /// callable and a module.
    type_captures: &'graph Cell<&'graph [Coordinate]>,
    /// Why a code shape's code does not type, where the load pass found a refusal in it.
    typing_refusal: &'graph Cell<Option<&'graph ShapeError<'graph>>>,
    /// The value channel's static types and narrowings, fixed by the language's load pass.
    statics: &'graph Cell<Option<Statics<'graph>>>,
}

/// One entry of a `MODULE` or `GROUP` body's `OVER` list: a name, or a registration's key written
/// with `_` in each slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Listed<'graph> {
    Name(BinderSymbol),
    Key(&'graph [KeyElement]),
}

/// A top-level binding a module's `OVER` list names: a slot of the program's own shape, or a
/// builtin. The run never reads one through a capture, so it is part of the module's content alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TopLevel {
    Root(Slot),
    Builtin(BuiltinIndex),
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

    /// A `MODULE` or `GROUP` body's `OVER` list, where it writes one.
    pub fn over(&self) -> Option<&'graph [Listed<'graph>]> {
        self.over
    }

    /// The top-level bindings a module body's `OVER` list names, sorted.
    pub fn listed_top(&self) -> &'graph [TopLevel] {
        self.listed_top
    }

    /// The body's code digest: its code as resolved, a read of the program's top level named by
    /// the binding it reads ([build/digest.rs](shape/build/digest.rs)).
    pub fn code_digest(&self) -> ContentDigest {
        self.code
    }

    /// Whether capture `capture` reads the program's top level, which the run reads where it
    /// lives: the code digest names it, so a closure's or a module's digest does not compose it.
    pub fn composes(&self, capture: CaptureSlot) -> bool {
        self.top_captures
            .get(capture.index())
            .is_none_or(|top| top.is_none())
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

    /// The type the value binder at `slot` is annotated with — the type part of the
    /// `LET <name> <type> = <value>` declaring it — or `None` for any other binder.
    pub fn annotation(&self, slot: Slot) -> Option<&'graph ExpressionPart<'graph>> {
        let statement = self.names.get(slot.index()).statement_index()?;
        let node = self.body.get(statement)?.statement_spine();
        let form = node.cache().builtin_shape()?;
        if form.id != BuiltinShapeId::LetAnnotated {
            return None;
        }
        form.roles()
            .zip(node.parts)
            .find(|(role, _)| *role == Role::TypeExpression)
            .map(|(_, part)| &part.value)
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

    /// The solution this callable body's `FOR ALL` is born instantiated at, where the load solved
    /// its group at the type its value is wanted at: a literal written where a type fixes it, or
    /// the right-hand side of a binder whose declared type does.
    pub fn born_instance(&self) -> Option<StaticSolution<'graph>> {
        match self.born_instance.get() {
            Static::Unknown => None,
            solution => Some(solution),
        }
    }

    /// Written once, by the load pass.
    pub fn fix_born_instance(&self, solution: StaticSolution<'graph>) {
        debug_assert_eq!(self.kind, ShapeKind::Callable);
        debug_assert!(matches!(self.born_instance.get(), Static::Unknown));
        debug_assert!(!matches!(solution, Static::Unknown));
        self.born_instance.set(solution);
    }

    /// Each lexical variable this body declares, by level, beside where its activation holds the
    /// type a run binds it to: a `FOR ALL` name's slot, a type binder's slot, or the capture a
    /// surfaced type name is read through.
    pub fn declared_variables(&self) -> &'graph [(usize, Target)] {
        self.declared_variables.get()
    }

    /// Written once, by the type channel.
    pub fn fix_declared_variables(&self, variables: &'graph [(usize, Target)]) {
        debug_assert!(self.declared_variables.get().is_empty());
        self.declared_variables.set(variables);
    }

    /// Each type capture the static pass added past [`captures`](Self::captures): a type name the
    /// body never writes but a call in it contributes, read at birth from this coordinate of the
    /// enclosing activation into closure slot `captures().len() + i`.
    pub fn type_captures(&self) -> &'graph [Coordinate] {
        self.type_captures.get()
    }

    /// Written once, by the load pass.
    pub fn fix_type_captures(&self, captures: &'graph [Coordinate]) {
        debug_assert!(matches!(self.kind, ShapeKind::Callable | ShapeKind::Module));
        debug_assert!(self.type_captures.get().is_empty());
        self.type_captures.set(captures);
    }

    /// How many closure slots a birth fills: the builder's captures, then the type captures.
    pub fn capture_count(&self) -> usize {
        self.captures.len() + self.type_captures.get().len()
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

    /// The solution the load instantiated the quantified function read at `site` at, where it read
    /// one there.
    pub fn instance_at(&self, site: Site) -> Option<StaticSolution<'graph>> {
        let instances = self.statics.get()?.instances;
        let index = instances.binary_search_by_key(&site, |(at, _)| *at).ok()?;
        Some(instances[index].1)
    }

    /// What each argument of the keyworded use at `site` contributes to its candidates' solves:
    /// empty where every argument contributes its carried type, or the load fixed nothing.
    pub fn contributions(&self, site: Site) -> &'graph [StaticType<'graph>] {
        let Some(statics) = self.statics.get() else {
            return &[];
        };
        self.candidates
            .binary_search_by_key(&site, |(use_site, _)| *use_site)
            .ok()
            .and_then(|index| statics.contributions.get(index).copied())
            .unwrap_or(&[])
    }

    /// What each parameter of the call by name whose argument part sits at `site` is solved from,
    /// per parameter in symbol order: empty where every parameter reads its carried type.
    pub fn named_contributions(&self, site: Site) -> &'graph [StaticType<'graph>] {
        let Some(statics) = self.statics.get() else {
            return &[];
        };
        statics
            .named
            .binary_search_by_key(&site, |(at, _)| *at)
            .map_or(&[], |index| statics.named[index].1)
    }

    /// Written once, by the language's load pass.
    pub fn fix_statics(&self, statics: Statics<'graph>) {
        debug_assert!(self.statics.get().is_none());
        debug_assert_eq!(statics.narrowings.len(), self.candidates.len());
        debug_assert_eq!(statics.contributions.len(), self.candidates.len());
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
    /// The last statement of a body whose value is read — a callable's, an arm's, a quote's —
    /// binding a quantified function by a keyworded form at `at`: the statement's value is the
    /// body's, and a keyworded form's function is read only at the head of a call.
    QuantifiedValue { at: SourceRef },
    /// A quantified function read or bound where nothing fixes `variables`, in group order: no type
    /// is wanted there, the `wanted` type is no function type, or it reaches none of them.
    Unfixed {
        variables: &'graph [TypeSymbol],
        wanted: Option<KType>,
        at: SourceRef,
    },
    /// A quantified function wanted at a type no instance of it lies under.
    NoInstance {
        scheme: Scheme,
        wanted: KType,
        at: SourceRef,
    },
    /// A keyworded use whose kept candidates want a quantified function argument at different
    /// instances.
    AmbiguousInstance {
        key: &'graph [KeyElement],
        at: SourceRef,
    },
    /// A keyworded use of several candidates, each dropped where none takes a quantified function
    /// argument at a type that fixes its group.
    NoInstanceAtCandidates {
        key: &'graph [KeyElement],
        at: SourceRef,
    },
    /// A name a keyworded form binds or a `USING … SCOPE` surfaces, bound to a quantified function
    /// and read at `at` anywhere but the head of a call; or a module's quantified member read
    /// through `$` or offered by an `EVAL`.
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
    /// A keyworded use none of whose candidates can admit its arguments' static types: each
    /// argument's upper end, or an instance argument's scheme.
    NoAdmittingCandidate {
        key: &'graph [KeyElement],
        arguments: &'graph [DeclaredType<Parametric>],
        at: SourceRef,
    },
    /// A keyworded use whose last candidate a builtin's type rule dropped, the argument's static
    /// type `of` naming no field `field` the rule needs.
    NoField {
        of: KType,
        field: BinderSymbol,
        at: SourceRef,
    },
    /// A member read whose operand's static type `of`, a signature, names no member `member`.
    NoMember {
        of: KType,
        member: BinderSymbol,
        at: SourceRef,
    },
    /// A `MODULE` or `GROUP` body reading `name` from outside it — bound neither by the body nor by
    /// a top-level statement — where its `OVER` list does not name it. A registration is named by
    /// its key.
    Unlisted { name: BinderSymbol, at: SourceRef },
    /// A quantified member read anywhere but a call's head whose scheme names `parameter`, a head
    /// parameter its module's signature leaves unpinned, which the load cannot name there.
    UnpinnedMember {
        member: BinderSymbol,
        parameter: BinderSymbol,
        at: SourceRef,
    },
    /// A keyworded use every candidate of which always admits its arguments' static types, none of
    /// which ranks first, and no builtin among them.
    Ambiguous {
        key: &'graph [KeyElement],
        arguments: &'graph [DeclaredType<Parametric>],
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
    /// An annotated binder whose value's static type can never satisfy its annotation.
    AnnotationNeverSatisfied {
        value: KType,
        annotated: KType,
        at: SourceRef,
    },
    /// A call by name whose callee is exactly an unquantified function its argument's static type
    /// can never satisfy the parameters of, or a quantified one whose group the argument's static
    /// type can never solve.
    CallNeverSatisfied {
        callee: DeclaredType<KType>,
        arguments: KType,
        at: SourceRef,
    },
    /// An `EVAL` whose operand's static type can never be code.
    NotCode { value: KType, at: SourceRef },
    /// A view ascription whose operand's static type can never be a module.
    NotAModule { value: KType, at: SourceRef },
    /// A view ascription at a type the view door never takes: no one application of a signature.
    NotASignature { ascribed: KType, at: SourceRef },
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
            | ShapeError::QuantifiedValue { at }
            | ShapeError::Unfixed { at, .. }
            | ShapeError::NoInstance { at, .. }
            | ShapeError::AmbiguousInstance { at, .. }
            | ShapeError::NoInstanceAtCandidates { at, .. }
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
            | ShapeError::NoMember { at, .. }
            | ShapeError::Unlisted { at, .. }
            | ShapeError::UnpinnedMember { at, .. }
            | ShapeError::Ambiguous { at, .. }
            | ShapeError::ReturnNeverSatisfied { at, .. }
            | ShapeError::AscriptionNeverSatisfied { at, .. }
            | ShapeError::AnnotationNeverSatisfied { at, .. }
            | ShapeError::CallNeverSatisfied { at, .. }
            | ShapeError::NotCode { at, .. }
            | ShapeError::NotAModule { at, .. }
            | ShapeError::NotASignature { at, .. }
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
            ShapeError::QuantifiedValue { .. } => f.write_str(
                "this keyworded form binds a quantified function as the body's value, which is \
                 read only at the head of a call; end the body with another statement",
            ),
            ShapeError::Unfixed {
                variables, wanted, ..
            } => {
                let types = |f: &mut fmt::Formatter<'_>| {
                    for (index, variable) in variables.iter().enumerate() {
                        let gap = if index == 0 { "" } else { " " };
                        write!(f, "{gap}`{}`", self.symbols.display(variable.symbol()))?;
                    }
                    Ok(())
                };
                match wanted {
                    None => {
                        f.write_str("nothing fixes ")?;
                        types(f)?;
                        f.write_str(
                            " here: a quantified function is read only at the head of a call, as \
                             the binding of a `MODULE` member, or where the type it is wanted at solves \
                             its group",
                        )
                    }
                    Some(wanted) => {
                        write!(
                            f,
                            "{} does not fix ",
                            display_name(*wanted, self.types, self.symbols)
                        )?;
                        types(f)
                    }
                }
            }
            ShapeError::NoInstance { scheme, wanted, .. } => write!(
                f,
                "{} has no instance under {}",
                display_name(*scheme, self.types, self.symbols),
                display_name(*wanted, self.types, self.symbols)
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
            ShapeError::AmbiguousInstance { key, .. } => write!(
                f,
                "the overloads of `{}` this call keeps want this function at different types; \
                 ascribe it",
                self.key(key)
            ),
            ShapeError::NoInstanceAtCandidates { key, .. } => write!(
                f,
                "no overload of `{}` takes this function at a type that fixes its group",
                self.key(key)
            ),
            ShapeError::NoAdmittingCandidate { key, arguments, .. } => {
                selection_refused(f, key, arguments, None, self.symbols, self.types)
            }
            ShapeError::NoField { of, field, .. } => write!(
                f,
                "{} has no field {}",
                display_name(*of, self.types, self.symbols),
                name(field)
            ),
            ShapeError::NoMember { of, member, .. } => write!(
                f,
                "{} has no member {}",
                display_name(*of, self.types, self.symbols),
                name(member)
            ),
            ShapeError::Unlisted { name: read, .. } => {
                // A key is listed as its use is written, in parentheses.
                let listed = match read {
                    BinderSymbol::Key(_) => format!("({})", name(read)),
                    _ => name(read).to_string(),
                };
                write!(
                    f,
                    "`{}` is read from outside this module; list it under its `OVER`, as \
                     `OVER #[{listed}]`",
                    name(read)
                )
            }
            ShapeError::UnpinnedMember {
                member, parameter, ..
            } => write!(
                f,
                "`{}` is quantified over `{}`, which the load cannot name here; call it, or \
                 ascribe its module with `{}` pinned",
                name(member),
                name(parameter),
                name(parameter)
            ),
            ShapeError::Ambiguous {
                key,
                arguments,
                count,
                ..
            } => selection_refused(f, key, arguments, Some(*count), self.symbols, self.types),
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
            ShapeError::AnnotationNeverSatisfied {
                value, annotated, ..
            } => write!(
                f,
                "this value is {}, which can never satisfy its annotation {}",
                display_name(*value, self.types, self.symbols),
                display_name(*annotated, self.types, self.symbols)
            ),
            ShapeError::CallNeverSatisfied {
                callee, arguments, ..
            } => write!(
                f,
                "{} can never be called with {}",
                display_name(*callee, self.types, self.symbols),
                display_name(*arguments, self.types, self.symbols)
            ),
            ShapeError::NotAModule { value, .. } => write!(
                f,
                "{} is no module to ascribe",
                display_name(*value, self.types, self.symbols)
            ),
            ShapeError::NotASignature { ascribed, .. } => write!(
                f,
                "{} is no signature",
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
}

/// A refused selection at `key` worded — no overload admits `arguments`, or (`ambiguous`) that
/// many admit and none ranks first — for the load's refusal and the run's fault alike.
pub fn selection_refused<A: Copy + Into<DeclaredType<Parametric>>>(
    f: &mut fmt::Formatter<'_>,
    key: &[KeyElement],
    arguments: &[A],
    ambiguous: Option<usize>,
    symbols: &SymbolInterner,
    types: &TypeRegistry<'_>,
) -> fmt::Result {
    let key = spelled(key, symbols);
    match ambiguous {
        None => write!(f, "no overload of `{key}` admits (")?,
        Some(count) => write!(f, "ambiguous call of `{key}`: {count} overloads admit (")?,
    }
    for (index, argument) in arguments.iter().enumerate() {
        if index > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{}", display_name(*argument, types, symbols))?;
    }
    f.write_str(")")?;
    match ambiguous {
        None => Ok(()),
        Some(_) => f.write_str(" and none ranks first"),
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
