//! A caught fault copied out into an `Error` ([README.md § Catching](README.md#catching)).
//!
//! A `TRY` or `CATCH` runs its body as a block, a child of its evaluation, and parks at
//! [`CAUGHT`]. A value finishes it — a `CATCH`'s as `Result.Ok` of it. A fault a `TRY`'s arms do
//! not take by its kind is handed on as it came, read and released nowhere. Any other fault is
//! walked outermost first: the links in the evaluation's own home read in place, and each kept cell's
//! in a step entered beside it at [`GATHERING`], which the drain releases the cell after. Each
//! trace line is merged as [`Trace`](crate::scope::Trace) merges it ([`Hop::merges`]) and emitted as a
//! frame record into the evaluation's region, carried across steps in [`Catching`]; at the fault
//! itself its fields are copied in — a value by the deep copy a crossing makes, a type adopted from
//! the kept cell's branch — and the `Error` record is built. A `TRY` runs its arm on it as a `MATCH`
//! runs one; a `CATCH` finishes with `Result.Error` of it.

use crate::elaborate::arm_shapes;
use crate::knot::{KFault, KInner, KValue};
use crate::memory::{Bump, BumpAllocator, BumpVec, Writer, resident};
use crate::parse::{ExpressionPart, KExpression};
use crate::program::{Catching, Entered, FrameLink, KBundle, KState, Program, entered_block};
use crate::scheduler::{Action, Held, Received, StepError};
use crate::scope::{Bottom, ErrorField, FaultKind, Guard, Hop, Inner, Line, ungrouped};
use crate::symbols::{BinderSymbol, SymbolInterner, TypeSymbol};
use crate::type_lattice::{KType, display_name};
use crate::values::{Chain, List, Record, Tagged, TypeValue, Value, construction, representation};

use super::Evaluation;
use super::evaluate::{
    BORN, CAUGHT, GATHERING, Taking, arm_guards, arm_ran, built, evaluate, faulted, finish,
    finish_built, inside, park,
};
use super::select;

/// `TRY <body> WITH <arms>`, or, not `tries`, `CATCH <body>`, at `stage`, beside what it has
/// `copied` of a caught fault where it is entered beside a kept cell.
pub(super) fn caught<'step, 'graph, 'here>(
    mut step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
    node: &'step KExpression<'step>,
    tries: bool,
    stage: u32,
    copied: Option<Catching<'graph, 'here>>,
) -> Action<'graph, KBundle> {
    match (stage, copied) {
        (BORN, _) => {
            let shape = at.view.shape();
            let body = (shape.site_of_body(node))
                .and_then(|site| shape.nested(site))
                .expect("a caught body is a block its shape holds");
            // A `CATCH`'s value wraps its body's, so the body's is asked for as a part.
            let use_ = if tries { inside(at) } else { built(at) };
            let request = entered_block(
                at.program,
                body,
                at.view.view(),
                Entered::Plain,
                None,
                use_,
                at.within,
            );
            let asked = step.spawn(request);
            park(step, at, asked, CAUGHT)
        }
        (CAUGHT, _) => {
            let received = step.results_with(&at.types).next();
            caught_body(step, at, node, tries, received)
        }
        (GATHERING, Some(copied)) => beside(step, at, node, copied),
        _ => step.failed(StepError::Refused),
    }
}

/// The body's outcome is in: a value finishes, and a fault is handed on or walked.
fn caught_body<'step, 'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
    node: &'step KExpression<'step>,
    tries: bool,
    received: Option<Result<Received<'graph, 'here, '_>, StepError>>,
) -> Action<'graph, KBundle> {
    match received {
        Some(Ok(Received::Here(held))) if tries => finish(step, at, held),
        Some(Ok(Received::Here(held))) => {
            let program = at.program;
            finish_built(step, at, &[held], move |writer, types, values| {
                result(program, writer, types, "Ok", values[0])
            })
        }
        Some(Ok(Received::Fault(inner))) => {
            let arm = match tries {
                true => match chosen(at, node, inner.kind()) {
                    Some(arm) => Some(arm),
                    None => return faulted(step, at, inner),
                },
                false => None,
            };
            let copied = Catching {
                arm,
                frames: None,
                pending: None,
            };
            walked(step, at, node, copied, inner)
        }
        _ => step.failed(StepError::Unredeemable),
    }
}

