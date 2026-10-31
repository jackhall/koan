//! The step every evaluation runs: what a node is, and how its value is reached.
//!
//! A node is read off the shape that holds it — never the parse — as one of:
//!
//! - a **leaf**: a literal, lowered; a name, read through its mention's coordinate; a quote, born
//!   through the [quote door](crate::knot::quote); a list, dict or record literal, its parts
//!   evaluated and the container built; a type expression, elaborated to a type value;
//! - a **block** a pairwise operator run was rewritten into, run through the
//!   [block door](crate::program::block) with its last statement's value its own;
//! - a `FN`, born through the [lambda door](crate::knot::lambda), an untraced body over the code
//!   its body part evaluates to, asked for first;
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
//! - an **application** `(head argument)`: a construction when the head is a type, a read when it is
//!   a list, a dict or a string ([`lookup`](super::lookup)), and otherwise a call by name of the
//!   head over the argument record.
//!
//! A part the node needs is read in place when it is a literal, a name or a quote, and otherwise
//! asked for — a tenant of this cell, kept — all at one park; the wake reads the same parts again.
//! Each value is held beside the kept part it is where it has one ([`Held`]): a name its frame keeps
//! a carrier of, a capture of the frame's callee as that part of its kept callee, a child's pinned
//! result, and a field or module member read over any of them, its label or slot appended to the
//! path. A value finished with a kept part crosses from its true home, and every call hands its
//! callee and arguments on through theirs ([`Passing`]). A construction — a list,
//! dict or record literal, a newtype's, or a read — is built in the evaluation's home over its parts, which
//! it asks for forwarded, so it is never built here only to be crossed whole. A frame's hand-off
//! asks for each part of the frame body's threaded set with `Threads`, so what its tail call hands
//! on that the frame built is built in the loop's cell, and marks those arguments in the skeleton
//! it lays.
//! An error value received from a part is this evaluation's value, unchanged. Born under a frame's
//! [`Contract`], the evaluation owes the frame its value: a call whose callee's declared return
//! satisfies the contract hops to the callee's frame, and every other value is held to the contract
//! where it is finished.
//!
//! In a caching body the step runs on the body's chain ([`evaluate`]), asks for blocks with `Keeps`,
//! and adopts what leaves the body into its region through an [`Exit`]: a call's callee, arguments,
//! contributions and the contract it hands on, an `EVAL`'s code, offered record and declared type,
//! and a value owed a frame's contract. A step lays nothing in the agent tier: an invariant site's
//! types are there already, rested by the settlement its frame runs under.
//!
//! A tail call reads its route, which the load's walk wrote beside it where it knows the callee:
//! whether each candidate keeps the frame's contract, where the frame's return and the candidate's
//! solution are closed, and whether it hops into the frame's own body. Only what the route leaves
//! open, and whether the callee is behind a barrier, is decided at the call.
//!
//! A call to a body keyed on its bindings is handed to the drain with what it seeks ([`seek`]), so
//! its frame is born under its settlement; a hop its route marks runs under the frame's, with no
//! lookup. The call reads its callee's keys once.
//!
//! [`statics`](super::statics) reads nodes through this file's [`Form`], so the load types each node
//! as it evaluates. Debug builds check that a finished value's carried type lies within its node's
//! static type; the narrowing law (`tests/narrowing.rs`) is what holds a narrowed call to full
//! selection.

use crate::knot::module::layout;
use crate::knot::module::view::{self, Ascription, Unascribable};
use crate::knot::{KValue, Knotted, flattened, instance, lambda, quote, refused_construction};
use crate::memory::{Active, Bump, BumpAllocator, BumpVec, Writer, collect, resident};
use crate::parse::BuiltinShapeId;
use crate::parse::Role;
use crate::parse::{
    ExpressionPart, KExpression, KLiteral, KTemplate, ListItem, TemplateSlot, template_fields,
};
use crate::program::{
    CallKind, Contract, Evaluated, Exit, Handed, KBirth, KBundle, KState, Passing, block,
    called_function, seek, surfaced_block,
};
use crate::scheduler::{
    Action, Held, KeptPart, Placement, Received, Request, Slot as Asked, Step, StepError, Taken,
    Use,
};
use crate::scope::{
    AtRun, BodyShape, CandidateList, CandidateRoute, Coordinate, Elaboration, Elimination,
    HandedTo, InstanceRead, Located, Narrowing, ParameterBinding, ShapeKind, Site, StaticType,
};
use crate::symbols::{BinderSymbol, Symbol, SymbolInterner};
use crate::type_lattice::{KType, TypeNode, Verdict};
#[cfg(debug_assertions)]
use crate::type_lattice::{bound_above, lower_end_outside, satisfied_by};
use crate::values::{
    Chain, Circular, Concatenation, Dict, Key, List, PathSegment, Piece, Record, Tagged, Text,
    TypeValue, Value, satisfies,
};

use super::builtins::{self, Native};
use super::errors::Raised;
use super::lookup;
use super::select::{self, Selection};
use super::{Evaluation, Operand};

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
pub(super) enum Form<'step> {
    Leaf(&'step ExpressionPart<'step>),
    /// A name: an identifier, a type name or a marked name, read through its mention.
    Name(&'step ExpressionPart<'step>),
    Block(&'step BodyShape<'step>),
    Lambda(&'step KExpression<'step>),
    /// `<value> :! <Type>`, or `<module> :| <Sig>`, beside which operator it is.
    Ascribe(&'step KExpression<'step>, Ascription),
    /// `EVAL <code> -> <Type>`.
    Eval(&'step KExpression<'step>),
    /// `USING <module> SCOPE <body>`.
    Using(&'step KExpression<'step>),
    Declaration,
    Call(&'step KExpression<'step>, &'step CandidateList<'step>),
    /// `ATTR <operand> <label>` with its label written bare: a member read.
    Member {
        node: &'step KExpression<'step>,
        list: &'step CandidateList<'step>,
        operand: &'step ExpressionPart<'step>,
        label: BinderSymbol,
    },
    /// `(<head> <argument>)`: a call by name, or a construction.
    Apply(&'step KExpression<'step>),
    Unevaluable(&'step KExpression<'step>),
}

/// A part an evaluation needs: its value, or — an `ATTR` label written bare — the name as written.
#[derive(Clone, Copy)]
pub(super) enum Wanted<'step> {
    Evaluated(&'step ExpressionPart<'step>),
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
/// the caching chain where it routes or its body caches, and its region's anywhere else.
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
    let (code, facts) = step.door();
    let shape = code.shape(view.segment());
    #[cfg(test)]
    if within.routes || facts.caches(shape) {
        let address = std::ptr::from_ref(shape).addr();
        crate::scheduler::CACHING.set(Some(address));
    }
    if within.routes || facts.caches(shape) {
        let types = step.caching_types();
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
    types: Chain<'tier, 'here, 'reg, '_>,
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
    let (code, facts) = step.door();
    let view = view.reading(&code, facts);
    let at = Evaluation {
        program,
        code,
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
            let block = block(program, shape, view.view(), inside(&at), within);
            let asked = step.spawn(block);
            park(step, &at, asked, FINISHING)
        }
        Form::Lambda(node) => born_lambda(step, &at, node, stage),
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

/// A `FN` no binder names, born through the lambda door: an untraced body over the code its body
/// part evaluates to, asked for first. A refused birth — a part that is no code among them — is an
/// error value.
fn born_lambda<'step, 'graph, 'here>(
    mut step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
    node: &'step KExpression<'step>,
    stage: u32,
) -> Action<'graph, KBundle> {
    let program = at.program;
    let shape = at.view.shape();
    let site = shape.site_of_body(node).expect("a `FN` has a body");
    let scratch = Bump::new();
    let code = match shape.nested(site).is_some_and(|body| body.deferred()) {
        false => None,
        true => {
            let Located::Part(part) = shape.located(site) else {
                unreachable!("a body's site locates its part")
            };
            let wanted = [Wanted::Evaluated(part)];
            match gathered(&mut step, at, &wanted, Use::Keeps, stage, &scratch) {
                Gathered::Ready(operands) => match operands[..] {
                    [Operand::Value(Held { value, .. })] => Some(value),
                    _ => unreachable!("an untraced body's part is evaluated"),
                },
                other => return unready(step, at, other),
            }
        }
    };
    let writer = step.writer();
    let written = program.written_under(at.view.facts(), at.within.root);
    let value = match lambda(
        writer,
        &at.view,
        written,
        site,
        code,
        at.within.born(),
        &at.types,
        &scratch,
    ) {
        Ok(member) => Value::Knotted(member),
        Err(refused) => program.error(
            writer,
            &at.types,
            refused.display(program.symbols(), &at.types),
        ),
    };
    finish(step, at, value)
}

