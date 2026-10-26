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
//! Born under a frame's [`Contract`], it owes the frame its value: a `WHEN` tails into its branch
//! with the contract, a call whose callee's declared return satisfies it tails into the callee's
//! frame, and every other value is held to it where it is finished. An `EVAL` refused, or an
//! error value received from a call, finishes with the error value.
//!
//! A step is a bare `fn`, so what it observes it records in a thread-local for the test around it.

use std::cell::RefCell;

use crate::knot::{KBuiltins, KReading, KValue, Knotted, lambda, quote};
use crate::memory::{Active, Bump, BumpAllocator, Writer, collect};
use crate::parse::BuiltinShapeId;
use crate::parse::{ExpressionPart, KExpression, KLiteral, Spanned};
use crate::program::{
    CallKind, Contract, Evaluated, KBirth, KBundle, KState, Language, Program, call, eval,
};
use crate::scheduler::{
    Action, Held, NativeStep, Placement, Received, Request, Slot as Asked, Step, StepError, Taken,
    Use,
};
use crate::scope::{BodyShape, Builtins, CaptureSlot, Code, Located, Offer, Position, Site, Slot};
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

    fn evaluator<'graph>() -> NativeStep<'graph, KBundle> {
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
    Call(&'step [Spanned<ExpressionPart<'step>>]),
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
        ExpressionPart::Expression(node) => of_node(node.reference()),
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
        parts => Form::Call(parts),
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

/// The value one received slot holds, wherever it was delivered.
fn number(received: Received<'_, '_, '_>) -> Option<f64> {
    match received {
        Received::Scratch(Value::Number(number))
        | Received::Here(Held {
            value: Value::Number(number),
            ..
        }) => Some(number),
        _ => None,
    }
}

