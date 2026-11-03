//! The evaluator: what a node is, and how its value is reached. Handed a node in flight and what
//! was last delivered to it by the [evaluation stack](crate::program::Next), it answers with what
//! the node needs next, and never ends a step.
//!
//! A node is read off the shape that holds it — never the parse — as one of:
//!
//! - a **leaf**: a literal, lowered; a name, read through its mention's coordinate; a quote, born
//!   through the [quote door](crate::knot::quote); a list, dict or record literal, its parts
//!   evaluated and the container built; a type expression, elaborated to a type value;
//! - a **block** a pairwise operator run was rewritten into, run with its last statement's value
//!   its own;
//! - a `FN`, born through the [lambda door](crate::knot::lambda), an untraced body over the code
//!   its body part evaluates to, asked for first;
//! - an **ascription** `<value> :! <Type>`: its operand checked against the type, unless the load
//!   settled it, and retyped to it; a module operand, under `:!` or `:|`, is seen as its signature
//!   through the view door;
//! - an **`EVAL`** `<code> -> <Type>`: its code run in a frame that owes the declared type, as a
//!   called frame owes its return;
//! - a **`USING … SCOPE`**: its body run as a block entered on the module, each surfaced name bound
//!   to the member it names and each surfaced key to the module's functions there;
//! - a **`MATCH`**: its scrutinee selecting an arm over what the load kept of them
//!   ([`select::arm`](super::select::arm)), the arm run as a block entered on its `it`, owing
//!   the evaluation's contract where the arm is in tail position;
//! - a **`TRY`** or **`CATCH`**: its body run as a block above the node, and a fault it ends with
//!   copied out into an `Error` ([`caught`](super::caught));
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
//! asked for, one at a time in source order, each delivered back to the node as it comes in.
//! Each value is held beside the kept part it is where it has one ([`Held`]): a name its frame keeps
//! a carrier of, a capture of the frame's callee as that part of its kept callee, a child's pinned
//! result, and a field or module member read over any of them, its label or slot appended to the
//! path. A value finished with a kept part crosses from its true home, and every call hands its
//! callee and arguments on through theirs ([`Passing`]). A construction — a list,
//! dict or record literal, a newtype's, or a read — is built in the evaluation's home over its parts, which
//! it asks for forwarded, so it is never built here only to be crossed whole. A node in a frame's
//! tail position asks for each part of the frame body's threaded set with `Threads`, so what its
//! tail call hands on that the frame built is built in the loop's cell, and marks those arguments
//! in the skeleton it lays. A value is placed where the node's `Use` says through the scheduler's
//! `built_as` and `crossed_as`.
//! A fault received from a part ends this evaluation, and the parts after it are never asked. Born
//! under what a frame [owes](crate::program::Owed), the evaluation owes the frame its value: a call
//! whose callee's declared return satisfies the contract hops to the callee's frame, and every
//! other value is handed on as it is held, to the frame's runner, which holds it to the contract
//! and crosses it once where the frame ends.
//!
//! In a caching body the step runs on the body's chain ([`evaluate`]), asks for blocks with `Keeps`,
//! and adopts what leaves the body into its region through an [`Exit`]: a call's callee, arguments,
//! contributions and the contract it hands on, and an `EVAL`'s code, offered record and declared
//! type. A step lays nothing in the agent tier: an invariant site's
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

use std::cell::Cell;

use crate::elaborate::{arm_shapes, variant_named};
use crate::knot::module::layout;
use crate::knot::module::view::{self, Ascription, Unascribable};
use crate::knot::{
    KFault, KInner, KValue, KValueFamily, Knotted, flattened, instance, lambda, quote,
};
use crate::memory::{Active, Bump, BumpAllocator, BumpVec, Writer, collect, resident};
use crate::parse::BuiltinShapeId;
use crate::parse::Role;
use crate::parse::{
    ExpressionPart, KExpression, KLiteral, KTemplate, ListItem, TemplateSlot, template_fields,
};
use crate::program::{
    ArmKept, BlockEntry, CallKind, Delivered, Entered, Evaluated, Exit, Handed, KBirth, KBundle,
    Next, Node, Owed, Passing, Program, Tailed, Within, called_function, seek,
};
use crate::scheduler::{Held, KeptPart, Request, Slot as Asked, Step, StepError, Taken, Use};
use crate::scope::Inner;
use crate::scope::{
    AtRun, BodyShape, CandidateList, CandidateRoute, Coordinate, Elaboration, Elimination, Fault,
    Guard, HandedTo, IMPLICIT, InstanceRead, Located, Narrowing, ParameterBinding, Scalar,
    ShapeKind, Site, StaticType,
};
use crate::symbols::{BinderSymbol, Symbol, SymbolInterner};
use crate::type_lattice::{KType, TypeNode, Verdict};
#[cfg(debug_assertions)]
use crate::type_lattice::{bound_above, lower_end_outside, satisfied_by};
use crate::values::{
    Chain, Circular, Concatenation, Dict, Key, List, PathSegment, Piece, Record, Tagged, Text,
    TypeValue, Value, construction, representation, satisfies,
};

use super::builtins::{self, Native};
use super::caught;
use super::lookup;
use super::select::{self, Selection};
use super::{Evaluation, Operand};

/// How far a node got: nothing asked yet, or gathering its parts.
pub(super) const BORN: u32 = 0;
/// The one child whose value is its own — a frame, a block, an `EVAL` — is in.
pub(super) const FINISHING: u32 = 2;
/// A `TRY` or `CATCH` body's outcome is in.
pub(super) const CAUGHT: u32 = 3;
/// A `TRY` or `CATCH` is entered beside a kept cell of the fault it caught.
pub(super) const GATHERING: u32 = 4;