/// How an evaluation asks for a block whose value is its own: kept in a caching body, so a value
/// leaves the body only where it adopts, and forwarded elsewhere.
fn inside(at: &Evaluation<'_, '_, '_, '_, '_, '_>) -> Use {
    match at.view.facts().caches(at.view.shape()) {
        true => Use::Keeps,
        false => Use::Forwards,
    }
}

/// What `node`, read off `shape`, is.
pub(super) fn form<'step>(shape: &'step BodyShape<'step>, node: Evaluated) -> Form<'step> {
    match node {
        Evaluated::Statement(index) => of_node(shape, &shape.body()[index as usize]),
        Evaluated::Part(site) => match shape.located(site) {
            Located::Part(part) => of_part(shape, part),
            Located::Node(_) => unreachable!("an evaluated part's site locates a part"),
        },
    }
}

pub(super) fn of_part<'step>(
    shape: &'step BodyShape<'step>,
    part: &'step ExpressionPart<'step>,
) -> Form<'step> {
    match part {
        ExpressionPart::Expression(node) => match shape.nested(shape.site(part)) {
            Some(nested) if nested.kind() == ShapeKind::Block => Form::Block(nested),
            _ => of_node(shape, node),
        },
        // A mark says where the use it wraps resolved, which the candidate list already holds.
        ExpressionPart::MarkedUse(_, node) | ExpressionPart::FilledUse(_, _, node) => {
            of_node(shape, node)
        }
        part if part.is_name() => Form::Name(part),
        part => Form::Leaf(part),
    }
}

pub(super) fn of_node<'step>(
    shape: &'step BodyShape<'step>,
    node: &'step KExpression<'step>,
) -> Form<'step> {
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
    if let Some(list) = shape.candidates(shape.site_of_node(node)) {
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
fn leaf<'step, 'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
    part: &'step ExpressionPart<'step>,
    stage: u32,
) -> Action<'graph, KBundle> {
    let program = at.program;
    let types = &at.types;
    match part {
        ExpressionPart::Literal(_) | ExpressionPart::QuotedExpression(_) => read(step, at, part),
        ExpressionPart::SigiledTypeExpr(_) | ExpressionPart::RecordType(_) => {
            let writer = step.writer();
            let facts = at.view.facts();
            let value = match facts.denoted(types, at.view.shape(), at.view.shape().site(part)) {
                Ok(handle) => Value::Type(TypeValue::new(writer, handle, types)),
                Err(refused) => {
                    program.error(writer, types, refused.display(program.symbols(), types))
                }
            };
            finish(step, at, value)
        }
        ExpressionPart::ListLiteral(_)
        | ExpressionPart::Template(_)
        | ExpressionPart::DictLiteral(_)
        | ExpressionPart::RecordLiteral(_) => container(step, at, part, stage),
        ExpressionPart::Keyword(_)
        | ExpressionPart::Expression(_)
        | ExpressionPart::MarkedUse(..)
        | ExpressionPart::FilledUse(..) => step.failed(StepError::Refused),
        _ => unreachable!("a name is a `Form::Name`, read by `read`"),
    }
}

/// `value`, what a field read of `operands` came to, beside the record operand's kept part
/// extended by the field's label, where the record operand has one and `value` is the part the
/// label reaches or a restamp of it.
fn field_held<'graph, 'here>(
    writer: Writer<'here, '_>,
    operands: &[Operand<'graph, 'here>],
    value: KValue<'graph, 'here>,
) -> Held<'graph, 'here> {
    let [Operand::Value(record), label] = operands else {
        return Held::bare(value);
    };
    let label = match label {
        Operand::Label(name) => name.symbol(),
        Operand::Value(_) => return Held::bare(value),
    };
    let kept = record
        .kept
        .filter(|_| (record.value.along(label)).is_some_and(|stored| value.same_storage(&stored)));
    let kept = kept.map(|part| part.extended(writer, PathSegment::Label(label)));
    Held { value, kept }
}

/// `value`, what a module member read of `operands` came to, beside the module operand's kept part
/// extended by the slot its node stores `value`'s storage at, where the module operand has one and
/// stores it.
fn member_held<'graph, 'here>(
    writer: Writer<'here, '_>,
    operands: &[Operand<'graph, 'here>],
    value: KValue<'graph, 'here>,
) -> Held<'graph, 'here> {
    let [Operand::Value(module), _] = operands else {
        return Held::bare(value);
    };
    let kept = module.kept.and_then(|part| {
        let slot = layout::member_slot(module.value, value)?;
        Some(part.extended(writer, PathSegment::Member(slot as u32)))
    });
    Held { value, kept }
}

