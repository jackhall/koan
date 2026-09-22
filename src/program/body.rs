//! The body runner: the one step that performs a body's units, in the order its shape emitted
//! them — the top level's, as a tenant of the root born as each call's root work, and a called
//! body's, in the frame's own cell.
//!
//! The runner claims nothing ahead and no unit has a cell of its own. A unit of type binders is
//! declared through the elaborator's door and bound in the same step; a module binder's body runs
//! inline, its activation laid down in the running region and the enclosing body's place kept in a
//! resident [`Outer`]; a component the knot ties is tied, and a refusal naming eager parts becomes
//! one evaluation per part, all asked for at one park; a lone `LET` and a statement that binds
//! nothing are one evaluation each. The only children the runner asks for are evaluations, through
//! the program record's `evaluate`. See [README.md § The body runner](README.md#the-body-runner).

use crate::elaborate::type_declarations;
use crate::knot::module::body_activation;
use crate::knot::{KActivation, KValue, Knotted, Supplied, Untieable, tie};
use crate::memory::{Bump, BumpVec, resident};
use crate::parse::ExpressionPart;
use crate::scheduler::{
    Action, Placement, Received, Request, Slot as Asked, Step, StepError, Taken, Use,
};
use crate::scope::{BodyShape, Component, Position, ShapeKind, Site, Slot, Unit, UnitWork};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{Collector, KType, TypeNode, Variance, admits_with};
use crate::values::{TypeValue, Value};

use super::bundle::{KBirth, KBundle, KState};
use super::record::{Evaluated, Program};

/// The body runner's state between units: which body it is performing, how far it got, and where
/// it parked.
#[derive(Clone, Copy)]
pub struct Runner<'graph, 'cell> {
    program: &'graph Program<'graph>,
    /// Invariant: it binds. It rides the parked state family, which never crosses.
    activation: &'cell KActivation<'graph, 'cell>,
    unit: u32,
    /// A frame's value so far: set when the unit holding its last statement completes.
    result: Option<KValue<'graph, 'cell>>,
    /// The enclosing bodies of a module body being run inline, innermost first.
    outer: Option<&'cell Outer<'graph, 'cell>>,
    level: Level,
    stage: Stage,
}

/// Whether the runner performs the top level or a called body.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    Top,
    Frame,
}

/// Where the runner parked.
#[derive(Clone, Copy)]
enum Stage {
    /// Nothing in flight: perform `unit` next.
    Next,
    /// One evaluation asked for the unit's value.
    Evaluation,
    /// One evaluation per eager part a refused tie named; the sites are in scratch.
    Supplying,
}

/// An enclosing body's place, kept while a module body it binds runs inline: written once when the
/// module body is entered, in the running region.
#[derive(Clone, Copy)]
struct Outer<'graph, 'cell> {
    activation: &'cell KActivation<'graph, 'cell>,
    unit: u32,
    result: Option<KValue<'graph, 'cell>>,
    /// The module binder's slot in `activation`, bound when the body ends.
    binder: Slot,
    outer: Option<&'cell Outer<'graph, 'cell>>,
}

#[cfg(test)]
thread_local! {
    /// How many times a runner has woken with the parts a tie named: what a test reads to see one
    /// wake however many parts.
    pub(super) static SUPPLIED_WAKES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A step that took both its states.
type Taking<'a, 'graph, 'step, 'here, 'scratch> =
    Step<'a, 'graph, 'step, 'here, 'scratch, KBundle, Taken, Taken>;

/// The step: the one body runner. Born as the top level's root work, or as a call's frame.
pub fn run<'graph>(step: Step<'_, 'graph, '_, '_, '_, KBundle>) -> Action<'graph, KBundle> {
    let (step, state) = step.state();
    let (mut step, scratch) = step.scratch();
    let runner = match state {
        KState::Born(KBirth::Program { program }) => {
            let writer = step.writer();
            let activation = resident(
                writer,
                KActivation::of_program(writer, program.shape(), program.builtins()),
            );
            Runner::at(program, activation, Level::Top)
        }
        KState::Born(KBirth::Call {
            program,
            callee,
            arguments,
        }) => match frame(&step, program, callee, arguments) {
            Some(activation) => Runner::at(program, activation, Level::Frame),
            None => return step.failed(StepError::Refused),
        },
        KState::Runner(runner) => match woken(&mut step, runner, scratch) {
            Ok(runner) => runner,
            Err(error) => return step.failed(error),
        },
        KState::Born(_) | KState::Evaluating { .. } => return step.failed(StepError::Refused),
    };
    perform(step, runner)
}

