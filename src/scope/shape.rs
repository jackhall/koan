//! The **shape**: one per body, built once into program storage and shared by every activation of
//! that body.
//!
//! It holds the body's declared names as one two-channel run — value names first, type names
//! after, each channel sorted by symbol and each name at its slot — every mention with its class
//! and coordinate,
//! the capture layout a callable's closure bindings are born through, the strongly connected
//! components of the body's bindings, the shapes nested in it by site, the form node a callable's
//! body sits in, the callable body each binder births, each `LET` binder's right-hand side, and
//! each type binder's declaration node, and the order its units run in. [`build`] is the one
//! builder every kind goes through.
//!
//! **Visibility** is one comparison, [`Position::sees`]: a binding is visible to a reader whose
//! position is strictly greater than the binding's own. A parameter writes at `0`, statement `i` at
//! `i + 1`, and the body's end is one past its last statement. An eager mention reads at its
//! statement's position and a deferred one at the end.
//!
//! See [README.md § Resolution](README.md#resolution).

use std::fmt;

use crate::memory::{BumpAllocator, ProgramBrand};
use crate::parse::builtin_shapes::BuiltinShapeId;
use crate::parse::{ExpressionPart, KExpression};
use crate::symbols::{BinderSymbol, KeywordSymbol, SymbolInterner};
use crate::type_lattice::DeclaredGroup;
use crate::values::{Knotted, KnottedFamily};

use super::activation::ActivationView;
use super::builtins::Builtins;
use super::channels::Channels;
use super::groups::GroupFrame;

mod build;

/// An index into an activation's slot run: value slots first, type slots after.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Slot(pub(crate) u32);

/// An index into a callable's closure bindings.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct CaptureSlot(pub(crate) u32);

/// An index into the builtin table: values first, types after.
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

/// The four kinds of body a shape describes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShapeKind {
    /// The top level: no captures and no enclosing activation.
    Program,
    /// A `FN`, `EXPR` or `OP` body: captures, and is a deferring boundary.
    Callable,
    /// A `MODULE` or `GROUP` body: captures, and is an eager context.
    Module,
    /// A `MATCH` or `TRY` arm, or an `EVAL` body: activated in the enclosing frame beside a pointer
    /// to the enclosing activation.
    Block,
}

/// One closure binding a callable's birth fills, and where from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CaptureSpec {
    pub name: BinderSymbol,
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
    /// boundary, the enclosing body's end for a deferred one, and `EVAL`'s own for an `EVAL` body.
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
    keeps_defining_scope: bool,
}

