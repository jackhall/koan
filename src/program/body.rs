//! The body runner: the one step that performs a body's units, in the order its shape emitted
//! them — the top level's, as a tenant of the root born as each call's root work, and a called
//! body's, the code an `EVAL` runs, or a block's, in the frame's own cell.
//!
//! The runner claims nothing ahead and no unit has a cell of its own. A unit of type binders is
//! bound to the handles the load pass fixed for it when they are closed, and otherwise declared
//! through the elaborator's door and bound in the same step; a module binder's body runs
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
//! **A declared parameter is an ascription.** A frame binds each value parameter to its argument
//! retyped to the parameter's declared type, with the call's solution substituted. So is an
//! annotated binder, `LET <name> <type> = <value>`: its value is checked against the type, unless
//! the load settled it, and retyped to it before it is bound.
//!
//! **Frames end under a contract.** A called frame owes its caller a value satisfying its declared
//! return, with its own type-parameter solution substituted, retyped to it; a miss is an error
//! value. When the frame's last unit is a statement that binds nothing, the runner **tails** into
//! the evaluator with that [`Contract`], and the evaluation owes it instead. A frame a tail hop
//! reached is checked against its own return and retyped to the one the hop owed, so a chain of
//! hops returns at the outermost contract. An `EVAL`'s frame owes the `EVAL`'s declared return as a
//! called frame owes its own; a block never tails.
//!
//! **A caching body interns in scratch.** Each step picks one chain ([`run`]). A caching body
//! keeps its blocks' and its last statement's values rather than forwarding them, so a value
//! leaves it only through an [`Exit`]: the frame's end, and — in a `Fresh` frame holding its
//! scratch — the hand-off of its last statement, which adopts the slots the statement reads and
//! the frame's contract. An invariant nominal's types are adopted into the warm tier.

use std::fmt;

use crate::elaborate::{declares_nominal, denoted, type_declarations};
use crate::knot::body_activation;
use crate::knot::module::coerce::{self, CoercionRefused};
use crate::knot::module::surface::surface;
use crate::knot::{KActivation, KActivationView, KValue, Knotted, Supplied, Tied, Untieable, tie};
use crate::memory::{Bump, BumpVec, Writer, resident};
use crate::parse::ExpressionPart;
use crate::scheduler::{
    Action, Placement, Received, Request, Slot as Asked, Step, StepError, Taken, Use,
};
use crate::scope::{BodyShape, Component, Mint, Position, ShapeKind, Site, Slot, Unit, UnitWork};
use crate::scope::{CaptureSource, ClosureBindings, ShapeError};
use crate::scope::{ParameterBinding, Static};
use crate::symbols::{BinderSymbol, Symbol, SymbolInterner, TypeSymbol};
use crate::type_lattice::{
    Collector, DeclaredType, DeclaredView, KType, Parametric, Record as TypeRecord, TypeNode,
    Variance, admits_with, display_name, substitute_quantified,
};
use crate::values::{BuildRef, CALL_ONLY, Chain, Link, List, Record, TypeValue, Value};

use super::bundle::{KBirth, KBundle, KState, Within};
use super::exit::Exit;
use super::record::{CallKind, Contract, Evaluated, Program, rendered};
use super::specialize::called_function;

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
    /// What a called frame or an `EVAL`'s owes; `None` everywhere else.
    contract: Option<Contract>,
    /// The outermost barrier a frame was called through, which its value crosses outwards.
    barrier: Option<Barrier<'graph, 'cell>>,
    stage: Stage,
    /// What the body runs within: what it hands each evaluation and block it asks for
    /// ([`KBirth::Evaluate`]), and the build cell a callable born in it is born under.
    within: Within,
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
type Taking<'a, 'graph, 'step, 'here, 'scratch, 'lent> =
    Step<'a, 'graph, 'step, 'here, 'scratch, 'lent, KBundle, Taken, Taken>;

/// The step: the one body runner. Born as the top level's root work, as a call's or an `EVAL`'s
/// frame, or as a block.
///
/// Each step runs on one chain ([type tiers](../../design/type-tiers.md#where-each-body-interns)):
/// the reading chain in the own body of a routing caching frame — a `Fresh` one, which holds its
/// scratch at its first step, or a `Shares` one borrowing a specialization; the `Shares` chain
/// elsewhere in a caching body; and its region's anywhere else.
pub fn run<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, KBundle>) -> Action<'graph, KBundle> {
    let (step, state) = step.state();
    let (mut step, scratch) = step.scratch();
    let shape = match &state {
        KState::Born(KBirth::Call { callee, .. }) => called_shape(*callee),
        KState::Born(KBirth::Eval { code, .. }) => (code.as_code())
            .and_then(|member| member.code())
            .map(|node| node.shape()),
        KState::Born(KBirth::Block { shape, .. }) => Some(*shape),
        KState::Runner(runner) => Some(runner.shape()),
        KState::Born(_) | KState::Evaluating { .. } => None,
    };
    let caches = shape.is_some_and(BodyShape::caches);
    let routes = match &state {
        // A tree cell holds its scratch whatever it borrows; a tenant routes where it borrows a
        // specialization, writing its host's scratch.
        KState::Born(KBirth::Call { .. }) => {
            caches && (step.hold_scratch() || step.borrows_specialization())
        }
        KState::Born(KBirth::Block { within, .. }) => within.routes,
        KState::Runner(runner) => runner.within.routes,
        KState::Born(_) | KState::Evaluating { .. } => false,
    };
    #[cfg(test)]
    if caches {
        let address = shape.map(|shape| std::ptr::from_ref(shape).addr());
        crate::scheduler::CACHING.set(address);
    }
    if routes {
        let types = step.reading_types().expect("a routing frame has a lender");
        ran(step, state, scratch, routes, &types)
    } else if caches {
        let types = step.shares_types();
        ran(step, state, scratch, false, &types)
    } else {
        let types = step.types();
        ran(step, state, scratch, false, &types)
    }
}