/// What an evaluator asks for to call `callee` over `arguments`, a record of its parameters by
/// name: a frame running the callee's body, placed by the bit its return type derives.
pub fn call<'graph, 'here>(
    program: &'graph Program<'graph>,
    callee: KValue<'graph, 'here>,
    arguments: KValue<'graph, 'here>,
    use_: Use,
) -> Request<'graph, 'here, KBundle> {
    let returns =
        callee
            .as_callable()
            .and_then(Knotted::function)
            .and_then(|function| match program.types().node(function.ktype()) {
                TypeNode::KFunction { ret, .. } => Some(ret),
                _ => None,
            });
    Request {
        placement: returns.map_or(Placement::Shares, placement_of),
        use_,
        work: crate::scheduler::Work {
            step: run,
            state: KBirth::Call {
                program,
                callee,
                arguments,
            },
        },
    }
}

/// The derived placement bit: `Fresh` when the return type is `Number`, `Bool` or `Null` — the
/// types no value of which shares bytes with an argument — and `Shares` otherwise.
pub fn placement_of(returns: KType) -> Placement {
    if [KType::NUMBER, KType::BOOL, KType::NULL].contains(&returns) {
        Placement::Fresh
    } else {
        Placement::Shares
    }
}

impl<'graph, 'cell> Runner<'graph, 'cell> {
    fn at(
        program: &'graph Program<'graph>,
        activation: &'cell KActivation<'graph, 'cell>,
        level: Level,
    ) -> Self {
        Runner {
            program,
            activation,
            unit: 0,
            result: None,
            outer: None,
            level,
            stage: Stage::Next,
        }
    }

    fn shape(&self) -> &'graph BodyShape<'graph> {
        self.activation.shape()
    }

    fn current(&self) -> Unit {
        self.shape().units()[self.unit as usize]
    }

    /// Whether `unit`'s value is the body's: a frame's last statement, outside any module body.
    fn yields(&self, unit: Unit) -> bool {
        unit.last && self.level == Level::Frame && self.outer.is_none()
    }

    /// Where an evaluation this runner asks for lives: a `Fresh` tree child of the root at the top
    /// level, so what it allocates beside its value dies with it; a tenant of the frame in a called
    /// body, so what it builds is at the frame's `'here`.
    fn placement(&self) -> Placement {
        match self.level {
            Level::Top => Placement::Fresh,
            Level::Frame => Placement::Shares,
        }
    }

    /// Bind `slot` and, when `unit` yields the body's value, keep it.
    fn bind(&mut self, unit: Unit, slot: Slot, value: KValue<'graph, 'cell>) {
        self.activation
            .bind(slot, value)
            .expect("a unit binds its slots once, in the shape's order");
        if self.yields(unit) && self.result.is_none() {
            self.result = Some(value);
        }
    }
}

