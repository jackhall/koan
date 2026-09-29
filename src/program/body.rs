//! The body runner: the one step that performs a body's units, in the order its shape emitted
//! them — the top level's, as a tenant of the root born as each call's root work, and a called
//! body's, the code an `EVAL` runs, or a block's, in the frame's own cell.
//!
//! The runner claims nothing ahead and no unit has a cell of its own. A unit of type binders is
//! declared through the elaborator's door and bound in the same step; a module binder's body runs
//! inline, its activation laid down in the running region and the enclosing body's place kept in a
//! resident [`Outer`]; a component the knot ties is tied, and a refusal naming eager parts becomes
//! one evaluation per part, all asked for at one park; a lone `LET` and a statement that binds
//! nothing are one evaluation each. The only children the runner asks for are evaluations, through
//! the program record's `evaluate`. See [README.md § The body runner](README.md#the-body-runner).
//!
//! **Error values.** A unit that receives an error value, or that the program refuses — a tie or a
//! declaration refused, a callee that is no function, arguments that do not fit it — ends the body
//! with an error value: a frame finishes with it, and the top level writes `error: <message>` to
//! the error sink, marks the run uncaught, and leaves its view at rest as it always does.
//! [`StepError::Refused`] is left for invariant breaks alone.
//!
//! **Frames end under a contract.** A called frame owes its caller a value satisfying its declared
//! return, with its own type-parameter solution substituted, retyped to it; a miss is an error
//! value. When the frame's last unit is a statement that binds nothing, the runner **tails** into
//! the evaluator with that [`Contract`], and the evaluation owes it instead. An `EVAL`'s frame tails
//! with none; a block never tails.

use std::fmt;

use crate::elaborate::type_declarations;
use crate::scope::Canonical;
use crate::knot::module::body_activation;
use crate::knot::{KActivation, KActivationView, KValue, Knotted, Supplied, Untieable, tie};
use crate::memory::{Bump, BumpVec, resident};
use crate::parse::ExpressionPart;
use crate::scheduler::{
    Action, Placement, Received, Request, Slot as Asked, Step, StepError, Taken, Use,
};
use crate::scope::{BodyShape, Component, Position, ShapeKind, Site, Slot, Unit, UnitWork};
use crate::scope::{CaptureSource, ClosureBindings, ShapeError};
use crate::symbols::{BinderSymbol, SymbolInterner, TypeSymbol};
use crate::type_lattice::{
    Collector, KType, TypeNode, TypeRegistry, Variance, admits_with, display_name,
    substitute_quantified,
};
use crate::values::{Link, List, TypeValue, Value};

use super::bundle::{KBirth, KBundle, KState};
use super::record::{CallKind, Contract, Evaluated, Program, rendered};

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
    /// What a called frame owes its caller; `None` everywhere else.
    contract: Option<Contract>,
    stage: Stage,
}

/// Whether the runner performs the top level, a frame — a called body or the code an `EVAL` runs
/// — or a block.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    Top,
    Frame,
    Block,
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

/// Why a unit could not go on: the program raised an error value with this message, or an
/// invariant broke.
enum Stopped<'a> {
    Raised(&'a str),
    Broken(StepError),
}

impl From<StepError> for Stopped<'_> {
    fn from(error: StepError) -> Self {
        Stopped::Broken(error)
    }
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

/// The step: the one body runner. Born as the top level's root work, as a call's or an `EVAL`'s
/// frame, or as a block.
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
            Runner::at(program, activation, Level::Top, None)
        }
        KState::Born(KBirth::Call {
            program,
            callee,
            arguments,
            kind,
        }) => match frame(&step, program, callee, arguments, kind) {
            Ok((activation, contract)) => {
                Runner::at(program, activation, Level::Frame, Some(contract))
            }
            Err(message) => {
                let error = program.error(step.writer(), message);
                return step.finish(error);
            }
        },
        KState::Born(KBirth::Eval {
            program,
            code,
            offered,
        }) => match code_frame(&step, program, code, offered) {
            Some(activation) => Runner::at(program, activation, Level::Frame, None),
            None => return step.failed(StepError::Refused),
        },
        KState::Born(KBirth::Block {
            program,
            shape,
            enclosing,
        }) => {
            let writer = step.writer();
            let enclosing = resident(writer, enclosing);
            let activation = resident(writer, KActivation::of_block(writer, shape, enclosing));
            Runner::at(program, activation, Level::Block, None)
        }
        KState::Runner(mut runner) => match woken(&mut step, &mut runner, scratch) {
            Ok(()) => runner,
            Err(stopped) => return stop(step, runner, stopped),
        },
        KState::Born(_) | KState::Evaluating { .. } => return step.failed(StepError::Refused),
    };
    perform(step, runner)
}

