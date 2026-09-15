//! The **shape**: one per body, built once into program storage and shared by every activation of
//! that body.
//!
//! It holds the body's two declared-name runs — the value channel as `parse`'s [`SlotLayout`], the
//! type channel as a sorted run laid out after it — every mention with its class and coordinate,
//! the capture layout a callable's closure bindings are born through, the strongly connected
//! components of the body's bindings, and the shapes nested in it by site. [`build`] is the one
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
use crate::parse::forms::FormId;
use crate::parse::{BinderSymbol, ExpressionPart, KExpression, LabelInterner, SlotLayout};
use crate::parse::{TypeSymbol, ValueSymbol};

use super::activation::Activation;
use super::builtins::Builtins;

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

    pub fn get(self) -> u32 {
        self.0
    }
}

/// Where a resolved read lands once the enclosing block activations are stepped through.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    Local(Slot),
    Capture(CaptureSlot),
    Builtin(BuiltinIndex),
}

/// A resolved read: `hops` enclosing block activations to step through (`0` for the reader's own),
/// then `target` in the activation reached. A builtin reads through any activation's header, so its
/// `hops` is `0`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Coordinate {
    pub hops: u32,
    pub target: Target,
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
    /// The statement of this shape the mention sits in.
    pub statement: u32,
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
    Member { component: u32, index: u32 },
}

/// A strongly connected component of a shape's bindings.
#[derive(Clone, Copy, Debug)]
pub struct Component<'graph> {
    /// The members, in slot order.
    pub members: &'graph [Slot],
    /// Whether every mention between members is deferred — the component a knot can tie.
    pub deferred_only: bool,
}

/// One body's resolved lexical structure. See the module documentation.
#[derive(Clone, Copy)]
pub struct Shape<'graph> {
    kind: ShapeKind,
    values: &'graph SlotLayout<'graph>,
    types: &'graph [(TypeSymbol, Position)],
    statements: u32,
    /// The position in the enclosing shape this one is entered at: the statement's for an eager
    /// boundary, the enclosing body's end for a deferred one, and `EVAL`'s own for an `EVAL` body.
    entered_at: Position,
    component_of: &'graph [u32],
    components: &'graph [Component<'graph>],
    mentions: &'graph [Mention],
    captures: &'graph [CaptureSpec],
    nested: &'graph [(Site, &'graph Shape<'graph>)],
    keeps_defining_scope: bool,
}

