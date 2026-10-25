//! **Closure bindings**: one run per callable, born from the enclosing activation through the
//! callable's capture layout and held in the callable value.
//!
//! A slot is a [`Link`]: the captured binding's value word, or an [`Edge`] into the knot the callable
//! is born in when what it captures is a fellow member of its own component — a function or a data
//! node. Birth is two steps: every
//! source is read into scratch first, and only a finished read is laid down — so a closure binding
//! is never a placeholder.
//!
//! A callable's closure also holds what the activation it was born in had **decided**
//! ([build/README.md § Resumption](../build/README.md#resumption)): its encloser's walk decided the
//! callable's units under the names bound before its birth, so a call of it reads those layers
//! first. They lie in the region the closure does, and a copy of the closure into another region
//! lays them again there, its types made to resolve by the crossing beforehand.

use crate::memory::{BumpAllocator, BumpVec, Edge, Writer, collect, resident};
use crate::values::{ArenaRef, Knotted, KnottedFamily, Link, Value, Weight};

use super::activation::Reading;
use super::settlement_facts::Activated;
use super::shape::{BodyShape, CaptureSlot, CaptureSource};

/// A callable's closure bindings, in capture-slot order, beside the arena node the callable was
/// born under, where a call of it looks up its settlement, and what the activation it was born in
/// had decided. Neither is content, so the closure's digest and equality read the slots alone.
#[derive(Clone, Copy)]
pub struct ClosureBindings<'cell, X = crate::values::Nothing> {
    slots: &'cell [Link<'cell, X>],
    build: ArenaRef,
    decided: Option<&'cell Activated<'cell>>,
}

impl<'graph, 'cell, X: Knotted> ClosureBindings<'cell, X> {
    /// The bindings of a shape that captures nothing — a program's, a block's.
    pub fn empty() -> &'cell ClosureBindings<'cell, X> {
        &ClosureBindings {
            slots: &[],
            build: ArenaRef::ROOT,
            decided: None,
        }
    }

    /// Every closure binding of a callable of `shape`, read from `enclosing` into `scratch`.
    ///
    /// A `Read` source is what `enclosing` reads there — a capture of the enclosing callable's own
    /// knot arrives as the sibling member it names. A `Member` source is `edge(index)`, the edge the
    /// caller minted for member `index` of the component the callable is born in. Each type capture
    /// follows, read from `enclosing` at its coordinate. Nothing is written to a region.
    pub fn read_captures<'x, XF: KnottedFamily<'graph, Closed<'cell> = X>>(
        shape: &BodyShape<'_>,
        enclosing: &Reading<'_, 'graph, 'cell, XF>,
        scratch: BumpAllocator<'x>,
        mut edge: impl FnMut(u32) -> Edge,
    ) -> BumpVec<'x, Link<'cell, X>> {
        let captures = shape.captures();
        let load = enclosing.facts();
        let mut read = BumpVec::with_capacity_in(load.capture_count(shape), scratch);
        read.extend(captures.iter().map(|capture| match capture.source {
            CaptureSource::Read(coordinate) => Link::Value(enclosing.read(coordinate)),
            CaptureSource::Member { index, .. } => Link::Edge(edge(index)),
            CaptureSource::Hole | CaptureSource::Offered | CaptureSource::Entry { .. } => {
                unreachable!(
                    "only a code shape's captures are open, and the `EVAL` running it fills them"
                )
            }
        }));
        read.extend((load.type_captures(shape).iter()).map(|at| Link::Value(enclosing.read(*at))));
        read
    }

    /// A finished read laid down in `writer`'s region, born under the arena node `build` in an
    /// activation that had `decided` what it had.
    pub fn of(
        writer: Writer<'cell, '_>,
        captures: &[Link<'cell, X>],
        build: ArenaRef,
        decided: Option<&'cell Activated<'cell>>,
    ) -> &'cell ClosureBindings<'cell, X> {
        let slots = collect(writer, captures.iter().copied());
        resident(
            writer,
            ClosureBindings {
                slots,
                build,
                decided,
            },
        )
    }

    /// The arena node the callable was born under.
    pub fn build(&self) -> ArenaRef {
        self.build
    }

    /// What the activation the callable was born in had decided.
    pub fn decided(&self) -> Option<&'cell Activated<'cell>> {
        self.decided
    }

    /// Every value these bindings hold, in the order [`copied`](Self::copied) asks for them.
    pub fn held(&self, out: &mut dyn FnMut(Value<'cell, X>)) {
        for link in self.slots {
            if let Link::Value(value) = link {
                out(*value);
            }
        }
    }

    /// These bindings rebuilt in `writer`'s region: each value through `copy`, each edge verbatim —
    /// an edge names a node by index, so it means the same in a copy of its knot — and what was
    /// decided laid again.
    pub fn copied<'to, Y: Knotted>(
        &self,
        writer: Writer<'to, '_>,
        mut copy: impl FnMut(&Value<'cell, X>) -> Value<'to, Y>,
    ) -> &'to ClosureBindings<'to, Y> {
        let source = self.slots;
        let slots = writer.fill(source.len(), |at| source[at].copied(&mut copy));
        resident(
            writer,
            ClosureBindings {
                slots,
                build: self.build,
                decided: self.decided.map(|decided| decided.copied(writer)),
            },
        )
    }

    /// What [`copied`](Self::copied) writes: the resident run header, every binding's word, what
    /// each captured value points at, and the decided layers' records.
    pub fn weight(&self) -> Weight {
        let decided = self
            .decided
            .map_or(Weight::flat::<()>(), |decided| decided.weight());
        self.slots
            .iter()
            .fold(Weight::flat::<Self>().plus(decided), |weight, link| {
                weight.plus(link.weight())
            })
    }

    /// The binding at `slot`. Panics past the end, like a slice index.
    pub fn get(&self, slot: CaptureSlot) -> Link<'cell, X> {
        self.slots[slot.index()]
    }

    /// Every binding, in capture-slot order.
    pub fn links(&self) -> &'cell [Link<'cell, X>] {
        self.slots
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}