/// The shape a call of `callee` runs: its function's, behind every barrier. `None` for a callee
/// that is no function, which the frame refuses.
fn called_shape<'graph>(callee: KValue<'graph, '_>) -> Option<&'graph BodyShape<'graph>> {
    Some(called_function(callee)?.shape())
}

/// The build cell `callee`'s function was born under, behind every barrier: where a frame of it
/// that borrows no specialization runs.
fn born_under(callee: KValue<'_, '_>) -> BuildRef {
    called_function(callee).map_or(BuildRef::ROOT, |function| function.closure().build())
}

/// [`run`] on the chain `types` it picked; `routes` is a call's, taken at its frame's first step.
fn ran<'graph, 'here, 'scratch>(
    mut step: Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    state: KState<'graph, 'here>,
    scratch: Option<&'scratch [Site]>,
    routes: bool,
    types: &Chain<'graph, '_, 'here, '_, '_>,
) -> Action<'graph, KBundle> {
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
            contributed,
            owed,
        }) => match frame(
            &step,
            types,
            program,
            callee,
            arguments,
            kind,
            contributed,
            owed,
        ) {
            Ok((activation, contract, barrier)) => Runner {
                barrier,
                within: Within {
                    routes,
                    build: step.build_ref(born_under(callee)),
                },
                ..Runner::at(program, activation, Level::Frame, Some(contract))
            },
            // The message lies in the region, so the error value needs no adopt.
            Err(message) => {
                let region = step.types();
                let error = program.error(step.writer(), &region, message);
                return step.finish(error);
            }
        },
        KState::Born(KBirth::Eval {
            program,
            code,
            offered,
            contract,
        }) => match code_frame(&step, types, program, code, offered) {
            Some(activation) => Runner::at(program, activation, Level::Frame, Some(contract)),
            None => return step.failed(StepError::Refused),
        },
        KState::Born(KBirth::Block {
            program,
            shape,
            enclosing,
            module,
            within,
        }) => {
            let writer = step.writer();
            let enclosing = resident(writer, enclosing);
            let activation = resident(writer, KActivation::of_block(writer, shape, enclosing));
            // A `USING` body's parameters are the module's surfaced names, which its shape and the
            // module cannot disagree on: a refusal is an invariant break.
            if let Some(module) = module {
                let bump = Bump::new();
                if surface(writer, module, activation, types, &bump).is_err() {
                    return step.failed(StepError::Refused);
                }
            }
            Runner {
                within,
                ..Runner::at(program, activation, Level::Block, None)
            }
        }
        KState::Runner(mut runner) => match woken(&mut step, &mut runner, scratch, types) {
            Ok(()) => runner,
            Err(stopped) => return stop(step, runner, stopped),
        },
        KState::Born(_) | KState::Evaluating { .. } => return step.failed(StepError::Refused),
    };
    perform(step, runner, types)
}

/// What an evaluator asks for to call `callee` over `arguments`, a record of its parameters by name
/// — and, for a quantified callee a keyworded call selected, of its type parameters, each a type
/// value: a frame running the callee's body, placed by the bit its return type, read through
/// `types`, derives. `kind` says whether the arguments were admitted before the call or are checked
/// by the frame, and `contributed` what a call by name solves each parameter from, per parameter in
/// symbol order — empty, or `None` at a parameter, where the frame reads the carried type.
#[allow(clippy::too_many_arguments)]
pub fn call<'graph, 'here>(
    program: &'graph Program<'graph>,
    types: &Chain<'_, '_, '_, '_, '_>,
    callee: KValue<'graph, 'here>,
    arguments: KValue<'graph, 'here>,
    kind: CallKind,
    contributed: &'here [Option<KType>],
    owed: Option<Contract>,
    use_: Use,
) -> Request<'graph, 'here, KBundle> {
    // The return the caller sees: the value is retyped to it, a header's included.
    let returns = callee
        .as_callable()
        .filter(|member| member.function().is_some())
        .and_then(|_| match callee.ktype().view(types).body {
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
                contributed,
                owed,
            },
        },
    }
}

/// What an evaluator asks for to run the block `shape`, a synthesized block part's, beside
/// `enclosing`, the view it sits in: a tenant of the asker, since the view borrows its region.
/// `within` is [`KBirth::Block`]'s.
pub fn block<'graph, 'here>(
    program: &'graph Program<'graph>,
    shape: &'graph BodyShape<'graph>,
    enclosing: KActivationView<'graph, 'here>,
    use_: Use,
    within: Within,
) -> Request<'graph, 'here, KBundle> {
    surfaced_block(program, shape, enclosing, None, use_, within)
}

/// What an evaluator asks for to run `shape`, a `USING … SCOPE` body, inside `enclosing`, its
/// parameters bound to what `module` surfaces: [`block`]'s request, the block entered on the
/// module.
pub fn surfaced_block<'graph, 'here>(
    program: &'graph Program<'graph>,
    shape: &'graph BodyShape<'graph>,
    enclosing: KActivationView<'graph, 'here>,
    module: Option<KValue<'graph, 'here>>,
    use_: Use,
    within: Within,
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
                module,
                within,
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
    pub fn display<'x, 'g, 'run, 'p, 'reg>(
        &'x self,
        symbols: &'x SymbolInterner,
        types: &'x Chain<'g, 'run, 'p, 'reg, '_>,
    ) -> CodeRefusedDisplay<'x, 'graph, 'g, 'run, 'p, 'reg> {
        CodeRefusedDisplay {
            refused: self,
            symbols,
            types,
        }
    }
}

/// A [`CodeRefused`] beside the interner and registry it renders through.
pub struct CodeRefusedDisplay<'x, 'graph, 'g, 'run, 'p, 'reg> {
    refused: &'x CodeRefused<'graph>,
    symbols: &'x SymbolInterner,
    types: &'x Chain<'g, 'run, 'p, 'reg, 'x>,
}

