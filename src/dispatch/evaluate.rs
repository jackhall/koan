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
//! as it evaluates. Debug builds check both halves of that agreement: a finished value's carried type
//! lies under its node's static type, and a narrowed or selected call runs what selection over the
//! full list would.

use crate::elaborate::type_expression;
use crate::knot::{KValue, Knotted, lambda, quote, refused_construction};
use crate::memory::{Bump, BumpVec};
use crate::parse::builtin_shapes::BuiltinShapeId;
use crate::parse::builtin_shapes::role::Role;
use crate::parse::{ExpressionPart, KExpression};
use crate::program::{CallKind, Evaluated, KBirth, KBundle, KState, Program, block};
use crate::scheduler::{
    Action, Placement, Received, Request, Slot as Asked, Step, StepError, Taken, Use,
};
use crate::scope::{BodyShape, CandidateList, Narrowing, ShapeKind, Site};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{TypeNode, Verdict, bound_above, satisfied_by, substitute_levels};
use crate::values::{Dict, Key, List, Record, Tagged, TypeValue, Value};

use super::builtins::{self, Native, Ran};
use super::errors::Raised;
use super::select::{self, Selection};
use super::{Evaluation, Operand};

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
    Declaration,
    Call(&'graph KExpression<'graph>, &'graph CandidateList<'graph>),
    Apply(
        &'graph ExpressionPart<'graph>,
        &'graph ExpressionPart<'graph>,
    ),
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
        Form::Call(node, list) => call(step, &at, node, list, stage),
        Form::Apply(head, argument) => apply(step, &at, head, argument, stage),
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
        [head, argument] if !keyword(&head.value) && !keyword(&argument.value) => {
            Form::Apply(&head.value, &argument.value)
        }
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
            // The load fixed the type where it could; only what it left unknown is elaborated here.
            let loaded = at.view.shape().typed_expression(Site::of(part)).solved(
                &at.view,
                &scratch,
                |value, bindings| substitute_levels(types, &scratch, value, bindings),
            );
            debug_assert!(
                loaded.is_none() || loaded == type_expression(part, &at.view, types, &scratch).ok(),
                "the load-time type agrees with elaborating where it runs"
            );
            let value = match loaded
                .map_or_else(|| type_expression(part, &at.view, types, &scratch), Ok)
            {
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
                match Key::of(&pair[0], types, &scratch) {
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
    let narrowing = at.view.shape().narrowing(Site::of_node(node));
    let full = || {
        let maybe = list.candidates.iter().map(|c| (*c, Verdict::Maybe));
        select::selected(at, maybe, &arguments, &scratch)
    };
    let selection = match narrowing {
        Narrowing::Full => full(),
        Narrowing::Kept(kept) => select::selected(at, kept.iter().copied(), &arguments, &scratch),
        Narrowing::Selected(coordinate) => select::chosen(at, coordinate, &arguments, &scratch),
    };
    #[cfg(debug_assertions)]
    if !matches!(narrowing, Narrowing::Full) {
        debug_assert!(
            select::agree(&selection, &full()),
            "static selection runs what full selection would"
        );
    }
    let raised = match selection {
        Selection::Builtin(builtin) => {
            return match builtins::run(Native::of(builtin.id()), at, writer, node, &operands) {
                Ran::Value(value) => finish(step, at, value),
                Ran::Frame(request) => {
                    let asked = step.spawn(request);
                    park(step, at, asked, FINISHING)
                }
            };
        }
        Selection::Function {
            callee,
            registered,
            solution,
        } => {
            let arguments =
                select::arguments(types, writer, registered, &operands, solution, &scratch);
            let call = |owed| {
                crate::program::call(
                    program,
                    callee,
                    arguments,
                    CallKind::Keyworded,
                    owed,
                    Use::Forwards,
                )
            };
            if let Some(contract) = at.contract
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
    let call = |owed| {
        crate::program::call(
            program,
            head,
            argument,
            CallKind::ByName,
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
    match program.types().node(function.ktype()) {
        TypeNode::KFunction {
            quantifiers: [],
            ret,
            ..
        } => satisfied_by(program.types(), &Bump::new(), returns, ret),
        _ => false,
    }
}

/// Check that `value`, unless it is an error value, carries a type within its node's static type.
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
        ) && (types.contains_rigid(expected.lower)
            || satisfied_by(types, &scratch, carried, expected.lower)),
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

/// The value of a part read in place: a literal lowered, a name read through its mention, a quote
/// born. `None` for a name the shape resolved no mention of, which is an invariant break.
fn in_place<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_>,
    at: &Evaluation<'graph, 'here>,
    part: &'graph ExpressionPart<'graph>,
    scratch: &Bump,
) -> Option<KValue<'graph, 'here>> {
    let writer = step.writer();
    match part {
        ExpressionPart::Literal(_) => Value::lower_part(writer, part, at.program.types(), scratch),
        ExpressionPart::QuotedExpression(_) => {
            Some(Value::Knotted(quote(writer, &at.view, part, scratch)))
        }
        _ => {
            let mention = at.view.shape().mention(Site::of(part))?;
            Some(at.view.read(mention.coordinate))
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
    for result in step.results() {
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
        Gathered::Raised(error) => step.finish(error),
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
    let received = step.results().next();
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
    step.finish(value)
}