/// A part read in place ([`in_place`]) as this evaluation's value: a literal, a name or a quote.
fn read<'step, 'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
    part: &'step ExpressionPart<'step>,
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
fn ascribe<'step, 'graph, 'here>(
    mut step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
    node: &'step KExpression<'step>,
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
    let operands = match gathered(&mut step, at, &wanted, Use::Keeps, stage, &scratch) {
        Gathered::Ready(operands) => operands,
        other => return unready(step, at, other),
    };
    let [Operand::Value(operand)] = operands[..] else {
        unreachable!("an ascription's operand is evaluated")
    };
    let value = operand.value;
    let writer = step.writer();
    let facts = at.view.facts();
    let ascribed = match facts.denoted(
        types,
        at.view.shape(),
        at.view.shape().site(&ascribed.value),
    ) {
        Ok(ascribed) => ascribed,
        Err(refused) => {
            let error = program.error(writer, types, refused.display(program.symbols(), types));
            return finish(step, at, error);
        }
    };
    // `:|` views its operand; `:!` retypes it.
    if mode == Ascription::Opaque {
        debug_assert!(
            at.view
                .facts()
                .mints_at(at.view.shape(), at.view.shape().site_of_node(node)),
            "the load recorded the opaque view at {:?} as a minting site",
            node.source,
        );
        if value.as_module().is_none() {
            let raised = Raised::NotAModule {
                value: value.concrete_ktype(),
            };
            return finish(step, at, raised.raise(program, writer, &at.types));
        }
        if let Some(slots) = at
            .view
            .facts()
            .view_slots(at.view.shape(), at.view.shape().site_of_node(node))
        {
            view::fill_at_view(value, slots, &at.view, types, &scratch);
        }
        let picks = (at.view.facts()).picks(at.view.shape(), at.view.shape().site_of_node(node));
        let raised = match view::ascribe(writer, value, ascribed, picks, types, &scratch) {
            Ok(view) => {
                // The load lays a view's types over a closed tree, so that tree is the view's
                // content.
                debug_assert!(
                    at.view
                        .facts()
                        .view_tree(types, at.view.shape(), at.view.shape().site_of_node(node))
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
    let (facts, site) = (at.view.facts(), at.view.shape().site_of_node(node));
    if facts.settled(at.view.shape(), site) {
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
    let retype = facts.retype(types, at.view.shape(), site);
    let retyped = value.retyped_as(writer, ascribed, retype, types, &scratch);
    finish(step, at, operand.becoming(retyped))
}

/// `USING <module> SCOPE <body>`: the body run as a block whose parameters are bound to what the
/// module surfaces, its last statement's value its own.
fn using<'step, 'graph, 'here>(
    mut step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
    node: &'step KExpression<'step>,
    stage: u32,
) -> Action<'graph, KBundle> {
    let program = at.program;
    let scratch = Bump::new();
    let [_, operand, _, _] = node.parts else {
        unreachable!("a `USING … SCOPE` has its keyword, its module, `SCOPE` and a body")
    };
    let wanted = [Wanted::Evaluated(&operand.value)];
    let operands = match gathered(&mut step, at, &wanted, Use::Keeps, stage, &scratch) {
        Gathered::Ready(operands) => operands,
        other => return unready(step, at, other),
    };
    let [Operand::Value(Held { value: module, .. })] = operands[..] else {
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
    let body = (at.view.shape().site_of_body(node))
        .and_then(|site| at.view.shape().nested(site))
        .expect("a `USING … SCOPE` body is a block its shape holds");
    let request = surfaced_block(
        program,
        body,
        at.view.view(),
        Some((module, at.view.shape().site_of_node(node))),
        inside(at),
        at.within,
    );
    let asked = step.spawn(request);
    park(step, at, asked, FINISHING)
}

/// `EVAL <code> -> <Type>`: the code run in a frame over the names and keys the `EVAL` offers,
/// owing the declared type as a called frame owes its return; the frame reports the code's first
/// registration overlapping a builtin. An operand that is no code, and every refusal, is an error
/// value.
fn evaluated<'step, 'graph, 'here>(
    mut step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
    node: &'step KExpression<'step>,
    stage: u32,
) -> Action<'graph, KBundle> {
    let program = at.program;
    let (types, symbols) = (&at.types, program.symbols());
    let scratch = Bump::new();
    let [_, operand, _, declared] = node.parts else {
        unreachable!("an `EVAL` has its keyword, its code, `->` and a type")
    };
    let wanted = [Wanted::Evaluated(&operand.value)];
    let operands = match gathered(&mut step, at, &wanted, Use::Keeps, stage, &scratch) {
        Gathered::Ready(operands) => operands,
        other => return unready(step, at, other),
    };
    let [Operand::Value(Held { value, .. })] = operands[..] else {
        unreachable!("an `EVAL`'s operand is evaluated")
    };
    let writer = step.writer();
    let Some(code) = value.as_code() else {
        let raised = Raised::NotCode {
            value: value.concrete_ktype(),
        };
        return finish(step, at, raised.raise(program, writer, &at.types));
    };
    // A code view runs laid flat, here in the evaluation's home.
    let code = flattened(writer, code, &at.types);
    let facts = at.view.facts();
    let returns = match facts.denoted(
        types,
        at.view.shape(),
        at.view.shape().site(&declared.value),
    ) {
        Ok(returns) => returns,
        Err(refused) => {
            let error = program.error(writer, types, refused.display(symbols, types));
            return finish(step, at, error);
        }
    };
    let offered = offered(at, writer, &operand.value, &scratch);
    exits(&step, at, &scratch, |exit| {
        exit.value(&value);
        exit.value(&offered);
        exit.handle(returns);
    });
    let door = (&at.code, at.view.facts());
    match crate::program::eval(
        program,
        door,
        &at.types,
        code,
        offered,
        returns,
        at.within.inherit,
        Use::Forwards,
    ) {
        Ok(request) => {
            // Code composed at run time runs under its composition, where its shape lies.
            let composed = (code.code()).and_then(|node| node.flat()?.root());
            let asked = match composed {
                Some(root) => step.spawn_composing(request, root.digest),
                None => step.spawn(request),
            };
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
fn offered<'step, 'graph, 'here>(
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
    writer: Writer<'here, '_>,
    operand: &'step ExpressionPart<'step>,
    scratch: &Bump,
) -> KValue<'graph, 'here> {
    let offers = at.view.shape().offers(at.view.shape().site(operand));
    crate::knot::offered(writer, &at.view, offers, &at.types, scratch)
}

/// A list literal with a spread among its items, over its items' and spread lists' values `parts`:
/// a view concatenating each spread list and the runs of items between them, built in the home as
/// any literal is. A spread of what is no list is an error value.
fn spread<'step, 'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
    part: &'step ExpressionPart<'step>,
    items: &'step [ListItem<'step>],
    parts: &[Held<'graph, 'here>],
) -> Action<'graph, KBundle> {
    let listed = |value: KValue<'_, '_>| {
        matches!(value, Value::List(_))
            || matches!(value.as_circular(), Some((_, Circular::List(_))))
    };
    let spread = items
        .iter()
        .zip(parts)
        .find(|(item, held)| matches!(item, ListItem::Spread(_)) && !listed(held.value));
    if let Some((_, held)) = spread {
        let value = held.value.concrete_ktype();
        let error = Raised::NotAList { value }.raise(at.program, step.writer(), &at.types);
        return finish(step, at, error);
    }
    let shape = at.view.shape();
    let decided = (at.view.facts())
        .value_type(&at.types, shape, shape.site(part))
        .filter(|typed| typed.is_exact())
        .and_then(|typed| at.types.concrete(typed.upper));
    finish_built(step, at, parts, |writer, types, values| {
        let scratch = Bump::new();
        // Each spread is a piece, and so is each run of items between two.
        let mut pieces = BumpVec::new_in(&scratch);
        let mut run = 0;
        for (at, item) in items.iter().enumerate() {
            if let ListItem::Spread(_) = item {
                if run < at {
                    pieces.push(Piece::Items(&values[run..at]));
                }
                pieces.push(Piece::Spread(values[at]));
                run = at + 1;
            }
        }
        if run < values.len() {
            pieces.push(Piece::Items(&values[run..]));
        }
        let list = List::concatenated(writer, &pieces, types, &scratch);
        debug_assert!(
            decided.is_none_or(|ktype| !types.resolves(ktype) || ktype == list.ktype()),
            "the load decides a spread literal's type as the run derives it"
        );
        Value::List(list)
    })
}

