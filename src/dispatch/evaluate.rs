//! The step every evaluation runs: what a node is, and how its value is reached.
//!
//! A node is read off the shape that holds it — never the parse — as one of:
//!
//! - a **leaf**: a literal, lowered; a name, read through its mention's coordinate; a quote, born
//!   through the [quote door](crate::knot::quote); a list, dict or record literal, its parts
//!   evaluated and the container built; a type expression, elaborated to a type value;
//! - a **block** a pairwise operator run was rewritten into, run through the
//!   [block door](crate::program::block) with its last statement's value its own;
//! - a `FN`, born through the [lambda door](crate::knot::lambda);
//! - an **ascription** `<value> :! <Type>`: its operand checked against the type, unless the load
//!   settled it, and retyped to it; a module operand, under `:!` or `:|`, is seen as its signature
//!   through the view door;
//! - an **`EVAL`** `<code> -> <Type>`: its code run in a frame that owes the declared type, as a
//!   called frame owes its return;
//! - a **`USING … SCOPE`**: its body run as a block entered on the module, each surfaced name bound
//!   to the member it names and each surfaced key to the module's functions there;
//! - a bucket declaration, which is `Null`;
//! - a **keyworded call**, which evaluates its slots and runs what [`select`](super::select) picks
//!   among the candidates [the load](super::statics) kept, or what the load selected: a builtin's
//!   native, or a registration's function in a frame;
//! - an **application** `(head argument)`: a construction when the head is a type, and otherwise a
//!   call by name of the head over the argument record.
//!
//! A part the node needs is read in place when it is a literal, a name or a quote, and otherwise
//! asked for — a tenant of this cell, kept — all at one park; the wake reads the same parts again.
//! An error value received from a part is this evaluation's value, unchanged. Born under a frame's
//! [`Contract`], the evaluation owes the frame its value: a call whose callee's declared return
//! satisfies the contract hops to the callee's frame, and every other value is held to the contract
//! where it is finished.
//!
//! [`statics`](super::statics) reads nodes through this file's [`Form`], so the load types each node
//! as it evaluates. Debug builds check that a finished value's carried type lies within its node's
//! static type; the narrowing law (`tests/narrowing.rs`) is what holds a narrowed call to full
//! selection.

use crate::elaborate::denoted;
use crate::knot::module::view::{self, Ascription, Unascribable};
use crate::knot::{KValue, Knotted, instance, lambda, quote, refused_construction};
use crate::memory::{Bump, BumpVec, Writer, collect};
use crate::parse::BuiltinShapeId;
use crate::parse::Role;
use crate::parse::{ExpressionPart, KExpression};
use crate::program::{
    CallKind, Evaluated, KBirth, KBundle, KState, Program, block, surfaced_block,
};
use crate::scheduler::{
    Action, Placement, Received, Request, Slot as Asked, Step, StepError, Taken, Use,
};
use crate::scope::{
    BodyShape, Candidate, CandidateList, Narrowing, Offer, ShapeKind, Site, Static, StaticType,
};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{
    DeclaredType, KType, TypeNode, Verdict, satisfied_by, substitute_levels,
};
#[cfg(debug_assertions)]
use crate::type_lattice::{bound_above, lower_end_outside};
use crate::values::{Dict, Key, List, Record, Tagged, TypeValue, Value, satisfies};

use super::builtins::{self, Native};
use super::errors::Raised;
use super::select::{self, Selection};
use super::{Evaluation, Operand, check};

/// Where an evaluation parked.
const BORN: u32 = 0;
/// The parts it asked for are in.
const GATHERED: u32 = 1;
/// The one child whose value is its own — a frame, a block, an `EVAL` — is in.
const FINISHING: u32 = 2;

/// A step that took both its states.
type Taking<'a, 'graph, 'step, 'here, 'scratch> =
    Step<'a, 'graph, 'step, 'here, 'scratch, KBundle, Taken, Taken>;

/// What a node is to the evaluator.
pub(super) enum Form<'graph> {
    Leaf(&'graph ExpressionPart<'graph>),
    Block(&'graph BodyShape<'graph>),
    Lambda(&'graph KExpression<'graph>),
    /// `<value> :! <Type>`, or `<module> :| <Sig>`.
    Ascribe(&'graph KExpression<'graph>),
    /// `EVAL <code> -> <Type>`.
    Eval(&'graph KExpression<'graph>),
    /// `USING <module> SCOPE <body>`.
    Using(&'graph KExpression<'graph>),
    Declaration,
    Call(&'graph KExpression<'graph>, &'graph CandidateList<'graph>),
    /// `(<head> <argument>)`: a call by name, or a construction.
    Apply(&'graph KExpression<'graph>),
    Unevaluable(&'graph KExpression<'graph>),
}