impl fmt::Display for CodeRefusedDisplay<'_, '_, '_, '_, '_, '_> {
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
/// bound neither by a `USING` nor by `offered`, read through `types`. The frame owes `returns`, the
/// type the `EVAL` declares, as a called frame owes its own. Nothing builds a shape here: the
/// code's was built where the program loaded.
pub fn eval<'graph, 'here>(
    program: &'graph Program<'graph>,
    types: &Chain<'_, '_, '_, '_, '_>,
    code: Knotted<'graph, 'here>,
    offered: KValue<'graph, 'here>,
    returns: KType,
    use_: Use,
) -> Result<Request<'graph, 'here, KBundle>, CodeRefused<'graph>> {
    let node = code.code().expect("an `EVAL` is handed a quote's code");
    let shape = node.shape();
    if let Some(error) = shape.refusal() {
        return Err(CodeRefused::Shape(error));
    }
    let scratch = Bump::new();
    let offers = |name: BinderSymbol| offered.field(name.symbol(), types, &scratch).is_some();
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
                contract: Contract {
                    callee: None,
                    returns,
                    retype: returns,
                    checked: false,
                },
            },
        },
    })
}

/// The derived placement bit: `Fresh` when the return type is `Number`, `Bool` or `Null` — the
/// types no value of which shares bytes with an argument — and `Shares` otherwise.
pub fn placement_of(returns: impl Into<Parametric>) -> Placement {
    let returns = returns.into();
    if [KType::NUMBER, KType::BOOL, KType::NULL]
        .into_iter()
        .any(|leaf| returns == leaf.into())
    {
        Placement::Fresh
    } else {
        Placement::Shares
    }
}

/// Why a callee's position read under its solution is concrete: a function is born with no
/// variable outside its own group, and a run-time solution is concrete.
const SOLVED: &str = "a run-time callee holds only its own group's variables";

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
            barrier: None,
            stage: Stage::Next,
            within: Within::TOP,
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
    /// which the runner hands to the evaluator by a tail rather than waiting on. A frame called
    /// through a barrier never tails: its value crosses the barrier where the frame ends.
    fn tails(&self, unit: Unit) -> bool {
        self.level == Level::Frame
            && self.barrier.is_none()
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

    /// Stop with an error value if `value` is one, read through `types`.
    fn check<'at>(
        &self,
        value: &KValue<'graph, 'at>,
        types: &Chain<'_, '_, '_, '_, '_>,
    ) -> Result<(), Stopped<'at>> {
        match self.program.message(value, types) {
            Some(message) => Err(Stopped::Raised(message)),
            None => Ok(()),
        }
    }
}

/// A frame's activation, laid down for `callee` with every value parameter bound from `arguments`,
/// retyped to its declared type with the call's solution substituted, and every type parameter
/// bound to its solution — the one an instance carries, the one a keyworded call's selection
/// carried in `arguments`, or, for a call by name, the one solved here while each argument is
/// admitted against its parameter's declared type — at its entry in `contributed`, per parameter
/// in symbol order, and its carried type where there is none — beside the contract the frame ends
/// under, and the barrier its value crosses out through. The error value's message when the callee
/// is no function, the arguments do not name its parameters exactly, an argument does not fit its
/// parameter, or the group has no solution.
///
/// A callee has two types: the one it shows, `callee.ktype()`, and the one its body was checked
/// against, [`Knotted::declared`]. Where they differ — a header, or a member born at its annotation
/// — and through a barrier, the arguments are admitted and retyped at the shown parameters, the
/// node is called by name past them over the parameters it declares, and its value is retyped to
/// the shown return: through the contract, or past a barrier once it has crossed out.
#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
fn frame<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    types: &Chain<'graph, '_, 'here, '_, '_>,
    program: &'graph Program<'graph>,
    callee: KValue<'graph, 'here>,
    arguments: KValue<'graph, 'here>,
    kind: CallKind,
    contributed: &[Option<KType>],
    owed: Option<Contract>,
) -> Result<
    (
        &'here KActivation<'graph, 'here>,
        Contract,
        Option<Barrier<'graph, 'here>>,
    ),
    &'here str,