const _: () = assert!(!std::mem::needs_drop::<BodyShape<'static>>());

impl<'graph> BodyShape<'graph> {
    /// The shape of a program's top-level statements.
    pub fn of_program<X: Knotted>(
        brand: ProgramBrand<'graph>,
        statements: &[KExpression<'graph>],
        builtins: &Builtins<'_, '_, X>,
        scratch: BumpAllocator<'_>,
    ) -> Result<&'graph BodyShape<'graph>, ShapeError> {
        build::program(brand, statements, builtins, scratch)
    }

    /// The block shape of `body` evaluated by an `EVAL` reading at `at` in `site`: every free name
    /// resolves by name over `site`'s chain, and a binder in `body` binds in this block.
    pub fn for_eval<XF: KnottedFamily<'graph>>(
        brand: ProgramBrand<'graph>,
        body: &KExpression<'graph>,
        site: &ActivationView<'graph, '_, XF>,
        at: Position,
        scratch: BumpAllocator<'_>,
    ) -> Result<&'graph BodyShape<'graph>, ShapeError> {
        build::eval(brand, body, site, at, scratch)
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

    /// The operator-group frame this body was built under — what an `EVAL` written here roots its
    /// own frame at, and what decides which groups an operator run here may chain under.
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
    pub fn captures(&self) -> &'graph [CaptureSpec] {
        self.captures
    }

    pub fn components(&self) -> &'graph [Component<'graph>] {
        self.components
    }

    /// The body's units in the order they are performed: each after every unit it reads, two
    /// independent ones as they are written. A unit is a component whose members are not all
    /// parameters, or a statement that binds nothing; a statement containing `EVAL` also follows
    /// every unit binding a name declared before it, since no shape can enumerate what it reads.
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

    /// The `FN`, `EXPR` or `OP` node whose body this shape is — where a callable's signature and
    /// return type are read. `None` for every other kind.
    pub fn form(&self) -> Option<&'graph KExpression<'graph>> {
        self.form
    }

    /// The body the binder at `slot` births: `Some` for a binder whose right-hand side is a
    /// callable form at its root (`LET f = FN …`), for a combined form (`LET f = FN EXPR …`,
    /// `LET f = OP …`), and for a `MODULE` or `GROUP` binder, whose body shape is
    /// [`ShapeKind::Module`] and carries no [`form`](Self::form); `None` for a data binder, a
    /// parameter, and a body nested under anything else.
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

    /// The declaration node of the type binder at `slot` — a `NEWTYPE`, `UNION`, `SIG`, `TYPE` or
    /// a `LET` of a type name, whole, so the door reads which declaration it is and where its
    /// declared part sits off the node's builtin shape, as [`form`](Self::form) is where a
    /// callable's signature is read. `None` for a value binder and for a parameter.
    pub fn declarations(&self, slot: Slot) -> Option<&'graph KExpression<'graph>> {
        let index = self
            .declarations
            .binary_search_by_key(&slot, |(binder, _)| *binder)
            .ok()?;
        Some(self.declarations[index].1)
    }

    /// Whether this shape holds an `EVAL` or encloses a shape that does, so its activation must
    /// stay reachable from the shapes inside it.
    pub fn keeps_defining_scope(&self) -> bool {
        self.keeps_defining_scope
    }

    /// `name` by name within this shape alone: a local visible at `at`, else a capture.
    pub(crate) fn resolve_here(&self, name: BinderSymbol, at: Position) -> Option<Target> {
        resolve_here(self.names, self.captures, name, at)
    }
}

/// `name` read at `at` over one body's declared names and captures: a local visible at `at`, else a
/// capture of that name.
fn resolve_here(
    names: Channels<'_, Position>,
    captures: &[CaptureSpec],
    name: BinderSymbol,
    at: Position,
) -> Option<Target> {
    if let Some(index) = names.find(name)
        && at.sees(names.get(index))
    {
        return Some(Target::Local(Slot(index as u32)));
    }
    captures
        .iter()
        .position(|capture| capture.name == name)
        .map(|index| Target::Capture(CaptureSlot(index as u32)))
}

/// Why a shape could not be built.
#[derive(Debug, PartialEq, Eq)]
pub enum ShapeError {
    /// A name declared twice in one shape; the positions are where each declaration writes.
    Rebind {
        name: BinderSymbol,
        first: Position,
        second: Position,
    },
    /// A binding under a builtin's name, declared at `at`.
    ShadowsBuiltin { name: BinderSymbol, at: Position },
    /// A name read where no binding of it is visible.
    Unbound {
        name: BinderSymbol,
        site: Site,
        at: Position,
    },
    /// A component containing an eager mention of one of its own members.
    EagerCycle { members: Vec<BinderSymbol> },
    /// An `EVAL` at `eval` that may read `name`, declared before it, whose binding waits on the
    /// `EVAL`'s statement.
    EvalCycle { name: BinderSymbol, eval: Position },
    /// A form the shape builder does not resolve.
    Unsupported { form: BuiltinShapeId, at: Position },
    /// A form whose body or branches are not the shape it declares.
    Malformed { form: BuiltinShapeId, at: Position },
    /// A `USING` whose operand does not say, where the shape is built, which names it surfaces.
    Unsurfaced { at: Position, site: Site },
    /// An operator run naming a symbol whose group no enclosing body holds.
    Unchained { symbol: KeywordSymbol, at: Position },
    /// An operator run whose symbols chain under two different groups.
    MixedGroups {
        first: KeywordSymbol,
        second: KeywordSymbol,
        at: Position,
    },
    /// A `GROUP` — or a `USING` surfacing one — over a symbol another group already covers, or
    /// over one a `UNARY OP` has marked unary.
    RedeclaresGroup { symbol: KeywordSymbol, at: Position },
    /// A binary `OP` declaring a result type of its own whose symbol does not chain pairwise.
    ResultOutsidePairwise { symbol: KeywordSymbol, at: Position },
    /// An operator run whose chained node would spell a builtin form.
    SpellsForm { symbol: KeywordSymbol, at: Position },
    /// A declaration naming `!=`, which is always the opposite of `==` and is declared by nobody.
    Derived { symbol: KeywordSymbol, at: Position },
}