/// What an evaluator asks for to call `callee` over `arguments`, a record of its parameters by
/// name — and, for a quantified callee a keyworded call selected, of its type parameters, each a
/// type value: a frame running the callee's body, placed by the bit its return type derives.
/// `kind` says whether the arguments were admitted before the call or are checked by the frame.
pub fn call<'graph, 'here>(
    program: &'graph Program<'graph>,
    callee: KValue<'graph, 'here>,
    arguments: KValue<'graph, 'here>,
    kind: CallKind,
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
                kind,
            },
        },
    }
}

/// What an evaluator asks for to run the block `shape`, a synthesized block part's, beside
/// `enclosing`, the view it sits in: a tenant of the asker, since the view borrows its region.
pub fn block<'graph, 'here>(
    program: &'graph Program<'graph>,
    shape: &'graph BodyShape<'graph>,
    enclosing: KActivationView<'graph, 'here>,
    use_: Use,
) -> Request<'graph, 'here, KBundle> {
    debug_assert_eq!(shape.kind(), ShapeKind::Block);
    Request {
        placement: Placement::Shares,
        use_,
        work: crate::scheduler::Work {
            step: run,
            state: KBirth::Block {
                program,
                shape,
                enclosing,
            },
        },
    }
}

/// Why an `EVAL` refused to run its code.
#[derive(Clone, Copy, Debug)]
pub enum CodeRefused<'graph> {
    /// The code's shape, built where the program loaded, kept this error.
    Shape(&'graph ShapeError<'graph>),
    /// A hole no `USING` filled — a name, or a key some use selects from alone — or a `\` name the
    /// `EVAL` does not offer: the first in the shape's capture order.
    Unbound(BinderSymbol),
}

impl<'graph> CodeRefused<'graph> {
    /// The refusal as an error value's message: the shape's own error, or the unbound name or key.
    pub fn display<'x, 'run>(
        &'x self,
        symbols: &'x SymbolInterner,
        types: &'x TypeRegistry<'run>,
    ) -> CodeRefusedDisplay<'x, 'graph, 'run> {
        CodeRefusedDisplay {
            refused: self,
            symbols,
            types,
        }
    }
}

/// A [`CodeRefused`] beside the interner and registry it renders through.
pub struct CodeRefusedDisplay<'x, 'graph, 'run> {
    refused: &'x CodeRefused<'graph>,
    symbols: &'x SymbolInterner,
    types: &'x TypeRegistry<'run>,
}

impl fmt::Display for CodeRefusedDisplay<'_, '_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.refused {
            CodeRefused::Shape(error) => write!(f, "{}", error.display(self.symbols, self.types)),
            CodeRefused::Unbound(BinderSymbol::Key(key)) => {
                write!(f, "unbound key ({})", self.symbols.display(key.symbol()))
            }
            CodeRefused::Unbound(name) => {
                write!(f, "unbound name '{}'", self.symbols.display(name.symbol()))
            }
        }
    }
}

