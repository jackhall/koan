//! The miniature evaluator the program suites supply as their [`Language`]: not dispatch, and kept
//! small. Its builtin table is `origin = 0` and the scalar types, and it evaluates exactly a
//! literal, a quote, a name, a list of literals and names, `(WHEN c THEN a ELSE b)`,
//! `(a MINUS b)`, and a call `(f x)` of a function with one parameter. `WHEN`, `THEN`, `ELSE` and
//! `MINUS` are no builtin shapes, so the shape builder walks them as plain calls; anything else is
//! refused.
//!
//! A step is a bare `fn`, so what it observes it records in a thread-local for the test around it.

use std::cell::RefCell;

use crate::knot::{KActivationView, KBuiltins, KValue, Knotted};
use crate::memory::{Active, Bump, BumpAllocator, Writer};
use crate::parse::{ExpressionPart, KLiteral, Spanned};
use crate::program::{Evaluated, KBirth, KBundle, KState, Language, call};
use crate::scheduler::{
    Action, NativeStep, Placement, Received, Request, Slot as Asked, Step, StepError, Taken, Use,
};
use crate::scope::{Builtins, Position, Site, Slot};
use crate::symbols::{KeywordSymbol, SymbolInterner, TypeSymbol, ValueSymbol};
use crate::type_lattice::{KType, TypeRegistry};
use crate::values::{List, Record, TypeValue, Value};

thread_local! {
    static SEEN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Forget everything recorded so far.
pub(super) fn reset() {
    SEEN.with(|seen| seen.borrow_mut().clear());
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
        writer: Writer<'graph>,
        symbols: &'graph SymbolInterner,
        types: &'graph TypeRegistry<'graph>,
        scratch: BumpAllocator<'_>,
    ) -> &'graph KBuiltins<'graph, 'graph> {
        let origin = ValueSymbol::declared("origin", symbols).expect("a value token");
        let scalars: Vec<_> = [
            ("Number", KType::NUMBER),
            ("Str", KType::STR),
            ("Bool", KType::BOOL),
            ("Null", KType::NULL),
            ("Any", KType::ANY),
        ]
        .into_iter()
        .map(|(name, handle)| {
            let name = TypeSymbol::declared(name, symbols).expect("a Type token");
            (name, Value::Type(TypeValue::new(writer, handle, types)))
        })
        .collect();
        Builtins::new(writer, scratch, &[(origin, Value::Number(0.0))], &scalars)
    }

    fn evaluator<'graph>() -> NativeStep<'graph, KBundle> {
        evaluate
    }
}

/// What a node is to the evaluator: one part, or a formless call over several.
enum Form<'graph> {
    Leaf(&'graph ExpressionPart<'graph>),
    Call(&'graph [Spanned<ExpressionPart<'graph>>]),
}

fn form(node: Evaluated<'_>) -> Form<'_> {
    let expression = match node {
        Evaluated::Part(ExpressionPart::Expression(node)) => node.reference(),
        Evaluated::Part(part) => return Form::Leaf(part),
        Evaluated::Statement(expression) => expression,
    };
    match expression.parts {
        [only] => form(Evaluated::Part(&only.value)),
        parts => Form::Call(parts),
    }
}

fn keyword(part: &Spanned<ExpressionPart<'_>>, text: &str) -> bool {
    matches!(part.value, ExpressionPart::Keyword(keyword) if KeywordSymbol::of(text) == Some(keyword))
}

/// What `part`, a name, reads through `view`.
fn read<'graph, 'here>(
    view: &KActivationView<'graph, 'here>,
    part: &ExpressionPart<'graph>,
) -> Option<KValue<'graph, 'here>> {
    let mention = view.shape().mention(Site::of(part))?;
    Some(view.read(mention.coordinate))
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
        Received::Scratch(Value::Number(number)) | Received::Here(Value::Number(number)) => {
            Some(number)
        }
        _ => None,
    }
}

