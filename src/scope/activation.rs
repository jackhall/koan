//! The **activation**: one per call or block entry, laid down in the frame's region.
//!
//! Its header is four pointers — the shape in program storage, the callable's closure bindings, the
//! builtin table, and the enclosing activation of a block — and its body is one
//! [`SlotArray`] over the shape's slots. Nothing points into the activation itself, it carries no
//! drop glue, and it is `Copy`: a copy is its bytes.
//!
//! A slot is `Empty` until its binder is submitted, `Claimed` by the binder's cell while it runs,
//! and `Bound` once. The scheduler submits a body's binders in position order and claims each at
//! submission, so a slot visible to a running reader is never `Empty`; [`Activation::read`] panics
//! if it is.

use crate::memory::{CellHandle, Edge, SlotArray, SlotConflict, SlotState, Writer};
use crate::parse::BinderSymbol;
use crate::values::Value;

use super::builtins::Builtins;
use super::closure::{Capture, ClosureBindings};
use super::shape::{Coordinate, Position, Shape, ShapeKind, Site, Slot, Target};

/// One body's bindings for one call or one block entry.
#[derive(Clone, Copy)]
pub struct Activation<'graph, 'cell> {
    shape: &'graph Shape<'graph>,
    closure: &'cell ClosureBindings<'graph, 'cell>,
    builtins: &'cell Builtins<'graph, 'cell>,
    enclosing: Option<&'cell Activation<'graph, 'cell>>,
    slots: SlotArray<'cell, Value<'graph, 'cell>, CellHandle>,
}

const _: () = assert!(size_of::<Activation<'static, 'static>>() == 56);
const _: () = assert!(!std::mem::needs_drop::<Activation<'static, 'static>>());

/// What a read finds.
#[derive(Clone, Copy, Debug)]
pub enum Binding<'graph, 'cell> {
    Bound(Value<'graph, 'cell>),
    /// The slot's binder is still running, in this cell.
    Pending(CellHandle),
    /// A closure binding that is an edge into the callable's own knot.
    Edge(Edge),
}

impl<'graph, 'cell> Activation<'graph, 'cell> {
    /// A fresh activation of `shape`, every slot `Empty`.
    ///
    /// A block is activated beside its enclosing activation and captures nothing; every other kind
    /// has no enclosing activation, and only a callable or module has closure bindings.
    pub fn new(
        writer: Writer<'cell>,
        shape: &'graph Shape<'graph>,
        closure: &'cell ClosureBindings<'graph, 'cell>,
        builtins: &'cell Builtins<'graph, 'cell>,
        enclosing: Option<&'cell Activation<'graph, 'cell>>,
    ) -> Self {
        debug_assert_eq!(
            enclosing.is_some(),
            shape.kind() == ShapeKind::Block,
            "a block, and only a block, is activated beside an enclosing activation",
        );
        debug_assert_eq!(
            closure.len(),
            shape.captures().len(),
            "the closure bindings follow the shape's capture layout",
        );
        Activation {
            shape,
            closure,
            builtins,
            enclosing,
            slots: SlotArray::new(writer, shape.slots()),
        }
    }

    pub fn shape(&self) -> &'graph Shape<'graph> {
        self.shape
    }

    pub fn builtins(&self) -> &'cell Builtins<'graph, 'cell> {
        self.builtins
    }

    pub fn closure(&self) -> &'cell ClosureBindings<'graph, 'cell> {
        self.closure
    }

    pub fn enclosing(&self) -> Option<&'cell Activation<'graph, 'cell>> {
        self.enclosing
    }

    /// Mark `slot` as bound by the running cell `binder`.
    pub fn claim(&self, slot: Slot, binder: CellHandle) -> Result<(), SlotConflict<CellHandle>> {
        self.slots.claim(slot.index(), binder)
    }

    /// Bind `slot`, once.
    pub fn bind(
        &self,
        slot: Slot,
        value: Value<'graph, 'cell>,
    ) -> Result<(), SlotConflict<CellHandle>> {
        self.slots.bind(slot.index(), value)
    }

    /// The read every resolved name makes: `hops` enclosing loads, then one slot, capture or builtin.
    pub fn read(&self, at: Coordinate) -> Binding<'graph, 'cell> {
        let mut activation = self;
        for _ in 0..at.hops {
            activation = activation
                .enclosing
                .expect("a coordinate steps out only through enclosing block activations");
        }
        match at.target {
            Target::Local(slot) => match activation.slots.get(slot.index()) {
                SlotState::Bound(value) => Binding::Bound(value),
                SlotState::Claimed(binder) => Binding::Pending(binder),
                SlotState::Empty => panic!(
                    "a slot visible to a running reader is never empty: its binder is claimed when it \
                     is submitted"
                ),
            },
            Target::Capture(slot) => match activation.closure.get(slot) {
                Capture::Value(value) => Binding::Bound(value),
                Capture::Edge(edge) => Binding::Edge(edge),
            },
            Target::Builtin(index) => Binding::Bound(activation.builtins.get(index)),
        }
    }

    /// The read the mention at `site` of this activation's shape makes.
    pub fn read_site(&self, site: Site) -> Option<Binding<'graph, 'cell>> {
        let mention = self.shape.mention(site)?;
        Some(self.read(mention.coordinate))
    }

    /// Where `name` read at `at` lands, found by name: a builtin, a local visible at `at`, a capture,
    /// then each enclosing block activation at the position its block was entered at.
    pub fn coordinate_of(&self, name: BinderSymbol, at: Position) -> Option<Coordinate> {
        if let Some(index) = self.builtins.lookup(name) {
            return Some(Coordinate {
                hops: 0,
                target: Target::Builtin(index),
            });
        }
        if let Some(target) = self.shape.resolve_here(name, at) {
            return Some(Coordinate { hops: 0, target });
        }
        let outer = self
            .enclosing?
            .coordinate_of(name, self.shape.entered_at())?;
        Some(Coordinate {
            hops: outer.hops + 1,
            ..outer
        })
    }

    /// `EVAL`'s by-name read: [`coordinate_of`](Self::coordinate_of), then [`read`](Self::read).
    pub fn resolve_by_name(
        &self,
        name: BinderSymbol,
        at: Position,
    ) -> Option<Binding<'graph, 'cell>> {
        Some(self.read(self.coordinate_of(name, at)?))
    }
}