/// A list, dict or record literal or a template: lowered whole when every part is a literal, and
/// otherwise built from its parts' values.
fn container<'step, 'graph, 'here>(
    mut step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
    part: &'step ExpressionPart<'step>,
    stage: u32,
) -> Action<'graph, KBundle> {
    let scratch = Bump::new();
    if stage == BORN
        && let Some(value) = lowered(step.writer(), at, part, &scratch)
    {
        return finish(step, at, value);
    }
    let mut wanted = BumpVec::new_in(&scratch);
    match part {
        ExpressionPart::ListLiteral(items) => {
            wanted.extend(items.iter().map(|item| Wanted::Evaluated(item.part())));
        }
        ExpressionPart::DictLiteral(pairs) => {
            for (key, value) in pairs.iter() {
                wanted.extend([Wanted::Evaluated(key), Wanted::Evaluated(value)]);
            }
        }
        ExpressionPart::RecordLiteral(fields) => {
            wanted.extend(fields.iter().map(|(_, value)| Wanted::Evaluated(value)));
        }
        ExpressionPart::Template(template) => {
            wanted.extend(
                template
                    .slots
                    .iter()
                    .map(|slot| Wanted::Evaluated(slot.part())),
            );
        }
        _ => unreachable!("a container is a list, dict or record literal or a template"),
    }
    let operands = match gathered(&mut step, at, &wanted, built(at), stage, &scratch) {
        Gathered::Ready(operands) => operands,
        other => return unready(step, at, other),
    };
    let mut parts = BumpVec::with_capacity_in(operands.len(), &scratch);
    parts.extend(operands.iter().map(|operand| match operand {
        Operand::Value(held) => *held,
        Operand::Label(_) => unreachable!("a container's parts are evaluated"),
    }));
    if let ExpressionPart::DictLiteral(_) = part
        && let Some(rejected) = (parts.chunks(2)).find_map(|pair| Key::admits(&pair[0].value).err())
    {
        let error = Raised::NotAKey { rejected }.raise(at.program, step.writer(), &at.types);
        return finish(step, at, error);
    }
    if let ExpressionPart::ListLiteral(items) = part
        && items.iter().any(|item| item.as_part().is_none())
    {
        return spread(step, at, part, items, &parts);
    }
    // The load typed a list or dict literal whose elements are exact by the fill the run would
    // compute, so the run stamps that type rather than merging its cells' types again.
    let shape = at.view.shape();
    let decided = (at
        .view
        .facts()
        .value_type(&at.types, shape, shape.site(part)))
    .filter(|typed| typed.is_exact())
    .and_then(|typed| at.types.concrete(typed.upper));
    let symbols = at.program.symbols();
    let written = match part {
        ExpressionPart::Template(_) => match written_part(at, part) {
            Some(ExpressionPart::Template(template)) => Some(template.pieces),
            _ => None,
        },
        _ => None,
    };
    finish_built(step, at, &parts, |writer, types, values| {
        let scratch = Bump::new();
        let decided = decided.filter(|ktype| types.resolves(*ktype));
        match part {
            ExpressionPart::ListLiteral(_) => Value::List(match decided {
                Some(ktype) => {
                    debug_assert_eq!(
                        ktype,
                        crate::values::list_type(
                            types,
                            &scratch,
                            values.iter().map(Value::concrete_ktype)
                        ),
                        "the load decides a literal's type as the run fills it"
                    );
                    List::typed(writer, values.iter().copied(), ktype, types)
                }
                None => List::new(writer, values.iter().copied(), types, &scratch),
            }),
            ExpressionPart::DictLiteral(_) => {
                let mut entries = BumpVec::with_capacity_in(values.len() / 2, &scratch);
                for pair in values.chunks(2) {
                    let key =
                        Key::of(&pair[0], writer).expect("a dict literal's keys were checked");
                    entries.push((key, pair[1]));
                }
                Value::Dict(match decided {
                    Some(ktype) => Dict::typed(writer, &entries, ktype, types, &scratch),
                    None => Dict::new(writer, &entries, types, &scratch),
                })
            }
            ExpressionPart::RecordLiteral(fields) => {
                let mut named = BumpVec::with_capacity_in(fields.len(), &scratch);
                named.extend((fields.iter().map(|(name, _)| *name)).zip(values.iter().copied()));
                Value::Record(Record::new(writer, &named, types, &scratch))
            }
            ExpressionPart::Template(template) => {
                templated(writer, types, symbols, template, written, values)
            }
            _ => unreachable!("a container is a list, dict or record literal or a template"),
        }
    })
}

/// What `template` evaluates to over its braces' `values`: its text with each rendered brace's value
/// spliced in as `PRINT` renders it, or, where a brace keeps its value, the record of the strings
/// around the kept values and those values. Each string is a view over its parts; `written` is
/// the template's pieces where the load holds them, shared rather than copied.
fn templated<'graph, 'their>(
    writer: Writer<'their, '_>,
    types: &Chain<'_, 'their, '_, '_>,
    symbols: &SymbolInterner,
    template: &KTemplate<'_>,
    written: Option<&'graph [&'graph str]>,
    values: &[KValue<'graph, 'their>],
) -> KValue<'graph, 'their> {
    let scratch = Bump::new();
    let (mut strings, mut kept) = (BumpVec::new_in(&scratch), BumpVec::new_in(&scratch));
    let mut text = Concatenation::new();
    // A piece is shared where the load holds it; composed code may die first, so its pieces are
    // written into the region the string lands in.
    let piece = |at: usize| match (written, template.pieces[at]) {
        (Some(pieces), _) => Text::Flat(pieces[at]),
        (None, "") => Text::Flat(""),
        (None, piece) => Text::Flat(writer.text(piece)),
    };
    text.push(writer, &scratch, piece(0));
    for (index, (slot, value)) in template.slots.iter().zip(values).enumerate() {
        match slot {
            TemplateSlot::Rendered(_) => {
                text.push(
                    writer,
                    &scratch,
                    value.rendered_text(writer, types, symbols, &scratch),
                );
            }
            TemplateSlot::Kept(_) => {
                strings.push(std::mem::take(&mut text).finish(writer));
                kept.push(*value);
            }
        }
        text.push(writer, &scratch, piece(index + 1));
    }
    if kept.is_empty() {
        return text.finish(writer);
    }
    strings.push(text.finish(writer));
    let [strings_field, values_field] = template_fields();
    let fields = [
        (
            strings_field,
            Value::List(List::new(writer, strings.iter().copied(), types, &scratch)),
        ),
        (
            values_field,
            Value::List(List::new(writer, kept.iter().copied(), types, &scratch)),
        ),
    ];
    Value::Record(Record::new(writer, &fields, types, &scratch))
}

