//! Moving a value between regions: one verb over the substrate's two placement doors, and the
//! copy-or-pin verdict a graph is built with.
//!
//! A pinned operand arrives at the destination's region lifetime and is embedded as it is. A copied
//! one arrives at an unrelated lifetime, so the only rebuild that typechecks is [`copy_into`]'s deep
//! one through the destination's writer; its program nodes are `'graph` borrows and embed verbatim.

use crate::memory::{
    Active, CellHandle, CrossedOperand, Operand, Prices, Reattachable, Stale, StepContext, Verdict,
    Writer,
};

use super::{Dict, List, Record, Tagged, Value, ValueCarrier, ValueFamily, collect, text};

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
pub fn cross<'graph, 'step, 'here, C: Reattachable<'graph>>(
    context: &mut StepContext<'graph, 'step, 'here, C>,
    dest: impl Into<CellHandle>,
    carrier: &ValueCarrier<'graph, 'step>,
) -> Result<ValueCarrier<'graph, 'step>, Stale<CellHandle>> {
    let weight = context.read(carrier).value().weight();
    context.alloc_into::<ValueFamily, ValueFamily>(
        dest,
        &[Operand {
            carrier,
            copy_bytes: weight.bytes(),
        }],
        |writer, views| {
            Active::new(match views[0] {
                CrossedOperand::Pinned(value) => value,
                CrossedOperand::Copied(value) => copy_into(writer, &value),
            })
        },
    )
}

/// The own-cell crossing: `carrier`'s value made reachable at the executing cell's `'here`, where it
/// may be embedded in what the step builds or captured by its continuation.
pub fn cross_here<'graph, 'step, 'here, C: Reattachable<'graph>>(
    context: &mut StepContext<'graph, 'step, 'here, C>,
    carrier: &ValueCarrier<'graph, 'step>,
) -> Value<'graph, 'here> {
    let weight = context.read(carrier).value().weight();
    context.alloc_here(
        &[Operand {
            carrier,
            copy_bytes: weight.bytes(),
        }],
        |writer, views| match views[0] {
            CrossedOperand::Pinned(value) => value,
            CrossedOperand::Copied(value) => copy_into(writer, &value),
        },
    )
}

/// The deep copy: every region part of `value` rebuilt through `writer`, every program node
/// embedded as the same node, every memoized type and weight carried over. Total.
pub(crate) fn copy_into<'graph, 'cell>(
    writer: Writer<'cell>,
    value: &Value<'graph, '_>,
) -> Value<'graph, 'cell>
where
    'graph: 'cell,
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
            let cells = writer.fill(source.len(), |at| copy_into(writer, &source[at]));
            Value::List(List::from_run(writer, cells, list.ktype(), list.weight()))
        }
        Value::Dict(dict) => {
            let (source_keys, source_cells) = (dict.keys(), dict.cells());
            let keys = writer.fill(source_keys.len(), |at| source_keys[at].rehomed(writer));
            let cells = writer.fill(source_cells.len(), |at| {
                copy_into(writer, &source_cells[at])
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
            let cells = writer.fill(source.len(), |at| copy_into(writer, &source[at]));
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
            copy_into(writer, tagged.payload()),
            tagged.ktype(),
        )),
    }
}
