//! The **activation**: one per call or block entry, laid down in the frame's region.
//!
//! Its read half, the [`ActivationView`], is its shape's segment and three pointers — the
//! callable's closure bindings, the builtin table, and the enclosing block's view — beside the
//! callable the activation runs, a covariant view of one [`SlotArray`] over the shape's slots, and
//! the digest memos of the slots the load marked digested
//! ([`digest_memo`](Reading::digest_memo)): each empty until a `:|`, a call's hand-off or a
//! binding event fills it with the digest of the value its slot holds. Past the shape's slots the
//! array keeps a **held slot** per part the body holds in place ([`Facts::held_sites`]), each
//! with a digest memo past the digested slots' own: where the part's value is bound before its
//! unit runs, as a `LET`'s is at its statement. It names its shape by segment alone, so
//! it crosses a park; a step reads the shape and the facts about it through a [`Reading`], the view
//! beside the step's [`Code`] and [`Facts`].
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

use std::cell::Cell;
use std::ops::Deref;

use crate::memory::{Covariant, SlotArray, SlotConflict, SlotView, Writer};
use crate::values::{ContentDigest, Knotted, KnottedFamily, Link, NoKnot, Value, ValueFamily};

use super::builtins::Builtins;
use super::closure::ClosureBindings;
use super::facts::Binding;
use super::records::{CaptureRead, Facts};
use super::shape::{BodyShape, CaptureSlot, Coordinate, Segment, ShapeKind, Slot, Target};
use super::shapes::Code;

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
/// fn read<'graph, 'cell>(view: ActivationView<'graph, 'cell>, at: Coordinate) -> Value<'cell> {
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
    segment: Segment,
    /// How many of the slots past the shape's are held slots: the last `held` of `slots`, and the
    /// last `held` of `digests`.
    held: u32,
    closure: &'cell ClosureBindings<'cell, X>,
    builtins: &'cell Builtins<'cell, X>,
    enclosing: Option<&'cell ActivationView<'graph, 'cell, XF, X>>,
    /// The knot member this activation runs: `Some` for a callable's activation and every block
    /// inside one, `None` for the program's, a module's, and every block inside those. An edge
    /// capture resolves through it, and a module's captures are never edges — a module is alone in
    /// its component, so it runs no member of its own.
    callable: Option<X>,
    slots: SlotView<'graph, 'cell, ValueFamily<XF>>,
    /// One memo per slot the shape marks digested, in the order of its
    /// digested slots ([`Shapes::digested_index`](super::Shapes::digested_index)).
    digests: &'cell [Cell<Option<ContentDigest>>],
    /// The handle slab of its frame's body: the words of each binding-graph node's instance its
    /// frame reached, at the offsets the graph's layout fixes. A block and a module body share
    /// their frame's, since their units are their frame's statements.
    handles: &'cell [Cell<u64>],
    /// What its frame's slots bound the names of its bind sites to, as its last one read them: the
    /// binding a base record's types are closed under. `None` where its frame made no instances.
    /// A block and a module body read their frame's.
    binding: Option<Binding<'cell>>,
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
/// activation, a callable, a module and a quote's code each have closure bindings, and a block has
/// an enclosing activation whose builtin table it shares.
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