/// A keyworded call: its slots evaluated, a candidate selected, and the builtin's native run or the
/// registration's function called.
fn call<'step, 'graph, 'here>(
    mut step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
    node: &'step KExpression<'step>,
    list: &'step CandidateList<'step>,
    stage: u32,
) -> Action<'graph, KBundle> {
    let program = at.program;
    let types = &at.types;
    let scratch = Bump::new();
    let wanted = slots(node, &scratch);
    let operands = match gathered(&mut step, at, &wanted, Use::Keeps, stage, &scratch) {
        Gathered::Ready(operands) => operands,
        other => return unready(step, at, other),
    };
    let writer = step.writer();
    let mut arguments = BumpVec::with_capacity_in(operands.len(), &scratch);
    arguments.extend(operands.iter().map(Operand::ktype));
    let site = at.view.shape().site_of_node(node);
    let fixed = (at.view.facts()).use_statics(&at.types, at.view.shape(), site);
    let contributed = match contributed(at, site, fixed.contributions, &scratch) {
        Ok(contributed) => contributed,
        Err(refused) => {
            let refused = refused.display(program.symbols(), types);
            let error = program.error(writer, types, refused);
            return finish(step, at, error);
        }
    };
    let selection = match fixed.narrowing {
        Narrowing::Full => {
            let maybe = list.candidates.iter().map(|c| (*c, Verdict::Maybe));
            let open = Elimination::Open;
            select::selected(at, maybe, &[], open, &arguments, &contributed, &scratch)
        }
        Narrowing::Kept(kept) => {
            #[cfg(test)]
            if let Elimination::Table(table) = &kept.elimination {
                select::checked(at, kept.entries, table, &scratch);
            }
            let (entries, points) = (kept.entries.iter().copied(), kept.points);
            let elimination = kept.elimination;
            select::selected(
                at,
                entries,
                points,
                elimination,
                &arguments,
                &contributed,
                &scratch,
            )
        }
        Narrowing::Selected(coordinate) => {
            #[cfg(test)]
            if let Some(chose) = CHOSE.with(std::cell::Cell::get) {
                let shape = at.view.shape();
                chose(shape.segment(), shape.site_of_node(node));
            }
            let site = at.view.shape().site_of_node(node);
            let facts = at.view.facts();
            let points = (facts.selection(&at.types, at.view.shape(), site))
                .expect("a selected use records its points");
            select::chosen(at, coordinate, &points, &arguments, &contributed, &scratch)
        }
    };
    // Only a call in tail position reads its route.
    let route = at
        .contract
        .and_then(|_| (at.view.facts()).route(at.view.shape(), at.view.shape().site_of_node(node)));
    #[cfg(debug_assertions)]
    if let Some(route) = route {
        let reads = match fixed.narrowing {
            Narrowing::Full => list.candidates.len(),
            Narrowing::Kept(kept) => kept.entries.len(),
            Narrowing::Selected(_) => 1,
        };
        debug_assert_eq!(
            route.per.len(),
            reads,
            "a route lies beside the candidates read"
        );
    }
    let raised = match selection {
        Selection::Builtin(builtin) => {
            let native = Native::of(builtin.id());
            // A `USING` over code composed at run time fills its holes under its composition,
            // where its shape lies, and a `BUILD` of unbuilt code types it there. A code view is
            // laid flat first, here in the evaluation's home.
            let source = match native {
                Native::Using => operands[1].value().map(Some),
                Native::Build => Some(None),
                _ => None,
            };
            if let Some(source) = source
                && let Some(code) = operands[0].value()
                && let Some(member) = code.as_code()
                && (native == Native::Using || unbuilt(member, &at.types))
                && let flat = flattened(writer, member, &at.types)
                && let Some(root) = (flat.code()).and_then(|node| node.flat()?.root())
            {
                let code = if flat == member {
                    code
                } else {
                    Value::Knotted(flat)
                };
                let request =
                    crate::program::under_composition(program, code, source, Use::Forwards);
                let asked = step.spawn_composing(request, root.digest);
                return park(step, at, asked, FINISHING);
            }
            let value = builtins::run(native, at, node, writer, &operands, &scratch);
            let held = match (native, operands.get(1)) {
                (Native::Field, _) => field_held(writer, &operands, value),
                (Native::ModuleMember, _) => member_held(writer, &operands, value),
                // A projection restamps its record, sharing its runs.
                (Native::Project, Some(Operand::Value(record))) => record.becoming(value),
                _ => Held::bare(value),
            };
            return finish(step, at, held);
        }
        Selection::Function {
            callee,
            coordinate,
            registered,
            solution,
            position,
        } => {
            debug_assert!(
                select::carried_fit(types, &scratch, registered.shape, solution, &arguments),
                "each argument carries a type under its slot at the solution"
            );
            let arguments =
                select::arguments(types, writer, registered, &operands, solution, &scratch);
            let routed = route.and_then(|route| route.per.get(position).copied());
            let tails = at.contract.filter(|contract| {
                callee.as_callable().and_then(Knotted::coerced).is_none()
                    && keeps(routed, || {
                        contract.kept_by(types, registered.shape, Some(solution))
                    })
            });
            exits(&step, at, &scratch, |exit| {
                exit.value(&callee);
                exit.value(&arguments);
                tails.iter().for_each(|owed| exit.contract(owed));
            });
            let handed = handed(
                at,
                writer,
                at.view.shape().site_of_node(node),
                &scratch,
                |to| match (to, registered.parameters) {
                    (HandedTo::Position(at), ParameterBinding::Named(names)) => {
                        match names.get(at as usize)? {
                            BinderSymbol::Value(name) => Some(name.symbol()),
                            _ => None,
                        }
                    }
                    _ => None,
                },
            );
            // A call through a barrier never tails: its value crosses the barrier where its
            // frame ends.
            let hop = routed.is_some_and(|routed| routed.hop);
            let named = match registered.parameters {
                ParameterBinding::Named(names) => names,
                ParameterBinding::Operands => &[],
            };
            // Every call hands on what it holds beside a kept part; only a tail threads.
            let named = (named.iter().zip(&operands).zip(wanted.iter())).filter_map(
                |((name, operand), wanted)| match (operand, wanted) {
                    (Operand::Value(held), Wanted::Evaluated(part)) => {
                        Some((name.symbol(), *held, tails.is_some() && threads(at, part)))
                    }
                    _ => None,
                },
            );
            let kept_callee = coordinate.and_then(|read| kept_read(at, writer, read));
            let passing = passing(writer, at, arguments, named, kept_callee);
            let call = |owed: Option<Contract>| {
                crate::program::call(
                    program,
                    callee,
                    arguments,
                    CallKind::Keyworded,
                    &[],
                    handed,
                    owed,
                    at.within.inherit,
                    Some(passing),
                    Use::Forwards,
                )
            };
            return called(step, at, callee, tails, hop, call);
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

/// What a call over the record `arguments` hands its frame through kept parts: each
/// argument of `named`, by the name its parameter binds, that is held beside a kept part, the
/// callee, through the kept part it was read through, and the record's field names as its type
/// names them, in the record's order.
fn passing<'graph, 'here>(
    writer: Writer<'here, '_>,
    at: &Evaluation<'_, 'graph, 'here, '_, '_, '_>,
    arguments: KValue<'graph, 'here>,
    named: impl Iterator<Item = (Symbol, Held<'graph, 'here>, bool)>,
    callee: Option<KeptPart<'graph, 'here>>,
) -> &'here Passing<'graph, 'here> {
    let scratch = Bump::new();
    let mut kept = BumpVec::new_in(&scratch);
    let mut threaded = BumpVec::new_in(&scratch);
    for (name, held, threads) in named {
        kept.extend(held.kept.map(|part| (name, part)));
        threaded.extend(threads.then_some(name));
    }
    let mut binders = BumpVec::new_in(&scratch);
    if let TypeNode::Record { fields } = at.types.node(arguments.concrete_ktype()) {
        binders.extend(fields.keys());
    }
    binders.sort_unstable_by_key(|binder| binder.symbol());
    resident(
        writer,
        Passing {
            callee,
            arguments: collect(writer, kept.iter().copied()),
            binders: collect(writer, binders.iter().copied()),
            threaded: collect(writer, threaded.iter().copied()),
        },
    )
}

/// Whether `part`, an argument a tail call hands on, is one its frame built to outlive the hop: a
/// part of the frame body's threaded set, or a name landing on one of its threaded binders.
fn threads(at: &Evaluation<'_, '_, '_, '_, '_, '_>, part: &ExpressionPart<'_>) -> bool {
    let shape = at.view.shape();
    if shape
        .threaded_parts()
        .binary_search(&shape.site(part))
        .is_ok()
    {
        return true;
    }
    (part.is_name())
        .then(|| shape.mention(shape.site(part)))
        .flatten()
        .and_then(|mention| at.view.landing(mention.coordinate))
        .is_some_and(|(segment, slot)| {
            segment == shape.segment() && shape.threaded().binary_search(&slot).is_ok()
        })
}

/// The digest memos the call keyed by `site` hands its callee: each digested slot the load saw it
/// pass bare whose memo is filled, under the parameter `name_of` says that argument binds.
fn handed<'here>(
    at: &Evaluation<'_, '_, 'here, '_, '_, '_>,
    writer: Writer<'here, '_>,
    site: Site,
    scratch: &Bump,
    name_of: impl Fn(HandedTo) -> Option<Symbol>,
) -> &'here [Handed] {
    let mut handed = BumpVec::new_in(scratch);
    for handoff in at.view.facts().handoffs(at.view.shape(), site) {
        let Some(digest) = (at.view.digest_memo(handoff.from)).and_then(|memo| memo.get()) else {
            continue;
        };
        if let Some(name) = name_of(handoff.to) {
            handed.push(Handed {
                name,
                digest,
                carried: at.view.read(handoff.from).ktype(),
            });
        }
    }
    collect(writer, handed.iter().copied())
}

