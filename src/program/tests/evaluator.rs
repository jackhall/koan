//! The miniature evaluator the program suites supply as their [`Language`]: not dispatch, and kept
//! small. Its builtin table is `origin = 0` and the scalar types, and it evaluates exactly a
//! literal, a quote — born through the [quote door](crate::knot::quote) — a name, marked or not, a
//! list of literals, names and quotes, `(EVAL code -> <Type>)` through the
//! [`EVAL` door](crate::program::eval), whose declared type it does not read,
//! `(WHEN c THEN a ELSE b)`,
//! `(a MINUS b)`, `(FIRST xs)` over a list data node whose first cell is an edge, a call `(f x)` of
//! a function with one parameter, and a `FN`, born through the [lambda door](crate::knot::lambda).
//! Its load runs koan's walk, so every fact a run reads is the walk's. `WHEN`, `THEN`,
//! `ELSE`, `MINUS` and `FIRST` are no builtin shapes, so the shape builder walks them as plain
//! calls; anything else is refused.
//!
//! Owing a frame's contract, a node is in the frame's tail position: a `WHEN` evaluates its branch
//! in its own place, owing the contract, a call whose callee's declared return satisfies it tails
//! into the callee's frame, and every other value is held to it where the frame ends. An `EVAL`
//! refused, or a fault received from a call, ends the evaluation with that fault.
//!
//! A step is a bare `fn`, so what it observes it records in a thread-local for the test around it.

use std::cell::{Cell, RefCell};

use crate::knot::{KBuiltins, KReading, KValue, Knotted, lambda, quote};
use crate::memory::{Active, Bump, BumpAllocator, Writer, collect};
use crate::parse::BuiltinShapeId;
use crate::parse::{ExpressionPart, KExpression, KLiteral, Spanned};
use crate::program::{
    CallKind, Delivered, Evaluated, Evaluator, KBundle, Language, Next, Node, Program, Tailed,
    call, eval,
};
use crate::scheduler::{Held, Seek, Step, StepError, Taken, Use};
use crate::scope::{
    BodyShape, Builtins, CaptureSlot, Code, Inner, Located, Offer, Position, Site, Slot,
};
use crate::symbols::{KeywordSymbol, SymbolInterner, TypeSymbol, ValueSymbol};
use crate::type_lattice::KType;
use crate::values::{Chain, Circular, Knotted as _, Link, List, Record, TypeValue, Value};

thread_local! {
    static SEEN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// What every call `(f x)` hands its frame as the load's contributions: none, unless a test
    /// stands in for a load that recorded some.
    static CONTRIBUTED: RefCell<Vec<Option<KType>>> = const { RefCell::new(Vec::new()) };
}

/// Forget everything recorded so far, and every contribution a test set.
pub(super) fn reset() {
    SEEN.with(|seen| seen.borrow_mut().clear());
    CONTRIBUTED.with(|contributed| contributed.borrow_mut().clear());
}

/// Make every call `(f x)` hand its frame `contributed`, per parameter in symbol order, as a call
/// by name the load recorded contributions for does.
pub(super) fn contribute(contributed: Vec<Option<KType>>) {
    CONTRIBUTED.with(|cell| *cell.borrow_mut() = contributed);
}

/// Record one observation, in the order the drain produced it.
pub(super) fn record(what: String) {
    SEEN.with(|seen| seen.borrow_mut().push(what));
}

/// Everything recorded since the last reset.
pub(super) fn recorded() -> Vec<String> {
    SEEN.with(|seen| seen.borrow().clone())
}

/// The suites' language.
pub(super) struct Mini;

