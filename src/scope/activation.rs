//! The **activation**: one per call or block entry, laid down in the frame's region.
//!
//! Its header is four pointers — the shape in program storage, the callable's closure bindings, the
//! builtin table, and the enclosing activation of a block — beside the callable the activation runs,
//! and its body is one [`SlotArray`] over the shape's slots. Nothing points into the activation
//! itself, it carries no drop glue, and it is `Copy`: a copy is its bytes.
//!
//! A slot is `Empty` until its binder is submitted, `Claimed` by the binder's cell while it runs,
//! and `Bound` once. A deferred mention reads at the body's end and so sees later siblings, so the
//! scheduler claims every binder of a body, in position order, before any of its statements runs:
//! a slot visible to a running reader is never `Empty`, and [`Activation::read`] panics if it is.
//!
//! A closure binding that is an edge into the callable's own knot never reaches a reader: the read
//! resolves it through the callable the activation runs to the sibling callable it names.

use crate::memory::{CellHandle, SlotArray, SlotConflict, SlotState, Writer};
use crate::parse::BinderSymbol;
use crate::values::{Callable, Nothing, Value};

use super::builtins::Builtins;
use super::closure::{Capture, ClosureBindings};
use super::shape::{Coordinate, Position, Shape, ShapeKind, Slot, Target};

/// One body's bindings for one call or one block entry.
///
/// Each kind has its own constructor: a program has neither closure bindings nor an enclosing
/// activation, a callable or module has closure bindings, and a block has an enclosing activation
/// whose builtin table it shares.
#[derive(Clone, Copy)]
pub struct Activation<'graph, 'cell, X = Nothing> {
    shape: &'graph Shape<'graph>,
    closure: &'cell ClosureBindings<'graph, 'cell, X>,
    builtins: &'cell Builtins<'graph, 'cell, X>,
    enclosing: Option<&'cell Activation<'graph, 'cell, X>>,
    /// The value this activation runs: `Some` for a callable's or module's activation and every
    /// block inside one, `None` for the program's and every block inside it.
    callable: Option<X>,
    slots: SlotArray<'cell, Value<'graph, 'cell, X>, CellHandle>,
}

const _: () = assert!(size_of::<Activation<'static, 'static>>() == 56);
const _: () = assert!(!std::mem::needs_drop::<Activation<'static, 'static>>());

/// What a read finds.
#[derive(Clone, Copy, Debug)]
pub enum Binding<'graph, 'cell, X = Nothing> {
    Bound(Value<'graph, 'cell, X>),
    /// The slot's binder is still running, in this cell.
    Pending(CellHandle),
}

impl<'graph, 'cell, X: Callable> Activation<'graph, 'cell, X> {
    /// A fresh activation of the program shape `shape`, every slot `Empty`.
    pub fn of_program(
        writer: Writer<'cell>,
        shape: &'graph Shape<'graph>,
        builtins: &'cell Builtins<'graph, 'cell, X>,
    ) -> Self {
        debug_assert_eq!(shape.kind(), ShapeKind::Program);
        Activation {
            shape,
            closure: ClosureBindings::empty(),
            builtins,
            enclosing: None,
            callable: None,
            slots: SlotArray::new(writer, shape.slots()),
        }
    }

    /// A fresh activation of the callable or module shape `shape`, run by `callable` over its
    /// closure bindings, every slot `Empty`.
    pub fn of_callable(
        writer: Writer<'cell>,
        shape: &'graph Shape<'graph>,
        callable: X,
        closure: &'cell ClosureBindings<'graph, 'cell, X>,
        builtins: &'cell Builtins<'graph, 'cell, X>,
    ) -> Self {
        debug_assert!(matches!(
            shape.kind(),
            ShapeKind::Callable | ShapeKind::Module
        ));
        debug_assert_eq!(
            closure.len(),
            shape.captures().len(),
            "the closure bindings follow the shape's capture layout",
        );
        Activation {
            shape,
            closure,
            builtins,
            enclosing: None,
            callable: Some(callable),
            slots: SlotArray::new(writer, shape.slots()),
        }
    }

    /// A fresh activation of the block shape `shape` beside `enclosing`, every slot `Empty`.
    pub fn of_block(
        writer: Writer<'cell>,
        shape: &'graph Shape<'graph>,
        enclosing: &'cell Activation<'graph, 'cell, X>,
    ) -> Self {
        debug_assert_eq!(shape.kind(), ShapeKind::Block);
        Activation {
            shape,
            closure: ClosureBindings::empty(),
            builtins: enclosing.builtins,
            enclosing: Some(enclosing),
            callable: enclosing.callable,
            slots: SlotArray::new(writer, shape.slots()),
        }
    }

    pub fn shape(&self) -> &'graph Shape<'graph> {
        self.shape
    }

    pub fn builtins(&self) -> &'cell Builtins<'graph, 'cell, X> {
        self.builtins
    }

    /// The callable this activation runs, if it runs one.
    pub fn callable(&self) -> Option<X> {
        self.callable
    }

    /// Mark `slot` as bound by the running cell `binder`.
    pub fn claim(&self, slot: Slot, binder: CellHandle) -> Result<(), SlotConflict<CellHandle>> {
        self.slots.claim(slot.index(), binder)
    }

    /// Bind `slot`, once.
    pub fn bind(
        &self,
        slot: Slot,
        value: Value<'graph, 'cell, X>,
    ) -> Result<(), SlotConflict<CellHandle>> {
        self.slots.bind(slot.index(), value)
    }

    /// The read every resolved name makes: a builtin through the header, or `hops` enclosing loads
    /// then one slot or capture. A capture that is an edge reads as the sibling callable it names.
    pub fn read(&self, at: Coordinate) -> Binding<'graph, 'cell, X> {
        let (hops, target) = match at {
            Coordinate::Builtin(index) => return Binding::Bound(self.builtins.get(index)),
            Coordinate::Activation { hops, target } => (hops, target),
        };
        let mut activation = self;
        for _ in 0..hops {
            activation = activation
                .enclosing
                .expect("a coordinate steps out only through enclosing block activations");
        }
        match target {
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
                Capture::Edge(edge) => Binding::Bound(Value::Callable(
                    activation
                        .callable
                        .expect("an edge capture is a callable's, read in its own activation")
                        .sibling(edge),
                )),
            },
        }
    }

    /// Where `name` read at `at` lands, found by name: a builtin, a local visible at `at`, a capture,
    /// then each enclosing block activation at the position its block was entered at.
    pub fn coordinate_of(&self, name: BinderSymbol, at: Position) -> Option<Coordinate> {
        if let Some(index) = self.builtins.lookup(name) {
            return Some(Coordinate::Builtin(index));
        }
        self.through_chain(name, at)
    }

    /// [`coordinate_of`](Self::coordinate_of) for a name already known not to be a builtin.
    pub(super) fn through_chain(&self, name: BinderSymbol, at: Position) -> Option<Coordinate> {
        if let Some(target) = self.shape.resolve_here(name, at) {
            return Some(Coordinate::Activation { hops: 0, target });
        }
        let outer = self
            .enclosing?
            .through_chain(name, self.shape.entered_at())?;
        Some(outer.through_block())
    }
}