const _: () = assert!(size_of::<Activation<'static, 'static>>() == 112);
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
    /// The one constructor every kind goes through: the shape's slots and the body's held slots,
    /// all empty, and the view over them.
    fn laid_down(
        writer: Writer<'cell, '_>,
        shape: &BodyShape<'_>,
        facts: &Facts<'_>,
        closure: &'cell ClosureBindings<'cell, XF::Closed<'cell>>,
        builtins: &'cell Builtins<'cell, XF::Closed<'cell>>,
        enclosing: Option<&'cell ActivationView<'graph, 'cell, XF>>,
        callable: Option<XF::Closed<'cell>>,
    ) -> Self {
        let held = facts.held_sites(shape).len();
        let slots = SlotArray::new(writer, shape.slots() + held);
        let digests = writer.fill(facts.digested_count(shape) + held, |_| Cell::new(None));
        Activation {
            view: ActivationView {
                segment: shape.segment(),
                held: held as u32,
                closure,
                builtins,
                enclosing,
                callable,
                slots: slots.view(),
                digests,
                handles: enclosing.map_or(&[], |enclosing| enclosing.handles),
                binding: enclosing.and_then(|enclosing| enclosing.binding),
            },
            slots,
        }
    }

    /// A fresh activation of the program shape `shape`, every slot empty.
    pub fn of_program(
        writer: Writer<'cell, '_>,
        shape: &BodyShape<'_>,
        facts: &Facts<'_>,
        builtins: &'cell Builtins<'cell, XF::Closed<'cell>>,
    ) -> Self {
        debug_assert_eq!(shape.kind(), ShapeKind::Program);
        Self::laid_down(
            writer,
            shape,
            facts,
            ClosureBindings::empty(),
            builtins,
            None,
            None,
        )
    }

    /// A fresh activation of the callable shape `shape`, run by `callable` over its closure
    /// bindings, every slot empty.
    pub fn of_callable(
        writer: Writer<'cell, '_>,
        shape: &BodyShape<'_>,
        facts: &Facts<'_>,
        callable: XF::Closed<'cell>,
        closure: &'cell ClosureBindings<'cell, XF::Closed<'cell>>,
        builtins: &'cell Builtins<'cell, XF::Closed<'cell>>,
    ) -> Self {
        debug_assert_eq!(shape.kind(), ShapeKind::Callable);
        debug_assert_eq!(
            closure.len(),
            facts.capture_count(shape),
            "the closure bindings follow the shape's capture layout",
        );
        Self::laid_down(
            writer,
            shape,
            facts,
            closure,
            builtins,
            None,
            Some(callable),
        )
    }

    /// A fresh activation of the module shape `shape` over its closure bindings, every slot
    /// empty, sharing the handles and binding of the frame running it. It runs no knot
    /// member: a module's captures are never edges, and the caller ties the binder once this
    /// activation's every slot is bound.
    pub fn of_module(
        writer: Writer<'cell, '_>,
        shape: &BodyShape<'_>,
        facts: &Facts<'_>,
        closure: &'cell ClosureBindings<'cell, XF::Closed<'cell>>,
        builtins: &'cell Builtins<'cell, XF::Closed<'cell>>,
        (handles, binding): (&'cell [Cell<u64>], Option<Binding<'cell>>),
    ) -> Self {
        debug_assert_eq!(shape.kind(), ShapeKind::Module);
        debug_assert_eq!(
            closure.len(),
            facts.capture_count(shape),
            "the closure bindings follow the shape's capture layout",
        );
        let mut module = Self::laid_down(writer, shape, facts, closure, builtins, None, None);
        module.view.handles = handles;
        module.view.binding = binding;
        module
    }

    /// A fresh activation of the code shape `shape` over its closure bindings — its `$` names and
    /// filled marks, its keyworded holes and its offered names, in capture order, then the type
    /// names of the chain around it its layout carries — every slot empty. Like a module's, it runs
    /// no knot member and reads through no enclosing activation: code reaches nothing the `EVAL`
    /// running it does not hand it, and nothing its value does not carry.
    pub fn of_code(
        writer: Writer<'cell, '_>,
        shape: &BodyShape<'_>,
        facts: &Facts<'_>,
        closure: &'cell ClosureBindings<'cell, XF::Closed<'cell>>,
        builtins: &'cell Builtins<'cell, XF::Closed<'cell>>,
    ) -> Self {
        debug_assert_eq!(shape.kind(), ShapeKind::Code);
        debug_assert_eq!(
            closure.len(),
            facts.capture_count(shape),
            "the closure bindings follow the shape's capture layout",
        );
        Self::laid_down(writer, shape, facts, closure, builtins, None, None)
    }

    /// A fresh activation of the code shape `shape`, built alone binding the parameters of
    /// `callable`, whose untraced body it is, over a closure run in its capture order, every slot
    /// empty: a call of `callable` binds the parameter slots, as a frame of a callable shape does.
    pub fn of_composed(
        writer: Writer<'cell, '_>,
        shape: &BodyShape<'_>,
        facts: &Facts<'_>,
        callable: XF::Closed<'cell>,
        closure: &'cell ClosureBindings<'cell, XF::Closed<'cell>>,
        builtins: &'cell Builtins<'cell, XF::Closed<'cell>>,
    ) -> Self {
        debug_assert_eq!(shape.kind(), ShapeKind::Code);
        debug_assert_eq!(
            closure.len(),
            shape.captures().len(),
            "the closure bindings follow the shape's capture layout",
        );
        Self::laid_down(
            writer,
            shape,
            facts,
            closure,
            builtins,
            None,
            Some(callable),
        )
    }

    /// A fresh activation of the block shape `shape` beside the resident view `enclosing`, every
    /// slot empty. A view is all it needs, so an evaluation holding only a view can lay a block
    /// down.
    pub fn of_block(
        writer: Writer<'cell, '_>,
        shape: &BodyShape<'_>,
        facts: &Facts<'_>,
        enclosing: &'cell ActivationView<'graph, 'cell, XF>,
    ) -> Self {
        debug_assert_eq!(shape.kind(), ShapeKind::Block);
        Self::laid_down(
            writer,
            shape,
            facts,
            ClosureBindings::empty(),
            enclosing.builtins,
            Some(enclosing),
            enclosing.callable,
        )
    }

    /// This activation holding `handles`, its frame's handle slab.
    pub fn with_handles(self, handles: &'cell [Cell<u64>]) -> Self {
        Activation {
            view: ActivationView {
                handles,
                ..self.view
            },
            ..self
        }
    }

    /// This activation closing a base record's types under `binding`, what its frame's slots bound
    /// at its last bind site.
    pub fn binding(self, binding: Option<Binding<'cell>>) -> Self {
        match binding {
            None => self,
            Some(_) => Activation {
                view: ActivationView {
                    binding,
                    ..self.view
                },
                ..self
            },
        }
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
        value: Value<'cell, XF::Closed<'cell>>,
    ) -> Result<(), SlotConflict> {
        self.slots.bind(slot.index(), value)
    }

    /// Bind the held slot `index`, the rank of its part's site among the body's held sites, once.
    pub fn hold(
        &self,
        index: usize,
        value: Value<'cell, XF::Closed<'cell>>,
    ) -> Result<(), SlotConflict> {
        self.slots.bind(self.view.held_at(index), value)
    }
}