/// A part an evaluation needs: its value, or — an `ATTR` label written bare — the name as written.
#[derive(Clone, Copy)]
pub(super) enum Wanted<'graph> {
    Evaluated(&'graph ExpressionPart<'graph>),
    Label(BinderSymbol),
}

/// Where gathering a node's parts got to.
enum Gathered<'x, 'graph, 'here> {
    Asked(Asked),
    Ready(BumpVec<'x, Operand<'graph, 'here>>),
    /// A part's value was an error value.
    Raised(KValue<'graph, 'here>),
    Broken(StepError),
}

/// The step.
pub(super) fn evaluate<'graph>(
    step: Step<'_, 'graph, '_, '_, '_, KBundle>,
) -> Action<'graph, KBundle> {
    let (step, state) = step.state();
    let (step, _) = step.scratch();
    let (birth, stage) = match state {
        KState::Born(birth) => (birth, BORN),
        KState::Evaluating { birth, stage } => (birth, stage),
        KState::Runner(_) => return step.failed(StepError::Refused),
    };
    let KBirth::Evaluate {
        program,
        node,
        view,
        contract,
    } = birth
    else {
        return step.failed(StepError::Refused);
    };
    let at = Evaluation {
        program,
        view,
        contract,
        birth,
    };
    if stage == FINISHING {
        return finishing(step, &at);
    }
    match form(view.shape(), node) {
        Form::Leaf(part) => leaf(step, &at, part, stage),
        Form::Block(shape) => {
            let mut step = step;
            let asked = step.spawn(block(program, shape, view, Use::Forwards));
            park(step, &at, asked, FINISHING)
        }
        Form::Lambda(node) => {
            let site = Site::of_body(node).expect("a `FN` has a body");
            let scratch = Bump::new();
            let writer = step.writer();
            let value = match lambda(writer, &view, site, program.types(), &scratch) {
                Ok(member) => Value::Knotted(member),
                Err(refused) => {
                    program.error(writer, refused.display(program.symbols(), program.types()))
                }
            };
            finish(step, &at, value)
        }
        Form::Declaration => finish(step, &at, Value::Null),
        Form::Ascribe(node) => ascribe(step, &at, node, stage),
        Form::Eval(node) => evaluated(step, &at, node, stage),
        Form::Using(node) => using(step, &at, node, stage),
        Form::Call(node, list) => call(step, &at, node, list, stage),
        Form::Apply(node) => {
            let [head, argument] = node.parts else {
                unreachable!("an application is a head and its argument")
            };
            apply(step, &at, &head.value, &argument.value, stage)
        }
        Form::Unevaluable(node) => {
            let error = Raised::Unevaluable { node }.raise(program, step.writer());
            finish(step, &at, error)
        }
    }
}

/// What `node`, read off `shape`, is.
pub(super) fn form<'graph>(
    shape: &'graph BodyShape<'graph>,
    node: Evaluated<'graph>,
) -> Form<'graph> {
    match node {
        Evaluated::Statement(expression) => of_node(shape, expression),
        Evaluated::Part(part) => of_part(shape, part),
    }
}

pub(super) fn of_part<'graph>(
    shape: &'graph BodyShape<'graph>,
    part: &'graph ExpressionPart<'graph>,
) -> Form<'graph> {
    match part {
        ExpressionPart::Expression(node) => match shape.nested(Site::of(part)) {
            Some(nested) if nested.kind() == ShapeKind::Block => Form::Block(nested),
            _ => of_node(shape, node.reference()),
        },
        // A mark says where the use it wraps resolved, which the candidate list already holds.
        ExpressionPart::MarkedUse(_, node) => of_node(shape, node.reference()),
        part => Form::Leaf(part),
    }
}

pub(super) fn of_node<'graph>(
    shape: &'graph BodyShape<'graph>,
    node: &'graph KExpression<'graph>,
) -> Form<'graph> {
    let builtin = node.cache().builtin_shape().map(|builtin| builtin.id);
    match builtin {
        Some(BuiltinShapeId::Lambda | BuiltinShapeId::QuantifiedLambda) => {
            return Form::Lambda(node);
        }
        Some(BuiltinShapeId::AscribeTransparent | BuiltinShapeId::AscribeOpaque) => {
            return Form::Ascribe(node);
        }
        Some(BuiltinShapeId::Eval) => return Form::Eval(node),
        Some(BuiltinShapeId::UsingScope) => return Form::Using(node),
        Some(BuiltinShapeId::BucketDeclaration) => return Form::Declaration,
        _ => {}
    }
    if let Some(list) = shape.candidates(Site::of_node(node)) {
        return Form::Call(node, list);
    }
    if builtin.is_some() {
        return Form::Unevaluable(node);
    }
    let keyword = |part: &ExpressionPart<'_>| matches!(part, ExpressionPart::Keyword(_));
    match node.parts {
        [only] => of_part(shape, &only.value),
        [head, argument] if !keyword(&head.value) && !keyword(&argument.value) => Form::Apply(node),
        _ => Form::Unevaluable(node),
    }
}