/// What each recorded contribution, read at `site`, is where the call runs: a closed type as it
/// is, and `None` where the call reads the carried type. A contribution the run refuses is the
/// call's refusal.
fn contributed<'x>(
    at: &Evaluation<'_, '_, '_, '_, '_, '_>,
    site: Site,
    recorded: &[StaticType<'_>],
    scratch: &'x Bump,
) -> Result<BumpVec<'x, Option<KType>>, Elaboration> {
    let (facts, shape) = (at.view.facts(), at.view.shape());
    let mut contributed = BumpVec::with_capacity_in(recorded.len(), scratch);
    for each in recorded {
        contributed.push(match facts.at_run(&at.types, shape, site, *each) {
            AtRun::Closed(closed) => Some(closed),
            AtRun::Open => None,
            AtRun::Refused(refused) => return Err(refused),
        });
    }
    Ok(contributed)
}

/// The parts of a keyworded node its call needs: every slot evaluated, save an `ATTR` label
/// written bare, which is read as written.
pub(super) fn slots<'step, 'x>(
    node: &'step KExpression<'step>,
    scratch: &'x Bump,
) -> BumpVec<'x, Wanted<'step>> {
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

/// `(head argument)`, the node `node`: a construction when the head is a type, a read when it is a
/// list, a dict or a string ([lookup.rs](super::lookup)), and otherwise a call by name of the head
/// over the argument record.
fn apply<'step, 'graph, 'here>(
    mut step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
    node: &'step KExpression<'step>,
    head: &'step ExpressionPart<'step>,
    argument: &'step ExpressionPart<'step>,
    stage: u32,
) -> Action<'graph, KBundle> {
    let program = at.program;
    let types = &at.types;
    let scratch = Bump::new();
    let (argument_part, argument_site) = (argument, at.view.shape().site(argument));
    // A call's record literal is laid here over its fields, each evaluated as a part of the call,
    // so a tail hands each on through its own kept part. A type name's application is a
    // construction, built in the home its argument is asked into.
    let (literal, use_) = match (head, argument) {
        (ExpressionPart::Type(_), _) => (None, built(at)),
        (_, ExpressionPart::RecordLiteral(fields)) => (Some(&**fields), Use::Keeps),
        _ => (None, Use::Keeps),
    };
    let mut wanted = BumpVec::new_in(&scratch);
    wanted.push(Wanted::Evaluated(head));
    match literal {
        Some(fields) => wanted.extend(fields.iter().map(|(_, value)| Wanted::Evaluated(value))),
        None => wanted.push(Wanted::Evaluated(argument)),
    }
    let operands = match gathered(&mut step, at, &wanted, use_, stage, &scratch) {
        Gathered::Ready(operands) => operands,
        other => return unready(step, at, other),
    };
    let recorded = at
        .view
        .facts()
        .named_contributions(&at.types, at.view.shape(), argument_site);
    let held = |operand: &Operand<'graph, 'here>| match operand {
        Operand::Value(held) => *held,
        Operand::Label(_) => unreachable!("every part of an application is evaluated"),
    };
    let Held {
        value: head,
        kept: head_kept,
    } = held(&operands[0]);
    let fields = literal.map(|fields| {
        let mut named = BumpVec::with_capacity_in(fields.len(), &scratch);
        named.extend(
            (fields.iter().zip(&operands[1..]))
                .map(|((name, part), operand)| (*name, held(operand), threads(at, part))),
        );
        named
    });
    let argument = match &fields {
        Some(fields) => {
            let mut named = BumpVec::with_capacity_in(fields.len(), &scratch);
            named.extend(fields.iter().map(|(name, held, _)| (*name, held.value)));
            Held::bare(Value::Record(Record::new(
                step.writer(),
                &named,
                types,
                &scratch,
            )))
        }
        None => held(&operands[1]),
    };
    if let Value::Type(_) = head {
        let parts = [held(&operands[0]), argument];
        return finish_built(step, at, &parts, |writer, types, values| {
            let [Value::Type(head), argument] = values else {
                unreachable!("a construction's head is a type")
            };
            match Tagged::construct(writer, head, *argument, types, &Bump::new()) {
                Ok(tagged) => Value::Tagged(tagged),
                Err(refused) => program.error(
                    writer,
                    types,
                    refused_construction(&refused, program.symbols(), types),
                ),
            }
        });
    }
    // A list, a dict or a string at the head is read by its argument, built in the home as a
    // construction is.
    if lookup::readable(head) {
        let parts = [held(&operands[0]), argument];
        return finish_built(step, at, &parts, |writer, types, values| {
            let [head, argument] = values else {
                unreachable!("a read is its head and its argument")
            };
            lookup::read(program, writer, types, *head, *argument, &Bump::new())
        });
    }
    let writer = step.writer();
    // What the frame solves each parameter from, in symbol order: `None` where its carried type.
    let contributed = match contributed(at, argument_site, recorded, &scratch) {
        Ok(contributed) => collect(writer, contributed.iter().copied()),
        Err(refused) => {
            let refused = refused.display(program.symbols(), types);
            let error = program.error(writer, types, refused);
            return finish(step, at, error);
        }
    };
    // A call keeps its contract by the type its callee shows; a barrier never tails, since its
    // value crosses the barrier where its frame ends.
    let routed = at.contract.and_then(|_| {
        let route = (at.view.facts()).route(at.view.shape(), at.view.shape().site_of_node(node));
        route.map(|route| route.per[0])
    });
    let tails = at.contract.filter(|contract| {
        (head.as_callable().and_then(Knotted::function)).is_some()
            && keeps(routed, || contract.kept_by(&at.types, head.ktype(), None))
    });
    exits(&step, at, &scratch, |exit| {
        exit.value(&head);
        exit.value(&argument.value);
        contributed.iter().flatten().for_each(|kt| exit.handle(*kt));
        tails.iter().for_each(|owed| exit.contract(owed));
    });
    let handed = handed(at, writer, argument_site, &scratch, |to| match to {
        HandedTo::Field(name) => Some(name),
        HandedTo::Position(_) => None,
    });
    let hop = routed.is_some_and(|routed| routed.hop);
    // A call hands on each field of a literal through its own kept part, and each of a record
    // held whole through the record's, the field's label appended; only a tail threads.
    let tail = tails.is_some();
    let passing = match &fields {
        Some(fields) => passing(
            writer,
            at,
            argument.value,
            fields
                .iter()
                .map(|(name, held, threads)| (name.symbol(), *held, tail && *threads)),
            head_kept,
        ),
        None => {
            let names = match at.types.node(argument.value.concrete_ktype()) {
                TypeNode::Record { fields } => Some(fields.keys()),
                _ => None,
            };
            let threaded = tail && threads(at, argument_part);
            let fields = names.into_iter().flatten().map(|binder| {
                let name = binder.symbol();
                let kept =
                    (argument.kept).map(|part| part.extended(writer, PathSegment::Label(name)));
                let value = argument.value.named(name);
                (name, Held { value, kept }, threaded)
            });
            passing(writer, at, argument.value, fields, head_kept)
        }
    };
    let argument = argument.value;
    let call = |owed: Option<Contract>| {
        crate::program::call(
            program,
            head,
            argument,
            CallKind::ByName,
            contributed,
            handed,
            owed,
            at.within.inherit,
            Some(passing),
            Use::Forwards,
        )
    };
    called(step, at, head, tails, hop, call)
}