impl Language for Mini {
    fn builtins<'graph>(
        writer: Writer<'graph, '_>,
        symbols: &'graph SymbolInterner,
        types: &Chain<'graph, 'graph, 'graph, '_>,
        scratch: BumpAllocator<'_>,
    ) -> &'graph KBuiltins<'graph, 'graph> {
        let origin = ValueSymbol::declared("origin", symbols).expect("a value token");
        let scalars: Vec<_> = [
            ("Number", KType::NUMBER),
            ("Str", KType::STR),
            ("Bool", KType::BOOL),
            ("Null", KType::NULL),
            ("Any", KType::ANY),
            ("Value", KType::ANY_VALUE),
            ("Expression", KType::EXPRESSION),
            ("Code", KType::ANY_CODE),
        ]
        .into_iter()
        .map(|(name, handle)| {
            let name = TypeSymbol::declared(name, symbols).expect("a Type token");
            (name, Value::Type(TypeValue::new(writer, handle, types)))
        })
        .collect();
        // Its keyworded calls are no builtin shapes, so each key holds an overload for the shape
        // builder to find; the evaluator reads the keyword itself and never the overload.
        let overloads: Vec<_> = ["WHEN _ THEN _ ELSE _", "_ MINUS _", "FIRST _"]
            .into_iter()
            .map(|text| (symbols.key(text).expect("a key"), Value::Null))
            .collect();
        Builtins::new(
            writer,
            scratch,
            &[(origin, Value::Number(0.0))],
            &scalars,
            &overloads,
        )
    }

    fn evaluator<'graph>() -> Evaluator<'graph> {
        evaluate
    }

    fn facts<'graph: 'o, 'tier: 'o, 'o, S: crate::scope::Store<'o>>(
        shapes: &'graph crate::scope::Shapes<'graph>,
        builtins: &'graph KBuiltins<'graph, 'graph>,
        store: &S,
        types: &Chain<'tier, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
        at: crate::scope::Segment,
    ) -> Result<(), crate::scope::ShapeError<'o>> {
        crate::tests::walk(shapes, builtins, store, types, scratch, at)
    }
}

/// What a node is to the evaluator: one part, a `FN`, or a formless call over several.
enum Form<'step> {
    Leaf(&'step ExpressionPart<'step>),
    Lambda(&'step KExpression<'step>),
    Call(&'step KExpression<'step>),
}

fn form<'step>(shape: &'step BodyShape<'step>, node: Evaluated) -> Form<'step> {
    match node {
        Evaluated::Statement(index) => of_node(&shape.body()[index as usize]),
        Evaluated::Part(site) => match shape.located(site) {
            Located::Part(part) => of_part(part),
            Located::Node(_) => unreachable!("an evaluated part's site locates a part"),
        },
    }
}

fn of_part<'step>(part: &'step ExpressionPart<'step>) -> Form<'step> {
    match part {
        ExpressionPart::Expression(node) => of_node(node),
        part => Form::Leaf(part),
    }
}

fn of_node<'step>(expression: &'step KExpression<'step>) -> Form<'step> {
    let shape = expression.cache().builtin_shape().map(|shape| shape.id);
    if matches!(
        shape,
        Some(BuiltinShapeId::Lambda | BuiltinShapeId::QuantifiedLambda)
    ) {
        return Form::Lambda(expression);
    }
    match expression.parts {
        [only] => of_part(&only.value),
        _ => Form::Call(expression),
    }
}

fn keyword(part: &Spanned<ExpressionPart<'_>>, text: &str) -> bool {
    matches!(part.value, ExpressionPart::Keyword(keyword) if KeywordSymbol::of(text) == Some(keyword))
}

/// What `part`, a name, reads through `view`.
fn read<'graph, 'here>(
    view: &KReading<'_, 'graph, 'here>,
    part: &ExpressionPart<'_>,
) -> Option<KValue<'graph, 'here>> {
    let mention = view.shape().mention(view.shape().site(part))?;
    Some(view.read(mention.coordinate))
}

/// A lambda the door birthed, and each capture it was born with.
fn born<'graph>(
    member: Knotted<'graph, '_>,
    program: &'graph Program<'graph>,
    types: &Chain<'_, '_, '_, '_>,
) -> String {
    let closure = member
        .function()
        .expect("the door births a function")
        .closure();
    let captures: Vec<_> = (0..closure.len())
        .map(|at| match closure.get(CaptureSlot(at as u32)) {
            Link::Value(value) => super::describe(value, program, types),
            Link::Edge(_) => unreachable!("the door lays no edge"),
        })
        .collect();
    format!(
        "{} capturing {}",
        super::describe(Value::Knotted(member), program, types),
        captures.join(" ")
    )
}

