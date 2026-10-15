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
//!   native, or a registration's function in a frame; a member read `ATTR <operand> <label>` with
//!   its label bare is one, which the load reads as a possible instance site;
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
//! In a caching body the step runs on the body's chain ([`evaluate`]), asks for blocks with `Keeps`,
//! and adopts what leaves the body into its region through an [`Exit`]: a call's callee, arguments,
//! contributions and the contract it hands on, an `EVAL`'s code, offered record and declared type,
//! and a value owed a frame's contract. Under `routes` an invariant view's operand and view, and a
//! warm composite's own memo, are adopted into the warm tier.
//!
//! [`statics`](super::statics) reads nodes through this file's [`Form`], so the load types each node
//! as it evaluates. Debug builds check that a finished value's carried type lies within its node's
//! static type; the narrowing law (`tests/narrowing.rs`) is what holds a narrowed call to full
//! selection.

use crate::elaborate::{bound_path, denoted};
use crate::knot::module::view::{self, Ascription, Unascribable};
use crate::knot::{KValue, Knotted, KnottedFamily, instance, lambda, quote, refused_construction};
use crate::memory::{Bump, BumpVec, Writer, collect};
use crate::parse::BuiltinShapeId;
use crate::parse::Role;
use crate::parse::{ExpressionPart, KExpression};
use crate::program::{CallKind, Evaluated, Exit, KBirth, KBundle, KState, block, surfaced_block};
use crate::scheduler::{
    Action, Placement, Received, Request, Slot as Asked, Step, StepError, Taken, Use,
};
use crate::scope::{
    BodyShape, Candidate, CandidateList, InstanceRead, Mint, Narrowing, Offer, ShapeKind, Site,
    Static, StaticType,
};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{KType, Verdict, substitute_levels};
#[cfg(debug_assertions)]
use crate::type_lattice::{bound_above, lower_end_outside, satisfied_by};
use crate::values::{
    Chain, Dict, Key, List, Record, Tagged, TypeValue, Value, adopt_value, satisfies,
};

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
type Taking<'a, 'graph, 'step, 'here, 'scratch, 'lent> =
    Step<'a, 'graph, 'step, 'here, 'scratch, 'lent, KBundle, Taken, Taken>;

/// What a node is to the evaluator.
#[derive(Clone, Copy)]
pub(super) enum Form<'graph> {
    Leaf(&'graph ExpressionPart<'graph>),
    /// A name: an identifier, a type name or a marked name, read through its mention.
    Name(&'graph ExpressionPart<'graph>),
    Block(&'graph BodyShape<'graph>),
    Lambda(&'graph KExpression<'graph>),
    /// `<value> :! <Type>`, or `<module> :| <Sig>`, beside which operator it is.
    Ascribe(&'graph KExpression<'graph>, Ascription),
    /// `EVAL <code> -> <Type>`.
    Eval(&'graph KExpression<'graph>),
    /// `USING <module> SCOPE <body>`.
    Using(&'graph KExpression<'graph>),
    Declaration,
    Call(&'graph KExpression<'graph>, &'graph CandidateList<'graph>),
    /// `ATTR <operand> <label>` with its label written bare: a member read.
    Member {
        node: &'graph KExpression<'graph>,
        list: &'graph CandidateList<'graph>,
        operand: &'graph ExpressionPart<'graph>,
        label: BinderSymbol,
    },
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

/// The step. It runs on one chain, as the body runner's steps do ([`run`](crate::program::run)):
/// the reading chain where it routes, the `Shares` chain elsewhere in a caching body, and its
/// region's anywhere else.
pub(super) fn evaluate<'graph>(
    step: Step<'_, 'graph, '_, '_, '_, '_, KBundle>,
) -> Action<'graph, KBundle> {
    let (step, state) = step.state();
    let (step, _) = step.scratch();
    let (birth, stage) = match state {
        KState::Born(birth) => (birth, BORN),
        KState::Evaluating { birth, stage } => (birth, stage),
        KState::Runner(_) => return step.failed(StepError::Refused),
    };
    let KBirth::Evaluate { view, within, .. } = birth else {
        return step.failed(StepError::Refused);
    };
    #[cfg(test)]
    if within.routes || view.shape().caches() {
        let address = std::ptr::from_ref(view.shape()).addr();
        crate::scheduler::CACHING.set(Some(address));
    }
    if within.routes {
        let types = step
            .reading_types()
            .expect("a routing evaluation has a lender");
        evaluate_on(step, birth, stage, types)
    } else if view.shape().caches() {
        let types = step.shares_types();
        evaluate_on(step, birth, stage, types)
    } else {
        let types = step.types();
        evaluate_on(step, birth, stage, types)
    }
}