/// What an evaluator asks for to run `code` under `EVAL`, `offered` a record of the names and keys
/// the `EVAL` offers — a key's field a list of its functions: a frame running the code's shape,
/// which shares its operands' storage. Refused before anything is spawned when the code's shape
/// kept an error, or a name it reads — or a keyworded hole some use of it selects from alone — is
/// bound neither by a `USING` nor by `offered`. Nothing builds a shape here: the code's was built
/// where the program loaded.
pub fn eval<'graph, 'here>(
    program: &'graph Program<'graph>,
    code: Knotted<'graph, 'here>,
    offered: KValue<'graph, 'here>,
    use_: Use,
) -> Result<Request<'graph, 'here, KBundle>, CodeRefused<'graph>> {
    let node = code.code().expect("an `EVAL` is handed a quote's code");
    let shape = node.shape();
    if let Some(error) = shape.refusal() {
        return Err(CodeRefused::Shape(error));
    }
    let offers = |name: BinderSymbol| {
        offered
            .as_record()
            .is_some_and(|record| record.field(name.symbol()).is_some())
    };
    for capture in shape.captures() {
        let bound = match capture.source {
            CaptureSource::Read(_) | CaptureSource::Member { .. } => true,
            CaptureSource::Hole => {
                node.supplied()
                    .iter()
                    .any(|(name, _)| *name == capture.name)
                    || matches!(capture.name, BinderSymbol::Key(key) if shape.required_holes().binary_search(&key).is_err())
            }
            CaptureSource::Offered => offers(capture.name),
        };
        if !bound {
            return Err(CodeRefused::Unbound(capture.name));
        }
    }
    Ok(Request {
        placement: Placement::Shares,
        use_,
        work: crate::scheduler::Work {
            step: run,
            state: KBirth::Eval {
                program,
                code: Value::Knotted(code),
                offered,
            },
        },
    })
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
        contract: Option<Contract>,
    ) -> Self {
        Runner {
            program,
            activation,
            unit: 0,
            result: None,
            outer: None,
            level,
            contract,
            stage: Stage::Next,
        }
    }

    fn shape(&self) -> &'graph BodyShape<'graph> {
        self.activation.shape()
    }

    fn current(&self) -> Unit {
        self.shape().units()[self.unit as usize]
    }

    /// Whether `unit`'s value is the body's: a frame's or a block's last statement, outside any
    /// module body.
    fn yields(&self, unit: Unit) -> bool {
        unit.last && self.level != Level::Top && self.outer.is_none()
    }

    /// Whether `unit` is a frame's last, a statement binding nothing whose value is the frame's —
    /// which the runner hands to the evaluator by a tail rather than waiting on.
    fn tails(&self, unit: Unit) -> bool {
        self.level == Level::Frame
            && self.yields(unit)
            && self.unit as usize + 1 == self.shape().units().len()
            && matches!(unit.work, UnitWork::Statement(_))
    }

    /// Where an evaluation this runner asks for lives: a `Fresh` tree child of the root at the top
    /// level, so what it allocates beside its value dies with it; a tenant of the frame in a called
    /// body or a block, so what it builds is at the frame's `'here`.
    fn placement(&self) -> Placement {
        match self.level {
            Level::Top => Placement::Fresh,
            Level::Frame | Level::Block => Placement::Shares,
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

    /// The top level's activation, under every module body being run inline.
    fn outermost(&self) -> &'cell KActivation<'graph, 'cell> {
        let mut activation = self.activation;
        let mut outer = self.outer;
        while let Some(enclosing) = outer {
            activation = enclosing.activation;
            outer = enclosing.outer;
        }
        activation
    }

    /// Stop with an error value if `value` is one.
    fn check<'at>(&self, value: &KValue<'graph, 'at>) -> Result<(), Stopped<'at>> {
        match self.program.message(value) {
            Some(message) => Err(Stopped::Raised(message)),
            None => Ok(()),
        }
    }
}