> {
    let writer = step.writer();
    let name = |handle| display_name(handle, types, program.symbols());
    let shown = callee.ktype();
    let Some(node) = callee.as_callable() else {
        return Err(rendered(
            writer,
            format_args!("{} is not callable", name(shown)),
        ));
    };
    // `Bump::new` claims no chunk until something is put in it, so a callee with no group pays
    // nothing for having one in reach.
    let bump = Bump::new();
    let scratch = &bump;
    let outermost = node.coerced().is_some().then_some(node);
    // A bare barrier admits its arguments as they cross it, at the type it shows.
    let header = shown != node.declared();
    let past = outermost.is_some() || header;
    let unfit_at = |handle: DeclaredType<KType>, refused: Option<&dyn fmt::Display>| {
        let refused = refused
            .map(|refused| format!(": {refused}"))
            .unwrap_or_default();
        rendered(
            writer,
            format_args!(
                "{} cannot be called with {}{refused}",
                name(handle),
                name(arguments.ktype())
            ),
        )
    };
    let misnamed_at = |handle: DeclaredType<KType>| {
        rendered(
            writer,
            format_args!(
                "arguments {} do not name the parameters of {}",
                name(arguments.ktype()),
                name(handle)
            ),
        )
    };
    // The shown side: admitted and retyped at the parameters the callee shows, and the return its
    // caller sees. A keyworded call's selection admitted its arguments at the shape shown already,
    // and carried the solution of its group beside them.
    let (arguments, contributed, shown_return) = match (past, kind) {
        (false, _) => (arguments, contributed, None),
        (true, CallKind::ByName) if !header => {
            (arguments, contributed, shown_returns(types, shown))
        }
        (true, CallKind::ByName) => {
            let (admitted, returns) =
                shown_admitted(writer, types, scratch, shown, arguments, contributed).map_err(
                    |miss| match miss {
                        Miss::Misnamed => misnamed_at(shown),
                        Miss::Unfit | Miss::Unsolved => unfit_at(shown, None),
                    },
                )?;
            (admitted, &[][..], Some(returns))
        }
        (true, CallKind::Keyworded) => {
            let returns = shown_keyworded_returns(types, scratch, shown, arguments);
            (arguments, contributed, returns)
        }
    };
    // A barrier stands before the function a call runs: the arguments cross each one inwards, and
    // the function behind them is called by name, admitting and solving as a call by name does.
    let (member, arguments, kind, contributed) = match outermost {
        Some(outermost) => {
            let crossed = crossed_in(writer, types, outermost, arguments).map_err(|refusal| {
                unfit_at(shown, Some(&refusal.display(types, program.symbols())))
            })?;
            (crossed.0, crossed.1, CallKind::ByName, &[][..])
        }
        None if past => (node, arguments, CallKind::ByName, &[][..]),
        None => (node, arguments, kind, contributed),
    };
    let Some(function) = member.function() else {
        return Err(rendered(
            writer,
            format_args!("{} is not callable", name(shown)),
        ));
    };
    let declared = member.declared();
    let misnamed = || misnamed_at(declared);
    let unfit = || unfit_at(declared, None);
    let unsolved = || {
        rendered(
            writer,
            format_args!(
                "{} cannot be solved against {}",
                name(declared),
                name(arguments.ktype())
            ),
        )
    };
    arguments.as_record().ok_or_else(misnamed)?;
    let shape = function.shape();
    let quantified = matches!(declared, DeclaredType::Scheme(_));
    let view = declared.view(types);
    let TypeNode::KFunction { params, ret } = view.body else {
        unreachable!("a function's type is a function type")
    };
    // Past the shown type, the node binds only the parameters it declares: a function type's
    // parameters are a width subset of what a slot of it may show.
    let arguments = match past {
        true => restricted(writer, types, scratch, arguments, params),
        false => arguments,
    };
    let argument = |name: Symbol| arguments.field(name, types, scratch).ok_or_else(misnamed);
    let carried = |name: TypeSymbol| {
        arguments
            .field(name.symbol(), types, scratch)
            .and_then(|seen| seen.value().as_type())
            .map(|value| value.handle())
    };
    // The group's solution in group order. A keyworded call's selection carried it by name; a
    // call by name's is solved here, every value parameter's declared type against its argument's
    // carried type under one collector — which, for an unquantified callee, is the arguments'
    // admission alone. An instance's group was solved where the load made it, and is never
    // solved again.
    let mut solution: Option<BumpVec<'_, KType>> = function.instance().map(|instance| {
        let mut solved = BumpVec::with_capacity_in(instance.len(), scratch);
        solved.extend_from_slice(instance);
        solved
    });
    debug_assert!(
        solution.is_none() || kind == CallKind::ByName,
        "an instance has no registration, so only a call by name reaches it"
    );
    match kind {
        CallKind::Keyworded if quantified => {
            let mut solved = BumpVec::with_capacity_in(view.group.names.len(), scratch);
            solved.resize(view.group.names.len(), KType::NEVER);
            for (name, index) in function.quantifier_map().iter() {
                solved[index] = carried(name).ok_or_else(misnamed)?;
            }
            solution = Some(solved);
        }
        CallKind::Keyworded => {}
        // A header's arguments were admitted at the parameters it shows, which the order puts
        // under the node's own where neither type is quantified: nothing is left to solve.
        CallKind::ByName
            if header
                && outermost.is_none()
                && !quantified
                && matches!(shown, DeclaredType::Type(_)) => {}
        CallKind::ByName => {
            let solved = admitted(types, scratch, view, arguments, contributed).map_err(
                |miss| match miss {
                    Miss::Misnamed => misnamed(),
                    Miss::Unfit => unfit(),
                    Miss::Unsolved => unsolved(),
                },
            )?;
            if quantified {
                solution = solved;
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
        if shape.position_of(slot) != Position::PARAMETER {
            continue;
        }
        let value = match name {
            // A declared parameter is an ascription: the argument is retyped to its declared
            // type, with the call's solution substituted, so the body dispatches on what the
            // parameter declares.
            BinderSymbol::Value(name) => {
                parameters += 1;
                let argument = argument(name.symbol())?;
                let declared = params
                    .get(name.symbol())
                    .expect("every value parameter is declared");
                let declared = match &solution {
                    Some(solution) => substitute_quantified(types, scratch, declared, solution),
                    None => declared,
                };
                let declared = types.concrete(declared).expect(SOLVED);
                debug_assert!(
                    argument.satisfies(declared, types, scratch),
                    "a frame binds an argument its parameter admits"
                );
                argument
                    .seen_at(declared, types, scratch)
                    .restamped(writer, types)
            }
            // A `:Type` parameter — a type-channel parameter no `FOR ALL` group declares — is an
            // argument like any other, passed by keyword or by name.
            BinderSymbol::Type(name) if function.quantifier_index(name).is_none() => {
                parameters += 1;
                argument(name.symbol())?.value()
            }
            // A `FOR ALL` name is bound by **name**: the shape's type channel reaches here
            // symbol-sorted, not in the order the group was written, so a positional read would
            // hand one variable another's solution. Only a keyworded call's arguments carry one, so
            // a call by name's that names one does not name its parameters.
            BinderSymbol::Type(name) => {
                if kind == CallKind::Keyworded && carried(name).is_some() {
                    parameters += 1;
                }
                let index = function
                    .quantifier_index(name)
                    .expect("a name no group declares is a `:Type` parameter");
                let solved = solution
                    .as_ref()
                    .and_then(|solution| solution.get(index).copied())
                    .expect("a quantified callee's group is solved");
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
    if parameters
        != arguments
            .surface(types, scratch)
            .map_or(0, |record| record.len())
    {
        return Err(misnamed());
    }
    let returns = match &solution {
        Some(solution) => substitute_quantified(types, scratch, ret, solution),
        None => ret,
    };
    let returns = types.concrete(returns).expect(SOLVED);
    // A barrier's value is retyped to the shown return once it crosses out; any other callee's
    // through its contract, where the order does not promise the node's return lies under the
    // shown one when either is quantified.
    let (retype, checked) = match (outermost, shown_return) {
        (None, Some(shown_return)) => (
            owed.map_or(shown_return, |outer| outer.retype),
            quantified || matches!(shown, DeclaredType::Scheme(_)),
        ),
        _ => (owed.map_or(returns, |outer| outer.retype), false),
    };
    let contract = Contract {
        callee: Some(declared),
        returns,
        retype,
        checked,
    };
    let barrier = outermost.map(|outermost| Barrier {
        outermost,
        returns: shown_return,
    });
    Ok((activation, contract, barrier))
}

/// The return `shown`, a callee's type, gives every call where its group leaves it concrete: what
/// a barrier's value is retyped to once it crosses out. `None` for a quantified return, which the
/// barrier's own crossing types.
fn shown_returns(types: &Chain<'_, '_, '_, '_, '_>, shown: DeclaredType<KType>) -> Option<KType> {
    match shown.view(types).body {
        TypeNode::ExpressionShape { ret, .. } | TypeNode::KFunction { ret, .. } => {
            types.concrete(ret)
        }
        _ => None,
    }
}

/// The return `shown`, the expression shape a keyworded call selected a header or a barrier at,
/// gives the call over `arguments`: at a scheme, under the solution the selection carried by the
/// shape's own names. `None` where it carried none.
fn shown_keyworded_returns(
    types: &Chain<'_, '_, '_, '_, '_>,
    scratch: &Bump,
    shown: DeclaredType<KType>,
    arguments: KValue<'_, '_>,
) -> Option<KType> {
    let DeclaredType::Scheme(_) = shown else {
        return shown_returns(types, shown);
    };
    let view = shown.view(types);
    let mut solution = BumpVec::with_capacity_in(view.group.names.len(), scratch);
    for name in view.group.names {
        let seen = arguments.field(name.symbol(), types, scratch)?;
        solution.push(seen.value().as_type()?.handle());
    }
    let TypeNode::ExpressionShape { ret, .. } = view.body else {
        return None;
    };
    types.concrete(substitute_quantified(types, scratch, ret, &solution))
}

/// Why arguments do not reach a frame: they do not name its parameters, one does not fit its
/// parameter, or the group has no solution.
enum Miss {
    Misnamed,
    Unfit,
    Unsolved,
}

/// Admit `arguments`, a call by name's record, against the parameters of `view`, a function type,
/// every value parameter's declared type against its argument's carried type — or what the load
/// contributed for it, per parameter in symbol order — under one collector: the group's solution
/// where it is quantified.
fn admitted<'x, 'tier: 'x>(
    types: &Chain<'_, 'tier, '_, '_, '_>,
    scratch: &'x Bump,
    view: DeclaredView<'_>,
    arguments: KValue<'_, '_>,
    contributed: &[Option<KType>],
) -> Result<Option<BumpVec<'x, KType>>, Miss> {
    let TypeNode::KFunction { params, .. } = view.body else {
        return Err(Miss::Misnamed);
    };
    let mut collector = Collector::<KType>::new(types, scratch, view.group.bounds);
    // The load laid the parameters of the callee it saw out in symbol order, and records
    // contributions only where its argument names no field but those: a record as long as both
    // that list and this callee's parameters, naming each (read below), names the same ones. Any
    // other call is misnamed.
    let named = (arguments.surface(types, scratch)).map_or(0, |record| record.len());
    if !contributed.is_empty() && (contributed.len() != params.len() || named != params.len()) {
        return Err(Miss::Misnamed);
    }
    for (parameter, declared) in params.iter() {
        let argument = arguments
            .field(parameter.symbol(), types, scratch)
            .ok_or(Miss::Misnamed)?;
        let carried = argument.ktype().as_type().expect(CALL_ONLY);
        let solved_from = match contributed {
            [] => carried,
            // The load laid the parameters out in symbol order: this one's place is how many
            // sort before it.
            _ => contributed[params
                .keys()
                .filter(|other| other.symbol() < parameter.symbol())
                .count()]
            .unwrap_or(carried),
        };
        admits_with(
            types,
            scratch,
            declared,
            solved_from,
            Variance::Co,
            &mut collector,
        )
        .map_err(|_| Miss::Unfit)?;
    }
    if view.group.names.is_empty() {
        return Ok(None);
    }
    collector.solve(types).map(Some).map_err(|_| Miss::Unsolved)
}

/// `arguments` admitted at `shown`, the function type a callee shows, each retyped to its shown
/// parameter under the solution, beside the shown return under it: what a call by name hands a
/// callee past the type it shows.
fn shown_admitted<'graph, 'here>(
    writer: Writer<'here, '_>,
    types: &Chain<'_, '_, 'here, '_, '_>,
    scratch: &Bump,
    shown: DeclaredType<KType>,
    arguments: KValue<'graph, 'here>,
    contributed: &[Option<KType>],
) -> Result<(KValue<'graph, 'here>, KType), Miss> {
    let view = shown.view(types);
    let TypeNode::KFunction { params, ret } = view.body else {
        return Err(Miss::Misnamed);
    };
    arguments.as_record().ok_or(Miss::Misnamed)?;
    let named = (arguments.surface(types, scratch)).map_or(0, |record| record.len());
    if named != params.len() {
        return Err(Miss::Misnamed);
    }
    let solution = admitted(types, scratch, view, arguments, contributed)?;
    let solved = |declared: Parametric| {
        let declared = match &solution {
            Some(solution) => substitute_quantified(types, scratch, declared, solution),
            None => declared,
        };
        types.concrete(declared).expect(SOLVED)
    };
    let mut fields = BumpVec::with_capacity_in(params.len(), scratch);
    for (parameter, declared) in params.iter() {
        let argument = arguments
            .field(parameter.symbol(), types, scratch)
            .ok_or(Miss::Misnamed)?;
        let retyped = argument
            .seen_at(solved(declared), types, scratch)
            .restamped(writer, types);
        fields.push((parameter, retyped));
    }
    let record = Record::new(writer, &fields, types, scratch);
    Ok((Value::Record(record), solved(ret)))
}

/// `arguments`, a record, holding only the fields `params` names: what a node called past the type
/// it shows binds, since it may declare fewer parameters than its slot shows.
fn restricted<'graph, 'here>(
    writer: Writer<'here, '_>,
    types: &Chain<'_, '_, 'here, '_, '_>,
    scratch: &Bump,
    arguments: KValue<'graph, 'here>,
    params: TypeRecord<'_, Parametric>,
) -> KValue<'graph, 'here> {
    let named = (arguments.surface(types, scratch)).map_or(0, |record| record.len());
    if named == params.len() {
        return arguments;
    }
    let mut fields = BumpVec::with_capacity_in(params.len(), scratch);
    for (parameter, _) in params.iter() {
        if let Some(seen) = arguments.field(parameter.symbol(), types, scratch) {
            fields.push((parameter, seen.restamped(writer, types)));
        }
    }
    Value::Record(Record::new(writer, &fields, types, scratch))
}

/// The barrier a frame was called through, which its value crosses outwards, beside the return
/// its caller sees, which the crossed value is retyped to.
#[derive(Clone, Copy)]
struct Barrier<'graph, 'cell> {
    outermost: Knotted<'graph, 'cell>,
    returns: Option<KType>,
}

/// `arguments`, handed to a call through the barrier `outermost`, crossed inwards through it and
/// every barrier stacked behind it: the function behind them all, beside the record it takes.
fn crossed_in<'graph, 'here>(
    writer: Writer<'here, '_>,
    types: &Chain<'graph, '_, 'here, '_, '_>,
    outermost: Knotted<'graph, 'here>,
    arguments: KValue<'graph, 'here>,
) -> Result<(Knotted<'graph, 'here>, KValue<'graph, 'here>), CoercionRefused> {
    let bump = Bump::new();
    // A keyworded member's slots bind the names the function behind every barrier registers.
    let innermost = outermost.behind_barriers();
    let parameters = innermost
        .function()
        .and_then(|function| function.registered())
        .map_or(ParameterBinding::Named(&[]), |registered| {
            registered.parameters
        });
    let mut arguments = arguments;
    for barrier in outermost.barriers() {
        arguments = coerce::inward(writer, barrier, arguments, parameters, types, &bump)?;
    }
    Ok((innermost, arguments))
}

/// `value`, what the function behind the barrier `outermost` returned, crossed outwards through
/// every barrier stacked there, innermost first. A refusal is an error value.
fn crossed_out<'graph, 'here>(
    writer: Writer<'here, '_>,
    types: &Chain<'graph, '_, 'here, '_, '_>,
    program: &'graph Program<'graph>,
    outermost: Knotted<'graph, 'here>,
    value: KValue<'graph, 'here>,
) -> KValue<'graph, 'here> {
    if program.message(&value, types).is_some() {
        return value;
    }
    let bump = Bump::new();
    let mut stacked = BumpVec::new_in(&bump);
    for barrier in outermost.barriers() {
        stacked.push(barrier);
    }
    let mut value = value;
    for barrier in stacked.iter().rev() {
        value = match coerce::outward(writer, barrier, value, types, &bump) {
            Ok(value) => value,
            Err(refusal) => {
                let refusal = refusal.display(types, program.symbols());
                return program.error(
                    writer,
                    types,
                    format_args!("a value returned through a view cannot take its type: {refusal}"),
                );
            }
        };
    }
    value
}

/// An `EVAL`'s frame: the code's activation over a closure run assembled in its shape's capture
/// order — each `$` name from the bindings the code carries, each hole from those `USING` supplied,
/// each `\` name from `offered` — every edge resolved to the member it names, so the run holds value
/// words only. A keyworded hole nothing filled holds no function. `None` when a name is bound
/// nowhere, which [`eval`] refuses first.
fn code_frame<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    types: &Chain<'graph, '_, 'here, '_, '_>,
    program: &'graph Program<'graph>,
    code: KValue<'graph, 'here>,
    offered: KValue<'graph, 'here>,
) -> Option<&'here KActivation<'graph, 'here>> {
    let member = code.as_code()?;
    let node = member.code()?;
    let shape = node.shape();
    offered.as_record()?;
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
                    Value::List(List::new(writer, [].into_iter(), types, &bump))
                }
                None => return None,
            },
            CaptureSource::Offered => offered.field(capture.name.symbol(), types, &bump)?.value(),
        };
        links.push(Link::Value(value));
    }
    // A quote's lexical chain of build cells is not placed, so its frame seeks from the root.
    let closure = ClosureBindings::of(writer, &links, BuildRef::ROOT);
    Some(resident(
        writer,
        KActivation::of_code(writer, shape, closure, program.builtins()),
    ))
}