/// [`evaluate`] on the chain `types` it picked.
fn evaluate_on<'graph, 'here, 'tier, 'reg>(
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    birth: KBirth<'graph, 'here>,
    stage: u32,
    types: Chain<'graph, 'tier, 'here, 'reg, '_>,
) -> Action<'graph, KBundle> {
    let KBirth::Evaluate {
        program,
        node,
        view,
        contract,
        within,
    } = birth
    else {
        unreachable!("an evaluation is born to evaluate")
    };
    let at = Evaluation {
        program,
        view,
        contract,
        within,
        birth,
        types,
    };
    if stage == FINISHING {
        return finishing(step, &at);
    }
    match form(view.shape(), node) {
        Form::Leaf(part) => leaf(step, &at, part, stage),
        Form::Name(part) => read(step, &at, part),
        Form::Block(shape) => {
            let mut step = step;
            let asked = step.spawn(block(program, shape, view, inside(&at), within));
            park(step, &at, asked, FINISHING)
        }
        Form::Lambda(node) => {
            let site = Site::of_body(node).expect("a `FN` has a body");
            let scratch = Bump::new();
            let writer = step.writer();
            let value = match lambda(writer, &view, site, within.build, &at.types, &scratch) {
                Ok(member) => Value::Knotted(member),
                Err(refused) => program.error(
                    writer,
                    &at.types,
                    refused.display(program.symbols(), &at.types),
                ),
            };
            finish(step, &at, value)
        }
        Form::Declaration => finish(step, &at, Value::Null),
        Form::Ascribe(node, mode) => ascribe(step, &at, node, mode, stage),
        Form::Eval(node) => evaluated(step, &at, node, stage),
        Form::Using(node) => using(step, &at, node, stage),
        Form::Call(node, list) | Form::Member { node, list, .. } => {
            call(step, &at, node, list, stage)
        }
        Form::Apply(node) => {
            let [head, argument] = node.parts else {
                unreachable!("an application is a head and its argument")
            };
            apply(step, &at, node, &head.value, &argument.value, stage)
        }
        Form::Unevaluable(node) => {
            let error = Raised::Unevaluable { node }.raise(program, step.writer(), &at.types);
            finish(step, &at, error)
        }
    }
}

/// How an evaluation asks for a block whose value is its own: kept in a caching body, so a value
/// leaves the body only where it adopts, and forwarded elsewhere.
fn inside(at: &Evaluation<'_, '_, '_, '_, '_>) -> Use {
    match at.view.shape().caches() {
        true => Use::Keeps,
        false => Use::Forwards,
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
        part if part.is_name() => Form::Name(part),
        part => Form::Leaf(part),
    }
}