/// Whether a tail call's candidate keeps its frame's contract: as its route records it, and where
/// the load left that open, as `kept_by` decides at the call.
fn keeps(routed: Option<CandidateRoute>, kept_by: impl Fn() -> bool) -> bool {
    match routed.and_then(|routed| routed.kept) {
        Some(kept) => {
            debug_assert_eq!(kept, kept_by(), "a route's verdict is the call's");
            kept
        }
        None => {
            #[cfg(test)]
            DERIVED.with(|derived| derived.set(derived.get() + 1));
            kept_by()
        }
    }
}

#[cfg(test)]
thread_local! {
    /// How many tail calls decided at the call whether their candidate keeps the frame's contract.
    pub(crate) static DERIVED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many tail calls hopped under their frame's own settlement.
    pub(crate) static HOPPED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The most bytes an asker's region had handed out at a call it spawned.
    pub(crate) static ASKER_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Run the frame `call` asks for over `callee`: a hop owing `tails` where it keeps the contract,
/// else a child this evaluation parks on. A frame keyed on its bindings is born borrowing its
/// settlement: a hop of the frame this evaluation runs in borrows that frame's own where its route
/// says it `hop`s — a self tail edge of a body whose name does not escape, so every hop into it is
/// a tail edge the load read — and every other one is sought by the drain. The callee's keys are
/// read once.
fn called<'step, 'graph, 'here>(
    mut step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
    callee: KValue<'graph, 'here>,
    tails: Option<Contract>,
    hop: bool,
    call: impl Fn(Option<Contract>) -> Request<'graph, 'here, KBundle>,
) -> Action<'graph, KBundle> {
    debug_assert!(
        !hop || callee.as_callable() == at.view.callable(),
        "a hop's callee is its frame's own"
    );
    let mut request = call(tails);
    let function = called_function(callee);
    let keys = function.and_then(|function| function.body().keys);
    if tails.is_some() {
        // The frame's own callee, keyed, where the frame runs under a settlement: its arena node
        // is then not the one the callee was born under, which a frame born with none runs under.
        if hop
            && keys.is_some()
            && function.is_some_and(|function| at.within.build != function.closure().build())
        {
            #[cfg(test)]
            HOPPED.with(|hopped| hopped.set(hopped.get() + 1));
            // The hop runs under the frame's own settlement, so it keeps the frame's level.
            if let KBirth::Call { level, .. } = &mut request.work.state {
                *level = at.within.stamp.map(|stamp| stamp.level);
            }
            return step.tail_under(request.work, at.within.build);
        }
        let types = step.types();
        let seek = seek(&mut request.work.state, keys, &at.types, &types);
        return step.tail_seeking(request.work, seek);
    }
    let types = step.types();
    let seek = seek(&mut request.work.state, keys, &at.types, &types);
    #[cfg(test)]
    {
        let bytes = step.writer().in_use();
        ASKER_BYTES.with(|most| most.set(most.get().max(bytes)));
    }
    let asked = step.spawn_seeking(request, seek);
    park(step, at, asked, FINISHING)
}

/// Check that `value`, unless it is an error value, carries a type within its node's static type:
/// under its upper end read above its variables, and over its lower end read below them, as a
/// candidate's judgement reads an argument's — read through `types`, the chain of the half the
/// value lies in.
#[cfg(debug_assertions)]
fn carried_under_static<'step, 'graph, 'here>(
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
    types: &Chain<'_, '_, '_, '_>,
    value: &KValue<'graph, '_>,
) {
    if at.program.message(value, types).is_some() {
        return;
    }
    let shape = at.view.shape();
    let KBirth::Evaluate { node, .. } = at.birth else {
        return;
    };
    let facts = at.view.facts();
    let expected = match node {
        Evaluated::Part(site) => facts.value_type(types, shape, site),
        Evaluated::Statement(index) => facts.statement_type(types, shape, index as usize),
    };
    let Some(expected) = expected else {
        return;
    };
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

/// `part` lowered straight to a value, as [`Value::lower_part`] lowers it, its strings sharing
/// the program's text where the code is as written.
fn lowered<'graph, 'here>(
    writer: Writer<'here, '_>,
    at: &Evaluation<'_, 'graph, 'here, '_, '_, '_>,
    part: &ExpressionPart<'_>,
    scratch: &Bump,
) -> Option<KValue<'graph, 'here>> {
    // A scalar has no text to share, and a part that does not lower has nothing to find.
    let shares = match part {
        ExpressionPart::Literal(literal) => matches!(literal, KLiteral::String(_)),
        _ => KValue::lowers(part),
    };
    match shares.then(|| written_part(at, part)).flatten() {
        Some(part) => Value::lower_shared(writer, part, &at.types, scratch),
        None => Value::lower_part(writer, part, &at.types, scratch),
    }
}

/// `part` where the load holds it, which outlives every region, so a value may share its text;
/// `None` in composed code.
fn written_part<'graph>(
    at: &Evaluation<'_, 'graph, '_, '_, '_, '_>,
    part: &ExpressionPart<'_>,
) -> Option<&'graph ExpressionPart<'graph>> {
    let written = (at.program).written_under(at.view.facts(), at.within.root);
    written.part(at.view.segment(), part)
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
fn in_place<'step, 'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
    part: &'step ExpressionPart<'step>,
    scratch: &Bump,
) -> Option<Held<'graph, 'here>> {
    let writer = step.writer();
    let types = &at.types;
    match part {
        ExpressionPart::Literal(_) => lowered(writer, at, part, scratch).map(Held::bare),
        ExpressionPart::QuotedExpression(_) => {
            let written = (at.program).written_under(at.view.facts(), at.within.root);
            Some(Held::bare(
                match quote(writer, &at.view, written, part, types, scratch) {
                    Ok(code) => Value::Knotted(code),
                    Err(refused) => Raised::Uncomposed { refused }.raise(at.program, writer, types),
                },
            ))
        }
        _ => {
            let shape = at.view.shape();
            let mention = shape.mention(shape.site(part))?;
            let read = at.view.read(mention.coordinate);
            let facts = at.view.facts();
            let Some(InstanceRead::Solved(solution)) =
                facts.instance_at(&at.types, shape, shape.site(part))
            else {
                let kept = kept_read(at, writer, mention.coordinate);
                return Some(Held { value: read, kept });
            };
            let Value::Knotted(member) = read else {
                unreachable!(
                    "an instance site reads a bare quantified function: a header at a scheme \
                     comes only from a module member read, which makes its own instance"
                )
            };
            Some(Held::bare(Value::Knotted(instance(
                writer, member, solution, types, scratch,
            ))))
        }
    }
}