/// A leaf part: read, lowered or born in place, or built from its parts.
fn leaf<'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_>,
    at: &Evaluation<'graph, 'here>,
    part: &'graph ExpressionPart<'graph>,
    stage: u32,
) -> Action<'graph, KBundle> {
    let program = at.program;
    let types = program.types();
    let scratch = Bump::new();
    match part {
        ExpressionPart::Literal(_)
        | ExpressionPart::Identifier(_)
        | ExpressionPart::Type(_)
        | ExpressionPart::MarkedName(..)
        | ExpressionPart::QuotedExpression(_) => match in_place(&step, at, part, &scratch) {
            Some(value) => finish(step, at, value),
            None => step.failed(StepError::Refused),
        },
        ExpressionPart::SigiledTypeExpr(_) | ExpressionPart::RecordType(_) => {
            let writer = step.writer();
            let value = match denoted(part, &at.view, types, &scratch) {
                Ok(handle) => Value::Type(TypeValue::new(writer, handle, types)),
                Err(refused) => program.error(writer, refused.display(program.symbols(), types)),
            };
            finish(step, at, value)
        }
        ExpressionPart::ListLiteral(_)
        | ExpressionPart::DictLiteral(_)
        | ExpressionPart::RecordLiteral(_) => container(step, at, part, stage),
        ExpressionPart::Keyword(_)
        | ExpressionPart::Expression(_)
        | ExpressionPart::MarkedUse(..) => step.failed(StepError::Refused),
    }
}

/// `<value> :! <Type>`: the operand checked against the type — where the load did not settle it —
/// and retyped to it. A module operand ascribed a signature, or under `:|`, is seen as it through
/// the [view door](crate::knot::module::view), which checks it fits; `:|` over anything else
/// raises.
fn ascribe<'graph, 'here>(
    mut step: Taking<'_, 'graph, '_, 'here, '_>,
    at: &Evaluation<'graph, 'here>,
    node: &'graph KExpression<'graph>,
    stage: u32,
) -> Action<'graph, KBundle> {
    let program = at.program;
    let types = program.types();
    let scratch = Bump::new();
    let [operand, _, ascribed] = node.parts else {
        unreachable!("an ascription has an operand, its keyword and a type")
    };
    let wanted = [Wanted::Evaluated(&operand.value)];
    let operands = match gathered(&mut step, at, &wanted, stage, &scratch) {
        Gathered::Ready(operands) => operands,
        other => return unready(step, at, other),
    };
    let [Operand::Value(value)] = operands[..] else {
        unreachable!("an ascription's operand is evaluated")
    };
    let writer = step.writer();
    let ascribed = match denoted(&ascribed.value, &at.view, types, &scratch) {
        Ok(ascribed) => ascribed,
        Err(refused) => {
            let error = program.error(writer, refused.display(program.symbols(), types));
            return finish(step, at, error);
        }
    };
    let mode = match node.cache().builtin_shape().map(|shape| shape.id) {
        Some(BuiltinShapeId::AscribeOpaque) => Ascription::Opaque,
        _ => Ascription::Transparent,
    };
    let signature = matches!(
        types.node(ascribed),
        TypeNode::Signature { .. }
            | TypeNode::SignatureApply { .. }
            | TypeNode::SignatureMeet { .. }
    );
    if let Some(module) = value
        .as_module()
        .filter(|_| signature || mode == Ascription::Opaque)
    {
        let raised = match view::ascribe(writer, module, ascribed, mode, types, &scratch) {
            Ok(view) => return finish(step, at, Value::Knotted(view)),
            Err(Unascribable::Unsatisfied(_)) => Raised::Unascribable {
                value: value.concrete_ktype(),
                ascribed,
            },
            Err(Unascribable::NotASignature(ascribed)) => Raised::NotASignature { ascribed },
            Err(Unascribable::Coercion { name, refused }) => Raised::Coercion {
                name: name.symbol(),
                refused,
            },
            Err(Unascribable::NotAModule) => unreachable!("the operand is a module"),
        };
        return finish(step, at, raised.raise(program, writer));
    }
    if mode == Ascription::Opaque {
        let raised = Raised::NotAModule {
            value: value.concrete_ktype(),
        };
        return finish(step, at, raised.raise(program, writer));
    }
    if at.view.shape().settled(Site::of_node(node)) {
        debug_assert!(
            satisfies(ascribed, &value, types, &scratch),
            "a settled ascription's operand satisfies it"
        );
    } else if !satisfies(ascribed, &value, types, &scratch) {
        let raised = Raised::Unascribable {
            value: value.concrete_ktype(),
            ascribed,
        };
        return finish(step, at, raised.raise(program, writer));
    }
    finish(step, at, value.retyped(writer, ascribed, types, &scratch))
}