/// A frame's activation: laid down for `callee`, with every value parameter bound from
/// `arguments` and every type parameter bound to what the callee's `FOR ALL` group solves to.
/// `None` when the callee is no function, the arguments do not name its value parameters exactly,
/// or its group has no solution — which [`run`] turns into a refusal, as an arity mismatch is.
fn frame<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_>,
    program: &'graph Program<'graph>,
    callee: KValue<'graph, 'here>,
    arguments: KValue<'graph, 'here>,
) -> Option<&'here KActivation<'graph, 'here>> {
    let member = callee.as_callable()?;
    let function = member.function()?;
    let record = arguments.as_record()?;
    let shape = function.shape();
    let writer = step.writer();
    let types = program.types();
    // Solve the group first: every value parameter's declared type against the argument's carried
    // type, under one collector. A callee that binds no group — an unquantified function, or a
    // callable whose type is a shape — has nothing to solve and skips the walk, so its frame is
    // byte-for-byte what it was. `Bump::new` claims no chunk until something is put in it, so it
    // pays nothing for having one in reach either.
    let bump = Bump::new();
    let scratch = &bump;
    let mut solution = None;
    if let TypeNode::KFunction {
        quantifiers,
        params,
        ..
    } = types.node(function.ktype())
        && !quantifiers.is_empty()
    {
        let mut collector = Collector::new(scratch, quantifiers.len());
        for (name, declared) in params.iter() {
            let argument = record.field(name.symbol())?;
            admits_with(
                types,
                scratch,
                declared,
                argument.ktype(),
                Variance::Co,
                &mut collector,
            )
            .ok()?;
        }
        solution = Some(collector.solve(types).ok()?);
    }
    let activation = resident(
        writer,
        KActivation::of_callable(
            writer,
            shape,
            member,
            function.closure(),
            program.builtins(),
        ),
    );
    let mut parameters = 0;
    for slot in 0..shape.slots() {
        let slot = Slot(slot as u32);
        let name = shape.slot_name(slot);
        if shape.slot(name).map(|(_, at)| at) != Some(Position::PARAMETER) {
            continue;
        }
        let value = match name {
            BinderSymbol::Value(name) => {
                parameters += 1;
                *record.field(name.symbol())?
            }
            // A type parameter is bound by **name**: the shape's type channel reaches here
            // symbol-sorted, not in the order the `FOR ALL` group was written, so a positional
            // read would hand one variable another's solution. A name the map dropped takes
            // `Any` — every `FOR ALL` name's bound today — since there is nothing to solve for.
            BinderSymbol::Type(name) => {
                let solved = match function.canonical_quantifier(name) {
                    Some(canonical) => *solution.as_ref()?.get(canonical)?,
                    None => KType::ANY,
                };
                Value::Type(TypeValue::new(writer, solved, types))
            }
        };
        activation.bind(slot, value).ok()?;
    }
    (parameters == record.len()).then_some(activation)
}

/// A parked runner woken: the evaluation it asked for bound or kept, or the parts a tie named
/// supplied and the tie made.
fn woken<'graph, 'here, 'scratch>(
    step: &mut Taking<'_, 'graph, '_, 'here, 'scratch>,
    mut runner: Runner<'graph, 'here>,
    scratch: Option<&'scratch [Site]>,
) -> Result<Runner<'graph, 'here>, StepError> {
    let unit = runner.current();
    match runner.stage {
        Stage::Next => unreachable!("a runner parks only with an evaluation in flight"),
        Stage::Evaluation => {
            let received = step.results().next().ok_or(StepError::Unredeemable)??;
            match unit.work {
                UnitWork::Component(component) => {
                    let Received::Here(value) = received else {
                        unreachable!(
                            "a binding is asked for with `Keeps`, which never delivers to scratch"
                        )
                    };
                    let slot = runner.shape().components()[component.index()].members[0];
                    runner.bind(unit, slot, value);
                }
                UnitWork::Statement(_) => {
                    if runner.yields(unit)
                        && let Received::Here(value) = received
                    {
                        runner.result = Some(value);
                    }
                }
            }
        }
        Stage::Supplying => {
            #[cfg(test)]
            SUPPLIED_WAKES.with(|wakes| wakes.set(wakes.get() + 1));
            let Some(sites) = scratch else {
                unreachable!("a supplying runner parks the sites it asked for")
            };
            let UnitWork::Component(component) = unit.work else {
                unreachable!("only a component's tie names eager parts")
            };
            let local = Bump::new();
            let mut supplied: BumpVec<'_, (Site, KValue<'graph, 'here>)> =
                BumpVec::with_capacity_in(sites.len(), &local);
            for (site, received) in sites.iter().zip(step.results()) {
                let Received::Here(value) = received? else {
                    unreachable!("an eager part is asked for with `Keeps`")
                };
                supplied.push((*site, value));
            }
            supplied.sort_unstable_by_key(|(site, _)| *site);
            let component = &runner.shape().components()[component.index()];
            tied(step, &mut runner, unit, component, &supplied)?;
        }
    }
    runner.unit += 1;
    runner.stage = Stage::Next;
    Ok(runner)
}