impl ShapeError {
    /// The error rendered with its names spelled through `symbols`.
    pub fn display<'x>(&'x self, symbols: &'x SymbolInterner) -> ShapeErrorDisplay<'x> {
        ShapeErrorDisplay {
            error: self,
            symbols,
        }
    }
}

/// A [`ShapeError`] beside the interner its names render through.
pub struct ShapeErrorDisplay<'x> {
    error: &'x ShapeError,
    symbols: &'x SymbolInterner,
}

impl fmt::Display for ShapeErrorDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = |name: &BinderSymbol| self.symbols.display(name.symbol());
        let operator = |symbol: &KeywordSymbol| self.symbols.display(symbol.symbol());
        match self.error {
            ShapeError::Rebind {
                name: bound,
                first,
                second,
            } => write!(
                f,
                "`{}` is bound twice: at {first} and again at {second}",
                name(bound)
            ),
            ShapeError::ShadowsBuiltin { name: bound, at } => write!(
                f,
                "`{}` at {at} names a builtin, which cannot be rebound",
                name(bound)
            ),
            ShapeError::Unbound { name: read, at, .. } => {
                write!(f, "`{}` in {at} names no binding visible there", name(read))
            }
            ShapeError::EagerCycle { members } => {
                f.write_str("these bindings need each other's values before any of them exists:")?;
                for member in members {
                    write!(f, " `{}`", name(member))?;
                }
                Ok(())
            }
            ShapeError::EvalCycle { name: read, eval } => write!(
                f,
                "the `EVAL` in {eval} may read `{}`, which needs that statement's value first",
                name(read)
            ),
            ShapeError::Unsupported { form, at } => {
                write!(f, "`{form:?}` in {at} is not supported here yet")
            }
            ShapeError::Malformed { form, at } => {
                write!(f, "`{form:?}` in {at} is not the shape it declares")
            }
            ShapeError::Unsurfaced { at, .. } => write!(
                f,
                "`USING` in {at} cannot tell which names this module surfaces; \
                 ascribe it here: `USING (m :! Sig) SCOPE (…)`"
            ),
            ShapeError::Unchained { symbol, at } => write!(
                f,
                "`{}` in {at} chains under a group no body around it holds; \
                 surface it here: `USING <group> SCOPE (…)`",
                operator(symbol)
            ),
            ShapeError::MixedGroups { first, second, at } => write!(
                f,
                "`{}` and `{}` in {at} chain under different groups, so this run has no one \
                 shape; parenthesize it",
                operator(first),
                operator(second)
            ),
            ShapeError::RedeclaresGroup { symbol, at } => write!(
                f,
                "`{}` in {at} already chains another way; a symbol chains one way in one program",
                operator(symbol)
            ),
            ShapeError::ResultOutsidePairwise { symbol, at } => write!(
                f,
                "`{}` in {at} declares a result of its own, which only a pairwise operator may do",
                operator(symbol)
            ),
            ShapeError::SpellsForm { symbol, at } => write!(
                f,
                "a run of `{}` in {at} chains into a node spelling a builtin form",
                operator(symbol)
            ),
            ShapeError::Derived { symbol, at } => write!(
                f,
                "`{}` in {at} is always the opposite of `==` and is declared by nobody",
                operator(symbol)
            ),
        }
    }
}

/// A position as a diagnostic names it: a parameter, or a statement counted from one.
impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            0 => f.write_str("a parameter"),
            statement => write!(f, "statement {statement}"),
        }
    }
}
