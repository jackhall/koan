//! The **activation**: one per call or block entry, laid down in the frame's region.
//!
//! Its read half, the [`ActivationView`], is four pointers — the shape in program storage, the
//! callable's closure bindings, the builtin table, and the enclosing block's view — beside the
//! callable the activation runs and a covariant view of one [`SlotArray`] over the shape's slots.
//! The [`Activation`] is that view beside the array itself, which is the door that binds. Nothing
//! points into either, neither carries drop glue, and both are `Copy`: a copy is their bytes.
//!
//! The view is covariant in `'cell`, so it may ride a state into a shorter-lived cell — an
//! evaluation reads names through it — while the activation, which binds, is invariant and stays
//! where it was laid down. Both are generic over the member's family, `XF`, because a slot holds its
//! value erased and names its payload through the family.
//!
//! A slot is empty until its unit's turn and bound once. A body's units run in its shape's order,
//! each after every unit it reads, so a slot visible to a running reader is never empty, and
//! [`ActivationView::read`] panics if it is: that is a scheduler bug, never a wait.
//!
//! A closure binding that is an edge into the callable's own knot never reaches a reader: the read
//! resolves it through the knot member the activation runs to the sibling it names, a function or a
//! data node.

use std::ops::Deref;

use crate::memory::{Covariant, SlotArray, SlotConflict, SlotView, Writer};
use crate::symbols::BinderSymbol;
use crate::values::{Knotted, KnottedFamily, Link, NoKnot, Value, ValueFamily};

use super::builtins::Builtins;
use super::closure::ClosureBindings;
use super::shape::{BodyShape, Coordinate, Position, ShapeKind, Slot, Target};

/// The read half of one body's bindings for one call or one block entry: what an evaluation is
/// handed. Covariant in `'cell`, and without a door that binds a slot.
///
/// ```compile_fail,E0599
/// use koan::scope::{ActivationView, Slot};
/// use koan::values::Value;
///
/// fn bind(view: ActivationView<'_, '_>, slot: Slot) {
///     let _ = view.bind(slot, Value::Null);
/// }
/// ```
///
/// Reading is what it is for:
///
/// ```
/// use koan::scope::{ActivationView, Coordinate};
/// use koan::values::Value;
///
/// fn read<'graph, 'cell>(view: ActivationView<'graph, 'cell>, at: Coordinate) -> Value<'graph, 'cell> {
///     view.read(at)
/// }
/// ```
///
/// The member type `X` is the family's member at `'cell`, and nothing but its default is ever
/// meant: it is a parameter of its own so no field names `'cell` through a projection, which would
/// make the view invariant in it. The impl is over the default alone.
pub struct ActivationView<
    'graph,
    'cell,
    XF: KnottedFamily<'graph> = NoKnot,
    X = <XF as KnottedFamily<'graph>>::Closed<'cell>,
> where
    'graph: 'cell,
{
    shape: &'graph BodyShape<'graph>,
    closure: &'cell ClosureBindings<'graph, 'cell, X>,
    builtins: &'cell Builtins<'graph, 'cell, X>,
    enclosing: Option<&'cell ActivationView<'graph, 'cell, XF, X>>,
    /// The knot member this activation runs: `Some` for a callable's activation and every block
    /// inside one, `None` for the program's, a module's, and every block inside those. An edge
    /// capture resolves through it, and a module's captures are never edges — a module is alone in
    /// its component, so it runs no member of its own.
    callable: Option<X>,
    slots: SlotView<'graph, 'cell, ValueFamily<XF>>,
}

impl<'graph, XF: KnottedFamily<'graph>, X: Copy> Clone for ActivationView<'graph, '_, XF, X> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'graph, XF: KnottedFamily<'graph>, X: Copy> Copy for ActivationView<'graph, '_, XF, X> {}

/// One body's bindings for one call or one block entry: the view, and the door that binds a slot.
/// Invariant in `'cell`, since it binds; it reads as its view through `Deref`.
///
/// Each kind has its own constructor: a program has neither closure bindings nor an enclosing
/// activation, a callable and a module each have closure bindings, and a block has an enclosing
/// activation whose builtin table it shares.
pub struct Activation<'graph, 'cell, XF: KnottedFamily<'graph> = NoKnot>
where
    'graph: 'cell,
{
    view: ActivationView<'graph, 'cell, XF>,
    slots: SlotArray<'graph, 'cell, ValueFamily<XF>>,
}

impl<'graph, XF: KnottedFamily<'graph>> Clone for Activation<'graph, '_, XF> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'graph, XF: KnottedFamily<'graph>> Copy for Activation<'graph, '_, XF> {}

const _: () = assert!(size_of::<Activation<'static, 'static>>() == 64);
const _: () = assert!(!std::mem::needs_drop::<Activation<'static, 'static>>());

impl<'graph, 'cell, XF: KnottedFamily<'graph>> Deref for Activation<'graph, 'cell, XF> {
    type Target = ActivationView<'graph, 'cell, XF>;

    fn deref(&self) -> &Self::Target {
        &self.view
    }
}