/// A parked runner woken: the evaluation it asked for bound or kept, or the parts a tie named
/// supplied and the tie made. An error value received stops it. In a caching body every value
/// received carries its types in the body's chain `types`.
fn woken<'graph, 'here, 'scratch, 'lent>(
    step: &mut Taking<'_, 'graph, '_, 'here, 'scratch, 'lent>,
    runner: &mut Runner<'graph, 'here>,
    scratch: Option<&'scratch [Site]>,
    types: &Chain<'graph, '_, 'here, '_, '_>,
) -> Result<(), Stopped<'scratch>> {
    let unit = runner.current();
    let caches = runner.shape().caches();
    match runner.stage {
        Stage::Next => unreachable!("a runner parks only with an evaluation in flight"),
        Stage::Evaluation => {
            let received = step.results_with(types).next();
            let received = received.ok_or(StepError::Unredeemable)??;
            match received {
                Received::Here(value) => runner.check(&value, types)?,
                Received::Scratch(value) if caches => runner.check(&value, types)?,
                Received::Scratch(value) => runner.check(&value, &step.scratch_types())?,
            }
            match unit.work {
                UnitWork::Component(component) => {
                    let Received::Here(value) = received else {
                        unreachable!(
                            "a binding is asked for with `Keeps`, which never delivers to scratch"
                        )
                    };
                    let slot = runner.shape().components()[component.index()].members[0];
                    let value = held(step, types, runner, slot, value)?;
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
            for (site, received) in sites.iter().zip(step.results_with(types)) {
                let Received::Here(value) = received? else {
                    unreachable!("an eager part is asked for with `Keeps`")
                };
                runner.check(&value, types)?;
                supplied.push((*site, value));
            }
            supplied.sort_unstable_by_key(|(site, _)| *site);
            let component = &runner.shape().components()[component.index()];
            tied(step, types, runner, unit, component, &supplied)?;
        }
    }
    runner.unit += 1;
    runner.stage = Stage::Next;
    Ok(())
}

/// The message of a tie the program refused, rendered into the running region.
fn untied<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    types: &Chain<'graph, '_, 'here, '_, '_>,
    runner: &Runner<'graph, 'here>,
    error: &Untieable<'_>,
) -> Stopped<'here> {
    let program = runner.program;
    Stopped::Raised(rendered(
        step.writer(),
        error.display(program.symbols(), types),
    ))
}