/// What the frame keeps of the value a read at `coordinate` reads: a slot of its own body whole,
/// or a capture of its callee as that part of the kept callee. `None` outside a called frame, and
/// for a read it keeps nothing of.
fn kept_read<'graph, 'here>(
    at: &Evaluation<'_, 'graph, 'here, '_, '_, '_>,
    writer: Writer<'here, '_>,
    coordinate: Coordinate,
) -> Option<KeptPart<'graph, 'here>> {
    let table = at.within.kept?;
    if let Some((segment, slot)) = at.view.landing(coordinate) {
        let kept = table.slot(segment, slot)?;
        return Some(KeptPart { kept, path: &[] });
    }
    let (segment, slot) = at.view.capture_landing(coordinate)?;
    table.capture(writer, segment, slot)
}

/// How an evaluation asks for the parts of a construction it builds in its home: forwarded, so
/// each is built in that home too; kept in a caching body, which keeps its values.
fn built(at: &Evaluation<'_, '_, '_, '_, '_, '_>) -> Use {
    match at.view.facts().caches(at.view.shape()) {
        true => Use::Keeps,
        false => Use::Forwards,
    }
}

/// Ask for every wanted part not read in place, with `use_` — or, for a frame's hand-off, with
/// `Threads` for a part of the frame body's threaded set, so it is built where it outlives the hop —
/// or, once they are in, or when none needed asking, every operand, in order. An error value among
/// them is handed back alone.
fn gathered<'step, 'x, 'graph, 'here>(
    step: &mut Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
    wanted: &[Wanted<'step>],
    use_: Use,
    stage: u32,
    scratch: &'x Bump,
) -> Gathered<'x, 'graph, 'here> {
    let asked_for =
        |wanted: &&Wanted<'step>| matches!(wanted, Wanted::Evaluated(part) if !read_in_place(part));
    if stage == BORN {
        let mut asked = None;
        for wanted in wanted.iter().filter(asked_for) {
            let Wanted::Evaluated(part) = wanted else {
                unreachable!("only an evaluated part is asked for")
            };
            let shape = at.view.shape();
            let threads = at.contract.is_some()
                && (shape.threaded_parts())
                    .binary_search(&shape.site(part))
                    .is_ok();
            asked = Some(step.spawn(Request {
                placement: Placement::Shares,
                use_: if threads { Use::Threads } else { use_ },
                work: at.program.evaluate(
                    Evaluated::Part(at.view.shape().site(part)),
                    at.view.view(),
                    None,
                    at.within,
                ),
            }));
        }
        if let Some(asked) = asked {
            return Gathered::Asked(asked);
        }
    }
    let mut received = BumpVec::new_in(scratch);
    for result in step.results_with(&at.types) {
        match result {
            Ok(Received::Here(held)) => {
                if at.program.message(&held.value, &at.types).is_some() {
                    return Gathered::Raised(held.value);
                }
                received.push(held);
            }
            Ok(Received::Scratch(_)) => {
                unreachable!("a part is asked for with `Keeps` or `Forwards`, never to scratch")
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
fn unready<'step, 'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
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
fn park<'step, 'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
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
fn finishing<'step, 'graph, 'here>(
    mut step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
) -> Action<'graph, KBundle> {
    let received = step.results_with(&at.types).next();
    match received {
        Some(Ok(Received::Here(value))) => finish(step, at, value),
        _ => step.failed(StepError::Unredeemable),
    }
}

/// Finish with what `build` builds over `parts`, built in the evaluation's home and held to its
/// contract there, so nothing is built here to be crossed whole: the parts cross from their kept
/// parts where they have them. A caching body's value that stays inside the body is built here, on
/// the body's chain, and one that leaves it adopts its parts' types first.
fn finish_built<'step, 'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
    parts: &[Held<'graph, 'here>],
    build: impl for<'their> FnOnce(
        Writer<'their, '_>,
        &Chain<'_, 'their, '_, '_>,
        &[KValue<'graph, 'their>],
    ) -> KValue<'graph, 'their>,
) -> Action<'graph, KBundle> {
    let caches = at.view.facts().caches(at.view.shape());
    if caches && at.contract.is_none() {
        let scratch = Bump::new();
        let mut values = BumpVec::with_capacity_in(parts.len(), &scratch);
        values.extend(parts.iter().map(|part| part.value));
        let value = build(step.writer(), &at.types, &values);
        return finish(step, at, value);
    }
    let scratch = Bump::new();
    exits(&step, at, &scratch, |exit| {
        parts.iter().for_each(|part| exit.value(&part.value));
    });
    let (program, contract) = (at.program, at.contract);
    step.finish_in_home(parts, |writer, types, values, _| {
        let value = build(writer, types, values);
        #[cfg(debug_assertions)]
        carried_under_static(at, types, &value);
        Active::new(match contract {
            Some(contract) => program.fulfilled(writer, types, value, contract),
            None => value,
        })
    })
}

/// Finish with `value`, held to the contract the evaluation was born under: through its kept part
/// where the contract leaves it one storage.
fn finish<'step, 'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
    value: impl Into<Held<'graph, 'here>>,
) -> Action<'graph, KBundle> {
    let held = value.into();
    #[cfg(debug_assertions)]
    carried_under_static(at, &at.types, &held.value);
    let held = match at.contract {
        Some(contract) => {
            held.becoming((at.program).fulfilled(step.writer(), &at.types, held.value, contract))
        }
        None => held,
    };
    delivered(step, at, held)
}

/// Deliver `value` to the evaluation's asker. In a caching body a value owed a frame's contract
/// leaves it, so it is adopted first; any other stays inside, and its debug check reads the body's
/// chain.
fn delivered<'step, 'graph, 'here>(
    step: Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
    value: impl Into<Held<'graph, 'here>>,
) -> Action<'graph, KBundle> {
    let held = value.into();
    match (at.view.facts().caches(at.view.shape()), at.contract) {
        (true, Some(_)) => {
            // No step function's scratch reaches a delivery.
            let scratch = Bump::new();
            exits(&step, at, &scratch, |exit| exit.value(&held.value));
            step.finish(held)
        }
        (true, None) => step.finish_with(held, &at.types),
        (false, _) => step.finish(held),
    }
}

/// In a caching body, hand `adopt` the [`Exit`] a value leaving it is adopted through, into the
/// step's region, its walks laid in `scratch`; elsewhere nothing leaves a scratch tier, and `adopt`
/// is not called.
fn exits<'step, 'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    adopt: impl FnOnce(&Exit<'_, '_, 'here, '_>),
) {
    if at.view.facts().caches(at.view.shape()) {
        adopt(&Exit::new(step.types(), &at.types, scratch));
    }
}

/// What a test hands each use a frame finds selected: its body's segment and its node's site.
#[cfg(test)]
type Chose = fn(crate::scope::Segment, Site);

#[cfg(test)]
thread_local! {
    /// What each use a frame finds selected in its facts is handed to, by its body's segment and
    /// its node's site, while a test sets it: so a test sees a frame read the selection its facts
    /// hold.
    pub(super) static CHOSE: std::cell::Cell<Option<Chose>> = const { std::cell::Cell::new(None) };
}

/// Whether `code` is carried at a type no shape gave it, which a `BUILD` gives it: an unbuilt
/// kind, or `Code`, which is `UNBUILT Code`.
fn unbuilt(code: Knotted<'_, '_>, types: &Chain<'_, '_, '_, '_>) -> bool {
    matches!(
        types.node(Value::Knotted(code).concrete_ktype()),
        TypeNode::CodeUnbuilt { .. } | TypeNode::AnyCode
    )
}
