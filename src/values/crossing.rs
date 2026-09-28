//! Moving a value between regions: one verb over the substrate's two placement doors, and the
//! copy-or-pin verdict a graph is built with.
//!
//! A pinned operand arrives at the destination's region lifetime and is embedded as it is. A copied
//! one arrives at an unrelated lifetime, so the only rebuild that typechecks is a deep one through
//! the destination's writer, and a knot member rebuilds its whole knot through its family; what a
//! member borrows of program storage embeds verbatim.
//!
//! The deep copy ([`Copying`]) runs over an explicit stack in a bump of its own, so the stack it
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

use super::{
    Dict, Knotted, KnottedFamily, List, Record, Tagged, Value, ValueCarrier, ValueFamily, text,
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
        |writer, views| Active::new(cross_view(writer, &views[0])),
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
        |writer, views| cross_view(writer, &views[0]),
    )
}

/// One crossed operand as a value at the destination's brand: the pinned view as it is, a copied
/// view rebuilt through `writer`. The one door a copy of a value comes through, and it takes a
/// [`CrossedOperand`], which only a priced placement hands out — so every copy is one the graph
/// priced. See [README.md § Crossing](README.md#crossing).
pub fn cross_view<'graph, 'cell, XF: KnottedFamily<'graph>>(
    writer: Writer<'cell>,
    view: &CrossedOperand<'graph, 'cell, '_, ValueFamily<XF>>,
) -> Value<'cell, XF::Closed<'cell>> {
    match *view {
        CrossedOperand::Pinned { view: value, .. } => value,
        CrossedOperand::Copied { view: value, .. } => {
            let scratch = Bump::new();
            Copying::<XF>::new(writer, &scratch).value(&value)
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
) -> [Value<'cell, XF::Closed<'cell>>; N] {
    debug_assert!(
        matches!(operand, CrossedOperand::Copied { .. }),
        "a pinned operand's values embed as they are"
    );
    let scratch = Bump::new();
    let mut copying = Copying::<XF>::new(writer, &scratch);
    values.map(|value| copying.value(value))
}

/// One pending composite of the deep copy: the value its children are read from, how many of them
/// have been started, and where its finished children begin on the copy's `done` stack.
#[derive(Clone, Copy)]
enum Frame<'from, X> {
    /// A list, dict, record or tagged value.
    Composite {
        source: Value<'from, X>,
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

/// The deep copy of one placement: every region part of a value rebuilt through `writer`, every
/// program node embedded as the same node, every memoized type and weight carried over, a knot
/// member rebuilt by its family once per knot. Total. Built only by [`cross_view`] and
/// [`copy_severed`], so every copy is one the graph priced.
struct Copying<'graph, 'x, 'from, 'to, XF: KnottedFamily<'graph>>
where
    'graph: 'from,
    'graph: 'to,
{
    writer: Writer<'to>,
    frames: BumpVec<'x, Frame<'from, XF::Closed<'from>>>,
    /// Finished copies whose composite is still pending.
    done: BumpVec<'x, Value<'to, XF::Closed<'to>>>,
    /// The values each pending knot holds, as its family listed them.
    held: BumpVec<'x, Value<'from, XF::Closed<'from>>>,
    /// Each knot copied so far: its root, beside its copy's root.
    knots: BumpBackedMap<'x, XF::Closed<'from>, XF::Closed<'to>>,
}

impl<'graph, 'x, 'from, 'to, XF: KnottedFamily<'graph>> Copying<'graph, 'x, 'from, 'to, XF>
where
    'graph: 'from,
    'graph: 'to,
{
    fn new(writer: Writer<'to>, scratch: BumpAllocator<'x>) -> Self {
        Copying {
            writer,
            frames: BumpVec::new_in(scratch),
            done: BumpVec::new_in(scratch),
            held: BumpVec::new_in(scratch),
            knots: bump_table(scratch),
        }
    }

    /// `value` copied: a leaf at once, anything else by running the stack until it empties.
    fn value(&mut self, value: &Value<'from, XF::Closed<'from>>) -> Value<'to, XF::Closed<'to>> {
        if let Some(leaf) = self.start(*value) {
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

    /// Begin copying `value`: a leaf, or a knot already copied, is finished at once; any other
    /// value pushes its frame.
    fn start(
        &mut self,
        value: Value<'from, XF::Closed<'from>>,
    ) -> Option<Value<'to, XF::Closed<'to>>> {
        let writer = self.writer;
        let base = self.done.len();
        let finished = match value {
            Value::Number(number) => Value::Number(number),
            Value::Bool(flag) => Value::Bool(flag),
            Value::Null => Value::Null,
            Value::Str(source) => text(writer, source),
            Value::Type(type_value) => Value::Type(type_value.copied(writer)),
            Value::List(_) | Value::Dict(_) | Value::Record(_) | Value::Tagged(_) => {
                self.frames.push(Frame::Composite {
                    source: value,
                    next: 0,
                    base,
                });
                return None;
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
        frame: Frame<'from, XF::Closed<'from>>,
    ) -> Option<Value<'from, XF::Closed<'from>>> {
        match frame {
            Frame::Composite { source, next, .. } => {
                let (_, composite) = source.composite()?;
                (next < composite.len()).then(|| composite.child(next))
            }
            Frame::Knot {
                held, end, next, ..
            } => (held + next < end).then(|| self.held[held + next]),
        }
    }

    /// Mark the top frame's next child started.
    fn advance(&mut self) {
        match self.frames.last_mut() {
            Some(Frame::Composite { next, .. } | Frame::Knot { next, .. }) => *next += 1,
            None => unreachable!("a child is started under a frame"),
        }
    }

    /// Lay down `frame`'s composite over its finished children, and pop them.
    fn finish(&mut self, frame: Frame<'from, XF::Closed<'from>>) -> Value<'to, XF::Closed<'to>> {
        let writer = self.writer;
        let (base, finished) = match frame {
            Frame::Composite { source, base, .. } => {
                let built = &self.done[base..];
                let finished = match source {
                    Value::List(list) => {
                        let cells = writer.fill(built.len(), |at| built[at]);
                        Value::List(List::from_run(writer, cells, list.ktype(), list.weight()))
                    }
                    Value::Dict(dict) => {
                        let source_keys = dict.keys();
                        let keys =
                            writer.fill(source_keys.len(), |at| source_keys[at].rehomed(writer));
                        let cells = writer.fill(built.len(), |at| built[at]);
                        Value::Dict(Dict::from_runs(
                            writer,
                            keys,
                            cells,
                            dict.ktype(),
                            dict.weight(),
                        ))
                    }
                    Value::Record(record) => {
                        let names = collect(writer, record.names().iter().copied());
                        let cells = writer.fill(built.len(), |at| built[at]);
                        Value::Record(Record::from_runs(
                            writer,
                            names,
                            cells,
                            record.ktype(),
                            record.weight(),
                        ))
                    }
                    Value::Tagged(tagged) => Value::Tagged(Tagged::from_payload(
                        writer,
                        built[0],
                        tagged.ktype(),
                        tagged.weight(),
                    )),
                    _ => unreachable!("only a composite pushes a composite frame"),
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
