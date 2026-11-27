//! **Closure bindings**: one run per callable, born from the enclosing activation through the
//! callable's capture layout and held in the callable value.
//!
//! A slot is a [`Link`]: the captured binding's value word, or an [`Edge`] into the knot the callable
//! is born in when what it captures is a fellow member of its own component — a function or a data
//! node. Birth is two steps: every
//! source is read into scratch first, and only a finished read is laid down — so a closure binding
//! is never a placeholder.
//!
//! A callable's closure also holds its [`Encloser`]: the handle words of each frame around it as
//! they stood at its birth, through which a call answers a question an encloser's binding-graph
//! node owns. They lie in the region the closure does, and a copy of the closure into another
//! region lays them again there.

use crate::memory::{BumpAllocator, BumpVec, Edge, Writer, collect, resident};
use crate::values::{DepotRef, Knotted, KnottedFamily, Link, Value, Weight};

use super::activation::Reading;
use super::shape::{BodyShape, CaptureSlot, CaptureSource};
use super::site::{EncloserWords, Handles};

/// What a callable's closure keeps of the frames it was born in, none of it content: the handle
/// words of each frame around it, innermost first, as they stood at its birth.
#[derive(Clone, Copy, Default)]
pub struct Encloser<'cell> {
    pub words: &'cell [EncloserWords<'cell>],
}

impl<'cell> Encloser<'cell> {
    /// What a callable born in a frame holding `handles`, its units owned by the body `body`, keeps:
    /// the frame's slab as it stands, laid in `writer`'s region, before the words the frame's own
    /// closure holds. A frame with no slab adds none.
    pub fn born_in(
        writer: Writer<'cell, '_>,
        body: super::Segment,
        handles: Handles<'cell>,
    ) -> Self {
        let words = match handles.slab {
            [] => handles.enclosers,
            slab => {
                let own = EncloserWords {
                    body,
                    words: writer.fill(slab.len(), |at| slab[at].get()),
                };
                let outer = handles.enclosers;
                writer.fill(outer.len() + 1, |at| match at {
                    0 => own,
                    at => outer[at - 1],
                })
            }
        };
        Encloser { words }
    }

    /// What a callable born in `reading`'s frame keeps of it.
    pub fn of_reading<'graph, XF: KnottedFamily<'graph>>(
        writer: Writer<'cell, '_>,
        reading: &Reading<'_, 'graph, 'cell, XF>,
    ) -> Self
    where
        'graph: 'cell,
    {
        let view = reading.view();
        let body = reading.facts().shapes().owner(view.segment());
        Encloser::born_in(writer, body, view.handle_words())
    }

    /// These laid again in `writer`'s region: each frame's words.
    fn copied<'to>(&self, writer: Writer<'to, '_>) -> Encloser<'to> {
        let words = writer.fill(self.words.len(), |at| {
            let held = self.words[at];
            EncloserWords {
                body: held.body,
                words: collect(writer, held.words.iter().copied()),
            }
        });
        Encloser { words }
    }

    /// What [`copied`](Self::copied) writes.
    fn weight(&self) -> Weight {
        (self.words.iter()).fold(Weight::flat::<()>(), |weight, held| {
            (weight.plus(Weight::flat::<EncloserWords<'_>>()))
                .plus(Weight::run::<u64>(held.words.len()))
        })
    }
}

/// A callable's closure bindings, in capture-slot order, beside the depot slot the callable was
/// born under, where a call of it runs, and its [`Encloser`]. Neither is content, so the closure's
/// digest and equality read the slots alone.
#[derive(Clone, Copy)]
pub struct ClosureBindings<'cell, X = crate::values::Nothing> {
    slots: &'cell [Link<'cell, X>],
    build: DepotRef,
    encloser: Encloser<'cell>,
}

impl<'graph, 'cell, X: Knotted> ClosureBindings<'cell, X> {
    /// The bindings of a shape that captures nothing — a program's, a block's.
    pub fn empty() -> &'cell ClosureBindings<'cell, X> {
        &ClosureBindings {
            slots: &[],
            build: DepotRef::ROOT,
            encloser: Encloser { words: &[] },
        }
    }

    /// Every closure binding of a callable of `shape`, read from `enclosing` into `scratch`.
    ///
    /// A `Read` source is what `enclosing` reads there — a capture of the enclosing callable's own
    /// knot arrives as the sibling member it names. A `Member` source is `edge(index)`, the edge the
    /// caller minted for member `index` of the component the callable is born in. Each type capture
    /// and binder capture follows, read from `enclosing` at its coordinate. Nothing is written to a
    /// region.
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
        read.extend(
            (load.type_captures(shape).iter()).map(|at| Link::Value(enclosing.read_capture(*at))),
        );
        read
    }

    /// A finished read laid down in `writer`'s region, born under the depot slot `build`, keeping
    /// `encloser` of the frames it was born in.
    pub fn of(
        writer: Writer<'cell, '_>,
        captures: &[Link<'cell, X>],
        build: DepotRef,
        encloser: Encloser<'cell>,
    ) -> &'cell ClosureBindings<'cell, X> {
        let slots = collect(writer, captures.iter().copied());
        resident(
            writer,
            ClosureBindings {
                slots,
                build,
                encloser,
            },
        )
    }

    /// The depot slot the callable was born under.
    pub fn build(&self) -> DepotRef {
        self.build
    }

    /// What it keeps of the frames it was born in.
    pub fn encloser(&self) -> Encloser<'cell> {
        self.encloser
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
    /// what it keeps of its encloser laid again.
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
                encloser: self.encloser.copied(writer),
            },
        )
    }

    /// What [`copied`](Self::copied) writes: the resident run header, every binding's word, what
    /// each captured value points at, and what it keeps of its encloser.
    pub fn weight(&self) -> Weight {
        self.slots.iter().fold(
            Weight::flat::<Self>().plus(self.encloser.weight()),
            |weight, link| weight.plus(link.weight()),
        )
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