/// A frame's activation, laid down for `callee` with every value parameter bound from `arguments`
/// and every type parameter bound to its solution — the one a keyworded call's selection carried in
/// `arguments`, or, for a call by name, the one solved here while each argument is admitted against
/// its parameter's declared type — beside the contract the frame ends under. The error value's
/// message when the callee is no function, the arguments do not name its parameters exactly, an
/// argument does not fit its parameter, or the group has no solution.
fn frame<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_>,
    program: &'graph Program<'graph>,
    callee: KValue<'graph, 'here>,
    arguments: KValue<'graph, 'here>,
    kind: CallKind,
) -> Result<(&'here KActivation<'graph, 'here>, Contract), &'here str> {
    let types = program.types();
    let writer = step.writer();
    let name = |handle| display_name(handle, types, program.symbols());
    let Some((member, function)) = callee
        .as_callable()
        .and_then(|member| Some((member, member.function()?)))
    else {
        return Err(rendered(
            writer,
            format_args!("{} is not callable", name(callee.ktype())),
        ));
    };
    let misnamed = || {
        rendered(
            writer,
            format_args!(
                "arguments {} do not name the parameters of {}",
                name(arguments.ktype()),
                name(function.ktype())
            ),
        )
    };
    let unfit = || {
        rendered(
            writer,
            format_args!(
                "{} cannot be called with {}",
                name(function.ktype()),
                name(arguments.ktype())
            ),
        )
    };
    let unsolved = || {
        rendered(
            writer,
            format_args!(
                "{} cannot be solved against {}",
                name(function.ktype()),
                name(arguments.ktype())
            ),
        )
    };
    let record = arguments.as_record().ok_or_else(misnamed)?;
    let shape = function.shape();
    let TypeNode::KFunction {
        quantifiers,
        params,
        ret,
        ..
    } = types.node(function.ktype())
    else {
        unreachable!("a function's type is a function type")
    };
    // `Bump::new` claims no chunk until something is put in it, so a callee with no group pays
    // nothing for having one in reach.
    let bump = Bump::new();
    let scratch = &bump;
    let carried = |name: TypeSymbol| {
        record
            .field(name.symbol())
            .and_then(|value| value.as_type())
            .map(|value| value.handle())
    };
    // The group's solution in canonical order. A keyworded call's selection carried it by name; a
    // call by name's is solved here, every value parameter's declared type against its argument's
    // carried type under one collector — which, for an unquantified callee, is the arguments'
    // admission alone.
    let mut solution: Option<BumpVec<'_, KType>> = None;
    match kind {
        CallKind::Keyworded if !quantifiers.is_empty() => {
            let mut solved = BumpVec::with_capacity_in(quantifiers.len(), scratch);
            solved.resize(quantifiers.len(), KType::NEVER);
            for (name, canonical) in function.quantifier_map() {
                if let Canonical::At(canonical) = canonical {
                    solved[*canonical] = carried(*name).ok_or_else(misnamed)?;
                }
            }
            solution = Some(solved);
        }
        CallKind::Keyworded => {}
        CallKind::ByName => {
            let mut collector = Collector::new(scratch, quantifiers.len());
            for (parameter, declared) in params.iter() {
                let argument = record.field(parameter.symbol()).ok_or_else(misnamed)?;
                admits_with(
                    types,
                    scratch,
                    declared,
                    argument.ktype(),
                    Variance::Co,
                    &mut collector,
                )
                .map_err(|_| unfit())?;
            }
            if !quantifiers.is_empty() {
                solution = Some(collector.solve(types).map_err(|_| unsolved())?);
            }
        }
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
    for slot in (0..shape.slots()).map(|slot| Slot(slot as u32)) {
        let name = shape.slot_name(slot);
        if shape.slot(name).map(|(_, at)| at) != Some(Position::PARAMETER) {
            continue;
        }
        let value = match name {
            BinderSymbol::Value(name) => {
                parameters += 1;
                *record.field(name.symbol()).ok_or_else(misnamed)?
            }
            // A type parameter is bound by **name**: the shape's type channel reaches here
            // symbol-sorted, not in the order the `FOR ALL` group was written, so a positional
            // read would hand one variable another's solution. A name the map dropped takes its
            // bound, since there is nothing to solve for. Only a keyworded call's arguments carry
            // type parameters, so a call by name's that names one does not name its parameters.
            BinderSymbol::Type(name) => {
                if kind == CallKind::Keyworded && carried(name).is_some() {
                    parameters += 1;
                }
                let solved = match function.canonical_quantifier(name) {
                    Some(Canonical::At(canonical)) => solution
                        .as_ref()
                        .and_then(|solution| solution.get(canonical).copied())
                        .expect("a quantified callee's group is solved"),
                    Some(Canonical::Dropped { bound }) => bound,
                    // A type-class name the group does not declare has nothing to solve it from.
                    None => KType::ANY,
                };
                Value::Type(TypeValue::new(writer, solved, types))
            }
            BinderSymbol::Registration(_) | BinderSymbol::Key(_) => {
                unreachable!("a parameter is a written name")
            }
        };
        activation
            .bind(slot, value)
            .expect("a fresh frame binds each parameter once");
    }
    if parameters != record.len() {
        return Err(misnamed());
    }
    let returns = match &solution {
        Some(solution) => substitute_quantified(types, scratch, ret, solution),
        None => ret,
    };
    let contract = Contract {
        callee: function.ktype(),
        returns,
    };
    Ok((activation, contract))
}

/// An `EVAL`'s frame: the code's activation over a closure run assembled in its shape's capture
/// order — each `$` name from the bindings the code carries, each hole from those `USING` supplied,
/// each `\` name from `offered` — every edge resolved to the member it names, so the run holds value
/// words only. A keyworded hole nothing filled holds no function. `None` when a name is bound
/// nowhere, which [`eval`] refuses first.
fn code_frame<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_>,
    program: &'graph Program<'graph>,
    code: KValue<'graph, 'here>,
    offered: KValue<'graph, 'here>,
) -> Option<&'here KActivation<'graph, 'here>> {
    let member = code.as_code()?;
    let node = member.code()?;
    let shape = node.shape();
    let record = offered.as_record()?;
    let find = |run: &[(BinderSymbol, Link<'here, Knotted<'graph, 'here>>)], name| {
        let at = run.binary_search_by_key(&name, |(held, _)| *held).ok()?;
        Some(run[at].1.resolve(member))
    };
    let bump = Bump::new();
    let writer = step.writer();
    let mut links = BumpVec::with_capacity_in(shape.captures().len(), &bump);
    for capture in shape.captures() {
        let value = match capture.source {
            CaptureSource::Read(_) | CaptureSource::Member { .. } => {
                find(node.bound(), capture.name)?
            }
            CaptureSource::Hole => match find(node.supplied(), capture.name) {
                Some(value) => value,
                None if matches!(capture.name, BinderSymbol::Key(_)) => {
                    Value::List(List::new(writer, [].into_iter(), program.types(), &bump))
                }
                None => return None,
            },
            CaptureSource::Offered => *record.field(capture.name.symbol())?,
        };
        links.push(Link::Value(value));
    }
    let closure = ClosureBindings::of(writer, &links);
    Some(resident(
        writer,
        KActivation::of_code(writer, shape, closure, program.builtins()),
    ))
}

/// A parked runner woken: the evaluation it asked for bound or kept, or the parts a tie named
/// supplied and the tie made. An error value received stops it.
fn woken<'graph, 'here, 'scratch>(
    step: &mut Taking<'_, 'graph, '_, 'here, 'scratch>,
    runner: &mut Runner<'graph, 'here>,
    scratch: Option<&'scratch [Site]>,
) -> Result<(), Stopped<'scratch>> {
    let unit = runner.current();
    match runner.stage {
        Stage::Next => unreachable!("a runner parks only with an evaluation in flight"),
        Stage::Evaluation => {
            let received = step.results().next().ok_or(StepError::Unredeemable)??;
            match received {
                Received::Here(value) => runner.check(&value)?,
                Received::Scratch(value) => runner.check(&value)?,
            }
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
                runner.check(&value)?;
                supplied.push((*site, value));
            }
            supplied.sort_unstable_by_key(|(site, _)| *site);
            let component = &runner.shape().components()[component.index()];
            tied(step, runner, unit, component, &supplied)?;
        }
    }
    runner.unit += 1;
    runner.stage = Stage::Next;
    Ok(())
}