/// `USING <module> SCOPE <body>`: the body run as a block whose parameters are bound to what the
/// module surfaces, its last statement's value its own.
fn using<'graph, 'here>(
    mut step: Taking<'_, 'graph, '_, 'here, '_>,
    at: &Evaluation<'graph, 'here>,
    node: &'graph KExpression<'graph>,
    stage: u32,
) -> Action<'graph, KBundle> {
    let program = at.program;
    let scratch = Bump::new();
    let [_, operand, _, _] = node.parts else {
        unreachable!("a `USING … SCOPE` has its keyword, its module, `SCOPE` and a body")
    };
    let wanted = [Wanted::Evaluated(&operand.value)];
    let operands = match gathered(&mut step, at, &wanted, stage, &scratch) {
        Gathered::Ready(operands) => operands,
        other => return unready(step, at, other),
    };
    let [Operand::Value(module)] = operands[..] else {
        unreachable!("a `USING`'s operand is evaluated")
    };
    // The shape surfaces only an operand it reads a module's declaration off.
    if module.as_module().is_none() {
        let raised = Raised::NotAModule {
            value: module.concrete_ktype(),
        };
        let error = raised.raise(program, step.writer());
        return finish(step, at, error);
    }
    let body = Site::of_body(node)
        .and_then(|site| at.view.shape().nested(site))
        .expect("a `USING … SCOPE` body is a block its shape holds");
    let request = surfaced_block(program, body, at.view, Some(module), Use::Forwards);
    let asked = step.spawn(request);
    park(step, at, asked, FINISHING)
}

/// `EVAL <code> -> <Type>`: the code's shape checked for overlaps as a loaded program's is, then
/// run in a frame over the names and keys the `EVAL` offers, owing the declared type as a called
/// frame owes its return. An operand that is no code, and every refusal, is an error value.
fn evaluated<'graph, 'here>(
    mut step: Taking<'_, 'graph, '_, 'here, '_>,
    at: &Evaluation<'graph, 'here>,
    node: &'graph KExpression<'graph>,
    stage: u32,
) -> Action<'graph, KBundle> {
    let program = at.program;
    let (types, symbols) = (program.types(), program.symbols());
    let scratch = Bump::new();
    let [_, operand, _, declared] = node.parts else {
        unreachable!("an `EVAL` has its keyword, its code, `->` and a type")
    };
    let wanted = [Wanted::Evaluated(&operand.value)];
    let operands = match gathered(&mut step, at, &wanted, stage, &scratch) {
        Gathered::Ready(operands) => operands,
        other => return unready(step, at, other),
    };
    let [Operand::Value(value)] = operands[..] else {
        unreachable!("an `EVAL`'s operand is evaluated")
    };
    let writer = step.writer();
    let Some(code) = value.as_code() else {
        let raised = Raised::NotCode {
            value: value.concrete_ktype(),
        };
        return finish(step, at, raised.raise(program, writer));
    };
    let returns = match denoted(&declared.value, &at.view, types, &scratch) {
        Ok(returns) => returns,
        Err(refused) => {
            let error = program.error(writer, refused.display(symbols, types));
            return finish(step, at, error);
        }
    };
    let shape = code.code().expect("a quote's code").shape();
    if let Err(error) = check::overlaps(shape, program.builtins(), types, &scratch) {
        return finish(
            step,
            at,
            program.error(writer, error.display(symbols, types)),
        );
    }
    let offered = offered(at, writer, &operand.value, &scratch);
    match crate::program::eval(program, code, offered, returns, Use::Forwards) {
        Ok(request) => {
            let asked = step.spawn(request);
            park(step, at, asked, FINISHING)
        }
        Err(refused) => {
            let error = program.error(writer, refused.display(symbols, types));
            finish(step, at, error)
        }
    }
}

