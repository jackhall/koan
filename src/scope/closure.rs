//! **Closure bindings**: one run per callable, born from the enclosing activation through the
//! callable's capture layout and held in the callable value.
//!
//! A slot holds the captured binding's value word, or an [`Edge`] into the knot the callable is born
//! in when what it captures is a fellow member of its own component. Birth is two steps: every
//! source is read into scratch first, refusing while one is still pending, and only a finished read
//! is laid down — so a closure binding is never a placeholder, and a refused birth writes nothing.

use crate::memory::{BumpAllocator, BumpVec, CellHandle, Edge, Writer, collect, resident};
use crate::parse::BinderSymbol;
use crate::values::{Callable, Value, Weight};

use super::activation::{Activation, Binding};
use super::shape::{CaptureSlot, CaptureSource, Shape};

/// One closure binding.
#[derive(Clone, Copy, Debug)]
pub enum Capture<'graph, 'cell, X = crate::values::Nothing> {
    Value(Value<'graph, 'cell, X>),
    Edge(Edge),
}

/// A callable's closure bindings, in capture-slot order.
#[derive(Clone, Copy)]
pub struct ClosureBindings<'graph, 'cell, X = crate::values::Nothing> {
    slots: &'cell [Capture<'graph, 'cell, X>],
}

/// Why a callable cannot be born yet: the capture `name` reads a slot whose binder is `pending`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClosureRefused {
    pub name: BinderSymbol,
    pub pending: CellHandle,
}

impl<'graph, 'cell, X: Callable> ClosureBindings<'graph, 'cell, X> {
    /// The bindings of a shape that captures nothing — a program's, a block's.
    pub fn empty() -> &'cell ClosureBindings<'graph, 'cell, X> {
        &ClosureBindings { slots: &[] }
    }

    /// Every closure binding of a callable of `shape`, read from `enclosing` into `scratch`.
    ///
    /// A `Read` source is what `enclosing` reads there — a capture of the enclosing callable's own
    /// knot arrives as the sibling callable it names — and one still pending refuses the birth with
    /// its binder's handle. A `Member` source is `edge(index)`, the edge the caller minted for
    /// member `index` of the component the callable is born in. Nothing is written to a region.
    pub fn read_captures<'x>(
        shape: &Shape<'_>,
        enclosing: &Activation<'graph, 'cell, X>,
        scratch: BumpAllocator<'x>,
        mut edge: impl FnMut(u32) -> Edge,
    ) -> Result<BumpVec<'x, Capture<'graph, 'cell, X>>, ClosureRefused> {
        let captures = shape.captures();
        let mut read = BumpVec::with_capacity_in(captures.len(), scratch);
        for capture in captures {
            read.push(match capture.source {
                CaptureSource::Read(coordinate) => match enclosing.read(coordinate) {
                    Binding::Bound(value) => Capture::Value(value),
                    Binding::Pending(pending) => {
                        return Err(ClosureRefused {
                            name: capture.name,
                            pending,
                        });
                    }
                },
                CaptureSource::Member { index, .. } => Capture::Edge(edge(index)),
            });
        }
        Ok(read)
    }

    /// A finished read laid down in `writer`'s region.
    pub fn of(
        writer: Writer<'cell>,
        captures: &[Capture<'graph, 'cell, X>],
    ) -> &'cell ClosureBindings<'graph, 'cell, X> {
        let slots = collect(writer, captures.iter().copied());
        resident(writer, ClosureBindings { slots })
    }

    /// These bindings rebuilt in `writer`'s region: each value through `copy`, each edge verbatim —
    /// an edge names a node by index, so it means the same in a copy of its knot.
    pub fn copied<'to, Y: Callable>(
        &self,
        writer: Writer<'to>,
        mut copy: impl FnMut(&Value<'graph, 'cell, X>) -> Value<'graph, 'to, Y>,
    ) -> &'to ClosureBindings<'graph, 'to, Y> {
        let source = self.slots;
        let slots = writer.fill(source.len(), |at| match &source[at] {
            Capture::Value(value) => Capture::Value(copy(value)),
            Capture::Edge(edge) => Capture::Edge(*edge),
        });
        resident(writer, ClosureBindings { slots })
    }

    /// What [`copied`](Self::copied) writes: the resident run header, every binding's word, and
    /// what each captured value points at.
    pub fn weight(&self) -> Weight {
        self.slots
            .iter()
            .fold(Weight::flat::<Self>(), |weight, capture| {
                let referent = match capture {
                    Capture::Value(value) => value.referent_weight(),
                    Capture::Edge(_) => Weight::ZERO,
                };
                weight
                    .plus(Weight::flat::<Capture<'graph, 'cell, X>>())
                    .plus(referent)
            })
    }

    /// The binding at `slot`. Panics past the end, like a slice index.
    pub fn get(&self, slot: CaptureSlot) -> Capture<'graph, 'cell, X> {
        self.slots[slot.index()]
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}