/// The step every evaluation runs.
fn evaluate<'graph>(step: Step<'_, 'graph, '_, '_, '_, '_, KBundle>) -> Action<'graph, KBundle> {
    let (step, state) = step.state();
    let (mut step, _) = step.scratch();
    let (birth, stage) = match state {
        KState::Born(birth) => (birth, 0),
        KState::Evaluating { birth, stage } => (birth, stage),
        KState::Runner(_) => return step.failed(StepError::Refused),
    };
    let KBirth::Evaluate {
        program,
        node,
        view,
        contract,
        ..
    } = birth
    else {
        return step.failed(StepError::Refused);
    };
    let (door, facts) = step.door();
    let view = view.reading(&door, facts);
    let types = &step.types();
    let scratch = Bump::new();
    let parts = match form(view.shape(), node) {
        Form::Leaf(part @ ExpressionPart::Literal(literal)) => {
            if let KLiteral::Number(number) = literal {
                record(format!("literal {number}"));
            }
            return step.finish_fresh(|writer, types, _| {
                let value =
                    Value::lower_part(writer, part, types, &scratch).expect("a literal lowers");
                Active::new(held(program, contract, writer, types, value))
            });
        }
        Form::Leaf(part @ ExpressionPart::QuotedExpression(_)) => {
            let written = program.written();
            let member = quote(step.writer(), &view, written, part, types, &scratch);
            return {
                let writer = step.writer();
                step.finish(held(
                    program,
                    contract,
                    writer,
                    types,
                    Value::Knotted(member),
                ))
            };
        }
        // A Type-class name reads through the activation like any other mention: a frame binds
        // its callee's type parameters, so `Elt` inside a quantified body is an ordinary read.
        Form::Leaf(
            part @ (ExpressionPart::Identifier(_)
            | ExpressionPart::Type(_)
            | ExpressionPart::MarkedName(..)),
        ) => {
            let Some(value) = read(&view, part) else {
                return step.failed(StepError::Refused);
            };
            match value {
                Value::List(list) => record(format!("read {:p}", list)),
                Value::Type(_) => {
                    record(format!("read {}", super::describe(value, program, types)))
                }
                _ => {}
            }
            return {
                let writer = step.writer();
                step.finish(held(program, contract, writer, types, value))
            };
        }
        Form::Leaf(ExpressionPart::ListLiteral(items)) => {
            if items
                .iter()
                .all(|item| matches!(item, ExpressionPart::Literal(_)))
            {
                // Built in the home from the start: a fresh result, placed operand-free.
                return step.finish_fresh(|writer, types, _| {
                    let cells = items.iter().map(|item| {
                        Value::lower_part(writer, item, types, &scratch).expect("a literal lowers")
                    });
                    let list = List::new(writer, cells, types, &scratch);
                    record(format!("built {:p}", list));
                    Active::new(held(program, contract, writer, types, Value::List(list)))
                });
            }
            let writer = step.writer();
            let mut cells = Vec::new();
            for item in items.iter() {
                match item {
                    ExpressionPart::Identifier(_) => match read(&view, item) {
                        Some(value) => cells.push(value),
                        None => return step.failed(StepError::Refused),
                    },
                    ExpressionPart::QuotedExpression(_) => {
                        let written = program.written();
                        let member = quote(writer, &view, written, item, types, &scratch);
                        cells.push(Value::Knotted(member));
                    }
                    _ => match Value::lower_part(writer, item, types, &scratch) {
                        Some(value) => cells.push(value),
                        None => return step.failed(StepError::Refused),
                    },
                }
            }
            // Built here and crossed into the home at the verdict's price.
            let list = List::new(writer, cells.into_iter(), types, &scratch);
            record(format!("built {:p}", list));
            return step.finish(held(program, contract, writer, types, Value::List(list)));
        }
        Form::Leaf(_) => return step.failed(StepError::Refused),
        Form::Lambda(node) => {
            let site = view
                .shape()
                .site_of_body(node)
                .expect("a lambda has a body");
            let writer = step.writer();
            let Ok(member) = lambda(
                writer,
                &view,
                program.written(),
                site,
                (crate::values::ArenaRef::ROOT, None),
                types,
                &scratch,
            ) else {
                return step.failed(StepError::Refused);
            };
            record(format!("born {}", born(member, program, types)));
            return step.finish(held(
                program,
                contract,
                writer,
                types,
                Value::Knotted(member),
            ));
        }
        Form::Call(parts) => parts,
    };
    match (parts, stage) {
        ([when, condition, then, _, otherwise, _], 0)
            if keyword(when, "WHEN") && keyword(then, "THEN") && keyword(otherwise, "ELSE") =>
        {
            let asked = ask(
                &mut step,
                birth,
                view.shape().site(&condition.value),
                Use::Reads,
            );
            park(step, asked, birth, 1)
        }
        ([_, _, _, taken, _, other], 1) => {
            let Some(Ok(condition)) = step.results().next() else {
                return step.failed(StepError::Unredeemable);
            };
            let holds = match condition {
                Received::Scratch(value) => truthy(&value),
                Received::Here(held) => truthy(&held.value),
            };
            let branch = if holds { taken } else { other };
            if contract.is_some() {
                let work = program.evaluate(
                    Evaluated::Part(view.shape().site(&branch.value)),
                    view.view(),
                    contract,
                    crate::program::Within::TOP,
                );
                return step.tail(Placement::Shares, work);
            }
            let asked = ask(
                &mut step,
                birth,
                view.shape().site(&branch.value),
                Use::Forwards,
            );
            park(step, asked, birth, 2)
        }
        ([left, minus, right], 0) if keyword(minus, "MINUS") => {
            ask(&mut step, birth, view.shape().site(&left.value), Use::Reads);
            let asked = ask(
                &mut step,
                birth,
                view.shape().site(&right.value),
                Use::Reads,
            );
            park(step, asked, birth, 1)
        }
        ([_, _, _], 1) => {
            let operands: Vec<Option<f64>> = step
                .results()
                .map(|received| received.ok().and_then(number))
                .collect();
            let [Some(left), Some(right)] = operands[..] else {
                return step.failed(StepError::Refused);
            };
            step.finish_fresh(move |writer, types, _| {
                Active::new(held(
                    program,
                    contract,
                    writer,
                    types,
                    Value::Number(left - right),
                ))
            })
        }
        ([first, operand], 0) if keyword(first, "FIRST") => {
            let Some((holder, Circular::List(list))) =
                read(&view, &operand.value).and_then(|value| value.as_circular())
            else {
                return step.failed(StepError::Refused);
            };
            match list.cells().first() {
                Some(Link::Edge(edge)) => {
                    let writer = step.writer();
                    step.finish(held(
                        program,
                        contract,
                        writer,
                        types,
                        Value::Knotted(holder.sibling(*edge)),
                    ))
                }
                _ => step.failed(StepError::Refused),
            }
        }
        ([head, operand, _, _], 0) if keyword(head, "EVAL") => {
            let asked = ask(
                &mut step,
                birth,
                view.shape().site(&operand.value),
                Use::Keeps,
            );
            park(step, asked, birth, 3)
        }
        ([_, operand, _, _], 3) => {
            let Some(Ok(Received::Here(Held { value: code, .. }))) = step.results().next() else {
                return step.failed(StepError::Unredeemable);
            };
            let Some(code) = code.as_code() else {
                return step.failed(StepError::Refused);
            };
            let fields: Vec<_> = view
                .shape()
                .offers(view.shape().site(&operand.value))
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
                KType::ANY,
                None,
                Use::Forwards,
            ) {
                Ok(request) => {
                    let asked = step.spawn(request);
                    park(step, asked, birth, 2)
                }
                Err(refused) => {
                    let message = refused.display(program.symbols(), types).to_string();
                    record(format!("refused {message}"));
                    let error = program.error(step.writer(), types, message);
                    step.finish(error)
                }
            }
        }
        ([_, argument], 0) => {
            let asked = ask(
                &mut step,
                birth,
                view.shape().site(&argument.value),
                Use::Keeps,
            );
            park(step, asked, birth, 1)
        }
        ([head, _], 1) => {
            let Some(Ok(Received::Here(Held {
                value: argument, ..
            }))) = step.results().next()
            else {
                return step.failed(StepError::Unredeemable);
            };
            let Some(callee) = read(&view, &head.value) else {
                return step.failed(StepError::Refused);
            };
            let Some(parameter) = parameter(&door, callee) else {
                return step.failed(StepError::Refused);
            };
            let writer = step.writer();
            let arguments = Record::new(writer, &[(parameter, argument)], types, &scratch);
            let contributed =
                CONTRIBUTED.with(|cell| collect(writer, cell.borrow().iter().copied()));
            let request = |owed| {
                call(
                    program,
                    types,
                    callee,
                    Value::Record(arguments),
                    CallKind::ByName,
                    contributed,
                    &[],
                    owed,
                    None,
                    None,
                    Use::Forwards,
                )
            };
            if let Some(contract) = contract
                && (callee.as_callable().and_then(Knotted::function))
                    .is_some_and(|function| contract.kept_by(types, function.ktype(), None))
            {
                let request = request(Some(contract));
                return step.tail(request.placement, request.work);
            }
            let asked = step.spawn(request(None));
            park(step, asked, birth, 2)
        }
        (_, 2) => {
            let received = step.results().next();
            match received {
                Some(Ok(Received::Here(Held { value, .. }))) => {
                    let writer = step.writer();
                    step.finish(held(program, contract, writer, types, value))
                }
                _ => step.failed(StepError::Unredeemable),
            }
        }
        _ => step.failed(StepError::Refused),
    }
}