pub(super) fn of_node<'graph>(
    shape: &'graph BodyShape<'graph>,
    node: &'graph KExpression<'graph>,
) -> Form<'graph> {
    let builtin_shape = node.cache().builtin_shape();
    let builtin = builtin_shape.map(|builtin| builtin.id);
    match builtin {
        Some(BuiltinShapeId::Lambda | BuiltinShapeId::QuantifiedLambda) => {
            return Form::Lambda(node);
        }
        Some(BuiltinShapeId::AscribeTransparent) => {
            return Form::Ascribe(node, Ascription::Transparent);
        }
        Some(BuiltinShapeId::AscribeOpaque) => return Form::Ascribe(node, Ascription::Opaque),
        Some(BuiltinShapeId::Eval) => return Form::Eval(node),
        Some(BuiltinShapeId::UsingScope) => return Form::Using(node),
        Some(BuiltinShapeId::BucketDeclaration) => return Form::Declaration,
        _ => {}
    }
    if let Some(list) = shape.candidates(Site::of_node(node)) {
        if let Some(attribute) =
            builtin_shape.filter(|builtin| builtin.id == BuiltinShapeId::Attribute)
            && let [_, operand, _] = node.parts
            && let Some(label) = (attribute.roles().zip(node.parts))
                .find_map(|(role, part)| role.label_reads(&part.value))
        {
            return Form::Member {
                node,
                list,
                operand: &operand.value,
                label,
            };
        }
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
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'graph, 'here, '_, '_, '_>,
    part: &'graph ExpressionPart<'graph>,
    stage: u32,
) -> Action<'graph, KBundle> {
    let program = at.program;
    let types = &at.types;
    let scratch = Bump::new();
    match part {
        ExpressionPart::Literal(_) | ExpressionPart::QuotedExpression(_) => read(step, at, part),
        ExpressionPart::SigiledTypeExpr(_) | ExpressionPart::RecordType(_) => {
            let writer = step.writer();
            let value = match denoted(part, &at.view, types, &scratch) {
                Ok(handle) => Value::Type(TypeValue::new(writer, handle, types)),
                Err(refused) => {
                    program.error(writer, types, refused.display(program.symbols(), types))
                }
            };
            finish(step, at, value)
        }
        ExpressionPart::ListLiteral(_)
        | ExpressionPart::DictLiteral(_)
        | ExpressionPart::RecordLiteral(_) => container(step, at, part, stage),
        ExpressionPart::Keyword(_)
        | ExpressionPart::Expression(_)
        | ExpressionPart::MarkedUse(..) => step.failed(StepError::Refused),
        _ => unreachable!("a name is a `Form::Name`, read by `read`"),
    }
}

/// A part read in place ([`in_place`]) as this evaluation's value: a literal, a name or a quote.
fn read<'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'graph, 'here, '_, '_, '_>,
    part: &'graph ExpressionPart<'graph>,
) -> Action<'graph, KBundle> {
    match in_place(&step, at, part, &Bump::new()) {
        Some(value) => finish(step, at, value),
        None => step.failed(StepError::Refused),
    }
}

/// `<value> :! <Type>`: the operand checked against the type — where the load did not settle it —
/// and retyped to it, a module re-stamped at the application it fits. `<module> :| <Sig>` is seen
/// as it through the [view door](crate::knot::module::view), which checks it fits; `:|` over
/// anything else raises.
fn ascribe<'graph, 'here>(
    mut step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'graph, 'here, '_, '_, '_>,
    node: &'graph KExpression<'graph>,
    mode: Ascription,
    stage: u32,
) -> Action<'graph, KBundle> {
    let program = at.program;
    let types = &at.types;
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
            let error = program.error(writer, types, refused.display(program.symbols(), types));
            return finish(step, at, error);
        }
    };
    // `:|` views its operand; `:!` retypes it.
    if mode == Ascription::Opaque {
        debug_assert!(
            at.view.shape().mints_at(Site::of_node(node)),
            "the load recorded the opaque view at {:?} as a minting site",
            node.source,
        );
        if value.as_module().is_none() {
            let raised = Raised::NotAModule {
                value: value.concrete_ktype(),
            };
            return finish(step, at, raised.raise(program, writer, &at.types));
        }
        // Under `routes` an invariant site's operand and view are adopted into the warm tier, so
        // the next hop finds both there and the verdicts over them stay warm.
        let invariant = at.view.shape().mint_at(Site::of_node(node)) == Some(Mint::Invariant);
        let warm = (at.within.routes && invariant)
            .then(|| step.warm_types())
            .flatten();
        if let Some(warm) = &warm {
            adopt_value::<KnottedFamily>(warm, types, &value, &[]);
        }
        let raised = match view::ascribe(writer, value, ascribed, types, &scratch) {
            Ok(view) => {
                if let Some(warm) = &warm {
                    adopt_value::<KnottedFamily>(warm, types, &Value::Knotted(view), &[]);
                }
                // The load lays a view's types over a closed tree, so that tree is the view's
                // content.
                debug_assert!(
                    at.view
                        .shape()
                        .view_tree(Site::of_node(node))
                        .and_then(|tree| tree.digest())
                        .is_none_or(|digest| digest
                            == Value::Knotted(view).digest(types, &scratch)),
                    "the content tree at {:?} reads the digest the run computes",
                    node.source,
                );
                return finish(step, at, Value::Knotted(view));
            }
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
        return finish(step, at, raised.raise(program, writer, &at.types));
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
        return finish(step, at, raised.raise(program, writer, &at.types));
    }
    finish(step, at, value.retyped(writer, ascribed, types, &scratch))
}