/// The record of the names and keys an `EVAL` offers the code `operand` holds — a key as the list
/// of its functions, as a use at the key written at the `EVAL` resolves it.
fn offered<'graph, 'here>(
    at: &Evaluation<'graph, 'here>,
    writer: Writer<'here>,
    operand: &'graph ExpressionPart<'graph>,
    scratch: &Bump,
) -> KValue<'graph, 'here> {
    let types = at.program.types();
    let offers = at.view.shape().offers(Site::of(operand));
    let mut fields = BumpVec::with_capacity_in(offers.len(), scratch);
    for (name, offer) in offers {
        let offered = match offer {
            Offer::Name(coordinate) => at.view.read(*coordinate),
            Offer::Key(list) => {
                let mut functions = BumpVec::new_in(scratch);
                for candidate in list.candidates {
                    match candidate {
                        Candidate::One(coordinate) => functions.push(at.view.read(*coordinate)),
                        Candidate::Spread(coordinate) => {
                            let spread = at.view.read(*coordinate);
                            if spread.as_list().is_some() {
                                functions.extend(builtins::listed(spread, types, scratch));
                            }
                        }
                    }
                }
                Value::List(List::of_candidates(writer, functions.iter().copied()))
            }
        };
        fields.push((*name, offered));
    }
    Value::Record(Record::new(writer, &fields, types, scratch))
}

/// A list, dict or record literal: lowered whole when every part is a literal, and otherwise built
/// from its parts' values.
fn container<'graph, 'here>(
    mut step: Taking<'_, 'graph, '_, 'here, '_>,
    at: &Evaluation<'graph, 'here>,
    part: &'graph ExpressionPart<'graph>,
    stage: u32,
) -> Action<'graph, KBundle> {
    let types = at.program.types();
    let scratch = Bump::new();
    if stage == BORN
        && let Some(value) = Value::lower_part(step.writer(), part, types, &scratch)
    {
        return finish(step, at, value);
    }
    let mut wanted = BumpVec::new_in(&scratch);
    match part {
        ExpressionPart::ListLiteral(items) => {
            wanted.extend(items.iter().map(Wanted::Evaluated));
        }
        ExpressionPart::DictLiteral(pairs) => {
            for (key, value) in pairs.iter() {
                wanted.extend([Wanted::Evaluated(key), Wanted::Evaluated(value)]);
            }
        }
        ExpressionPart::RecordLiteral(fields) => {
            wanted.extend(fields.iter().map(|(_, value)| Wanted::Evaluated(value)));
        }
        _ => unreachable!("a container is a list, dict or record literal"),
    }
    let operands = match gathered(&mut step, at, &wanted, stage, &scratch) {
        Gathered::Ready(operands) => operands,
        other => return unready(step, at, other),
    };
    let writer = step.writer();
    let mut values = BumpVec::with_capacity_in(operands.len(), &scratch);
    values.extend(
        operands
            .iter()
            .map(|operand| operand.value().expect("a container's parts are evaluated")),
    );
    let value = match part {
        ExpressionPart::ListLiteral(_) => {
            Value::List(List::new(writer, values.iter().copied(), types, &scratch))
        }
        ExpressionPart::DictLiteral(_) => {
            let mut entries = BumpVec::with_capacity_in(values.len() / 2, &scratch);
            for pair in values.chunks(2) {
                match Key::of(&pair[0]) {
                    Ok(key) => entries.push((key, pair[1])),
                    Err(rejected) => {
                        let error = Raised::NotAKey { rejected }.raise(at.program, writer);
                        return finish(step, at, error);
                    }
                }
            }
            Value::Dict(Dict::new(writer, &entries, types, &scratch))
        }
        ExpressionPart::RecordLiteral(fields) => {
            let mut named = BumpVec::with_capacity_in(fields.len(), &scratch);
            named.extend(
                fields
                    .iter()
                    .map(|(name, _)| *name)
                    .zip(values.iter().copied()),
            );
            Value::Record(Record::new(writer, &named, types, &scratch))
        }
        _ => unreachable!("a container is a list, dict or record literal"),
    };
    finish(step, at, value)
}

