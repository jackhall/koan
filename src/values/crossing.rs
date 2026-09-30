//! Moving a value between regions: one verb over the substrate's two placement doors, and the
//! copy-or-pin verdict a graph is built with.
//!
//! A pinned operand arrives at the destination's region lifetime and is embedded as it is. A copied
//! one arrives at an unrelated lifetime, so the only rebuild that typechecks is a deep one through
//! the destination's writer, and a knot member rebuilds its whole knot through its family; what a
//! member borrows of program storage embeds verbatim.
//!
//! The deep copy ([`Copying`]) reads the value through [the door](super::surface): it lays down only
//! what each part's seen type names, at that type, and weighs what it lays down, so a copy drops
//! what a retype hid. A knot's data node seen at its own memo copies with its knot; one seen at
//! another type is laid down as a plain value of its kind, as a retype lays it down.
//!
//! It runs over an explicit stack in a bump of its own, so the stack it
//! uses does not grow with the value's depth: a composite is a frame whose children are copied
//! before it is laid down, and a knot member is a frame over the values its family
//! [lists](KnottedFamily::held), tied once they are all copied. One copy serves one placement and
//! keys every knot it rebuilds by its root, so two references to one knot arrive as members of one
//! copy of it. Plain values reached twice still copy twice.

use crate::memory::{
    Active, Bump, BumpAllocator, BumpBackedMap, BumpVec, CellHandle, Covariant, CrossedOperand,
    Delivery, Operand, Prices, Reattachable, ReattachableOverBoth, Stale, StepContext, Verdict,
    Writer, bump_table, collect,
};

use crate::type_lattice::TypeRegistry;

use super::surface::Parts;
use super::{
    Dict, Knotted, KnottedFamily, List, Record, Seen, Surface, Tagged, Value, ValueCarrier,
    ValueFamily, text,
};

/// How many copy bytes one pin byte is worth: an operand copies while its copy costs less than a
/// `COPY_RATIO`th of what pinning it would newly retain.
pub const COPY_RATIO: usize = 4;

/// The crossing verdict: copy when `copy_bytes * COPY_RATIO < pin_bytes`, saturating, pin otherwise.
/// The occupancy the prices also carry does not move it.
pub fn verdict(prices: Prices) -> Verdict {
    if prices.copy_bytes.saturating_mul(COPY_RATIO) < prices.pin_bytes {
        Verdict::Copy
    } else {
        Verdict::Pin
    }
}

/// Build `carrier`'s value into `dest`'s region and hand back the carrier resting there: the value
/// as it is under a pin, rebuilt deep under a copy.
pub fn cross<
    'graph,
    'step,
    C: Reattachable<'graph>,
    S: ReattachableOverBoth<'graph>,
    D: Delivery<'graph>,
    XF: KnottedFamily<'graph>,
>(
    context: &mut StepContext<'graph, 'step, '_, '_, C, S, D>,
    dest: impl Into<CellHandle>,
    carrier: &ValueCarrier<'graph, 'step, XF>,
    types: &TypeRegistry<'_>,
) -> Result<ValueCarrier<'graph, 'step, XF>, Stale<CellHandle>>
where
    ValueFamily<XF>: Covariant<'graph>,
{
    let weight = context.read(carrier).value().weight();
    context.alloc_into::<ValueFamily<XF>, ValueFamily<XF>>(
        dest,
        &[Operand {
            carrier,
            copy_bytes: weight.bytes(),
        }],
        |writer, views| Active::new(cross_view(writer, &views[0], types)),
    )
}

/// The own-cell crossing: `carrier`'s value made reachable at the executing cell's `'here`, where it
/// may be embedded in what the step builds or captured by its continuation.
pub fn cross_here<
    'graph,
    'step,
    'here,
    C: Reattachable<'graph>,
    S: ReattachableOverBoth<'graph>,
    D: Delivery<'graph>,
    XF: KnottedFamily<'graph>,
>(
    context: &mut StepContext<'graph, 'step, 'here, '_, C, S, D>,
    carrier: &ValueCarrier<'graph, 'step, XF>,
    types: &TypeRegistry<'_>,
) -> Value<'here, XF::Closed<'here>>
where
    ValueFamily<XF>: Covariant<'graph>,
{
    let weight = context.read(carrier).value().weight();
    context.alloc_here(
        &[Operand {
            carrier,
            copy_bytes: weight.bytes(),
        }],
        |writer, views| cross_view(writer, &views[0], types),
    )
}

/// One crossed operand as a value at the destination's brand: the pinned view as it is, a copied
/// view rebuilt through `writer`. The one door a copy of a value comes through, and it takes a
/// [`CrossedOperand`], which only a priced placement hands out — so every copy is one the graph
/// priced. See [README.md § Crossing](README.md#crossing).
pub fn cross_view<'graph, 'cell, XF: KnottedFamily<'graph>>(
    writer: Writer<'cell>,
    view: &CrossedOperand<'graph, 'cell, '_, ValueFamily<XF>>,
    types: &TypeRegistry<'_>,
) -> Value<'cell, XF::Closed<'cell>> {
    match *view {
        CrossedOperand::Pinned { view: value, .. } => value,
        CrossedOperand::Copied { view: value, .. } => {
            let scratch = Bump::new();
            Copying::<XF>::new(writer, types, &scratch).value(&value)
        }
    }
}

