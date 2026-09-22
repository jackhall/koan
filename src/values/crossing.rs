//! Moving a value between regions: one verb over the substrate's two placement doors, and the
//! copy-or-pin verdict a graph is built with.
//!
//! A pinned operand arrives at the destination's region lifetime and is embedded as it is. A copied
//! one arrives at an unrelated lifetime, so the only rebuild that typechecks is [`copy_into`]'s deep
//! one through the destination's writer; its program nodes are `'graph` borrows and embed verbatim,
//! and a knot member rebuilds its whole knot through its family, handed this same copy for every
//! value the knot holds.

use crate::memory::{
    Active, CellHandle, Covariant, CrossedOperand, Delivery, Operand, Prices, Reattachable,
    ReattachableOverBoth, Stale, StepContext, Verdict, Writer, collect,
};

use super::{Dict, KnottedFamily, List, Record, Tagged, Value, ValueCarrier, ValueFamily, text};

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
) -> Value<'graph, 'here, XF::Closed<'here>>
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
) -> Value<'graph, 'cell, XF::Closed<'cell>> {
    match *view {
        CrossedOperand::Pinned { view: value, .. } => value,
        CrossedOperand::Copied { view: value, .. } => copy_into::<XF>(writer, &value),
    }
}

/// The deep copy: every region part of `value` rebuilt through `writer`, every program node
/// embedded as the same node, every memoized type and weight carried over, a knot member rebuilt by
/// its family. Total. Reached only through [`cross_view`], so every copy is one the graph priced.
fn copy_into<'graph, 'from, 'to, XF: KnottedFamily<'graph>>(
    writer: Writer<'to>,
    value: &Value<'graph, 'from, XF::Closed<'from>>,
) -> Value<'graph, 'to, XF::Closed<'to>>
where
    'graph: 'from,
    'graph: 'to,
{
    match *value {
        Value::Number(number) => Value::Number(number),
        Value::Bool(flag) => Value::Bool(flag),
        Value::Null => Value::Null,
        Value::Expression(node) => Value::Expression(node),
        Value::Str(source) => text(writer, source),
        Value::Type(type_value) => Value::Type(type_value.copied(writer)),
        Value::List(list) => {
            let source = list.cells();
            let cells = writer.fill(source.len(), |at| copy_into::<XF>(writer, &source[at]));
            Value::List(List::from_run(writer, cells, list.ktype(), list.weight()))
        }
        Value::Dict(dict) => {
            let (source_keys, source_cells) = (dict.keys(), dict.cells());
            let keys = writer.fill(source_keys.len(), |at| source_keys[at].rehomed(writer));
            let cells = writer.fill(source_cells.len(), |at| {
                copy_into::<XF>(writer, &source_cells[at])
            });
            Value::Dict(Dict::from_runs(
                writer,
                keys,
                cells,
                dict.ktype(),
                dict.weight(),
            ))
        }
        Value::Record(record) => {
            let source = record.cells();
            let names = collect(writer, record.names().iter().copied());
            let cells = writer.fill(source.len(), |at| copy_into::<XF>(writer, &source[at]));
            Value::Record(Record::from_runs(
                writer,
                names,
                cells,
                record.ktype(),
                record.weight(),
            ))
        }
        Value::Tagged(tagged) => Value::Tagged(Tagged::hold(
            writer,
            copy_into::<XF>(writer, tagged.payload()),
            tagged.ktype(),
        )),
        Value::Knotted(member) => Value::Knotted(XF::copy_into(writer, &member, &mut |value| {
            copy_into::<XF>(writer, value)
        })),
    }
}