/// The arm of the `TRY` `node` a fault of `kind` selects, as a `MATCH … UNDER Error` selects one
/// over the load's record of the arms: a label naming the kind's variant, else `_`. `None` where
/// none takes it.
fn chosen(
    at: &Evaluation<'_, '_, '_, '_, '_, '_>,
    node: &KExpression<'_>,
    kind: FaultKind,
) -> Option<u16> {
    let (types, scratch) = (&at.types, Bump::new());
    let (shape, facts) = (at.view.shape(), at.view.facts());
    let arms = facts.arms(shape, shape.site_of_node(node))?;
    let error = at.program.error_type();
    let (_, guards) = arm_guards(types, &scratch, shape, node, Some(error), |_| None);
    let variant = at.program.error_variant(kind);
    let admits = |arm: usize| match guards[arm] {
        (Guard::Label(_), admits) => admits == Some(variant),
        (Guard::Default, _) => true,
        _ => false,
    };
    match select::arm(types, &scratch, arms, &guards, admits) {
        Ok(chosen) => Some(chosen as u16),
        Err(ambiguous) => {
            debug_assert!(ambiguous.is_none(), "a kind's label outranks `_`");
            None
        }
    }
}

/// Walk `inner`, laid in the evaluation's own home, gathering its lines: to the fault, which ends
/// the walk, or to the kept cell below, read in a step entered beside it.
fn walked<'step, 'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
    node: &'step KExpression<'step>,
    mut copied: Catching<'graph, 'here>,
    inner: KInner<'graph, 'here>,
) -> Action<'graph, KBundle> {
    // Frame records persist across the steps a walk takes, so they resolve in the region's chain.
    let (writer, symbols, region) = (step.writer(), at.program.symbols(), step.types());
    match inner.walked(|line| gathered(&mut copied, writer, &region, &at.types, symbols, line)) {
        Bottom::Below(kept) => {
            let state = KState::Catching {
                birth: at.birth,
                caught: copied,
            };
            step.read_beside(kept, evaluate, state, None)
        }
        Bottom::Fault(fault) => {
            let scratch = Bump::new();
            emitted(&mut copied, writer, &region);
            let record = payload(
                writer,
                &at.types,
                &at.types,
                symbols,
                fault,
                copied.frames,
                |values| {
                    let mut copies = BumpVec::with_capacity_in(values.len(), &scratch);
                    copies.extend_from_slice(values);
                    copies
                },
                |_| {},
                &scratch,
            );
            ended(step, at, node, copied.arm, fault.kind(), record)
        }
    }
}

/// What a step entered beside a kept cell read of it.
enum Read<'graph, 'here> {
    /// The kept cell below it, read next.
    Below(crate::values::KeptCell),
    /// The fault, copied in as its `Error` record, beside its kind.
    Built(KValue<'graph, 'here>, FaultKind),
}

/// Entered beside a kept cell: gather its links' lines, then read the kept cell below it, or build
/// the `Error` record from the fault.
fn beside<'step, 'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
    node: &'step KExpression<'step>,
    mut copied: Catching<'graph, 'here>,
) -> Action<'graph, KBundle> {
    // What is read beside is copied into the region, whose chain the copy adopts its types into.
    let (writer, symbols, region) = (step.writer(), at.program.symbols(), step.types());
    let types = &region;
    let read = step.beside(|beside| {
        let lines = |line| gathered(&mut copied, writer, types, beside.types(), symbols, line);
        match Inner::Link(beside.link()).walked(lines) {
            Bottom::Below(kept) => Read::Below(kept),
            Bottom::Fault(fault) => {
                let scratch = Bump::new();
                emitted(&mut copied, writer, types);
                let record = payload(
                    writer,
                    types,
                    beside.types(),
                    symbols,
                    fault,
                    copied.frames,
                    |values| beside.copy_into(writer, types, values, &scratch),
                    |handle| beside.adopt_into(types, handle),
                    &scratch,
                );
                Read::Built(record, fault.kind())
            }
        }
    });
    match read {
        Some(Read::Below(kept)) => {
            let state = KState::Catching {
                birth: at.birth,
                caught: copied,
            };
            step.read_beside(kept, evaluate, state, None)
        }
        Some(Read::Built(record, kind)) => ended(step, at, node, copied.arm, kind, record),
        None => step.failed(StepError::Refused),
    }
}