/// A step that took both its states.
pub(super) type Taking<'a, 'graph, 'step, 'here, 'scratch, 'lent> =
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
    /// `MATCH <scrutinee> WITH <arms>`, or, `under`, `MATCH <scrutinee> UNDER <union> WITH <arms>`.
    Match {
        node: &'step KExpression<'step>,
        under: bool,
    },
    /// `TRY <body> WITH <arms>`.
    Try(&'step KExpression<'step>),
    /// `CATCH <body>`.
    Catch(&'step KExpression<'step>),
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
    /// The next part not read in place: evaluate it, with this use.
    Part {
        site: Site,
        use_: Use,
    },
    Ready(BumpVec<'x, Operand<'graph, 'here>>),
    /// A part ended with a fault: what lies inside it.
    Raised(KInner<'graph, 'here>),
    Broken(StepError),
}

/// The evaluator: `node`, handed what was last `delivered` to it, answers with what it needs next.
/// It runs on one chain, as the body runner's steps do ([`run`](crate::program::run)): the caching
/// chain where it routes or its body caches, and its region's anywhere else.
pub(super) fn evaluate<'graph, 'here, 'scratch>(
    step: &mut Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    node: &mut Node<'graph, 'here, 'scratch>,
    delivered: Option<Delivered<'graph, 'here>>,
) -> Next<'graph, 'here> {
    let (code, facts) = step.door();
    let shape = code.shape(node.view.segment());
    #[cfg(test)]
    if node.within.routes || facts.caches(shape) {
        let address = std::ptr::from_ref(shape).addr();
        crate::scheduler::CACHING.set(Some(address));
    }
    if node.within.routes || facts.caches(shape) {
        let types = step.caching_types();
        evaluate_on(step, node, delivered, types)
    } else {
        let types = step.types();
        evaluate_on(step, node, delivered, types)
    }
}

/// [`evaluate`] on the chain `types` it picked: how far the node got is written back to it.
fn evaluate_on<'graph, 'here, 'scratch, 'tier, 'reg>(
    step: &mut Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    node: &mut Node<'graph, 'here, 'scratch>,
    delivered: Option<Delivered<'graph, 'here>>,
    types: Chain<'tier, 'here, 'reg, '_>,
) -> Next<'graph, 'here> {
    let (code, facts) = step.door();
    let at = Evaluation {
        program: node.program,
        code,
        view: node.view.reading(&code, facts),
        owed: node.owed,
        within: node.within,
        node: node.node,
        use_: node.use_,
        types,
        stage: Cell::new(node.stage),
        received: Cell::new(node.received),
        caught: Cell::new(node.caught),
        delivered: Cell::new(delivered),
    };
    let next = evaluated_node(step, &at);
    node.stage = at.stage.get();
    node.received = at.received.get();
    node.caught = at.caught.get();
    next
}

/// What the node `at` evaluates needs next, by its form.
fn evaluated_node<'graph, 'here, 'scratch>(
    step: &mut Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    at: &Evaluation<'_, 'graph, 'here, '_, '_, '_, 'scratch>,
) -> Next<'graph, 'here> {
    if at.stage.get() == FINISHING {
        return finishing(step, at);
    }
    match form(at.view.shape(), at.node) {
        Form::Leaf(part) => leaf(step, at, part),
        Form::Name(part) => read(step, at, part),
        Form::Block(shape) => {
            let entry = BlockEntry {
                segment: shape.segment(),
                enclosing: at.view.view(),
                entered: Entered::Plain,
                owed: None,
                use_: inside(at),
                within: at.within,
            };
            ran_block(at, entry, FINISHING)
        }
        Form::Lambda(node) => born_lambda(step, at, node),
        Form::Declaration => finish(step, at, Value::Null),
        Form::Ascribe(node, mode) => ascribe(step, at, node, mode),
        Form::Eval(node) => evaluated(step, at, node),
        Form::Using(node) => using(step, at, node),
        Form::Call(node, list) | Form::Member { node, list, .. } => call(step, at, node, list),
        Form::Apply(node) => {
            let [head, argument] = node.parts else {
                unreachable!("an application is a head and its argument")
            };
            apply(step, at, node, &head.value, &argument.value)
        }
        Form::Match { node, under } => matched(step, at, node, under),
        Form::Try(node) => caught::caught(step, at, node, true),
        Form::Catch(node) => caught::caught(step, at, node, false),
        Form::Unevaluable(node) => raise(step, Fault::Unevaluable { node }),
    }
}

/// A `FN` no binder names, born through the lambda door: an untraced body over the code its body
/// part evaluates to, asked for first. A refused birth — a part that is no code among them — is a
/// fault.
fn born_lambda<'step, 'graph, 'here, 'scratch>(
    step: &mut Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_, 'scratch>,
    node: &'step KExpression<'step>,
) -> Next<'graph, 'here> {
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
            match gathered(step, at, &wanted, Use::Keeps, &scratch) {
                Gathered::Ready(operands) => match operands[..] {
                    [Operand::Value(Held { value, .. })] => Some(value),
                    _ => unreachable!("an untraced body's part is evaluated"),
                },
                other => return unready(other),
            }
        }
    };
    let writer = step.writer();
    let written = program.written_under(at.view.facts(), at.within.root);
    match lambda(
        writer,
        &at.view,
        written,
        site,
        code,
        at.within.born(),
        &at.types,
        &scratch,
    ) {
        Ok(member) => finish(step, at, Value::Knotted(member)),
        Err(refused) => raise(step, Fault::Untied(refused)),
    }
}