/// Tie `component` again with `supplied` answering every eager part it names by site, and bind
/// every member from the knot.
fn tied<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    types: &Chain<'graph, '_, 'here, '_, '_>,
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
        runner.within.build,
        types,
        &scratch,
        &mut |site, _| {
            let at = supplied
                .binary_search_by_key(&site, |(supplied, _)| *supplied)
                .ok()?;
            Some(Supplied::Value(supplied[at].1))
        },
    );
    match tied {
        Ok(tied) => {
            bound_members(step, types, runner, unit, component, &tied)?;
            Ok(())
        }
        Err(Untieable::Eager { .. }) => {
            unreachable!("a tie asks for every eager part at once, and every one was supplied")
        }
        Err(error) => Err(untied(step, types, runner, &error)),
    }
}

/// Bind every member of `component` from `tied`, each held to its annotation as the tie resolved
/// it. The tie births an annotated member at its annotation, so each slot binds the bare member.
fn bound_members<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    types: &Chain<'graph, '_, 'here, '_, '_>,
    runner: &mut Runner<'graph, 'here>,
    unit: Unit,
    component: &Component<'graph>,
    tied: &Tied<'graph, 'here, '_>,
) -> Result<(), Stopped<'here>> {
    let shape = runner.shape();
    for (index, slot) in component.members.iter().enumerate() {
        let member = Value::Knotted(Knotted::of(tied.knot, index));
        let resolved = tied.annotations.get(index).copied().flatten();
        let value = match (shape.annotation(*slot), resolved) {
            (Some(written), Some(annotation)) => {
                held_at(step, types, runner, written, annotation, member)?
            }
            // An annotation the tie could not resolve faults as `held` resolves it.
            _ => held(step, types, runner, *slot, member)?,
        };
        debug_assert!(
            !matches!(value, Value::Retyped(_)),
            "a member is born at its annotation"
        );
        runner.bind(unit, *slot, value);
    }
    Ok(())
}