/// Gather `line`'s hops into `copied`, outermost first, its callees named through `words`: a hop at
/// the pending line's callee and call adds its count, and any other emits the pending line as a
/// frame record and becomes the pending one.
fn gathered<'graph, 'here>(
    copied: &mut Catching<'graph, 'here>,
    writer: Writer<'here, '_>,
    types: &Chain<'_, 'here, '_, '_>,
    words: &Chain<'_, '_, '_, '_>,
    symbols: &SymbolInterner,
    line: &Line<'_>,
) {
    for hop in line.hops() {
        match copied.pending {
            Some((pending, named)) if pending.merges(&hop) => {
                let count = pending.count.saturating_add(hop.count);
                copied.pending = Some((Hop { count, ..pending }, named));
            }
            _ => {
                emitted(copied, writer, types);
                let named = match hop.callee {
                    Some(callee) => writer.text(&display_name(callee, words, symbols).to_string()),
                    None => writer.text("`EVAL`"),
                };
                copied.pending = Some((hop, named));
            }
        }
    }
}

/// Emit the pending line of `copied`, where there is one, as a frame record at the head of its
/// frames: `{callee, call, at, count}`, worded as a trace line words it.
fn emitted<'graph, 'here>(
    copied: &mut Catching<'graph, 'here>,
    writer: Writer<'here, '_>,
    types: &Chain<'_, 'here, '_, '_>,
) {
    let Some((hop, named)) = copied.pending.take() else {
        return;
    };
    let call = hop.call.text();
    let fields = [
        (label("callee"), Value::Str(named)),
        (label("call"), Value::Str(writer.text(ungrouped(&call)))),
        (label("at"), Value::Str(writer.text(&hop.call.to_string()))),
        (label("count"), Value::Number(f64::from(hop.count))),
    ];
    let frame = Value::Record(Record::new(writer, &fields, types, &Bump::new()));
    let next = copied.frames;
    copied.frames = Some(resident(writer, FrameLink { frame, next }));
}

/// `fault`'s `Error` record, built through `writer` in the evaluation's region: `message`, worded
/// through `words`, the chain `fault` resolves in; `frames`, the frame records from `frames`, the
/// innermost first; and each of the kind's own fields, every value they hold brought in through one
/// `copy` and every type through `adopt`.
#[allow(clippy::too_many_arguments)]
fn payload<'graph, 'here, 'from, 'list>(
    writer: Writer<'here, '_>,
    types: &Chain<'_, 'here, '_, '_>,
    words: &Chain<'_, '_, '_, '_>,
    symbols: &SymbolInterner,
    fault: &KFault<'graph, 'from>,
    frames: Option<&FrameLink<'graph, 'here>>,
    copy: impl FnOnce(&[KValue<'graph, 'from>]) -> BumpVec<'list, KValue<'graph, 'here>>,
    adopt: impl Fn(KType),
    scratch: &'list Bump,
) -> KValue<'graph, 'here> {
    let mut held = BumpVec::new_in(scratch);
    fault.each_field(symbols, words, &mut |_, field| match field {
        ErrorField::Value(value) => held.push(value),
        ErrorField::Values(values) => held.extend_from_slice(values),
        ErrorField::Text(_) | ErrorField::Number(_) | ErrorField::Type(_) => {}
    });
    let mut copies = copy(&held).into_iter();
    let mut listed = BumpVec::new_in(scratch);
    let mut link = frames;
    while let Some(frame) = link {
        listed.push(frame.frame);
        link = frame.next;
    }
    let message = writer.text(&fault.display(symbols, words).to_string());
    let mut fields = BumpVec::new_in(scratch);
    fields.push((label("message"), Value::Str(message)));
    let frames = List::new(writer, listed.into_iter(), types, scratch);
    fields.push((label("frames"), Value::List(frames)));
    fault.each_field(symbols, words, &mut |name, field| {
        let value = match field {
            ErrorField::Value(_) => copies.next().expect("one copy per value held"),
            ErrorField::Values(values) => {
                let listed = copies.by_ref().take(values.len());
                Value::List(List::new(writer, listed, types, scratch))
            }
            ErrorField::Text(text) => Value::Str(writer.text(&text.to_string())),
            ErrorField::Number(count) => Value::Number(count as f64),
            ErrorField::Type(Some(handle)) => {
                adopt(handle);
                Value::Type(TypeValue::new(writer, handle, types))
            }
            ErrorField::Type(None) => Value::Null,
        };
        fields.push((label(name), value));
    });
    #[cfg(debug_assertions)]
    {
        let declared = fault.kind().fields().iter().map(|(name, _)| label(name));
        let built = fields.iter().skip(2).map(|(name, _)| *name);
        debug_assert!(
            built.eq(declared),
            "the fields are the kind's own, in order"
        );
    }
    Value::Record(Record::new(writer, &fields, types, scratch))
}