/// Tie `component` again with `supplied` answering every eager part it names by site, and bind
/// every member from the knot.
fn tied<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_>,
    runner: &mut Runner<'graph, 'here>,
    unit: Unit,
    component: &Component<'graph>,
    supplied: &[(Site, KValue<'graph, 'here>)],
) -> Result<(), StepError> {
    let scratch = Bump::new();
    let tied = tie(
        step.writer(),
        runner.activation,
        component,
        runner.program.types(),
        &scratch,
        &mut |site, _| {
            let at = supplied
                .binary_search_by_key(&site, |(supplied, _)| *supplied)
                .ok()?;
            Some(Supplied::Value(supplied[at].1))
        },
    );
    match tied {
        Ok(knot) => {
            for (index, slot) in component.members.iter().enumerate() {
                runner.bind(unit, *slot, Value::Knotted(Knotted::of(knot, index)));
            }
            Ok(())
        }
        Err(Untieable::Eager { .. }) => {
            unreachable!("a tie asks for every eager part at once, and every one was supplied")
        }
        Err(_) => Err(StepError::Refused),
    }
}

/// Perform units until one parks or the body ends.
fn perform<'graph, 'here>(
    mut step: Taking<'_, 'graph, '_, 'here, '_>,
    mut runner: Runner<'graph, 'here>,
) -> Action<'graph, KBundle> {
    loop {
        let shape = runner.shape();
        if runner.unit as usize == shape.units().len() {
            match runner.outer {
                Some(outer) => match left(&step, runner, outer) {
                    Ok(resumed) => runner = resumed,
                    Err(error) => return step.failed(error),
                },
                None => return ended(step, runner),
            }
            continue;
        }
        let unit = runner.current();
        let component = match unit.work {
            UnitWork::Statement(statement) => {
                let node = Evaluated::Statement(&shape.body()[statement as usize]);
                let use_ = if runner.yields(unit) {
                    Use::Forwards
                } else {
                    Use::Reads
                };
                let asked = ask(&mut step, &runner, node, use_);
                runner.stage = Stage::Evaluation;
                return step.park(asked, run, KState::Runner(runner), None);
            }
            UnitWork::Component(component) => &shape.components()[component.index()],
        };
        let types_only = component
            .members
            .iter()
            .any(|slot| matches!(shape.slot_name(*slot), BinderSymbol::Type(_)));
        if types_only {
            if let Err(error) = declared(&step, &mut runner, unit, component) {
                return step.failed(error);
            }
            runner.unit += 1;
            continue;
        }
        if let [slot] = component.members
            && shape
                .births(*slot)
                .is_some_and(|body| body.kind() == ShapeKind::Module)
        {
            runner = entered(&step, runner, *slot);
            continue;
        }
        let births = component
            .members
            .iter()
            .all(|slot| shape.births(*slot).is_some());
        if component.cyclic || births {
            match first_tie(&mut step, &mut runner, unit, component) {
                Ok(None) => {
                    runner.unit += 1;
                    continue;
                }
                Ok(Some((asked, sites))) => {
                    runner.stage = Stage::Supplying;
                    return step.park(asked, run, KState::Runner(runner), Some(sites));
                }
                Err(error) => return step.failed(error),
            }
        }
        let [slot] = component.members else {
            return step.failed(StepError::Refused);
        };
        let Some(rhs) = shape.rhs(*slot) else {
            return step.failed(StepError::Refused);
        };
        let asked = ask(&mut step, &runner, Evaluated::Part(rhs), Use::Keeps);
        runner.stage = Stage::Evaluation;
        return step.park(asked, run, KState::Runner(runner), None);
    }
}

/// Ask for one evaluation of `node` through the program's `evaluate`, at the runner's placement.
fn ask<'graph, 'here>(
    step: &mut Taking<'_, 'graph, '_, 'here, '_>,
    runner: &Runner<'graph, 'here>,
    node: Evaluated<'graph>,
    use_: Use,
) -> Asked {
    let work = runner.program.evaluate(node, runner.activation.view());
    step.spawn(Request {
        placement: runner.placement(),
        use_,
        work,
    })
}