/// The message of a tie the program refused, rendered into the running region.
fn untied<'here>(
    step: &Taking<'_, '_, '_, 'here, '_>,
    runner: &Runner<'_, 'here>,
    error: &Untieable<'_>,
) -> Stopped<'here> {
    let program = runner.program;
    Stopped::Raised(rendered(
        step.writer(),
        error.display(program.symbols(), program.types()),
    ))
}

/// Tie `component` again with `supplied` answering every eager part it names by site, and bind
/// every member from the knot.
fn tied<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_>,
    runner: &mut Runner<'graph, 'here>,
    unit: Unit,
    component: &Component<'graph>,
    supplied: &[(Site, KValue<'graph, 'here>)],
) -> Result<(), Stopped<'here>> {
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
        Err(error) => Err(untied(step, runner, &error)),
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
                    Err(stopped) => return stop(step, runner, stopped),
                },
                None => return ended(step, runner),
            }
            continue;
        }
        let unit = runner.current();
        let component = match unit.work {
            UnitWork::Statement(statement) => {
                let node = Evaluated::Statement(&shape.body()[statement as usize]);
                if runner.tails(unit) {
                    let program = runner.program;
                    let work = program.evaluate(node, runner.activation.view(), runner.contract);
                    return step.tail(Placement::Shares, work);
                }
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
            if let Err(stopped) = declared(&step, &mut runner, unit, component) {
                return stop(step, runner, stopped);
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
                Err(stopped) => return stop(step, runner, stopped),
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
    let work = runner
        .program
        .evaluate(node, runner.activation.view(), None);
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
) -> Result<Option<(Asked, &'scratch [Site])>, Stopped<'here>> {
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
        Err(error) => Err(untied(step, runner, &error)),
    }
}

/// Declare a component of type binders through the elaborator's door, and bind each member.
fn declared<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_>,
    runner: &mut Runner<'graph, 'here>,
    unit: Unit,
    component: &Component<'graph>,
) -> Result<(), Stopped<'here>> {
    let scratch = Bump::new();
    let program = runner.program;
    let types = program.types();
    let writer = step.writer();
    let handles =
        type_declarations(component, runner.activation, types, &scratch).map_err(|error| {
            Stopped::Raised(rendered(writer, error.display(program.symbols(), types)))
        })?;
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
) -> Result<Runner<'graph, 'here>, Stopped<'here>> {
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
    .map_err(|error| untied(step, &body, &error))?;
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