impl<'graph, 'cell, XF: KnottedFamily<'graph>> ActivationView<'graph, 'cell, XF> {
    /// The segment of the shape this activation binds the slots of.
    pub fn segment(&self) -> Segment {
        self.segment
    }

    /// This view beside the step's door, the shape it binds the slots of read through `code`.
    pub fn reading<'step>(
        self,
        code: &Code<'step>,
        facts: Facts<'step>,
    ) -> Reading<'step, 'graph, 'cell, XF>
    where
        'cell: 'step,
    {
        Reading {
            view: self,
            shape: code.shape(self.segment),
            facts: facts.framed(self.handle_words(), self.binding),
        }
    }

    /// Its frame's handle slab.
    pub fn handles(&self) -> &'cell [Cell<u64>] {
        self.handles
    }

    /// The handle words of each frame around the callable its frame runs, as they stood at the
    /// callable's birth, innermost first: its closure's, a block's frame's.
    pub fn enclosers(&self) -> &'cell [crate::scope::EncloserWords<'cell>] {
        match (self.closure.encloser().words, self.enclosing) {
            ([], Some(enclosing)) => enclosing.enclosers(),
            (words, _) => words,
        }
    }

    /// The handle words its frame answers a site question through.
    pub fn handle_words(&self) -> crate::scope::Handles<'cell> {
        crate::scope::Handles {
            slab: self.handles,
            enclosers: self.enclosers(),
        }
    }

    /// What its frame's slots bound at its last bind site.
    pub fn frame_binding(&self) -> Option<Binding<'cell>> {
        self.binding
    }

    /// The view of the activation around this one, a block's or a module body's frame's.
    pub fn enclosing(&self) -> Option<&'cell ActivationView<'graph, 'cell, XF>> {
        self.enclosing
    }

    pub fn builtins(&self) -> &'cell Builtins<'cell, XF::Closed<'cell>> {
        self.builtins
    }

    /// The callable this activation runs, if it runs one.
    pub fn callable(&self) -> Option<XF::Closed<'cell>> {
        self.callable
    }

    /// The closure bindings this activation reads its captures through, its type captures after
    /// its shape's.
    pub fn closure(&self) -> &'cell ClosureBindings<'cell, XF::Closed<'cell>> {
        self.closure
    }

    /// Every slot in order and what it is bound to. An empty slot panics, exactly as
    /// [`read`](ActivationView::read) does: a body is read whole only once its every unit has run.
    pub fn slots(
        &self,
    ) -> impl ExactSizeIterator<Item = (Slot, Value<'cell, XF::Closed<'cell>>)> + '_ {
        (0..self.slots.len() - self.held as usize).map(|index| {
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
    pub fn read(&self, at: Coordinate) -> Value<'cell, XF::Closed<'cell>> {
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

    /// The value a birth reads for a capture past its builder captures: [`read`](Self::read) at
    /// a coordinate, or a held slot `hops` enclosing block activations out.
    pub fn read_capture(&self, at: CaptureRead) -> Value<'cell, XF::Closed<'cell>> {
        match at {
            CaptureRead::At(at) => self.read(at),
            CaptureRead::Held { hops, index } => {
                let mut view = self;
                for _ in 0..hops {
                    view = view.enclosing.expect(
                        "a held capture steps out only through enclosing block activations",
                    );
                }
                view.held(index as usize)
                    .expect("a part held in place is bound before a birth reading it")
            }
        }
    }

    /// The value bound at `slot`, or `None` while it is empty.
    pub fn bound(&self, slot: Slot) -> Option<Value<'cell, XF::Closed<'cell>>> {
        self.slots.get(slot.index())
    }

    /// The value bound at the held slot `index`, or `None` while it is empty.
    pub fn held(&self, index: usize) -> Option<Value<'cell, XF::Closed<'cell>>> {
        self.slots.get(self.held_at(index))
    }

    /// Where the held slot `index` sits in the slot array: past the shape's slots.
    fn held_at(&self, index: usize) -> usize {
        assert!(index < self.held as usize, "a held slot the body keeps");
        self.slots.len() - self.held as usize + index
    }

    /// The segment of the activation `at` reads a slot of, stepping out as
    /// [`read`](ActivationView::read) does, beside that slot: `None` for a builtin and a capture.
    pub fn landing(&self, at: Coordinate) -> Option<(Segment, Slot)> {
        let Coordinate::Activation {
            hops,
            target: Target::Local(slot),
        } = at
        else {
            return None;
        };
        let mut view = self;
        for _ in 0..hops {
            view = view.enclosing?;
        }
        Some((view.segment, slot))
    }

    /// The segment of the activation a capture read at `at` lands in, stepping out as
    /// [`read`](ActivationView::read) does, beside its capture slot: `None` for a builtin and a
    /// slot.
    pub fn capture_landing(&self, at: Coordinate) -> Option<(Segment, CaptureSlot)> {
        let Coordinate::Activation {
            hops,
            target: Target::Capture(slot),
        } = at
        else {
            return None;
        };
        let mut view = self;
        for _ in 0..hops {
            view = view.enclosing?;
        }
        Some((view.segment, slot))
    }
}

/// An activation's view beside what the step reads it through: its shape, read through the step's
/// [`Code`], and the step's [`Facts`], both at the step's own lifetime `'step`. Built where a step
/// starts reading and never kept; it reads as its view through `Deref`. The member type `X` is a
/// parameter of its own for the reason [`ActivationView`]'s is.
pub struct Reading<
    'step,
    'graph,
    'cell,
    XF: KnottedFamily<'graph> = NoKnot,
    X = <XF as KnottedFamily<'graph>>::Closed<'cell>,
> where
    'graph: 'cell,
    'cell: 'step,
{
    view: ActivationView<'graph, 'cell, XF, X>,
    shape: &'step BodyShape<'step>,
    facts: Facts<'step>,
}

impl<'graph, XF: KnottedFamily<'graph>, X: Copy> Clone for Reading<'_, 'graph, '_, XF, X> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'graph, XF: KnottedFamily<'graph>, X: Copy> Copy for Reading<'_, 'graph, '_, XF, X> {}

impl<'graph, 'cell, XF: KnottedFamily<'graph>, X> Deref for Reading<'_, 'graph, 'cell, XF, X> {
    type Target = ActivationView<'graph, 'cell, XF, X>;

    fn deref(&self) -> &Self::Target {
        &self.view
    }
}

impl<'step, 'graph, 'cell, XF: KnottedFamily<'graph>> Reading<'step, 'graph, 'cell, XF> {
    /// The shape whose slots the activation binds.
    pub fn shape(&self) -> &'step BodyShape<'step> {
        self.shape
    }

    /// What the step reads a fact through.
    pub fn facts(&self) -> &Facts<'step> {
        &self.facts
    }

    /// The view alone, which crosses a park.
    pub fn view(&self) -> ActivationView<'graph, 'cell, XF> {
        self.view
    }

    /// `view`, another activation's, read through this step's door.
    pub fn beside(&self, view: ActivationView<'graph, 'cell, XF>) -> Self {
        Reading {
            view,
            shape: self.facts.shapes().shape_at(view.segment),
            facts: self.facts,
        }
    }

    /// The digest memo of the slot `at` names, stepping `hops` enclosing loads as
    /// [`read`](ActivationView::read) does: `None` where it names no slot the load marked digested.
    pub fn digest_memo(&self, at: Coordinate) -> Option<&'cell Cell<Option<ContentDigest>>> {
        let Coordinate::Activation {
            hops,
            target: Target::Local(slot),
        } = at
        else {
            return None;
        };
        let mut view = &self.view;
        for _ in 0..hops {
            view = view.enclosing?;
        }
        let index = self.facts.shapes().digested_index(view.segment, slot)?;
        Some(&view.digests[index])
    }

    /// The digest memo of the held slot `index` of this activation.
    pub fn held_memo(&self, index: usize) -> &'cell Cell<Option<ContentDigest>> {
        let digests = self.view.digests;
        assert!(
            index < self.view.held as usize,
            "a held slot the body keeps"
        );
        &digests[digests.len() - self.view.held as usize + index]
    }
}