/// A component's first tie: bound, or one evaluation asked for per eager part it named, beside the
/// sites laid down in scratch for the wake.
fn first_tie<'graph, 'here, 'scratch>(
    step: &mut Taking<'_, 'graph, '_, 'here, 'scratch>,
    runner: &mut Runner<'graph, 'here>,
    unit: Unit,
    component: &Component<'graph>,
) -> Result<Option<(Asked, &'scratch [Site])>, StepError> {
    let scratch = Bump::new();
    let mut missed: BumpVec<'_, (Site, &'graph ExpressionPart<'graph>)> = BumpVec::new_in(&scratch);
    let tied = tie(
        step.writer(),
        runner.activation,
        component,
        runner.program.types(),
        &scratch,
        &mut |site, part| {
            missed.extend(part.map(|part| (site, part)));
            None
        },
    );
    match tied {
        Ok(knot) => {
            for (index, slot) in component.members.iter().enumerate() {
                runner.bind(unit, *slot, Value::Knotted(Knotted::of(knot, index)));
            }
            Ok(None)
        }
        Err(Untieable::Eager { .. }) if !missed.is_empty() => {
            let sites = step.scratch_writer().fill(missed.len(), |at| missed[at].0);
            let mut asked = None;
            for (_, part) in missed.iter() {
                asked = Some(ask(step, runner, Evaluated::Part(part), Use::Keeps));
            }
            let asked = asked.expect("a tie refused on an eager part named one");
            Ok(Some((asked, sites)))
        }
        Err(_) => Err(StepError::Refused),
    }
}

/// Declare a component of type binders through the elaborator's door, and bind each member.
fn declared<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_>,
    runner: &mut Runner<'graph, 'here>,
    unit: Unit,
    component: &Component<'graph>,
) -> Result<(), StepError> {
    let scratch = Bump::new();
    let types = runner.program.types();
    let handles = type_declarations(component, runner.activation, types, &scratch)
        .map_err(|_| StepError::Refused)?;
    let writer = step.writer();
    for (slot, handle) in component.members.iter().zip(handles) {
        runner.bind(
            unit,
            *slot,
            Value::Type(TypeValue::new(writer, *handle, types)),
        );
    }
    Ok(())
}

/// Enter the body the module binder `slot` births: its activation laid down in the running region,
/// the enclosing body's place kept beside it.
fn entered<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_>,
    runner: Runner<'graph, 'here>,
    slot: Slot,
) -> Runner<'graph, 'here> {
    let writer = step.writer();
    let scratch = Bump::new();
    let body = resident(
        writer,
        body_activation(writer, runner.activation, slot, &scratch),
    );
    let outer = resident(
        writer,
        Outer {
            activation: runner.activation,
            unit: runner.unit,
            result: runner.result,
            binder: slot,
            outer: runner.outer,
        },
    );
    Runner {
        activation: body,
        unit: 0,
        result: None,
        outer: Some(outer),
        ..runner
    }
}

/// A module body ended: tie its binder from the finished body and resume the enclosing body past
/// it.
fn left<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_>,
    body: Runner<'graph, 'here>,
    outer: &'here Outer<'graph, 'here>,
) -> Result<Runner<'graph, 'here>, StepError> {
    let scratch = Bump::new();
    let activation = outer.activation;
    let knot = tie(
        step.writer(),
        activation,
        activation.shape().component_of(outer.binder),
        body.program.types(),
        &scratch,
        &mut |_, _| Some(Supplied::Body(body.activation)),
    )
    .map_err(|_| StepError::Refused)?;
    let mut resumed = Runner {
        activation,
        unit: outer.unit,
        result: outer.result,
        outer: outer.outer,
        ..body
    };
    let unit = resumed.current();
    resumed.bind(unit, outer.binder, Value::Knotted(Knotted::of(knot, 0)));
    resumed.unit += 1;
    Ok(resumed)
}

/// The body's end: the top level leaves its activation's view at rest for a later root work, and a
/// frame finishes with its value.
fn ended<'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_>,
    runner: Runner<'graph, 'here>,
) -> Action<'graph, KBundle> {
    match runner.level {
        Level::Top => step.leave(KBirth::Inspect {
            program: runner.program,
            view: runner.activation.view(),
        }),
        Level::Frame => step.finish(runner.result.unwrap_or(Value::Null)),
    }
}