const _: () = assert!(!std::mem::needs_drop::<Shape<'static>>());

impl<'graph> Shape<'graph> {
    /// The shape of a program's top-level statements.
    pub fn of_program(
        brand: ProgramBrand<'graph>,
        statements: &[KExpression<'graph>],
        builtins: &Builtins<'_, '_>,
        scratch: BumpAllocator<'_>,
    ) -> Result<&'graph Shape<'graph>, ShapeError> {
        build::program(brand, statements, builtins, scratch)
    }

    /// The block shape of `body` evaluated by an `EVAL` reading at `at` in `site`: every free name
    /// resolves by name over `site`'s chain, and a binder in `body` binds in this block.
    pub fn for_eval(
        brand: ProgramBrand<'graph>,
        body: &KExpression<'graph>,
        site: &Activation<'graph, '_>,
        at: Position,
        scratch: BumpAllocator<'_>,
    ) -> Result<&'graph Shape<'graph>, ShapeError> {
        build::eval(brand, body, site, at, scratch)
    }

    pub fn kind(&self) -> ShapeKind {
        self.kind
    }

    /// How many slots an activation of this shape holds.
    pub fn slots(&self) -> usize {
        self.values.len() + self.types.len()
    }

    pub fn statements(&self) -> u32 {
        self.statements
    }

    /// One past the last statement — where a deferred mention reads.
    pub fn end(&self) -> Position {
        Position(self.statements + 1)
    }

    pub fn entered_at(&self) -> Position {
        self.entered_at
    }

    /// The value channel's declared names.
    pub fn values(&self) -> &'graph SlotLayout<'graph> {
        self.values
    }

    /// The type channel's declared names, sorted; the name at index `i` takes slot
    /// `values().len() + i`.
    pub fn types(&self) -> &'graph [(TypeSymbol, Position)] {
        self.types
    }

    /// The slot and declared position of the value name `name`.
    pub fn value_slot(&self, name: ValueSymbol) -> Option<(Slot, Position)> {
        let slot = self.values.slot_of(name)?;
        Some((
            Slot(slot as u32),
            Position(self.values.position(slot) as u32),
        ))
    }

    /// The slot and declared position of the type name `name`.
    pub fn type_slot(&self, name: TypeSymbol) -> Option<(Slot, Position)> {
        let index = self
            .types
            .binary_search_by_key(&name, |(symbol, _)| *symbol)
            .ok()?;
        Some((
            Slot((self.values.len() + index) as u32),
            self.types[index].1,
        ))
    }

    /// The slot and declared position of `name` in its own channel.
    pub fn slot(&self, name: BinderSymbol) -> Option<(Slot, Position)> {
        match name {
            BinderSymbol::Value(name) => self.value_slot(name),
            BinderSymbol::Type(name) => self.type_slot(name),
        }
    }

    /// The name declared at `slot`.
    pub fn slot_name(&self, slot: Slot) -> BinderSymbol {
        match slot.index().checked_sub(self.values.len()) {
            None => BinderSymbol::Value(self.values.name(slot.index())),
            Some(ty) => BinderSymbol::Type(self.types[ty].0),
        }
    }

    /// The mention at `site`, if this shape's own statements read a name there.
    pub fn mention(&self, site: Site) -> Option<&'graph Mention> {
        let index = self
            .mentions
            .binary_search_by_key(&site, |mention| mention.site)
            .ok()?;
        Some(&self.mentions[index])
    }

    /// Every mention, sorted by site.
    pub fn mentions(&self) -> &'graph [Mention] {
        self.mentions
    }

    /// The capture layout, in closure-slot order. Empty for a program and a block.
    pub fn captures(&self) -> &'graph [CaptureSpec] {
        self.captures
    }

    pub fn components(&self) -> &'graph [Component<'graph>] {
        self.components
    }

    /// The component `slot` belongs to, by index into [`components`](Self::components).
    pub fn component_index(&self, slot: Slot) -> u32 {
        self.component_of[slot.index()]
    }

    pub fn component_of(&self, slot: Slot) -> &'graph Component<'graph> {
        &self.components[self.component_index(slot) as usize]
    }

    /// The shape of the body or arm whose part sits at `site`.
    pub fn nested(&self, site: Site) -> Option<&'graph Shape<'graph>> {
        let index = self
            .nested
            .binary_search_by_key(&site, |(nested, _)| *nested)
            .ok()?;
        Some(self.nested[index].1)
    }

    /// Every nested shape by site.
    pub fn nested_shapes(&self) -> &'graph [(Site, &'graph Shape<'graph>)] {
        self.nested
    }

    /// Whether this shape holds an `EVAL` or encloses a shape that does, so its activation must
    /// stay reachable from the shapes inside it.
    pub fn keeps_defining_scope(&self) -> bool {
        self.keeps_defining_scope
    }

    /// `name` by name within this shape alone: a local visible at `at`, else a capture.
    pub(crate) fn resolve_here(&self, name: BinderSymbol, at: Position) -> Option<Target> {
        if let Some((slot, declared)) = self.slot(name)
            && at.sees(declared)
        {
            return Some(Target::Local(slot));
        }
        self.captures
            .iter()
            .position(|capture| capture.name == name)
            .map(|index| Target::Capture(CaptureSlot(index as u32)))
    }
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
        statement: u32,
    },
    /// A component containing an eager mention of one of its own members.
    EagerCycle { members: Vec<BinderSymbol> },
    /// A form the shape builder does not resolve.
    Unsupported { form: FormId, statement: u32 },
    /// A form whose body or branches are not the shape it declares.
    Malformed { form: FormId, statement: u32 },
}

impl ShapeError {
    /// The error rendered with its names spelled through `labels`.
    pub fn display<'x>(&'x self, labels: &'x LabelInterner) -> ShapeErrorDisplay<'x> {
        ShapeErrorDisplay {
            error: self,
            labels,
        }
    }
}

/// A [`ShapeError`] beside the interner its names render through.
pub struct ShapeErrorDisplay<'x> {
    error: &'x ShapeError,
    labels: &'x LabelInterner,
}

impl fmt::Display for ShapeErrorDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = |name: &BinderSymbol| self.labels.display(name.symbol());
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
            ShapeError::Unbound {
                name: read,
                statement,
                ..
            } => write!(
                f,
                "`{}` in {} names no binding visible there",
                name(read),
                Position::statement(*statement as usize)
            ),
            ShapeError::EagerCycle { members } => {
                f.write_str("these bindings need each other's values before any of them exists:")?;
                for member in members {
                    write!(f, " `{}`", name(member))?;
                }
                Ok(())
            }
            ShapeError::Unsupported { form, statement } => write!(
                f,
                "`{form:?}` in {} is not supported here yet",
                Position::statement(*statement as usize)
            ),
            ShapeError::Malformed { form, statement } => write!(
                f,
                "`{form:?}` in {} is not the shape it declares",
                Position::statement(*statement as usize)
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