/// How an evaluation asks for a block whose value is its own: kept in a caching body, so a value
/// leaves the body only where it adopts, and forwarded elsewhere.
pub(super) fn inside(at: &Evaluation<'_, '_, '_, '_, '_, '_, '_>) -> Use {
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
        Some(BuiltinShapeId::Match) => return Form::Match { node, under: false },
        Some(BuiltinShapeId::MatchUnder) => return Form::Match { node, under: true },
        Some(BuiltinShapeId::Try) => return Form::Try(node),
        Some(BuiltinShapeId::Catch) => return Form::Catch(node),
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
fn leaf<'step, 'graph, 'here, 'scratch>(
    step: &mut Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_, 'scratch>,
    part: &'step ExpressionPart<'step>,
) -> Next<'graph, 'here> {
    let types = &at.types;
    match part {
        ExpressionPart::Literal(_) | ExpressionPart::QuotedExpression(_) => read(step, at, part),
        ExpressionPart::SigiledTypeExpr(_) | ExpressionPart::RecordType(_) => {
            let writer = step.writer();
            let facts = at.view.facts();
            match facts.denoted(types, at.view.shape(), at.view.shape().site(part)) {
                Ok(handle) => finish(step, at, Value::Type(TypeValue::new(writer, handle, types))),
                Err(refused) => raise(step, Fault::Unelaborated(refused)),
            }
        }
        ExpressionPart::ListLiteral(_)
        | ExpressionPart::Template(_)
        | ExpressionPart::DictLiteral(_)
        | ExpressionPart::RecordLiteral(_) => container(step, at, part),
        ExpressionPart::Keyword(_)
        | ExpressionPart::Expression(_)
        | ExpressionPart::MarkedUse(..)
        | ExpressionPart::FilledUse(..) => Next::Failed(StepError::Refused),
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
    step: &mut Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_, '_>,
    part: &'step ExpressionPart<'step>,
) -> Next<'graph, 'here> {
    match in_place(step, at, part, &Bump::new()) {
        Some(Ok(value)) => finish(step, at, value),
        Some(Err(fault)) => raise(step, fault),
        None => Next::Failed(StepError::Refused),
    }
}

/// `<value> :! <Type>`: the operand checked against the type — where the load did not settle it —
/// and retyped to it, a module re-stamped at the application it fits. `<module> :| <Sig>` is seen
/// as it through the [view door](crate::knot::module::view), which checks it fits; `:|` over
/// anything else raises.
fn ascribe<'step, 'graph, 'here, 'scratch>(
    step: &mut Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_, 'scratch>,
    node: &'step KExpression<'step>,
    mode: Ascription,
) -> Next<'graph, 'here> {
    let types = &at.types;
    let scratch = Bump::new();
    let [operand, _, ascribed] = node.parts else {
        unreachable!("an ascription has an operand, its keyword and a type")
    };
    let wanted = [Wanted::Evaluated(&operand.value)];
    let operands = match gathered(step, at, &wanted, Use::Keeps, &scratch) {
        Gathered::Ready(operands) => operands,
        other => return unready(other),
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
        Err(refused) => return raise(step, Fault::Unelaborated(refused)),
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
            let raised = Fault::NotAModule { value };
            return raise(step, raised);
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
            Err(Unascribable::Unsatisfied(_)) => Fault::Unascribable { value, ascribed },
            Err(Unascribable::NotASignature(ascribed)) => Fault::NotASignature { ascribed },
            Err(Unascribable::Coercion { name, refused }) => Fault::Coercion {
                name: name.symbol(),
                refused,
            },
            Err(Unascribable::NotAModule) => unreachable!("the operand is a module"),
        };
        return raise(step, raised);
    }
    let (facts, site) = (at.view.facts(), at.view.shape().site_of_node(node));
    if facts.settled(at.view.shape(), site) {
        debug_assert!(
            satisfies(ascribed, &value, types, &scratch),
            "a settled ascription's operand satisfies it"
        );
    } else if !satisfies(ascribed, &value, types, &scratch) {
        let raised = Fault::Unascribable { value, ascribed };
        return raise(step, raised);
    }
    let retype = facts.retype(types, at.view.shape(), site);
    let retyped = value.retyped_as(writer, ascribed, retype, types, &scratch);
    finish(step, at, operand.becoming(retyped))
}

/// `USING <module> SCOPE <body>`: the body run as a block whose parameters are bound to what the
/// module surfaces, its last statement's value its own.
fn using<'step, 'graph, 'here, 'scratch>(
    step: &mut Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_, 'scratch>,
    node: &'step KExpression<'step>,
) -> Next<'graph, 'here> {
    let scratch = Bump::new();
    let [_, operand, _, _] = node.parts else {
        unreachable!("a `USING … SCOPE` has its keyword, its module, `SCOPE` and a body")
    };
    let wanted = [Wanted::Evaluated(&operand.value)];
    let operands = match gathered(step, at, &wanted, Use::Keeps, &scratch) {
        Gathered::Ready(operands) => operands,
        other => return unready(other),
    };
    let [Operand::Value(Held { value: module, .. })] = operands[..] else {
        unreachable!("a `USING`'s operand is evaluated")
    };
    // The shape surfaces only an operand it reads a module's declaration off.
    if module.as_module().is_none() {
        let raised = Fault::NotAModule { value: module };
        return raise(step, raised);
    }
    let body = (at.view.shape().site_of_body(node))
        .and_then(|site| at.view.shape().nested(site))
        .expect("a `USING … SCOPE` body is a block its shape holds");
    let entry = BlockEntry {
        segment: body.segment(),
        enclosing: at.view.view(),
        entered: Entered::Surfaced(module, at.view.shape().site_of_node(node)),
        owed: None,
        use_: inside(at),
        within: at.within,
    };
    ran_block(at, entry, FINISHING)
}

/// `MATCH <scrutinee> WITH <arms>`, or `… UNDER <union> WITH <arms>`: the scrutinee, evaluated,
/// selects an arm over the load's record of the arms ([`select::arm`]), which runs as a block whose
/// `it` is the scrutinee retyped to its guard — under a union, its payload retyped to its variant's
/// representation — its value the `MATCH`'s. No arm admitting, or several ranking none first, is a
/// fault. A tail arm runs in tail position owing the evaluation's contract, so a recursion through
/// it tails.
fn matched<'step, 'graph, 'here, 'scratch>(
    step: &mut Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_, 'scratch>,
    node: &'step KExpression<'step>,
    under: bool,
) -> Next<'graph, 'here> {
    let types = &at.types;
    let scratch = Bump::new();
    let wanted = [Wanted::Evaluated(&node.parts[1].value)];
    let operands = match gathered(step, at, &wanted, Use::Keeps, &scratch) {
        Gathered::Ready(operands) => operands,
        other => return unready(other),
    };
    let [Operand::Value(held @ Held { value, .. })] = operands[..] else {
        unreachable!("a `MATCH`'s scrutinee is evaluated")
    };
    let (shape, facts) = (at.view.shape(), at.view.facts());
    let Some(arms) = facts.arms(shape, shape.site_of_node(node)) else {
        return Next::Failed(StepError::Refused);
    };
    let denoted = |part: &ExpressionPart<'_>| facts.denoted(types, shape, shape.site(part)).ok();
    let union = match under {
        true => match denoted(&node.parts[3].value) {
            Some(union) => Some(union),
            None => return Next::Failed(StepError::Refused),
        },
        false => None,
    };
    let (bodies, guards) = arm_guards(types, &scratch, shape, node, union, denoted);
    let admits = |arm: usize| match guards[arm] {
        (Guard::Value(scalar), _) => {
            (value.equals(&scalar_value(scalar), types, &scratch)).unwrap_or(false)
        }
        (Guard::Default, _) => union.is_none_or(|union| satisfies(union, &value, types, &scratch)),
        (_, Some(admits)) => satisfies(admits, &value, types, &scratch),
        (_, None) => false,
    };
    let chosen = match select::arm(types, &scratch, arms, &guards, admits) {
        Ok(chosen) => chosen,
        Err(ambiguous) => {
            let raised = match ambiguous {
                None => Fault::NoArm {
                    scrutinee: value,
                    union,
                },
                Some(count) => Fault::AmbiguousArms {
                    scrutinee: value,
                    count,
                },
            };
            return raise(step, raised);
        }
    };
    let Some(ExpressionPart::DictLiteral(written)) = node.parts.last().map(|arms| &arms.value)
    else {
        unreachable!("a `MATCH`'s last part is its arm set")
    };
    let retype = facts.retype(types, shape, shape.site(&written[chosen].1));
    let writer = step.writer();
    // `it` is held where its scrutinee rests: as a restamp of it, or one tagged layer below it.
    let payload = |value: KValue<'graph, 'here>| Held {
        value,
        kept: (held.kept).map(|part| part.extended(writer, PathSegment::Payload)),
    };
    let it = match guards[chosen] {
        (Guard::Type(_), Some(guard)) => {
            held.becoming(value.retyped_as(writer, guard, retype, types, &scratch))
        }
        (Guard::Label(_), Some(variant)) => {
            let Some(payload) = value.untagged().map(payload) else {
                return Next::Failed(StepError::Refused);
            };
            match representation(types, &scratch, variant) {
                Some(represented) => payload.becoming((payload.value).retyped_as(
                    writer,
                    represented,
                    retype,
                    types,
                    &scratch,
                )),
                None => payload,
            }
        }
        // `_` under a union binds the payload, and a structural member is its own.
        (Guard::Default, _) if under => value.untagged().map_or(held, payload),
        _ => held,
    };
    arm_ran(step, at, bodies[chosen], it)
}

/// Each arm of the `MATCH` or `TRY` `node` beside its guard and the type the guard admits by: a
/// type guard's denoted type, and a label's variant of `union`.
pub(super) fn arm_guards<'list, 'step>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: &'list Bump,
    shape: &'step BodyShape<'step>,
    node: &'step KExpression<'step>,
    union: Option<KType>,
    denoted: impl Fn(&ExpressionPart<'_>) -> Option<KType>,
) -> (
    BumpVec<'list, &'step BodyShape<'step>>,
    BumpVec<'list, (Guard<'step>, Option<KType>)>,
) {
    let mut bodies: BumpVec<'list, &'step BodyShape<'step>> = BumpVec::new_in(scratch);
    bodies.extend(arm_shapes(shape, node));
    let mut guards = BumpVec::with_capacity_in(bodies.len(), scratch);
    for body in bodies.iter() {
        let guard = body.arm().expect("an arm knows it is one").guard;
        let admits = match guard {
            Guard::Type(quote) => denoted(quote),
            Guard::Label(label) => union.and_then(|union| variant_named(types, union, label)),
            Guard::Value(_) | Guard::Default => None,
        };
        guards.push((guard, admits));
    }
    (bodies, guards)
}