/// Values held inside a copied operand of another family — a birth that carries values — rebuilt
/// through `writer`, all through one copy, so they share one copy of each knot they reach. The
/// operand is the proof: only a priced placement hands one out, and its severed brand is the one
/// `values` are read at, so this is still a copy the graph priced.
pub fn copy_severed<
    'graph,
    'cell,
    'severed,
    F: Reattachable<'graph>,
    XF: KnottedFamily<'graph>,
    const N: usize,
>(
    writer: Writer<'cell>,
    operand: &CrossedOperand<'graph, 'cell, 'severed, F>,
    values: [&Value<'severed, XF::Closed<'severed>>; N],
    types: &TypeRegistry<'_>,
) -> [Value<'cell, XF::Closed<'cell>>; N] {
    debug_assert!(
        matches!(operand, CrossedOperand::Copied { .. }),
        "a pinned operand's values embed as they are"
    );
    let scratch = Bump::new();
    let mut copying = Copying::<XF>::new(writer, types, &scratch);
    values.map(|value| copying.value(value))
}

/// One pending composite of the deep copy: the surface its children are read from, how many of
/// them have been started, and where its finished children begin on the copy's `done` stack.
#[derive(Clone, Copy)]
enum Frame<'x, 'from, X> {
    /// A list, dict, record or tagged value, or a data node seen at a type other than its memo,
    /// opened at the type it is seen at.
    Composite {
        surface: Surface<'x, 'from, X>,
        next: usize,
        base: usize,
    },
    /// A knot member whose knot is not yet copied: its held values sit on the copy's `held` stack
    /// from `held` to `end`.
    Knot {
        member: X,
        held: usize,
        end: usize,
        next: usize,
        base: usize,
    },
}

/// The deep copy of one placement: every region part of a value rebuilt through `writer` at the
/// type it is seen at, laying down only what that type names and weighing what it lays down; every
/// program node embedded as the same node; a knot member rebuilt by its family once per knot, its
/// memos and weights carried over. Total. Built only by [`cross_view`] and
/// [`copy_severed`], so every copy is one the graph priced.
struct Copying<'graph, 'x, 'from, 'to, 'run, XF: KnottedFamily<'graph>>
where
    'graph: 'from,
    'graph: 'to,
{
    writer: Writer<'to>,
    /// The registry the types a copy reads are interned in.
    types: &'x TypeRegistry<'run>,
    /// Where the surfaces a copy opens are staged, beside its stacks.
    scratch: BumpAllocator<'x>,
    frames: BumpVec<'x, Frame<'x, 'from, XF::Closed<'from>>>,
    /// Finished copies whose composite is still pending.
    done: BumpVec<'x, Value<'to, XF::Closed<'to>>>,
    /// The values each pending knot holds, as its family listed them.
    held: BumpVec<'x, Value<'from, XF::Closed<'from>>>,
    /// Each knot copied so far: its root, beside its copy's root.
    knots: BumpBackedMap<'x, XF::Closed<'from>, XF::Closed<'to>>,
}

impl<'graph, 'x, 'from, 'to, 'run, XF: KnottedFamily<'graph>>
    Copying<'graph, 'x, 'from, 'to, 'run, XF>
