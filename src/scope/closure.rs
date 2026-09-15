//! **Closure bindings**: one run per callable, born from the enclosing activation through the
//! callable's capture layout and held in the callable value.
//!
//! A slot holds the captured binding's value word, or an [`Edge`] into the knot the callable is born
//! in when what it captures is a fellow member of its own component. Birth reads every source first
//! and refuses while one is still pending, so a closure binding is never a placeholder.

use crate::memory::{CellHandle, Edge, Writer, collect, resident};
use crate::parse::BinderSymbol;
use crate::values::Value;

use super::activation::{Activation, Binding};
use super::shape::{CaptureSlot, CaptureSource, ComponentIndex, Shape};

/// One closure binding.
#[derive(Clone, Copy, Debug)]
pub enum Capture<'graph, 'cell> {
    Value(Value<'graph, 'cell>),
    Edge(Edge),
}

/// A callable's closure bindings, in capture-slot order.
#[derive(Clone, Copy)]
pub struct ClosureBindings<'graph, 'cell> {
    slots: &'cell [Capture<'graph, 'cell>],
}

/// Why a callable cannot be born yet: the capture `name` reads a slot whose binder is `pending`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClosureRefused {
    pub name: BinderSymbol,
    pub pending: CellHandle,
}

impl<'graph, 'cell> ClosureBindings<'graph, 'cell> {
    /// The bindings of a shape that captures nothing — a program's, a block's.
    pub const EMPTY: &'static ClosureBindings<'static, 'static> = &ClosureBindings { slots: &[] };

    /// Bindings for a callable of `shape`, born from `enclosing` and laid down in `writer`'s region.
    ///
    /// A `Read` source is read from `enclosing`; one still pending refuses the birth with its
    /// binder's handle, and one that is itself an edge — a capture of the enclosing callable's own
    /// knot — is resolved to a value by `follow`. A `Member` source takes `edge(component, index)`,
    /// the edge the caller minted for member `index` of the enclosing shape's component `component`
    /// — the knot the callable is born in.
    pub fn born(
        writer: Writer<'cell>,
        shape: &Shape<'_>,
        enclosing: &Activation<'graph, 'cell>,
        mut edge: impl FnMut(ComponentIndex, u32) -> Edge,
        mut follow: impl FnMut(Edge) -> Value<'graph, 'cell>,
    ) -> Result<&'cell ClosureBindings<'graph, 'cell>, ClosureRefused> {
        let captures = shape.captures();
        for capture in captures {
            if let CaptureSource::Read(coordinate) = capture.source
                && let Binding::Pending(pending) = enclosing.read(coordinate)
            {
                return Err(ClosureRefused {
                    name: capture.name,
                    pending,
                });
            }
        }
        let slots = collect(
            writer,
            captures.iter().map(|capture| match capture.source {
                CaptureSource::Read(coordinate) => match enclosing.read(coordinate) {
                    Binding::Bound(value) => Capture::Value(value),
                    Binding::Edge(through) => Capture::Value(follow(through)),
                    Binding::Pending(_) => unreachable!("a pending source refused the birth above"),
                },
                CaptureSource::Member { component, index } => Capture::Edge(edge(component, index)),
            }),
        );
        Ok(resident(writer, ClosureBindings { slots }))
    }

    /// The binding at `slot`. Panics past the end, like a slice index.
    pub fn get(&self, slot: CaptureSlot) -> Capture<'graph, 'cell> {
        self.slots[slot.index()]
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}