/// Run the arm `body` as a block entered on `it`, its value the evaluation's: a tail arm runs
/// in tail position owing the evaluation's contract, as a frame's last statement is, so a recursion
/// through it tails; any other's value finishes this one.
pub(super) fn arm_ran<'step, 'graph, 'here>(
    step: &mut Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_, '_>,
    body: &'step BodyShape<'step>,
    it: Held<'graph, 'here>,
) -> Next<'graph, 'here> {
    let writer = step.writer();
    let it_slot = body.slot(BinderSymbol::Value(IMPLICIT.it.symbol()));
    let arms = match (it.kept, it_slot) {
        (Some(part), Some((slot, _))) => Some(resident(
            writer,
            ArmKept {
                segment: body.segment(),
                slot,
                part,
                outer: at.within.arms,
            },
        )),
        _ => at.within.arms,
    };
    let tails = at.owed.filter(|_| body.arm().is_some_and(|arm| arm.tail));
    let entry = BlockEntry {
        segment: body.segment(),
        enclosing: at.view.view(),
        entered: Entered::Arm(it.value),
        owed: tails,
        use_: inside(at),
        within: Within { arms, ..at.within },
    };
    ran_block(at, entry, FINISHING)
}

/// The value a value guard's scalar is.
fn scalar_value<'graph>(scalar: Scalar<'graph>) -> Value<'graph> {
    match scalar.literal() {
        KLiteral::Number(number) => Value::Number(number),
        KLiteral::String(text) => Value::Str(text),
        KLiteral::Boolean(truth) => Value::Bool(truth),
        KLiteral::Null => Value::Null,
    }
}