/// The step every evaluation runs.
fn evaluate<'graph>(step: Step<'_, 'graph, '_, '_, '_, KBundle>) -> Action<'graph, KBundle> {
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
    } = birth
    else {
        return step.failed(StepError::Refused);
    };
    let types = program.types();
    let scratch = Bump::new();
    let parts = match form(node) {
        Form::Leaf(part @ (ExpressionPart::Literal(_) | ExpressionPart::QuotedExpression(_))) => {
            if let ExpressionPart::Literal(KLiteral::Number(number)) = part {
                record(format!("literal {number}"));
            }
            return step.finish_fresh(|writer, _| {
                Active::new(
                    Value::lower_part(writer, part, types, &scratch)
                        .expect("a literal or a quote lowers"),
                )
            });
        }
        // A Type-class name reads through the activation like any other mention: a frame binds
        // its callee's type parameters, so `Elt` inside a quantified body is an ordinary read.
        Form::Leaf(part @ (ExpressionPart::Identifier(_) | ExpressionPart::Type(_))) => {
            let Some(value) = read(&view, part) else {
                return step.failed(StepError::Refused);
            };
            if let Value::List(list) = value {
                record(format!("read {:p}", list));
            }
            return step.finish(value);
        }
        Form::Leaf(ExpressionPart::ListLiteral(items)) => {
            if items
                .iter()
                .all(|item| matches!(item, ExpressionPart::Literal(_)))
            {
                // Built in the home from the start: a fresh result, placed operand-free.
                return step.finish_fresh(|writer, _| {
                    let cells = items.iter().map(|item| {
                        Value::lower_part(writer, item, types, &scratch).expect("a literal lowers")
                    });
                    let list = List::new(writer, cells, types, &scratch);
                    record(format!("built {:p}", list));
                    Active::new(Value::List(list))
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
                    _ => match Value::lower_part(writer, item, types, &scratch) {
                        Some(value) => cells.push(value),
                        None => return step.failed(StepError::Refused),
                    },
                }
            }
            // Built here and crossed into the home at the verdict's price.
            let list = List::new(writer, cells.into_iter(), types, &scratch);
            record(format!("built {:p}", list));
            return step.finish(Value::List(list));
        }
        Form::Leaf(_) => return step.failed(StepError::Refused),
        Form::Call(parts) => parts,
    };
    match (parts, stage) {
        ([when, condition, then, _, otherwise, _], 0)
            if keyword(when, "WHEN") && keyword(then, "THEN") && keyword(otherwise, "ELSE") =>
        {
            let asked = ask(&mut step, birth, &condition.value, Use::Reads);
            park(step, asked, birth, 1)
        }
        ([_, _, _, taken, _, other], 1) => {
            let Some(Ok(condition)) = step.results().next() else {
                return step.failed(StepError::Unredeemable);
            };
            let holds = match condition {
                Received::Scratch(value) => truthy(&value),
                Received::Here(value) => truthy(&value),
            };
            let branch = if holds { taken } else { other };
            let asked = ask(&mut step, birth, &branch.value, Use::Forwards);
            park(step, asked, birth, 2)
        }
        ([left, minus, right], 0) if keyword(minus, "MINUS") => {
            ask(&mut step, birth, &left.value, Use::Reads);
            let asked = ask(&mut step, birth, &right.value, Use::Reads);
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
            step.finish_fresh(move |_, _| Active::new(Value::Number(left - right)))
        }
        ([_, argument], 0) => {
            let asked = ask(&mut step, birth, &argument.value, Use::Keeps);
            park(step, asked, birth, 1)
        }
        ([head, _], 1) => {
            let Some(Ok(Received::Here(argument))) = step.results().next() else {
                return step.failed(StepError::Unredeemable);
            };
            let Some(callee) = read(&view, &head.value) else {
                return step.failed(StepError::Refused);
            };
            let Some(parameter) = parameter(callee) else {
                return step.failed(StepError::Refused);
            };
            let writer = step.writer();
            let arguments = Record::new(writer, &[(parameter, argument)], types, &scratch);
            let asked = step.spawn(call(
                program,
                callee,
                Value::Record(arguments),
                Use::Forwards,
            ));
            park(step, asked, birth, 2)
        }
        (_, 2) => {
            let received = step.results().next();
            match received {
                Some(Ok(Received::Here(value))) => step.finish(value),
                _ => step.failed(StepError::Unredeemable),
            }
        }
        _ => step.failed(StepError::Refused),
    }
}

/// A step that took both its states.
type Taking<'a, 'graph, 'step, 'here, 'scratch> =
    Step<'a, 'graph, 'step, 'here, 'scratch, KBundle, Taken, Taken>;

/// Ask for one child evaluating `part` through the view `birth` holds, in a region of its own.
fn ask<'graph, 'here>(
    step: &mut Taking<'_, 'graph, '_, 'here, '_>,
    birth: KBirth<'graph, 'here>,
    part: &'graph ExpressionPart<'graph>,
    use_: Use,
) -> Asked {
    let KBirth::Evaluate { program, view, .. } = birth else {
        unreachable!("an evaluator holds its evaluation's birth")
    };
    step.spawn(Request {
        placement: Placement::Fresh,
        use_,
        work: program.evaluate(Evaluated::Part(part), view),
    })
}

/// Park on the children asked for, resuming at `stage`.
fn park<'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_>,
    asked: Asked,
    birth: KBirth<'graph, 'here>,
    stage: u32,
) -> Action<'graph, KBundle> {
    step.park(asked, evaluate, KState::Evaluating { birth, stage }, None)
}

/// The name of `callee`'s one **value** parameter. A quantified callee's type-parameter slots are
/// bound by the frame from its solved group, not by the caller, so they are skipped here.
fn parameter(callee: KValue<'_, '_>) -> Option<crate::symbols::BinderSymbol> {
    let shape = callee.as_callable().and_then(Knotted::function)?.shape();
    (0..shape.slots())
        .map(|slot| shape.slot_name(Slot(slot as u32)))
        .filter(|name| matches!(name, crate::symbols::BinderSymbol::Value(_)))
        .find(|name| shape.slot(*name).map(|(_, at)| at) == Some(Position::PARAMETER))
}