/// `USING <module> SCOPE <body>`: the body run as a block whose parameters are bound to what the
/// module surfaces, its last statement's value its own.
fn using<'graph, 'here>(
    mut step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'graph, 'here, '_, '_, '_>,
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
        let error = raised.raise(program, step.writer(), &at.types);
        return finish(step, at, error);
    }
    let body = Site::of_body(node)
        .and_then(|site| at.view.shape().nested(site))
        .expect("a `USING … SCOPE` body is a block its shape holds");
    let request = surfaced_block(program, body, at.view, Some(module), inside(at), at.within);
    let asked = step.spawn(request);
    park(step, at, asked, FINISHING)
}

/// `EVAL <code> -> <Type>`: the code's shape checked for overlaps as a loaded program's is, then
/// run in a frame over the names and keys the `EVAL` offers, owing the declared type as a called
/// frame owes its return. An operand that is no code, and every refusal, is an error value.
fn evaluated<'graph, 'here>(
    mut step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'graph, 'here, '_, '_, '_>,
    node: &'graph KExpression<'graph>,
    stage: u32,
) -> Action<'graph, KBundle> {
    let program = at.program;
    let (types, symbols) = (&at.types, program.symbols());
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
        return finish(step, at, raised.raise(program, writer, &at.types));
    };
    let returns = match denoted(&declared.value, &at.view, types, &scratch) {
        Ok(returns) => returns,
        Err(refused) => {
            let error = program.error(writer, types, refused.display(symbols, types));
            return finish(step, at, error);
        }
    };
    let shape = code.code().expect("a quote's code").shape();
    if let Err(error) = check::overlaps(shape, program.builtins(), types, &scratch) {
        return finish(
            step,
            at,
            program.error(writer, types, error.display(symbols, types)),
        );
    }
    let offered = offered(at, writer, &operand.value, &scratch);
    exits(&step, at, &[], |exit| {
        exit.value(&value);
        exit.value(&offered);
        exit.handle(returns);
    });
    match crate::program::eval(program, &at.types, code, offered, returns, Use::Forwards) {
        Ok(request) => {
            let asked = step.spawn(request);
            park(step, at, asked, FINISHING)
        }
        Err(refused) => {
            let error = program.error(writer, types, refused.display(symbols, types));
            finish(step, at, error)
        }
    }
}

/// The record of the names and keys an `EVAL` offers the code `operand` holds — a key as the list
/// of its functions, as a use at the key written at the `EVAL` resolves it.
fn offered<'graph, 'here>(
    at: &Evaluation<'graph, 'here, '_, '_, '_>,
    writer: Writer<'here, '_>,
    operand: &'graph ExpressionPart<'graph>,
    scratch: &Bump,
) -> KValue<'graph, 'here> {
    let types = &at.types;
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
    mut step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'graph, 'here, '_, '_, '_>,
    part: &'graph ExpressionPart<'graph>,
    stage: u32,
) -> Action<'graph, KBundle> {
    let types = &at.types;
    let scratch = Bump::new();
    if stage == BORN
        && let Some(value) = Value::lower_part(step.writer(), part, types, &scratch)
    {
        warmed(&step, at, Site::of(part), &value);
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
                        let error =
                            Raised::NotAKey { rejected }.raise(at.program, writer, &at.types);
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
    warmed(&step, at, Site::of(part), &value);
    finish(step, at, value)
}