/// `EVAL <code> -> <Type>`: the code run in a frame over the names and keys the `EVAL` offers,
/// owing the declared type as a called frame owes its return; the frame reports the code's first
/// registration overlapping a builtin. An operand that is no code, and every refusal, is a fault.
fn evaluated<'step, 'graph, 'here, 'scratch>(
    step: &mut Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_, 'scratch>,
    node: &'step KExpression<'step>,
) -> Next<'graph, 'here> {
    let program = at.program;
    let types = &at.types;
    let scratch = Bump::new();
    let [_, operand, _, declared] = node.parts else {
        unreachable!("an `EVAL` has its keyword, its code, `->` and a type")
    };
    let wanted = [Wanted::Evaluated(&operand.value)];
    let operands = match gathered(step, at, &wanted, Use::Keeps, &scratch) {
        Gathered::Ready(operands) => operands,
        other => return unready(other),
    };
    let [Operand::Value(Held { value, .. })] = operands[..] else {
        unreachable!("an `EVAL`'s operand is evaluated")
    };
    let writer = step.writer();
    let Some(code) = value.as_code() else {
        let raised = Fault::NotCode { value };
        return raise(step, raised);
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
        Err(refused) => return raise(step, Fault::Unelaborated(refused)),
    };
    let offered = offered(at, writer, &operand.value, &scratch);
    exits(step, at, &scratch, |exit| {
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
        (returns, node.source),
        at.within.inherit,
        at.use_,
    ) {
        Ok(request) => {
            // Code composed at run time runs under its composition, where its shape lies.
            let composed = (code.code()).and_then(|node| node.flat()?.root());
            let asked = match composed {
                Some(root) => step.spawn_composing(request, root.digest),
                None => step.spawn(request),
            };
            park(at, asked, FINISHING)
        }
        Err(refused) => raise(step, refused),
    }
}

/// The record of the names and keys an `EVAL` offers the code `operand` holds — a key as the list
/// of its functions, as a use at the key written at the `EVAL` resolves it.
fn offered<'step, 'graph, 'here>(
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_, '_>,
    writer: Writer<'here, '_>,
    operand: &'step ExpressionPart<'step>,
    scratch: &Bump,
) -> KValue<'graph, 'here> {
    let offers = at.view.shape().offers(at.view.shape().site(operand));
    crate::knot::offered(writer, &at.view, offers, &at.types, scratch)
}