/// `value`, about to be bound at `slot`, held to the type its `LET <name> <type> = …` states, if
/// it states one ([`Program::annotated`]).
fn held<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    types: &Chain<'graph, '_, 'here, '_, '_>,
    runner: &Runner<'graph, 'here>,
    slot: Slot,
    value: KValue<'graph, 'here>,
) -> Result<KValue<'graph, 'here>, Stopped<'here>> {
    let shape = runner.shape();
    let Some(written) = shape.annotation(slot) else {
        return Ok(value);
    };
    let program = runner.program;
    let (writer, scratch) = (step.writer(), Bump::new());
    match denoted(written, &runner.activation.view(), types, &scratch) {
        Ok(annotation) => held_at(step, types, runner, written, annotation, value),
        Err(refused) => {
            let error = program.error(writer, types, refused.display(program.symbols(), types));
            runner.check(&error, types)?;
            Ok(error)
        }
    }
}

/// `value` held to `annotation`, what the annotation `written` denotes.
fn held_at<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    types: &Chain<'graph, '_, 'here, '_, '_>,
    runner: &Runner<'graph, 'here>,
    written: &'graph ExpressionPart<'graph>,
    annotation: KType,
    value: KValue<'graph, 'here>,
) -> Result<KValue<'graph, 'here>, Stopped<'here>> {
    let settled = runner.shape().settled(Site::of(written));
    let held = (runner.program).annotated(step.writer(), types, value, annotation, settled);
    runner.check(&held, types)?;
    Ok(held)
}