/// Under `routes`, adopt the own memo of `value`, built at `site`, into the warm tier where the
/// load found the site warm — a composite whose parts read invariant views alone — so the next hop
/// finds it there. Only the memo: a warm composite's parts are no deeper walk's.
fn warmed<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'graph, 'here, '_, '_, '_>,
    site: Site,
    value: &KValue<'graph, 'here>,
) {
    if at.within.routes
        && at.view.shape().warm_at(site)
        && let Some(warm) = step.warm_types()
    {
        warm.adopt_from(&at.types, value.ktype());
    }
}

/// A keyworded call: its slots evaluated, a candidate selected, and the builtin's native run or the
/// registration's function called.
fn call<'graph, 'here>(
    mut step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'graph, 'here, '_, '_, '_>,
    node: &'graph KExpression<'graph>,
    list: &'graph CandidateList<'graph>,
    stage: u32,
) -> Action<'graph, KBundle> {
    let program = at.program;
    let types = &at.types;
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
    let fixed = shape.use_statics(Site::of_node(node));
    let contributed = contributed(at, fixed.contributions, &scratch);
    let selection = match fixed.narrowing {
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
            let tails = at.contract.filter(|contract| {
                callee.as_callable().and_then(Knotted::coerced).is_none()
                    && contract.kept_by(types, registered.shape, Some(solution))
            });
            let passed = passed_operands(at, &wanted, &operands, &scratch);
            exits(&step, at, &passed, |exit| {
                exit.value(&callee);
                exit.value(&arguments);
                tails.iter().for_each(|owed| exit.contract(owed));
            });
            let call = |owed| {
                crate::program::call(
                    program,
                    &at.types,
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
            if let Some(contract) = tails {
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
    let error = raised.raise(program, writer, &at.types);
    finish(step, at, error)
}

/// What each recorded contribution is where the call runs: a closed type as it is, a rigid one at
/// the types the run binds its variables to, and `None` where the call reads the carried type.
fn contributed<'x, 'graph>(
    at: &Evaluation<'graph, '_, '_, '_, '_>,
    recorded: &[StaticType<'graph>],
    scratch: &'x Bump,
) -> BumpVec<'x, Option<KType>> {
    let types = &at.types;
    let mut contributed = BumpVec::with_capacity_in(recorded.len(), scratch);
    contributed.extend(recorded.iter().map(|each| {
        match each {
            Static::Unknown => None,
            known => Some(
                known
                    .solved(
                        &at.view,
                        scratch,
                        &|at_, entry| bound_path(&at.view, types, at_, entry),
                        |value, bindings| {
                            types.concrete(substitute_levels(types, scratch, value, bindings))
                        },
                    )
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
    match node.cache().builtin_shape() {
        Some(shape) => {
            for (role, part) in shape.roles().zip(node.parts) {
                if role == Role::Keyword {
                    continue;
                }
                wanted.push(match role.label_reads(&part.value) {
                    Some(label) => Wanted::Label(label),
                    None => Wanted::Evaluated(&part.value),
                });
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
    mut step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'graph, 'here, '_, '_, '_>,
    node: &'graph KExpression<'graph>,
    head: &'graph ExpressionPart<'graph>,
    argument: &'graph ExpressionPart<'graph>,
    stage: u32,
) -> Action<'graph, KBundle> {
    let program = at.program;
    let types = &at.types;
    let scratch = Bump::new();
    let wanted = [Wanted::Evaluated(head), Wanted::Evaluated(argument)];
    let operands = match gathered(&mut step, at, &wanted, stage, &scratch) {
        Gathered::Ready(operands) => operands,
        other => return unready(step, at, other),
    };
    let recorded = at.view.shape().named_contributions(Site::of(argument));
    let part = argument;
    let [Operand::Value(head), Operand::Value(argument)] = operands[..] else {
        unreachable!("both parts of an application are evaluated")
    };
    let writer = step.writer();
    if let Value::Type(head) = head {
        let value = match Tagged::construct(writer, head, argument, types, &scratch) {
            Ok(tagged) => {
                let value = Value::Tagged(tagged);
                warmed(&step, at, Site::of_node(node), &value);
                value
            }
            Err(refused) => program.error(
                writer,
                types,
                refused_construction(&refused, program.symbols(), types),
            ),
        };
        return finish(step, at, value);
    }
    // What the frame solves each parameter from, in symbol order: `None` where its carried type.
    let contributed = collect(writer, contributed(at, recorded, &scratch).iter().copied());
    // A call keeps its contract by the type its callee shows; a barrier never tails, since its
    // value crosses the barrier where its frame ends.
    let tails = at.contract.filter(|contract| {
        (head.as_callable().and_then(Knotted::function)).is_some()
            && contract.kept_by(&at.types, head.ktype(), None)
    });
    let passed = passed_fields(at, part, argument, &scratch);
    exits(&step, at, &passed, |exit| {
        exit.value(&head);
        exit.value(&argument);
        contributed.iter().flatten().for_each(|kt| exit.handle(*kt));
        tails.iter().for_each(|owed| exit.contract(owed));
    });
    let call = |owed| {
        crate::program::call(
            program,
            &at.types,
            head,
            argument,
            CallKind::ByName,
            contributed,
            owed,
            Use::Forwards,
        )
    };
    if let Some(contract) = tails {
        let request = call(Some(contract));
        return step.tail(request.placement, request.work);
    }
    let asked = step.spawn(call(None));
    park(step, at, asked, FINISHING)
}

/// Check that `value`, unless it is an error value, carries a type within its node's static type:
/// under its upper end read above its variables, and over its lower end read below them, as a
/// candidate's judgement reads an argument's.
#[cfg(debug_assertions)]
fn carried_under_static<'graph, 'here>(
    at: &Evaluation<'graph, 'here, '_, '_, '_>,
    value: &KValue<'graph, 'here>,
) {
    if at.program.message(value, &at.types).is_some() {
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
    let types = &at.types;
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
    part.is_name()
        || matches!(
            part,
            ExpressionPart::Literal(_) | ExpressionPart::QuotedExpression(_)
        )
}

/// The value of a part read in place: a literal lowered, a name read through its mention — a
/// quantified function at the instance the load solved there, where it solved one — a quote born.
/// `None` for a name the shape resolved no mention of, which is an invariant break.
fn in_place<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'graph, 'here, '_, '_, '_>,
    part: &'graph ExpressionPart<'graph>,
    scratch: &Bump,
) -> Option<KValue<'graph, 'here>> {
    let writer = step.writer();
    let types = &at.types;
    match part {
        ExpressionPart::Literal(_) => Value::lower_part(writer, part, types, scratch),
        ExpressionPart::QuotedExpression(_) => {
            Some(Value::Knotted(quote(writer, &at.view, part, scratch)))
        }
        _ => {
            let shape = at.view.shape();
            let mention = shape.mention(Site::of(part))?;
            let read = at.view.read(mention.coordinate);
            let Some(InstanceRead::Solved(solution)) = shape.instance_at(Site::of(part)) else {
                return Some(read);
            };
            let Value::Knotted(member) = read else {
                unreachable!(
                    "an instance site reads a bare quantified function: a header at a scheme \
                     comes only from a module member read, which makes its own instance"
                )
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
    step: &mut Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'graph, 'here, '_, '_, '_>,
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
            asked = Some(
                step.spawn(Request {
                    placement: Placement::Shares,
                    use_: Use::Keeps,
                    work: at
                        .program
                        .evaluate(Evaluated::Part(part), at.view, None, at.within),
                }),
            );
        }
        if let Some(asked) = asked {
            return Gathered::Asked(asked);
        }
    }
    let mut received = BumpVec::new_in(scratch);
    for result in step.results_with(&at.types) {
        match result {
            Ok(Received::Here(value)) => {
                if at.program.message(&value, &at.types).is_some() {
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
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'graph, 'here, '_, '_, '_>,
    gathered: Gathered<'_, 'graph, 'here>,
) -> Action<'graph, KBundle> {
    match gathered {
        Gathered::Asked(asked) => park(step, at, asked, GATHERED),
        Gathered::Raised(error) => delivered(step, at, error),
        Gathered::Broken(error) => step.failed(error),
        Gathered::Ready(_) => unreachable!("a ready gathering is not unready"),
    }
}

/// Park on the children asked for, resuming at `stage`.
fn park<'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'graph, 'here, '_, '_, '_>,
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
    mut step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'graph, 'here, '_, '_, '_>,
) -> Action<'graph, KBundle> {
    let received = step.results_with(&at.types).next();
    match received {
        Some(Ok(Received::Here(value))) => finish(step, at, value),
        _ => step.failed(StepError::Unredeemable),
    }
}

/// Finish with `value`, held to the contract the evaluation was born under.
fn finish<'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'graph, 'here, '_, '_, '_>,
    value: KValue<'graph, 'here>,
) -> Action<'graph, KBundle> {
    #[cfg(debug_assertions)]
    carried_under_static(at, &value);
    let value = match at.contract {
        Some(contract) => at
            .program
            .fulfilled(step.writer(), &at.types, value, contract),
        None => value,
    };
    delivered(step, at, value)
}

/// Deliver `value` to the evaluation's asker. In a caching body a value owed a frame's contract
/// leaves it, so it is adopted first; any other stays inside, and its debug check reads the body's
/// chain.
fn delivered<'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'graph, 'here, '_, '_, '_>,
    value: KValue<'graph, 'here>,
) -> Action<'graph, KBundle> {
    match (at.view.shape().caches(), at.contract) {
        (true, Some(_)) => {
            exits(&step, at, &[], |exit| exit.value(&value));
            step.finish(value)
        }
        (true, None) => step.finish_with(value, &at.types),
        (false, _) => step.finish(value),
    }
}

/// In a caching body, hand `adopt` the [`Exit`] a value leaving it is adopted through, into the
/// step's region, beside the values `passed` that it passes on and its frame received; elsewhere
/// nothing leaves a scratch tier, and `adopt` is not called.
fn exits<'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'graph, 'here, '_, '_, '_>,
    passed: &[KValue<'graph, 'here>],
    adopt: impl FnOnce(&Exit<'_, '_, '_, 'graph, 'here, '_>),
) {
    if at.view.shape().caches() {
        let scratch = Bump::new();
        adopt(&Exit::new(
            step.types(),
            &at.types,
            &at.view,
            passed,
            &scratch,
        ));
    }
}

/// The operands of a keyworded call, evaluated from `wanted`, that the load recorded as passing on
/// what the frame body received.
fn passed_operands<'x, 'graph, 'here>(
    at: &Evaluation<'graph, 'here, '_, '_, '_>,
    wanted: &[Wanted<'graph>],
    operands: &[Operand<'graph, 'here>],
    scratch: &'x Bump,
) -> BumpVec<'x, KValue<'graph, 'here>> {
    let shape = at.view.shape();
    let mut passed = BumpVec::new_in(scratch);
    for (wanted, operand) in wanted.iter().zip(operands) {
        if let (Wanted::Evaluated(part), Operand::Value(value)) = (wanted, operand)
            && shape.received_at(Site::of(part))
        {
            passed.push(*value);
        }
    }
    passed
}

/// What the argument `value` of a call by name, written `part`, passes on that the load recorded
/// the frame body received: the whole, or a record literal's fields.
fn passed_fields<'x, 'graph, 'here>(
    at: &Evaluation<'graph, 'here, '_, '_, '_>,
    part: &'graph ExpressionPart<'graph>,
    value: KValue<'graph, 'here>,
    scratch: &'x Bump,
) -> BumpVec<'x, KValue<'graph, 'here>> {
    let shape = at.view.shape();
    let mut passed = BumpVec::new_in(scratch);
    if shape.received_at(Site::of(part)) {
        passed.push(value);
    } else if let ExpressionPart::RecordLiteral(fields) = part {
        for (name, field) in fields.iter() {
            if shape.received_at(Site::of(field))
                && let Some(seen) = value.field(name.symbol(), &at.types, scratch)
            {
                passed.push(seen.value());
            }
        }
    }
    passed
}