/// A list literal with a spread among its items, over its items' and spread lists' values `parts`:
/// a view concatenating each spread list and the runs of items between them, built in the home as
/// any literal is. A spread of what is no list is a fault.
fn spread<'step, 'graph, 'here>(
    step: &mut Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_, '_>,
    part: &'step ExpressionPart<'step>,
    items: &'step [ListItem<'step>],
    parts: &[Held<'graph, 'here>],
) -> Next<'graph, 'here> {
    let listed = |value: KValue<'_, '_>| {
        matches!(value, Value::List(_))
            || matches!(value.as_circular(), Some((_, Circular::List(_))))
    };
    let spread = items
        .iter()
        .zip(parts)
        .find(|(item, held)| matches!(item, ListItem::Spread(_)) && !listed(held.value));
    if let Some((_, held)) = spread {
        return raise(step, Fault::NotAList { value: held.value });
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
fn container<'step, 'graph, 'here, 'scratch>(
    step: &mut Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_, 'scratch>,
    part: &'step ExpressionPart<'step>,
) -> Next<'graph, 'here> {
    let scratch = Bump::new();
    // Lowering is tried once, before any part is asked for.
    if at.received.get().is_empty()
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
    let operands = match gathered(step, at, &wanted, built(at), &scratch) {
        Gathered::Ready(operands) => operands,
        other => return unready(other),
    };
    let mut parts = BumpVec::with_capacity_in(operands.len(), &scratch);
    parts.extend(operands.iter().map(|operand| match operand {
        Operand::Value(held) => *held,
        Operand::Label(_) => unreachable!("a container's parts are evaluated"),
    }));
    if let ExpressionPart::DictLiteral(_) = part
        && let Some(pair) = (parts.chunks(2)).find(|pair| Key::admits(&pair[0].value).is_err())
    {
        return raise(step, Fault::NotAKey { key: pair[0].value });
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
fn call<'step, 'graph, 'here, 'scratch>(
    step: &mut Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_, 'scratch>,
    node: &'step KExpression<'step>,
    list: &'step CandidateList<'step>,
) -> Next<'graph, 'here> {
    let program = at.program;
    let types = &at.types;
    let scratch = Bump::new();
    let wanted = slots(node, &scratch);
    let operands = match gathered(step, at, &wanted, Use::Keeps, &scratch) {
        Gathered::Ready(operands) => operands,
        other => return unready(other),
    };
    let writer = step.writer();
    let mut arguments = BumpVec::with_capacity_in(operands.len(), &scratch);
    arguments.extend(operands.iter().map(Operand::ktype));
    let site = at.view.shape().site_of_node(node);
    let fixed = (at.view.facts()).use_statics(&at.types, at.view.shape(), site);
    let contributed = match contributed(at, site, fixed.contributions, &scratch) {
        Ok(contributed) => contributed,
        Err(refused) => return raise(step, Fault::Unelaborated(refused)),
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
        .owed
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
                let request = crate::program::under_composition(program, code, source, at.use_);
                let asked = step.spawn_composing(request, root.digest);
                return park(at, asked, FINISHING);
            }
            let value = match builtins::run(native, at, node, writer, &operands, &scratch) {
                Ok(value) => value,
                Err(fault) => return raise(step, fault),
            };
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
            let tails = at.owed.filter(|owed| {
                callee.as_callable().and_then(Knotted::coerced).is_none()
                    && keeps(routed, || {
                        (owed.contract).kept_by(types, registered.shape, Some(solution))
                    })
            });
            exits(step, at, &scratch, |exit| {
                exit.value(&callee);
                exit.value(&arguments);
                tails.iter().for_each(|owed| exit.contract(&owed.contract));
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
            let call = |owed: Option<Owed<'here>>| {
                crate::program::call(
                    program,
                    callee,
                    arguments,
                    CallKind::Keyworded,
                    &[],
                    handed,
                    (node.source, owed),
                    at.within.inherit,
                    Some(passing),
                    at.use_,
                )
            };
            return called(step, at, callee, tails, hop, call);
        }
        Selection::NoOverload => Fault::NoOverload {
            key: list.elements,
            arguments: argument_values(writer, program, &operands, &scratch),
            types: &arguments,
        },
        Selection::Ambiguous(count) => Fault::AmbiguousOverloads {
            key: list.elements,
            arguments: argument_values(writer, program, &operands, &scratch),
            types: &arguments,
            count,
        },
    };
    raise(step, raised)
}

/// The values a call's operands came to, for a fault naming them: a bare label as its name's text,
/// laid through `writer`.
fn argument_values<'graph, 'here, 'x>(
    writer: Writer<'here, '_>,
    program: &Program<'graph>,
    operands: &[Operand<'graph, 'here>],
    scratch: &'x Bump,
) -> &'x [KValue<'graph, 'here>] {
    let value = |operand: &Operand<'graph, 'here>| match operand {
        Operand::Value(held) => held.value,
        Operand::Label(name) => {
            let name = program.symbols().display(name.symbol()).to_string();
            Value::Str(writer.text(&name))
        }
    };
    collect(scratch, operands.iter().map(value))
}

/// What a call over the record `arguments` hands its frame through kept parts: each
/// argument of `named`, by the name its parameter binds, that is held beside a kept part, the
/// callee, through the kept part it was read through, and the record's field names as its type
/// names them, in the record's order.
fn passing<'graph, 'here>(
    writer: Writer<'here, '_>,
    at: &Evaluation<'_, 'graph, 'here, '_, '_, '_, '_>,
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
fn threads(at: &Evaluation<'_, '_, '_, '_, '_, '_, '_>, part: &ExpressionPart<'_>) -> bool {
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
    at: &Evaluation<'_, '_, 'here, '_, '_, '_, '_>,
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
    at: &Evaluation<'_, '_, '_, '_, '_, '_, '_>,
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
fn apply<'step, 'graph, 'here, 'scratch>(
    step: &mut Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_, 'scratch>,
    node: &'step KExpression<'step>,
    head: &'step ExpressionPart<'step>,
    argument: &'step ExpressionPart<'step>,
) -> Next<'graph, 'here> {
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
    let operands = match gathered(step, at, &wanted, use_, &scratch) {
        Gathered::Ready(operands) => operands,
        other => return unready(other),
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
    // A construction the rule refuses is refused here, before it is built in its home.
    if let Value::Type(named) = head {
        let identity = construction(
            types,
            &scratch,
            named.handle(),
            argument.value.concrete_ktype(),
        );
        if let Err(refused) = identity {
            return raise(step, Fault::Unconstructible(refused));
        }
        let parts = [held(&operands[0]), argument];
        return finish_built(step, at, &parts, |writer, types, values| {
            let [Value::Type(head), argument] = values else {
                unreachable!("a construction's head is a type")
            };
            let tagged = Tagged::construct(writer, head, *argument, types, &Bump::new());
            Value::Tagged(tagged.expect("a construction the rule refuses is refused before"))
        });
    }
    // A list, a dict or a string at the head is read by its argument, built in the home as a
    // construction is.
    if lookup::readable(head) {
        let reading = match lookup::reading(head, argument.value, types, &scratch) {
            Ok(reading) => reading,
            Err(fault) => return raise(step, fault),
        };
        let parts = [held(&operands[0]), argument];
        return finish_built(step, at, &parts, |writer, types, values| {
            let [head, argument] = values else {
                unreachable!("a read is its head and its argument")
            };
            lookup::read(
                program,
                writer,
                types,
                (*head, *argument),
                reading,
                &Bump::new(),
            )
        });
    }
    let writer = step.writer();
    // What the frame solves each parameter from, in symbol order: `None` where its carried type.
    let contributed = match contributed(at, argument_site, recorded, &scratch) {
        Ok(contributed) => collect(writer, contributed.iter().copied()),
        Err(refused) => return raise(step, Fault::Unelaborated(refused)),
    };
    // A call keeps its contract by the type its callee shows; a barrier never tails, since its
    // value crosses the barrier where its frame ends.
    let routed = at.owed.and_then(|_| {
        let route = (at.view.facts()).route(at.view.shape(), at.view.shape().site_of_node(node));
        route.map(|route| route.per[0])
    });
    let tails = at.owed.filter(|owed| {
        (head.as_callable().and_then(Knotted::function)).is_some()
            && keeps(routed, || {
                (owed.contract).kept_by(&at.types, head.ktype(), None)
            })
    });
    exits(step, at, &scratch, |exit| {
        exit.value(&head);
        exit.value(&argument.value);
        contributed.iter().flatten().for_each(|kt| exit.handle(*kt));
        tails.iter().for_each(|owed| exit.contract(&owed.contract));
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
    let call = |owed: Option<Owed<'here>>| {
        crate::program::call(
            program,
            head,
            argument,
            CallKind::ByName,
            contributed,
            handed,
            (node.source, owed),
            at.within.inherit,
            Some(passing),
            at.use_,
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
    step: &mut Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_, '_>,
    callee: KValue<'graph, 'here>,
    tails: Option<Owed<'here>>,
    hop: bool,
    call: impl Fn(Option<Owed<'here>>) -> Request<'graph, 'here, KBundle>,
) -> Next<'graph, 'here> {
    debug_assert!(
        !hop || callee.as_callable() == at.view.callable(),
        "a hop's callee is its frame's own"
    );
    // A tail hop counts the frame it replaces in the table its successor carries.
    let mut request = call(tails.map(|owed| owed.hopped(step.writer())));
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
            return Next::Tail(Tailed::Under(request.work, at.within.build));
        }
        let types = step.types();
        let seek = seek(&mut request.work.state, keys, &at.types, &types);
        return Next::Tail(Tailed::Seeking(request.work, seek));
    }
    let types = step.types();
    let seek = seek(&mut request.work.state, keys, &at.types, &types);
    #[cfg(test)]
    {
        let bytes = step.writer().in_use();
        ASKER_BYTES.with(|most| most.set(most.get().max(bytes)));
    }
    let asked = step.spawn_seeking(request, seek);
    park(at, asked, FINISHING)
}

/// Check that `value` carries a type within its node's static type:
/// under its upper end read above its variables, and over its lower end read below them, as a
/// candidate's judgement reads an argument's — read through `types`, the chain of the half the
/// value lies in.
#[cfg(debug_assertions)]
fn carried_under_static<'step, 'graph, 'here>(
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_, '_>,
    types: &Chain<'_, '_, '_, '_>,
    value: &KValue<'graph, '_>,
) {
    let shape = at.view.shape();
    let node = at.node;
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
    at: &Evaluation<'_, 'graph, 'here, '_, '_, '_, '_>,
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
    at: &Evaluation<'_, 'graph, '_, '_, '_, '_, '_>,
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
/// quantified function at the instance the load solved there, where it solved one — a quote born,
/// or the fault a quote whose splice does not compose raises. `None` for a name the shape resolved
/// no mention of, which is an invariant break.
fn in_place<'step, 'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_, '_>,
    part: &'step ExpressionPart<'step>,
    scratch: &Bump,
) -> Option<Result<Held<'graph, 'here>, KFault<'graph, 'here>>> {
    let writer = step.writer();
    let types = &at.types;
    match part {
        ExpressionPart::Literal(_) => lowered(writer, at, part, scratch).map(|v| Ok(Held::bare(v))),
        ExpressionPart::QuotedExpression(_) => {
            let written = (at.program).written_under(at.view.facts(), at.within.root);
            Some(
                match quote(writer, &at.view, written, part, types, scratch) {
                    Ok(code) => Ok(Held::bare(Value::Knotted(code))),
                    Err(refused) => Err(Fault::Uncomposed(refused)),
                },
            )
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
                return Some(Ok(Held { value: read, kept }));
            };
            let Value::Knotted(member) = read else {
                unreachable!(
                    "an instance site reads a bare quantified function: a header at a scheme \
                     comes only from a module member read, which makes its own instance"
                )
            };
            Some(Ok(Held::bare(Value::Knotted(instance(
                writer, member, solution, types, scratch,
            )))))
        }
    }
}

/// What the frame keeps of the value a read at `coordinate` reads: an arm's `it` as its arm keeps
/// it, a slot of its own body whole, or a capture of its callee as that part of the kept callee.
/// `None` outside a called frame or an arm, and for a read it keeps nothing of.
fn kept_read<'graph, 'here>(
    at: &Evaluation<'_, 'graph, 'here, '_, '_, '_, '_>,
    writer: Writer<'here, '_>,
    coordinate: Coordinate,
) -> Option<KeptPart<'graph, 'here>> {
    let landing = at.view.landing(coordinate);
    if let (Some((segment, slot)), Some(arms)) = (landing, at.within.arms)
        && let Some(part) = arms.slot(segment, slot)
    {
        return Some(part);
    }
    let table = at.within.kept?;
    if let Some((segment, slot)) = landing {
        let kept = table.slot(segment, slot)?;
        return Some(KeptPart { kept, path: &[] });
    }
    let (segment, slot) = at.view.capture_landing(coordinate)?;
    table.capture(writer, segment, slot)
}

/// How an evaluation asks for the parts of a construction it builds in its home: forwarded, so
/// each is built in that home too; kept in a caching body, which keeps its values.
pub(super) fn built(at: &Evaluation<'_, '_, '_, '_, '_, '_, '_>) -> Use {
    match at.view.facts().caches(at.view.shape()) {
        true => Use::Keeps,
        false => Use::Forwards,
    }
}

/// Every operand of `wanted`, in order, once each part not read in place has been delivered: the
/// next one not yet delivered is asked for, with `use_` — or, owing a frame's contract, with
/// `Threads` for a part of the frame body's threaded set, so it is built where it outlives the hop
/// — one at a time, in order. A fault delivered is handed back alone: what follows it is never
/// asked for.
fn gathered<'step, 'x, 'graph, 'here, 'scratch>(
    step: &mut Taking<'_, 'graph, '_, 'here, 'scratch, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_, 'scratch>,
    wanted: &[Wanted<'step>],
    use_: Use,
    scratch: &'x Bump,
) -> Gathered<'x, 'graph, 'here> {
    let asked = |wanted: &&Wanted<'step>| match wanted {
        Wanted::Evaluated(part) if !read_in_place(part) => Some(*part),
        _ => None,
    };
    let count = wanted.iter().filter_map(|part| asked(&part)).count();
    let mut received = at.received.get();
    if received.is_empty() && count > 0 {
        received = step.scratch_writer().fill(count, |_| Cell::new(None));
        at.received.set(received);
    }
    match at.delivered.take() {
        Some(Delivered::Value(held)) => match received.iter().find(|slot| slot.get().is_none()) {
            Some(slot) => slot.set(Some(held)),
            None => return Gathered::Broken(StepError::Unredeemable),
        },
        Some(Delivered::Fault(inner)) => return Gathered::Raised(inner),
        None => {}
    }
    if let Some(next) = received.iter().position(|slot| slot.get().is_none()) {
        let part = (wanted.iter().filter_map(|part| asked(&part)))
            .nth(next)
            .expect("one slot per part asked for");
        let shape = at.view.shape();
        let site = shape.site(part);
        let threads = at.owed.is_some() && shape.threaded_parts().binary_search(&site).is_ok();
        return Gathered::Part {
            site,
            use_: if threads { Use::Threads } else { use_ },
        };
    }
    let mut received = received.iter().map(Cell::get);
    let mut operands = BumpVec::with_capacity_in(wanted.len(), scratch);
    for wanted in wanted {
        let operand = match wanted {
            Wanted::Label(name) => Operand::Label(*name),
            Wanted::Evaluated(part) if read_in_place(part) => {
                match in_place(step, at, part, scratch) {
                    Some(Ok(value)) => Operand::Value(value),
                    Some(Err(fault)) => {
                        let fault = resident(step.writer(), fault.relaid(step.writer(), scratch));
                        return Gathered::Raised(Inner::Fault(fault));
                    }
                    None => return Gathered::Broken(StepError::Refused),
                }
            }
            Wanted::Evaluated(_) => match received.next().flatten() {
                Some(value) => Operand::Value(value),
                None => return Gathered::Broken(StepError::Unredeemable),
            },
        };
        operands.push(operand);
    }
    Gathered::Ready(operands)
}

/// Gathering that did not come back ready: ask for its next part, end with the fault a part ended
/// with, or fail.
fn unready<'graph, 'here>(gathered: Gathered<'_, 'graph, 'here>) -> Next<'graph, 'here> {
    match gathered {
        Gathered::Part { site, use_ } => Next::Part { site, use_ },
        Gathered::Raised(inner) => Next::Raised(inner),
        Gathered::Broken(error) => Next::Failed(error),
        Gathered::Ready(_) => unreachable!("a ready gathering is not unready"),
    }
}

/// Wait on the frame `asked` spawned, resuming at `stage`.
pub(super) fn park<'graph, 'here>(
    at: &Evaluation<'_, 'graph, 'here, '_, '_, '_, '_>,
    asked: Asked,
    stage: u32,
) -> Next<'graph, 'here> {
    at.stage.set(stage);
    Next::Spawned(asked)
}

/// Run the block `entry`, resuming at `stage` when its value or fault is in.
pub(super) fn ran_block<'graph, 'here>(
    at: &Evaluation<'_, 'graph, 'here, '_, '_, '_, '_>,
    entry: BlockEntry<'graph, 'here>,
    stage: u32,
) -> Next<'graph, 'here> {
    at.stage.set(stage);
    Next::Block(entry)
}

/// The one child whose value is this evaluation's is in: finish with it.
fn finishing<'step, 'graph, 'here>(
    step: &mut Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_, '_>,
) -> Next<'graph, 'here> {
    match at.delivered.take() {
        Some(Delivered::Value(value)) => finish(step, at, value),
        Some(Delivered::Fault(inner)) => Next::Raised(inner),
        None => Next::Failed(StepError::Unredeemable),
    }
}

/// Finish with what `build` builds over `parts`, built where the node's `Use` says, so nothing is
/// built here to be crossed whole: the parts cross from their kept parts where they have them. A
/// caching body's value is built here, on the body's chain, and leaves it only where it adopts.
pub(super) fn finish_built<'step, 'graph, 'here>(
    step: &mut Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_, '_>,
    parts: &[Held<'graph, 'here>],
    build: impl for<'their> FnOnce(
        Writer<'their, '_>,
        &Chain<'_, 'their, '_, '_>,
        &[KValue<'graph, 'their>],
    ) -> KValue<'graph, 'their>,
) -> Next<'graph, 'here> {
    let caches = at.view.facts().caches(at.view.shape());
    if caches {
        let scratch = Bump::new();
        let mut values = BumpVec::with_capacity_in(parts.len(), &scratch);
        values.extend(parts.iter().map(|part| part.value));
        let value = build(step.writer(), &at.types, &values);
        return finish(step, at, value);
    }
    let built = in_home(at, build);
    match step.built_as(at.use_, parts, None, built) {
        Some(held) => delivered(step, at, held),
        None => Next::Failed(StepError::Stale),
    }
}