/// Finish a caught fault of `kind`, its `Error` record built: a `TRY` runs `arm` on it, as a
/// `MATCH … UNDER Error` runs one — a label's `it` retyped to its variant's record — and a `CATCH`
/// (no `arm`) finishes with `Result.Error` of `Error.<kind>` over it.
fn ended<'step, 'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
    node: &'step KExpression<'step>,
    arm: Option<u16>,
    kind: FaultKind,
    record: KValue<'graph, 'here>,
) -> Action<'graph, KBundle> {
    let program = at.program;
    let Some(arm) = arm else {
        return finish_built(
            step,
            at,
            &[Held::from(record)],
            move |writer, types, values| {
                let scratch = Bump::new();
                let variant = program.error_variant(kind);
                let error = tagged(writer, types, variant, values[0], &scratch);
                result(program, writer, types, "Error", error)
            },
        );
    };
    let (types, scratch) = (&at.types, Bump::new());
    let (shape, facts) = (at.view.shape(), at.view.facts());
    let body = arm_shapes(shape, node)
        .nth(usize::from(arm))
        .expect("the chosen arm is one of the `TRY`'s");
    let it = match body.arm().map(|arm| arm.guard) {
        Some(Guard::Label(_)) => {
            let Some(ExpressionPart::DictLiteral(written)) =
                node.parts.last().map(|arms| &arms.value)
            else {
                unreachable!("a `TRY`'s last part is its arm set")
            };
            let retype = facts.retype(types, shape, shape.site(&written[usize::from(arm)].1));
            let variant = program.error_variant(kind);
            match representation(types, &scratch, variant) {
                Some(represented) => {
                    record.retyped_as(step.writer(), represented, retype, types, &scratch)
                }
                None => record,
            }
        }
        _ => record,
    };
    arm_ran(step, at, body, Held::from(it))
}

/// `value` wrapped in `variant`, a variant of `Error` or `Result`, through `writer`.
fn tagged<'graph, 'cell>(
    writer: Writer<'cell, '_>,
    types: &Chain<'_, 'cell, '_, '_>,
    variant: KType,
    value: KValue<'graph, 'cell>,
    scratch: BumpAllocator<'_>,
) -> KValue<'graph, 'cell> {
    let identity = construction(types, scratch, variant, value.concrete_ktype())
        .expect("an `Error` or `Result` variant wraps its payload");
    Value::Tagged(Tagged::hold(writer, value, identity, types))
}

/// `value` wrapped in the builtin `Result`'s variant `name`.
fn result<'graph, 'cell>(
    program: &Program<'graph>,
    writer: Writer<'cell, '_>,
    types: &Chain<'_, 'cell, '_, '_>,
    name: &str,
    value: KValue<'graph, 'cell>,
) -> KValue<'graph, 'cell> {
    let name = TypeSymbol::classify(name).expect("a Type token").symbol();
    let variant = (types.union_member_named(program.result_type(), name))
        .expect("the builtin `Result` names both variants");
    tagged(writer, types, variant, value, &Bump::new())
}

/// The field label `name`.
fn label(name: &str) -> BinderSymbol {
    BinderSymbol::classify(name).expect("a field label")
}