/// Whether a condition's value takes the `THEN` branch.
fn truthy(value: &KValue<'_, '_>) -> bool {
    match value {
        Value::Number(number) => *number != 0.0,
        Value::Bool(flag) => *flag,
        _ => false,
    }
}

/// The number a delivered value is.
fn number(held: &Held<'_, '_>) -> Option<f64> {
    match held.value {
        Value::Number(number) => Some(number),
        _ => None,
    }
}

/// Where a node's stage says it waits on the one child whose value is its own.
const FINISHING: u32 = 2;

/// The values of the parts at `sites`, each asked with its use one at a time in order, once all
/// are delivered: what the node needs next until then, or the fault one ended with.
#[expect(
    clippy::result_large_err,
    reason = "the answer is handed straight back, once per call"
)]
fn gathered<'graph, 'here, 'scratch>(
    step: &Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    node: &mut Node<'graph, 'here, 'scratch>,
    delivered: Option<Delivered<'graph, 'here>>,
    sites: &[(Site, Use)],
) -> Result<Vec<Held<'graph, 'here>>, Next<'graph, 'here>> {
    if node.received.is_empty() {
        node.received = step.scratch_writer().fill(sites.len(), |_| Cell::new(None));
    }
    match delivered {
        Some(Delivered::Value(held)) => match node.received.iter().find(|at| at.get().is_none()) {
            Some(slot) => slot.set(Some(held)),
            None => return Err(Next::Failed(StepError::Unredeemable)),
        },
        Some(Delivered::Fault(inner)) => return Err(Next::Raised(inner)),
        None => {}
    }
    match node.received.iter().position(|at| at.get().is_none()) {
        Some(next) => Err(Next::Part {
            site: sites[next].0,
            use_: sites[next].1,
        }),
        None => Ok(node.received.iter().filter_map(Cell::get).collect()),
    }
}