/// `build` as a home build: what it builds, checked under debug against its expression's static
/// type.
fn in_home<'step, 'graph, 'here>(
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_, '_>,
    build: impl for<'their> FnOnce(
        Writer<'their, '_>,
        &Chain<'_, 'their, '_, '_>,
        &[KValue<'graph, 'their>],
    ) -> KValue<'graph, 'their>,
) -> impl for<'their> FnOnce(
    Writer<'their, '_>,
    &Chain<'_, 'their, '_, '_>,
    &[KValue<'graph, 'their>],
    &'their &'graph (),
) -> Active<'graph, 'their, KValueFamily> {
    move |writer, types, values, _| {
        let value = build(writer, types, values);
        #[cfg(debug_assertions)]
        carried_under_static(at, types, &value);
        #[cfg(not(debug_assertions))]
        let _ = at;
        Active::new(value)
    }
}

/// Finish with `value`, checked under debug against its node's static type. A value owed a
/// frame's contract is held to it by the frame's runner, where the frame ends.
pub(super) fn finish<'step, 'graph, 'here>(
    step: &mut Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_, '_>,
    value: impl Into<Held<'graph, 'here>>,
) -> Next<'graph, 'here> {
    let held = value.into();
    #[cfg(debug_assertions)]
    carried_under_static(at, &at.types, &held.value);
    delivered(step, at, held)
}