/// A step that took both its states.
type Taking<'a, 'graph, 'step, 'here, 'scratch, 'lent> =
    Step<'a, 'graph, 'step, 'here, 'scratch, 'lent, KBundle, Taken, Taken>;

/// Ask for one child evaluating the part at `site` through the view `birth` holds, in a region of
/// its own.
fn ask<'graph, 'here>(
    step: &mut Taking<'_, 'graph, '_, 'here, '_, '_>,
    birth: KBirth<'graph, 'here>,
    site: Site,
    use_: Use,
) -> Asked {
    let KBirth::Evaluate { program, view, .. } = birth else {
        unreachable!("an evaluator holds its evaluation's birth")
    };
    step.spawn(Request {
        placement: Placement::Fresh,
        use_,
        work: program.evaluate(
            Evaluated::Part(site),
            view,
            None,
            crate::program::Within::TOP,
        ),
    })
}

/// Park on the children asked for, resuming at `stage`.
fn park<'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    asked: Asked,
    birth: KBirth<'graph, 'here>,
    stage: u32,
) -> Action<'graph, KBundle> {
    step.park(asked, evaluate, KState::Evaluating { birth, stage }, None)
}

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

/// `value` held to `contract`, when the evaluation was born under one.
fn held<'graph, 'cell>(
    program: &Program<'graph>,
    contract: Option<Contract>,
    writer: crate::memory::Writer<'cell, '_>,
    types: &Chain<'_, 'cell, '_, '_>,
    value: KValue<'graph, 'cell>,
) -> KValue<'graph, 'cell> {
    match contract {
        Some(contract) => program.fulfilled(writer, types, value, contract),
        None => value,
    }
}