/// The evaluator: what every node in flight is handed to.
fn evaluate<'graph, 'here, 'scratch>(
    step: &mut Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    node: &mut Node<'graph, 'here, 'scratch>,
    delivered: Option<Delivered<'graph, 'here>>,
) -> Next<'graph, 'here> {
    let program = node.program;
    let owed = node.owed;
    let (door, facts) = step.door();
    let view = node.view.reading(&door, facts);
    let types = &step.types();
    let scratch = Bump::new();
    let written = match form(view.shape(), node.node) {
        Form::Leaf(part @ ExpressionPart::Literal(literal)) => {
            if let KLiteral::Number(number) = literal {
                record(format!("literal {number}"));
            }
            // Built where the value goes from the start: a fresh result, placed operand-free.
            let built = step.built_as(node.use_, &[], None, |writer, types, _, _| {
                let value =
                    Value::lower_part(writer, part, types, &scratch).expect("a literal lowers");
                Active::new(value)
            });
            return built.map_or(Next::Failed(StepError::Stale), Next::Value);
        }
        Form::Leaf(part @ ExpressionPart::QuotedExpression(_)) => {
            let written = program.written();
            let member = quote(step.writer(), &view, written, part, types, &scratch)
                .expect("a test's quote composes");
            return finished(step, node, Value::Knotted(member));
        }
        // A Type-class name reads through the activation like any other mention: a frame binds
        // its callee's type parameters, so `Elt` inside a quantified body is an ordinary read.
        Form::Leaf(
            part @ (ExpressionPart::Identifier(_)
            | ExpressionPart::Type(_)
            | ExpressionPart::MarkedName(..)),
        ) => {
            let Some(value) = read(&view, part) else {
                return Next::Failed(StepError::Refused);
            };
            match value {
                Value::List(list) => record(format!("read {:p}", list)),
                Value::Type(_) => {
                    record(format!("read {}", super::describe(value, program, types)))
                }
                _ => {}
            }
            return finished(step, node, value);
        }
        Form::Leaf(ExpressionPart::ListLiteral(items)) => {
            if items
                .iter()
                .all(|item| matches!(item.part(), ExpressionPart::Literal(_)))
            {
                // Built in the home from the start: a fresh result, placed operand-free.
                let built = step.built_as(node.use_, &[], None, |writer, types, _, _| {
                    let cells = items.iter().map(|item| {
                        Value::lower_part(writer, item.part(), types, &scratch)
                            .expect("a literal lowers")
                    });
                    let list = List::new(writer, cells, types, &scratch);
                    record(format!("built {:p}", list));
                    Active::new(Value::List(list))
                });
                return built.map_or(Next::Failed(StepError::Stale), Next::Value);
            }
            let writer = step.writer();
            let mut cells = Vec::new();
            for item in items.iter().map(|item| item.part()) {
                match item {
                    ExpressionPart::Identifier(_) => match read(&view, item) {
                        Some(value) => cells.push(value),
                        None => return Next::Failed(StepError::Refused),
                    },
                    ExpressionPart::QuotedExpression(_) => {
                        let written = program.written();
                        let member = quote(writer, &view, written, item, types, &scratch)
                            .expect("a test's quote composes");
                        cells.push(Value::Knotted(member));
                    }
                    _ => match Value::lower_part(writer, item, types, &scratch) {
                        Some(value) => cells.push(value),
                        None => return Next::Failed(StepError::Refused),
                    },
                }
            }
            // Built here and crossed into the home at the verdict's price.
            let list = List::new(writer, cells.into_iter(), types, &scratch);
            record(format!("built {:p}", list));
            return finished(step, node, Value::List(list));
        }
        Form::Leaf(_) => return Next::Failed(StepError::Refused),
        Form::Lambda(expression) => {
            let site = view
                .shape()
                .site_of_body(expression)
                .expect("a lambda has a body");
            let writer = step.writer();
            let Ok(member) = lambda(
                writer,
                &view,
                program.written(),
                site,
                None,
                (crate::values::ArenaRef::ROOT, None),
                types,
                &scratch,
            ) else {
                return Next::Failed(StepError::Refused);
            };
            record(format!("born {}", born(member, program, types)));
            return finished(step, node, Value::Knotted(member));
        }
        Form::Call(written) => written,
    };
    if node.stage == FINISHING {
        return match delivered {
            Some(Delivered::Value(Held { value, .. })) => finished(step, node, value),
            Some(Delivered::Fault(inner)) => Next::Raised(inner),
            None => Next::Failed(StepError::Unredeemable),
        };
    }
    let site = |part: &Spanned<ExpressionPart<'_>>| view.shape().site(&part.value);
    match written.parts {
        [when, condition, then, taken, otherwise, other]
            if keyword(when, "WHEN") && keyword(then, "THEN") && keyword(otherwise, "ELSE") =>
        {
            let condition = match gathered(step, node, delivered, &[(site(condition), Use::Reads)])
            {
                Ok(values) => values[0],
                Err(next) => return next,
            };
            let branch = if truthy(&condition.value) {
                taken
            } else {
                other
            };
            // Owing a contract, the branch is the node's tail: evaluated in its place.
            if owed.is_some() {
                *node = Node {
                    node: Evaluated::Part(site(branch)),
                    stage: 0,
                    received: &[],
                    ..*node
                };
                return evaluate(step, node, None);
            }
            node.stage = FINISHING;
            Next::Part {
                site: site(branch),
                use_: Use::Forwards,
            }
        }
        [left, minus, right] if keyword(minus, "MINUS") => {
            let sites = [(site(left), Use::Reads), (site(right), Use::Reads)];
            let operands = match gathered(step, node, delivered, &sites) {
                Ok(values) => values,
                Err(next) => return next,
            };
            let (Some(left), Some(right)) = (number(&operands[0]), number(&operands[1])) else {
                return Next::Failed(StepError::Refused);
            };
            let built = step.built_as(node.use_, &[], None, move |_, _, _, _| {
                Active::new(Value::Number(left - right))
            });
            built.map_or(Next::Failed(StepError::Stale), Next::Value)
        }
        [first, operand] if keyword(first, "FIRST") => {
            let Some((holder, Circular::List(list))) =
                read(&view, &operand.value).and_then(|value| value.as_circular())
            else {
                return Next::Failed(StepError::Refused);
            };
            match list.links().next() {
                Some(Link::Edge(edge)) => {
                    finished(step, node, Value::Knotted(holder.sibling(edge)))
                }
                _ => Next::Failed(StepError::Refused),
            }
        }
        [head, operand, _, _] if keyword(head, "EVAL") => {
            let code = match gathered(step, node, delivered, &[(site(operand), Use::Keeps)]) {
                Ok(values) => values[0].value,
                Err(next) => return next,
            };
            let Some(code) = code.as_code() else {
                return Next::Failed(StepError::Refused);
            };
            let fields: Vec<_> = view
                .shape()
                .offers(site(operand))
                .iter()
                .map(|(name, offer)| match offer {
                    Offer::Name(at) => (*name, view.read(*at)),
                    Offer::Key(_) => unreachable!("the miniature evaluator offers names alone"),
                })
                .collect();
            let offered = Record::new(step.writer(), &fields, types, &scratch);
            match eval(
                program,
                (&door, &facts),
                types,
                code,
                Value::Record(offered),
                (KType::ANY, written.source),
                None,
                node.use_,
            ) {
                Ok(request) => {
                    node.stage = FINISHING;
                    Next::Spawned(step.spawn(request))
                }
                Err(refused) => {
                    let message = refused.display(program.symbols(), types).to_string();
                    record(format!("refused {message}"));
                    let fault = crate::memory::resident(
                        step.writer(),
                        refused.relaid(step.writer(), &scratch),
                    );
                    Next::Raised(Inner::Fault(fault))
                }
            }
        }
        [head, argument] => {
            let argument = match gathered(step, node, delivered, &[(site(argument), Use::Keeps)]) {
                Ok(values) => values[0].value,
                Err(next) => return next,
            };
            let Some(callee) = read(&view, &head.value) else {
                return Next::Failed(StepError::Refused);
            };
            let Some(parameter) = parameter(&door, callee) else {
                return Next::Failed(StepError::Refused);
            };
            let writer = step.writer();
            let arguments = Record::new(writer, &[(parameter, argument)], types, &scratch);
            let contributed =
                CONTRIBUTED.with(|cell| collect(writer, cell.borrow().iter().copied()));
            let request = |owed| {
                call(
                    program,
                    callee,
                    Value::Record(arguments),
                    CallKind::ByName,
                    contributed,
                    &[],
                    (written.source, owed),
                    None,
                    None,
                    node.use_,
                )
            };
            if let Some(owed) = owed
                && (callee.as_callable().and_then(Knotted::function))
                    .is_some_and(|function| owed.contract.kept_by(types, function.ktype(), None))
            {
                let request = request(Some(owed.hopped(step.writer())));
                let seek = Seek {
                    parent: crate::values::ArenaRef::ROOT,
                    settlement: None,
                    hands_back: false,
                };
                return Next::Tail(Tailed::Seeking(request.work, seek));
            }
            node.stage = FINISHING;
            Next::Spawned(step.spawn(request(None)))
        }
        _ => Next::Failed(StepError::Refused),
    }
}