/// Perform units until one parks or the body ends.
fn perform<'graph, 'here>(
    mut step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    mut runner: Runner<'graph, 'here>,
    types: &Chain<'graph, '_, 'here, '_, '_>,
) -> Action<'graph, KBundle> {
    loop {
        let shape = runner.shape();
        if runner.unit as usize == shape.units().len() {
            match runner.outer {
                Some(outer) => match left(&step, types, runner, outer) {
                    Ok(resumed) => runner = resumed,
                    Err(stopped) => return stop(step, runner, stopped),
                },
                None => return ended(step, runner, types),
            }
            continue;
        }
        let unit = runner.current();
        let component = match unit.work {
            UnitWork::Statement(statement) => {
                let node = Evaluated::Statement(&shape.body()[statement as usize]);
                if runner.tails(unit) {
                    // The frame's scratch departs after the successor's first step, while the
                    // statement still reads the frame's bindings: adopt what it reads first.
                    if runner.within.routes {
                        let view = runner.activation.view();
                        let exit = Exit::new(step.types(), types);
                        for slot in shape.last_reads() {
                            if let Some(value) = view.bound(*slot) {
                                exit.value(&value);
                            }
                        }
                        if let Some(contract) = &runner.contract {
                            exit.contract(contract);
                        }
                    }
                    let program = runner.program;
                    // The successor writes the frame's parent, whose scratch is its own tier.
                    let work = program.evaluate(
                        node,
                        runner.activation.view(),
                        runner.contract,
                        runner.within.handed_off(),
                    );
                    return step.tail(Placement::Shares, work);
                }
                // A caching body keeps its value, so it leaves only where the frame adopts it.
                let use_ = match runner.yields(unit) {
                    true if shape.caches() => Use::Keeps,
                    true => Use::Forwards,
                    false => Use::Reads,
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
            if let Err(stopped) = declared(&step, types, &mut runner, unit, component) {
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
            match first_tie(&mut step, types, &mut runner, unit, component) {
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
    step: &mut Taking<'_, 'graph, '_, 'here, '_, '_>,
    runner: &Runner<'graph, 'here>,
    node: Evaluated<'graph>,
    use_: Use,
) -> Asked {
    let work = runner
        .program
        .evaluate(node, runner.activation.view(), None, runner.within);
    step.spawn(Request {
        placement: runner.placement(),
        use_,
        work,
    })
}

/// A component's first tie: bound, or one evaluation asked for per eager part it named, beside the
/// sites laid down in scratch for the wake.
fn first_tie<'graph, 'here, 'scratch, 'lent>(
    step: &mut Taking<'_, 'graph, '_, 'here, 'scratch, 'lent>,
    types: &Chain<'graph, '_, 'here, '_, '_>,
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
        runner.within.build,
        types,
        &scratch,
        &mut |site, part| {
            missed.extend(part.map(|part| (site, part)));
            None
        },
    );
    match tied {
        Ok(tied) => {
            bound_members(step, types, runner, unit, component, &tied)?;
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
        Err(error) => Err(untied(step, types, runner, &error)),
    }
}

/// Bind each member of a component of type binders to the handle the load fixed for it when every
/// member's is closed, and otherwise declare the component through the elaborator's door, over the
/// activation.
fn declared<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    types: &Chain<'graph, '_, 'here, '_, '_>,
    runner: &mut Runner<'graph, 'here>,
    unit: Unit,
    component: &Component<'graph>,
) -> Result<(), Stopped<'here>> {
    let scratch = Bump::new();
    let program = runner.program;
    let writer = step.writer();
    let shape = runner.activation.shape();
    let mut loaded = BumpVec::with_capacity_in(component.members.len(), &scratch);
    for slot in component.members {
        match shape.declared_type(*slot) {
            Static::Closed(handle) => loaded.push(handle),
            _ => break,
        }
    }
    let handles: &[KType] = if loaded.len() == component.members.len() {
        debug_assert_eq!(
            type_declarations(component, runner.activation, types, &scratch).ok(),
            Some(&loaded[..]),
            "the load-time types agree with declaring where they run"
        );
        &loaded
    } else {
        debug_assert!(
            !(component.members.iter())
                .any(|member| shape.declarations(*member).is_some_and(declares_nominal))
                || shape.mints_at(Site::of_node(
                    shape
                        .declarations(component.members[0])
                        .expect("a type binder records its declaration node"),
                )),
            "the load recorded a nominal component declared where it runs as a minting site",
        );
        let declared =
            type_declarations(component, runner.activation, types, &scratch).map_err(|error| {
                Stopped::Raised(rendered(writer, error.display(program.symbols(), types)))
            })?;
        // Under `routes` an invariant nominal's types are adopted into the warm tier, so the next
        // hop finds them there.
        let site = (shape.declarations(component.members[0])).map(Site::of_node);
        if runner.within.routes
            && site.and_then(|site| shape.mint_at(site)) == Some(Mint::Invariant)
            && let Some(warm) = step.warm_types()
        {
            for handle in declared.iter() {
                warm.adopt_from(types, *handle);
            }
        }
        declared
    };
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
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
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
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    types: &Chain<'graph, '_, 'here, '_, '_>,
    body: Runner<'graph, 'here>,
    outer: &'here Outer<'graph, 'here>,
) -> Result<Runner<'graph, 'here>, Stopped<'here>> {
    let scratch = Bump::new();
    let activation = outer.activation;
    let knot = tie(
        step.writer(),
        activation,
        activation.shape().component_of(outer.binder),
        body.within.build,
        types,
        &scratch,
        &mut |_, _| Some(Supplied::Body(body.activation)),
    )
    .map_err(|error| untied(step, types, &body, &error))?;
    let mut resumed = Runner {
        activation,
        unit: outer.unit,
        result: outer.result,
        outer: outer.outer,
        ..body
    };
    let unit = resumed.current();
    resumed.bind(
        unit,
        outer.binder,
        Value::Knotted(Knotted::of(knot.knot, 0)),
    );
    resumed.unit += 1;
    Ok(resumed)
}

/// The body's end: the top level leaves its activation's view at rest for a later root work, a
/// called frame or an `EVAL`'s finishes with its value held to its contract, and a block with its
/// value.
fn ended<'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    runner: Runner<'graph, 'here>,
    types: &Chain<'graph, '_, 'here, '_, '_>,
) -> Action<'graph, KBundle> {
    let value = runner.result.unwrap_or(Value::Null);
    match (runner.level, runner.contract) {
        (Level::Top, _) => step.leave(KBirth::Inspect {
            program: runner.program,
            view: runner.activation.view(),
        }),
        // A block's value stays in the body it runs in, whose chain reads its types.
        (Level::Frame | Level::Block, None) if runner.shape().caches() => {
            step.finish_with(value, types)
        }
        (Level::Frame | Level::Block, None) => step.finish(value),
        (Level::Frame | Level::Block, Some(contract)) => {
            let value = runner
                .program
                .fulfilled(step.writer(), types, value, contract);
            let value = match runner.barrier {
                Some(barrier) => {
                    let crossed = crossed_out(
                        step.writer(),
                        types,
                        runner.program,
                        barrier.outermost,
                        value,
                    );
                    match barrier.returns {
                        Some(returns) if runner.program.message(&crossed, types).is_none() => {
                            crossed.retyped(step.writer(), returns, types, &Bump::new())
                        }
                        _ => crossed,
                    }
                }
                None => value,
            };
            if runner.shape().caches() {
                Exit::new(step.types(), types).value(&value);
            }
            step.finish(value)
        }
    }
}

/// A unit stopped. An error value ends the body — a frame or a block finishes with it; the top
/// level writes it to the error sink, marks the run uncaught, and leaves its view at rest — and a
/// broken invariant fails the step.
fn stop<'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
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
            let error = program.error(step.writer(), &step.types(), message);
            step.finish(error)
        }
    }
}