/// The body's end: the top level leaves its activation's view at rest for a later root work, a
/// called frame finishes with its value held to its contract, and an `EVAL`'s frame or a block
/// with its value.
fn ended<'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_>,
    runner: Runner<'graph, 'here>,
) -> Action<'graph, KBundle> {
    let value = runner.result.unwrap_or(Value::Null);
    match (runner.level, runner.contract) {
        (Level::Top, _) => step.leave(KBirth::Inspect {
            program: runner.program,
            view: runner.activation.view(),
        }),
        (Level::Frame | Level::Block, None) => step.finish(value),
        (Level::Frame | Level::Block, Some(contract)) => {
            let value = runner.program.fulfilled(step.writer(), value, contract);
            step.finish(value)
        }
    }
}

/// A unit stopped. An error value ends the body — a frame or a block finishes with it; the top
/// level writes it to the error sink, marks the run uncaught, and leaves its view at rest — and a
/// broken invariant fails the step.
fn stop<'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_>,
    runner: Runner<'graph, 'here>,
    stopped: Stopped<'_>,
) -> Action<'graph, KBundle> {
    let message = match stopped {
        Stopped::Raised(message) => message,
        Stopped::Broken(error) => return step.failed(error),
    };
    let program = runner.program;
    match runner.level {
        Level::Top => {
            (program.output().error)(rendered(step.writer(), format_args!("error: {message}")));
            program.uncaught().set(true);
            step.leave(KBirth::Inspect {
                program,
                view: runner.outermost().view(),
            })
        }
        Level::Frame | Level::Block => {
            let error = program.error(step.writer(), message);
            step.finish(error)
        }
    }
}