/// A step that took both its states.
type Taking<'a, 'graph, 'step, 'here, 'scratch, 'lent> =
    Step<'a, 'graph, 'step, 'here, 'scratch, 'lent, KBundle, Taken, Taken>;

/// The name of `callee`'s one **value** parameter. A quantified callee's type-parameter slots are
/// bound by the frame from its solved group, not by the caller, so they are skipped here.
fn parameter<'step>(
    code: &Code<'step>,
    callee: KValue<'_, '_>,
) -> Option<crate::symbols::BinderSymbol> {
    let function = callee.as_callable().and_then(Knotted::function)?;
    let shape = code.shape(function.body().segment);
    (0..shape.slots())
        .map(|slot| shape.slot_name(Slot(slot as u32)))
        .filter(|name| matches!(name, crate::symbols::BinderSymbol::Value(_)))
        .find(|name| shape.slot(*name).map(|(_, at)| at) == Some(Position::PARAMETER))
}

/// Hand `value` on, placed where the node's `Use` says.
fn finished<'graph, 'here>(
    step: &mut Taking<'_, 'graph, '_, 'here, '_, '_>,
    node: &Node<'graph, 'here, '_>,
    value: KValue<'graph, 'here>,
) -> Next<'graph, 'here> {
    match step.crossed_as(node.use_, Held::bare(value), None) {
        Some(held) => Next::Value(held),
        None => Next::Failed(StepError::Stale),
    }
}