/// A keyworded call: its slots evaluated, a candidate selected, and the builtin's native run or the
/// registration's function called.
fn call<'graph, 'here>(
    mut step: Taking<'_, 'graph, '_, 'here, '_>,
    at: &Evaluation<'graph, 'here>,
    node: &'graph KExpression<'graph>,
    list: &'graph CandidateList<'graph>,
    stage: u32,
) -> Action<'graph, KBundle> {
    let program = at.program;
    let types = program.types();
    let scratch = Bump::new();
    let wanted = slots(node, &scratch);
    let operands = match gathered(&mut step, at, &wanted, stage, &scratch) {
        Gathered::Ready(operands) => operands,
        other => return unready(step, at, other),
    };
    let writer = step.writer();
    let mut arguments = BumpVec::with_capacity_in(operands.len(), &scratch);
    arguments.extend(operands.iter().map(Operand::ktype));
    let shape = at.view.shape();
    let contributed = contributed(at, shape.contributions(Site::of_node(node)), &scratch);
    let narrowing = shape.narrowing(Site::of_node(node));
    let selection = match narrowing {
        Narrowing::Full => {
            let maybe = list.candidates.iter().map(|c| (*c, Verdict::Maybe));
            select::selected(at, maybe, &arguments, &contributed, &scratch)
        }
        Narrowing::Kept(kept) => {
            select::selected(at, kept.iter().copied(), &arguments, &contributed, &scratch)
        }
        Narrowing::Selected(coordinate) => {
            select::chosen(at, coordinate, &arguments, &contributed, &scratch)
        }
    };
    let raised = match selection {
        Selection::Builtin(builtin) => {
            let native = Native::of(builtin.id());
            let value = builtins::run(native, at, node, writer, &operands, &scratch);
            return finish(step, at, value);
        }
        Selection::Function {
            callee,
            registered,
            solution,
        } => {
            debug_assert!(
                select::carried_fit(types, &scratch, registered.shape, solution, &arguments),
                "each argument carries a type under its slot at the solution"
            );
            let arguments =
                select::arguments(types, writer, registered, &operands, solution, &scratch);
            let call = |owed| {
                crate::program::call(
                    program,
                    callee,
                    arguments,
                    CallKind::Keyworded,
                    &[],
                    owed,
                    Use::Forwards,
                )
            };
            // A call through a barrier never tails: its value crosses the barrier where its
            // frame ends.
            let barrier = callee.as_callable().and_then(Knotted::coerced).is_some();
            if let Some(contract) = at.contract
                && !barrier
                && select::keeps(types, registered.shape, solution, contract)
            {
                let request = call(Some(contract));
                return step.tail(request.placement, request.work);
            }
            let asked = step.spawn(call(None));
            return park(step, at, asked, FINISHING);
        }
        Selection::NoOverload => Raised::NoOverload {
            key: list.elements,
            arguments: &arguments,
        },
        Selection::Ambiguous(count) => Raised::Ambiguous {
            key: list.elements,
            arguments: &arguments,
            count,
        },
    };
    let error = raised.raise(program, writer);
    finish(step, at, error)
}

/// What each recorded contribution is where the call runs: a closed type as it is, a rigid one at
/// the types the run binds its variables to, and `None` where the call reads the carried type.
fn contributed<'x, 'graph>(
    at: &Evaluation<'graph, '_>,
    recorded: &[StaticType<'graph>],
    scratch: &'x Bump,
) -> BumpVec<'x, Option<KType>> {
    let types = at.program.types();
    let mut contributed = BumpVec::with_capacity_in(recorded.len(), scratch);
    contributed.extend(recorded.iter().map(|each| {
        match each {
            Static::Unknown => None,
            known => Some(
                known
                    .solved(&at.view, scratch, |value, bindings| {
                        types.concrete(substitute_levels(types, scratch, value, bindings))
                    })
                    .expect("a contribution's variables are bound where its call runs"),
            ),
        }
    }));
    contributed
}

/// The parts of a keyworded node its call needs: every slot evaluated, save an `ATTR` label
/// written bare, which is read as written.
pub(super) fn slots<'x, 'graph>(
    node: &'graph KExpression<'graph>,
    scratch: &'x Bump,
) -> BumpVec<'x, Wanted<'graph>> {
    let mut wanted = BumpVec::with_capacity_in(node.parts.len(), scratch);
    let bare = |part: &ExpressionPart<'_>| match part {
        ExpressionPart::Identifier(name) => Some(BinderSymbol::Value(*name)),
        ExpressionPart::Type(name) => Some(BinderSymbol::Type(*name)),
        _ => None,
    };
    match node.cache().builtin_shape() {
        Some(shape) => {
            for (role, part) in shape.roles().zip(node.parts) {
                match (role, bare(&part.value)) {
                    (Role::Keyword, _) => {}
                    (Role::Field, Some(label)) => wanted.push(Wanted::Label(label)),
                    _ => wanted.push(Wanted::Evaluated(&part.value)),
                }
            }
        }
        None => wanted.extend(
            node.parts
                .iter()
                .filter(|part| !matches!(part.value, ExpressionPart::Keyword(_)))
                .map(|part| Wanted::Evaluated(&part.value)),
        ),
    }
    wanted
}