where
    'graph: 'from,
    'graph: 'to,
{
    fn new(writer: Writer<'to>, types: &'x TypeRegistry<'run>, scratch: BumpAllocator<'x>) -> Self {
        Copying {
            writer,
            types,
            scratch,
            frames: BumpVec::new_in(scratch),
            done: BumpVec::new_in(scratch),
            held: BumpVec::new_in(scratch),
            knots: bump_table(scratch),
        }
    }

    /// `value` copied: a leaf at once, anything else by running the stack until it empties.
    fn value(&mut self, value: &Value<'from, XF::Closed<'from>>) -> Value<'to, XF::Closed<'to>> {
        if let Some(leaf) = self.start(Seen::of(*value)) {
            return leaf;
        }
        while let Some(&frame) = self.frames.last() {
            match self.next_child(frame) {
                Some(child) => {
                    self.advance();
                    if let Some(leaf) = self.start(child) {
                        self.done.push(leaf);
                    }
                }
                None => {
                    self.frames.pop();
                    let finished = self.finish(frame);
                    if self.frames.is_empty() {
                        return finished;
                    }
                    self.done.push(finished);
                }
            }
        }
        unreachable!("a started composite pushes a frame")
    }

    /// Begin copying `seen`: a leaf, or a knot already copied, is finished at once; any other
    /// value pushes its frame. A data node seen at a type other than its memo is laid down as a
    /// plain value of its kind at that type, so it pushes a composite frame, not its knot's.
    fn start(
        &mut self,
        seen: Seen<'from, XF::Closed<'from>>,
    ) -> Option<Value<'to, XF::Closed<'to>>> {
        let writer = self.writer;
        let base = self.done.len();
        let value = seen.value();
        let composite = match value {
            Value::List(_) | Value::Dict(_) | Value::Record(_) | Value::Tagged(_) => true,
            Value::Knotted(member) => seen.ktype() != member.ktype(),
            _ => false,
        };
        if composite {
            let surface = seen
                .surface(self.types, self.scratch)
                .expect("a container, a tagged value or a data node opens");
            self.frames.push(Frame::Composite {
                surface,
                next: 0,
                base,
            });
            return None;
        }
        let finished = match value {
            Value::Number(number) => Value::Number(number),
            Value::Bool(flag) => Value::Bool(flag),
            Value::Null => Value::Null,
            Value::Str(source) => text(writer, source),
            Value::Type(type_value) => Value::Type(type_value.copied(writer)),
            Value::List(_) | Value::Dict(_) | Value::Record(_) | Value::Tagged(_) => {
                unreachable!("a composite pushed its frame above")
            }
            Value::Knotted(member) => {
                if let Some(root) = self.knots.get(&member.root()) {
                    Value::Knotted(root.sibling(member.index()))
                } else {
                    let held = self.held.len();
                    XF::held(&member, &mut |value| self.held.push(value));
                    self.frames.push(Frame::Knot {
                        member,
                        held,
                        end: self.held.len(),
                        next: 0,
                        base,
                    });
                    return None;
                }
            }
        };
        Some(finished)
    }

    /// The next child `frame` has not started, if any.
    fn next_child(
        &self,
        frame: Frame<'x, 'from, XF::Closed<'from>>,
    ) -> Option<Seen<'from, XF::Closed<'from>>> {
        match frame {
            Frame::Composite { surface, next, .. } => {
                (next < surface.len()).then(|| surface.child(next, self.types, self.scratch))
            }
            Frame::Knot {
                held, end, next, ..
            } => (held + next < end).then(|| Seen::of(self.held[held + next])),
        }
    }

    /// Mark the top frame's next child started.
    fn advance(&mut self) {
        match self.frames.last_mut() {
            Some(Frame::Composite { next, .. } | Frame::Knot { next, .. }) => *next += 1,
            None => unreachable!("a child is started under a frame"),
        }
    }

    /// Lay down `frame`'s composite over its finished children, and pop them: a surface at the
    /// type it is seen at, weighed by what it lays down.
    fn finish(
        &mut self,
        frame: Frame<'x, 'from, XF::Closed<'from>>,
    ) -> Value<'to, XF::Closed<'to>> {
        let writer = self.writer;
        let (base, finished) = match frame {
            Frame::Composite { surface, base, .. } => {
                let built = &self.done[base..];
                let ktype = surface.ktype();
                let cells = || writer.fill(built.len(), |at| built[at]);
                let finished = match surface.parts() {
                    Parts::List { .. } => Value::List(List::weighed(writer, cells(), ktype)),
                    Parts::Dict { .. } => {
                        let keys = writer.fill(surface.len(), |at| surface.key(at).rehomed(writer));
                        Value::Dict(Dict::weighed(writer, keys, cells(), ktype))
                    }
                    Parts::Record { .. } => {
                        let names = collect(writer, (0..surface.len()).map(|at| surface.name(at)));
                        Value::Record(Record::weighed(writer, names, cells(), ktype))
                    }
                    Parts::Tagged { .. } => Value::Tagged(Tagged::hold(writer, built[0], ktype)),
                };
                (base, finished)
            }
            Frame::Knot {
                member,
                held,
                end,
                base,
                ..
            } => {
                let mut answers = self.held[held..end].iter().zip(&self.done[base..]);
                let mut cursor = |asked: &Value<'from, XF::Closed<'from>>| {
                    let (listed, copy) = answers
                        .next()
                        .expect("copy_into asks for each held value once");
                    debug_assert!(same_word(listed, asked), "copy_into asks in held's order");
                    *copy
                };
                let copied = XF::copy_into(writer, &member, &mut cursor);
                debug_assert!(
                    answers.next().is_none(),
                    "copy_into asks for every held value"
                );
                self.knots.insert(member.root(), copied.root());
                self.held.truncate(held);
                (base, Value::Knotted(copied))
            }
        };
        self.done.truncate(base);
        finished
    }
}

/// Whether two value words are the same word: the same variant over the same scalar or the same
/// resident. Never walks a value.
fn same_word<X: Knotted>(a: &Value<'_, X>, b: &Value<'_, X>) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a.to_bits() == b.to_bits(),
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Null, Value::Null) => true,
        (Value::Str(a), Value::Str(b)) => std::ptr::eq(*a, *b),
        (Value::Type(a), Value::Type(b)) => std::ptr::eq(*a, *b),
        (Value::List(a), Value::List(b)) => std::ptr::eq(*a, *b),
        (Value::Dict(a), Value::Dict(b)) => std::ptr::eq(*a, *b),
        (Value::Record(a), Value::Record(b)) => std::ptr::eq(*a, *b),
        (Value::Tagged(a), Value::Tagged(b)) => std::ptr::eq(*a, *b),
        (Value::Knotted(a), Value::Knotted(b)) => a == b,
        _ => false,
    }
}