/// End with `fault`, raised here: laid in this cell's write home, every borrow it holds copied.
fn raise<'graph, 'here>(
    step: &mut Taking<'_, 'graph, '_, 'here, '_, '_>,
    fault: Fault<'_, KValue<'graph, 'here>>,
) -> Next<'graph, 'here> {
    let fault = resident(step.writer(), fault.relaid(step.writer(), &Bump::new()));
    Next::Raised(Inner::Fault(fault))
}

/// Hand `value` on, placed where the node's `Use` says; in a caching body its debug check reads the
/// body's chain. A value in tail position is handed on as it is held: the frame's runner holds it
/// to the contract and crosses it once, where the frame ends.
fn delivered<'step, 'graph, 'here>(
    step: &mut Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_, '_>,
    value: impl Into<Held<'graph, 'here>>,
) -> Next<'graph, 'here> {
    let held = value.into();
    if at.owed.is_some() {
        return Next::Value(held);
    }
    let check = at.view.facts().caches(at.view.shape()).then_some(&at.types);
    match step.crossed_as(at.use_, held, check.map(|types| types as _)) {
        Some(held) => Next::Value(held),
        None => Next::Failed(StepError::Stale),
    }
}

/// In a caching body, hand `adopt` the [`Exit`] a value leaving it is adopted through, into the
/// step's region, its walks laid in `scratch`; elsewhere nothing leaves a scratch tier, and `adopt`
/// is not called.
fn exits<'step, 'graph, 'here>(
    step: &Taking<'_, 'graph, '_, 'here, '_, '_>,
    at: &Evaluation<'step, 'graph, 'here, '_, '_, '_, '_>,
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