/// `(head argument)`: a construction when the head is a type, and otherwise a call by name of the
/// head over the argument record.
fn apply<'graph, 'here>(
    mut step: Taking<'_, 'graph, '_, 'here, '_>,
    at: &Evaluation<'graph, 'here>,
    head: &'graph ExpressionPart<'graph>,
    argument: &'graph ExpressionPart<'graph>,
    stage: u32,
) -> Action<'graph, KBundle> {
    let program = at.program;
    let types = program.types();
    let scratch = Bump::new();
    let wanted = [Wanted::Evaluated(head), Wanted::Evaluated(argument)];
    let operands = match gathered(&mut step, at, &wanted, stage, &scratch) {
        Gathered::Ready(operands) => operands,
        other => return unready(step, at, other),
    };
    let recorded = at.view.shape().named_contributions(Site::of(argument));
    let [Operand::Value(head), Operand::Value(argument)] = operands[..] else {
        unreachable!("both parts of an application are evaluated")
    };
    let writer = step.writer();
    if let Value::Type(head) = head {
        let value = match Tagged::construct(writer, head, argument, types, &scratch) {
            Ok(tagged) => Value::Tagged(tagged),
            Err(refused) => program.error(
                writer,
                refused_construction(&refused, program.symbols(), types),
            ),
        };
        return finish(step, at, value);
    }
    // What the frame solves each parameter from, in symbol order: `None` where its carried type.
    let contributed = collect(writer, contributed(at, recorded, &scratch).iter().copied());
    let call = |owed| {
        crate::program::call(
            program,
            head,
            argument,
            CallKind::ByName,
            contributed,
            owed,
            Use::Forwards,
        )
    };
    if let Some(contract) = at.contract
        && returns_within(program, head, contract.returns)
    {
        let request = call(Some(contract));
        return step.tail(request.placement, request.work);
    }
    let asked = step.spawn(call(None));
    park(step, at, asked, FINISHING)
}

/// Whether `callee` is an unquantified function whose declared return satisfies `returns`.
fn returns_within(
    program: &Program<'_>,
    callee: KValue<'_, '_>,
    returns: crate::type_lattice::KType,
) -> bool {
    let Some(function) = callee.as_callable().and_then(Knotted::function) else {
        return false;
    };
    let DeclaredType::Type(ktype) = function.ktype() else {
        return false;
    };
    match program.types().node(ktype) {
        TypeNode::KFunction { ret, .. } => {
            satisfied_by(program.types(), &Bump::new(), returns, ret)
        }
        _ => false,
    }
}

/// Check that `value`, unless it is an error value, carries a type within its node's static type:
/// under its upper end read above its variables, and over its lower end read below them, as a
/// candidate's judgement reads an argument's.
#[cfg(debug_assertions)]
fn carried_under_static<'graph, 'here>(
    at: &Evaluation<'graph, 'here>,
    value: &KValue<'graph, 'here>,
) {
    if at.program.message(value).is_some() {
        return;
    }
    let shape = at.view.shape();
    let KBirth::Evaluate { node, .. } = at.birth else {
        return;
    };
    let expected = match node {
        Evaluated::Part(part) => shape.value_type(Site::of(part)),
        // A statement lies in its shape's body run, so its index is its offset there.
        Evaluated::Statement(statement) => (std::ptr::from_ref(statement) as usize)
            .checked_sub(shape.body().as_ptr() as usize)
            .and_then(|offset| shape.statement_type(offset / std::mem::size_of_val(statement))),
    };
    let Some(expected) = expected else {
        return;
    };
    let types = at.program.types();
    let scratch = Bump::new();
    let carried = value.ktype();
    debug_assert!(
        satisfied_by(
            types,
            &scratch,
            bound_above(types, &scratch, expected.upper),
            carried
        ) && !lower_end_outside(types, &scratch, expected, carried),
        "the carried type lies within the load-time static type"
    );
}

/// Whether `part` is read in place rather than asked for.
fn read_in_place(part: &ExpressionPart<'_>) -> bool {
    matches!(
        part,
        ExpressionPart::Literal(_)
            | ExpressionPart::Identifier(_)
            | ExpressionPart::Type(_)
            | ExpressionPart::MarkedName(..)
            | ExpressionPart::QuotedExpression(_)
    )
}

/// The value of a part read in place: a literal lowered, a name read through its mention — a
/// quantified function at the instance the load solved there, where it solved one — a quote born.
/// `None` for a name the shape resolved no mention of, which is an invariant break.
fn in_place<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_>,
    at: &Evaluation<'graph, 'here>,
    part: &'graph ExpressionPart<'graph>,
    scratch: &Bump,
) -> Option<KValue<'graph, 'here>> {
    let writer = step.writer();
    let types = at.program.types();
    match part {
        ExpressionPart::Literal(_) => Value::lower_part(writer, part, types, scratch),
        ExpressionPart::QuotedExpression(_) => {
            Some(Value::Knotted(quote(writer, &at.view, part, scratch)))
        }
        _ => {
            let shape = at.view.shape();
            let mention = shape.mention(Site::of(part))?;
            let read = at.view.read(mention.coordinate);
            let Some(solution) = shape.instance_at(Site::of(part)) else {
                return Some(read);
            };
            let Value::Knotted(member) = read else {
                unreachable!("an instance site reads a quantified function")
            };
            Some(Value::Knotted(instance(
                writer, member, solution, &at.view, types, scratch,
            )))
        }
    }
}