impl<'graph, 'cell, XF: KnottedFamily<'graph>> Activation<'graph, 'cell, XF>
where
    ValueFamily<XF>: Covariant<'graph>,
{
    /// The one constructor every kind goes through: `len` empty slots and the view over them.
    fn laid_down(
        writer: Writer<'cell>,
        shape: &'graph BodyShape<'graph>,
        closure: &'cell ClosureBindings<'graph, 'cell, XF::Closed<'cell>>,
        builtins: &'cell Builtins<'graph, 'cell, XF::Closed<'cell>>,
        enclosing: Option<&'cell ActivationView<'graph, 'cell, XF>>,
        callable: Option<XF::Closed<'cell>>,
    ) -> Self {
        let slots = SlotArray::new(writer, shape.slots());
        Activation {
            view: ActivationView {
                shape,
                closure,
                builtins,
                enclosing,
                callable,
                slots: slots.view(),
            },
            slots,
        }
    }

    /// A fresh activation of the program shape `shape`, every slot empty.
    pub fn of_program(
        writer: Writer<'cell>,
        shape: &'graph BodyShape<'graph>,
        builtins: &'cell Builtins<'graph, 'cell, XF::Closed<'cell>>,
    ) -> Self {
        debug_assert_eq!(shape.kind(), ShapeKind::Program);
        Self::laid_down(
            writer,
            shape,
            ClosureBindings::empty(),
            builtins,
            None,
            None,
        )
    }

    /// A fresh activation of the callable shape `shape`, run by `callable` over its closure
    /// bindings, every slot empty.
    pub fn of_callable(
        writer: Writer<'cell>,
        shape: &'graph BodyShape<'graph>,
        callable: XF::Closed<'cell>,
        closure: &'cell ClosureBindings<'graph, 'cell, XF::Closed<'cell>>,
        builtins: &'cell Builtins<'graph, 'cell, XF::Closed<'cell>>,
    ) -> Self {
        debug_assert_eq!(shape.kind(), ShapeKind::Callable);
        debug_assert_eq!(
            closure.len(),
            shape.captures().len(),
            "the closure bindings follow the shape's capture layout",
        );
        Self::laid_down(writer, shape, closure, builtins, None, Some(callable))
    }

    /// A fresh activation of the module shape `shape` over its closure bindings, every slot
    /// empty. It runs no knot member: a module's captures are never edges, and the caller ties
    /// the binder once this activation's every slot is bound.
    pub fn of_module(
        writer: Writer<'cell>,
        shape: &'graph BodyShape<'graph>,
        closure: &'cell ClosureBindings<'graph, 'cell, XF::Closed<'cell>>,
        builtins: &'cell Builtins<'graph, 'cell, XF::Closed<'cell>>,
    ) -> Self {
        debug_assert_eq!(shape.kind(), ShapeKind::Module);
        debug_assert_eq!(
            closure.len(),
            shape.captures().len(),
            "the closure bindings follow the shape's capture layout",
        );
        Self::laid_down(writer, shape, closure, builtins, None, None)
    }

    /// A fresh activation of the block shape `shape` beside `enclosing`, every slot empty.
    pub fn of_block(
        writer: Writer<'cell>,
        shape: &'graph BodyShape<'graph>,
        enclosing: &'cell Activation<'graph, 'cell, XF>,
    ) -> Self {
        debug_assert_eq!(shape.kind(), ShapeKind::Block);
        Self::laid_down(
            writer,
            shape,
            ClosureBindings::empty(),
            enclosing.builtins,
            Some(&enclosing.view),
            enclosing.callable,
        )
    }
}

impl<'graph, 'cell, XF: KnottedFamily<'graph>> Activation<'graph, 'cell, XF> {
    /// The read half, by value: what an evaluation is handed.
    pub fn view(&self) -> ActivationView<'graph, 'cell, XF> {
        self.view
    }

    /// Bind `slot`, once.
    pub fn bind(
        &self,
        slot: Slot,
        value: Value<'graph, 'cell, XF::Closed<'cell>>,
    ) -> Result<(), SlotConflict> {
        self.slots.bind(slot.index(), value)
    }
}

impl<'graph, 'cell, XF: KnottedFamily<'graph>> ActivationView<'graph, 'cell, XF> {
    pub fn shape(&self) -> &'graph BodyShape<'graph> {
        self.shape
    }

    pub fn builtins(&self) -> &'cell Builtins<'graph, 'cell, XF::Closed<'cell>> {
        self.builtins
    }

    /// The callable this activation runs, if it runs one.
    pub fn callable(&self) -> Option<XF::Closed<'cell>> {
        self.callable
    }

    /// Every slot in order and what it is bound to. An empty slot panics, exactly as
    /// [`read`](ActivationView::read) does: a body is read whole only once its every unit has run.
    pub fn slots(
        &self,
    ) -> impl ExactSizeIterator<Item = (Slot, Value<'graph, 'cell, XF::Closed<'cell>>)> + '_ {
        (0..self.shape.slots()).map(|index| {
            let value = self.slots.get(index).expect(
                "a slot read out of an activation is never empty: every unit of the body has run",
            );
            (Slot(index as u32), value)
        })
    }

    /// The read every resolved name makes: a builtin through the header, or `hops` enclosing loads
    /// then one slot or capture. A capture that is an edge reads as the sibling member it names.
    ///
    /// Panics on an empty slot: the shape orders a body's units so every binder runs before its
    /// readers, so an empty slot here is a scheduler bug.
    pub fn read(&self, at: Coordinate) -> Value<'graph, 'cell, XF::Closed<'cell>> {
        let (hops, target) = match at {
            Coordinate::Builtin(index) => return self.builtins.get(index),
            Coordinate::Activation { hops, target } => (hops, target),
        };
        let mut view = self;
        for _ in 0..hops {
            view = view
                .enclosing
                .expect("a coordinate steps out only through enclosing block activations");
        }
        match target {
            Target::Local(slot) => view.slots.get(slot.index()).expect(
                "a slot visible to a running reader is never empty: the shape orders its units so \
                 every binder runs first",
            ),
            Target::Capture(slot) => match view.closure.get(slot) {
                Link::Value(value) => value,
                Link::Edge(edge) => Value::Knotted(
                    view.callable
                        .expect("an edge capture is a callable's, read in its own activation")
                        .sibling(edge),
                ),
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