/// Ask for every wanted part not read in place, or — once they are in, or when none needed asking —
/// every operand, in order. An error value among them is handed back alone.
fn gathered<'x, 'graph, 'here>(
    step: &mut Taking<'_, 'graph, '_, 'here, '_>,
    at: &Evaluation<'graph, 'here>,
    wanted: &[Wanted<'graph>],
    stage: u32,
    scratch: &'x Bump,
) -> Gathered<'x, 'graph, 'here> {
    let asked_for = |wanted: &&Wanted<'graph>| matches!(wanted, Wanted::Evaluated(part) if !read_in_place(part));
    if stage == BORN {
        let mut asked = None;
        for wanted in wanted.iter().filter(asked_for) {
            let Wanted::Evaluated(part) = wanted else {
                unreachable!("only an evaluated part is asked for")
            };
            asked = Some(step.spawn(Request {
                placement: Placement::Shares,
                use_: Use::Keeps,
                work: at.program.evaluate(Evaluated::Part(part), at.view, None),
            }));
        }
        if let Some(asked) = asked {
            return Gathered::Asked(asked);
        }
    }
    let mut received = BumpVec::new_in(scratch);
    for result in step.results(at.program.types()) {
        match result {
            Ok(Received::Here(value)) => {
                if at.program.message(&value).is_some() {
                    return Gathered::Raised(value);
                }
                received.push(value);
            }
            Ok(Received::Scratch(_)) => {
                unreachable!("a part is asked for with `Keeps`, which never delivers to scratch")
            }
            Err(error) => return Gathered::Broken(error),
        }
    }
    let mut received = received.into_iter();
    let mut operands = BumpVec::with_capacity_in(wanted.len(), scratch);
    for wanted in wanted {
        let operand = match wanted {
            Wanted::Label(name) => Operand::Label(*name),
            Wanted::Evaluated(part) if read_in_place(part) => {
                match in_place(step, at, part, scratch) {
                    Some(value) => Operand::Value(value),
                    None => return Gathered::Broken(StepError::Refused),
                }
            }
            Wanted::Evaluated(_) => match received.next() {
                Some(value) => Operand::Value(value),
                None => return Gathered::Broken(StepError::Unredeemable),
            },
        };
        operands.push(operand);
    }
    Gathered::Ready(operands)
}

/// Gathering that did not come back ready: park on what it asked for, finish with the error value
/// a part came to, or fail.
fn unready<'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_>,
    at: &Evaluation<'graph, 'here>,
    gathered: Gathered<'_, 'graph, 'here>,
) -> Action<'graph, KBundle> {
    match gathered {
        Gathered::Asked(asked) => park(step, at, asked, GATHERED),
        Gathered::Raised(error) => step.finish(error, at.program.types()),
        Gathered::Broken(error) => step.failed(error),
        Gathered::Ready(_) => unreachable!("a ready gathering is not unready"),
    }
}

/// Park on the children asked for, resuming at `stage`.
fn park<'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_>,
    at: &Evaluation<'graph, 'here>,
    asked: Asked,
    stage: u32,
) -> Action<'graph, KBundle> {
    step.park(
        asked,
        evaluate,
        KState::Evaluating {
            birth: at.birth,
            stage,
        },
        None,
    )
}

/// The one child whose value is this evaluation's is in: finish with it.
fn finishing<'graph, 'here>(
    mut step: Taking<'_, 'graph, '_, 'here, '_>,
    at: &Evaluation<'graph, 'here>,
) -> Action<'graph, KBundle> {
    let received = step.results(at.program.types()).next();
    match received {
        Some(Ok(Received::Here(value))) => finish(step, at, value),
        _ => step.failed(StepError::Unredeemable),
    }
}

/// Finish with `value`, held to the contract the evaluation was born under.
fn finish<'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_>,
    at: &Evaluation<'graph, 'here>,
    value: KValue<'graph, 'here>,
) -> Action<'graph, KBundle> {
    #[cfg(debug_assertions)]
    carried_under_static(at, &value);
    let value = match at.contract {
        Some(contract) => at.program.fulfilled(step.writer(), value, contract),
        None => value,
    };
    step.finish(value, at.program.types())
}
