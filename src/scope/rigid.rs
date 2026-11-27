//! The **rigid answer**: what every binding gives a question, where one does
//! ([design/binding-graph.md](../../design/binding-graph.md)).
//!
//! One rule per question kind, [`rigid_answer`]: the closed door's answer where every type the
//! question judges is closed; else an answer only where no binding within the names' ends changes
//! it. Every door of the walk answers rigidly by it at the load (`elaborate::judge`), and the
//! binding graph answers an instance's positions left open by it (`dispatch::graph::ask`), so the
//! load and an instance read one rule and a stored site question decides alike under either.
//!
//! The helpers read a variable's declared bound, or a head no binding rebuilds as another, never
//! what a binding gives a name.

use crate::memory::BumpAllocator;
use crate::type_lattice::{
    DeclaredType, DerivedId, Interval, KType, Parametric, TypeHandle, TypeNode, Verdict,
    bound_above, fits, is_subtype_of, meet, scheme_bound_above,
};
use crate::values::Chain;

use super::derived::DerivedTerm;
use super::heads::{judge_signed, signature_outside};
use super::question::{Answer, Question, answer, never_overlaps, unsatisfiable};
use super::rule::Rule;

/// The answer every binding gives `question`, where one does: the closed door's over closed types;
/// over names, each kind's rule below. `terms` reads a derived name's term, for a name that is a
/// signature at every binding. `None` is undecided: a binding decides it.
pub fn rigid_answer<'term>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    terms: &dyn Fn(DerivedId) -> Option<DerivedTerm<'term>>,
    question: &Question<'_>,
) -> Option<Answer> {
    if closed(types, question) {
        return answer(types, scratch, question);
    }
    match *question {
        Question::Verdict { shape, arguments } => {
            let judged = judge_signed(types, scratch, shape.ty, arguments);
            rigid_verdict(types, question, judged.verdict)
        }
        // A rigid refusal holds at every binding ([`unsatisfiable`]); a fit at the upper end holds
        // within it.
        Question::NeverSatisfies { value, wanted } => {
            if unsatisfiable(types, scratch, value, wanted).is_some() {
                Some(Answer::Never)
            } else if fits(types, scratch, value.upper, wanted) {
                Some(Answer::Satisfiable)
            } else {
                None
            }
        }
        // *Fits* that holds over names holds at every binding within their ends.
        Question::Fits { a, b } => {
            (a.rebounds.is_empty() && fits(types, scratch, a.ty, b)).then_some(Answer::Bool(true))
        }
        // An upper end that fits at every binding holds the lower end within too.
        Question::LowerEndOutside { value, bound } => {
            if lower_end_outside_signed(types, scratch, value, bound) {
                Some(Answer::Bool(true))
            } else {
                fits(types, scratch, value.upper, bound).then_some(Answer::Bool(false))
            }
        }
        Question::Same { a, b } if a == b => Some(Answer::Bool(true)),
        Question::Same { a, b } => apart(types, scratch, a, b).then_some(Answer::Bool(false)),
        Question::Kind { ty } => (!kind_varies(types, ty))
            .then(|| Answer::Kind(bound_above(types, scratch, ty).kind_of(types))),
        Question::Module { ty } => module(types, scratch, terms, ty).map(Answer::Bool),
        // The order relates closed types alone; a variable whose declared bound lies under a closed
        // `b` does at every binding.
        Question::Under { a, b } => (types.concrete(b))
            .is_some_and(|b| rigid_within(types, scratch, a, b))
            .then_some(Answer::Bool(true)),
        // A candidate's and an arm's run answer by their own rule, *may* standing for every binding.
        Question::Runs { .. } | Question::Arms { .. } => answer(types, scratch, question),
        Question::Overlaps {
            registered,
            against,
        } => never_overlaps(types, scratch, registered, against).then_some(Answer::Overlaps(None)),
        // A signature is one at every pin, so a field read over it reads a module member.
        Question::Defined {
            rule: Rule::Fielded,
            operands: [DeclaredType::Type(ty)],
        } if matches!(
            types.node(*ty),
            TypeNode::Signature { .. }
                | TypeNode::SignatureApply { .. }
                | TypeNode::SignatureMeet { .. }
        ) =>
        {
            Some(Answer::Bool(true))
        }
        // A head no binding changes says whether a type is a function.
        Question::IsFunction { ty } => match types.node(ty) {
            TypeNode::KFunction { .. } => Some(Answer::Bool(true)),
            TypeNode::Union { .. } => None,
            node if is_variable(&node) => None,
            _ => Some(Answer::Bool(false)),
        },
        Question::Restamp { .. }
        | Question::Outranks { .. }
        | Question::Defined { .. }
        | Question::Closed { .. } => None,
    }
}

/// [`rigid_answer`] of the verdict question `question`, for a door that judged it already:
/// `verdict` is its [`judge_signed`] judgement, *always* and *never* holding at every binding
/// within the arguments' ends, and *maybe* the answer over closed types alone.
pub fn rigid_verdict(
    types: &Chain<'_, '_, '_, '_>,
    question: &Question<'_>,
    verdict: Verdict,
) -> Option<Answer> {
    // Over a scheme beside bounds written over names, judged at their upper ends: a tighter bound
    // narrows the scheme, so only *never* holds at every binding.
    if let Question::Verdict { shape, .. } = question
        && !shape.rebounds.is_empty()
    {
        return (verdict == Verdict::Never).then_some(Answer::Verdict(verdict));
    }
    (verdict != Verdict::Maybe || closed(types, question)).then_some(Answer::Verdict(verdict))
}

/// Whether every type `question` judges is closed.
fn closed(types: &Chain<'_, '_, '_, '_>, question: &Question<'_>) -> bool {
    let mut closed = true;
    question.each_type(&mut |ty| closed &= types.concrete_declared(ty).is_some());
    closed
}

/// Whether `value`'s lower end lies outside `bound` at every binding: its lower end read below its
/// variables lies outside `bound` read above its own ([`lower_end_outside`]
/// (crate::type_lattice::lower_end_outside)), or, under a closed bound, by the names a signature
/// declares, which it declares at every pin though the lattice reads it below as `Never`
/// ([`signature_outside`]).
pub fn lower_end_outside_signed(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    value: Interval,
    bound: DeclaredType<Parametric>,
) -> bool {
    let above = match bound {
        DeclaredType::Type(bound) => {
            DeclaredType::Type(Parametric::from(bound_above(types, scratch, bound)))
        }
        DeclaredType::Scheme(scheme) => {
            DeclaredType::Scheme(scheme_bound_above(types, scratch, scheme))
        }
    };
    crate::type_lattice::lower_end_outside(types, scratch, value, above)
        || (bound.as_type().and_then(|bound| types.concrete(bound)))
            .is_some_and(|asked| signature_outside(types, value.lower, asked))
}

/// Whether no binding makes `a` and `b` one type: two closed types that differ, a type that holds
/// a variable against a closed type it is at no binding ([`never_the`]), or two heads no binding
/// rebuilds that differ. A scheme is judged apart from nothing.
pub fn apart(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    a: DeclaredType<Parametric>,
    b: DeclaredType<Parametric>,
) -> bool {
    let (DeclaredType::Type(a), DeclaredType::Type(b)) = (a, b) else {
        return false;
    };
    match (types.concrete(a), types.concrete(b)) {
        (Some(a), Some(b)) => a != b,
        (None, Some(closed)) => never_the(types, scratch, a, closed),
        (Some(closed), None) => never_the(types, scratch, b, closed),
        (None, None) => matches!(
            (stable_head(&types.node(a)), stable_head(&types.node(b))),
            (Some(a), Some(b)) if a != b
        ),
    }
}

/// Whether no binding makes `open`, which holds a variable, the closed type `closed`. `Never` where
/// `open` is `Never` at no binding; a type outside the bound above every binding of `open`; a
/// union, which lies above each member at every binding, where a closed member lies outside
/// `closed`; a head no binding rebuilds, where `closed`'s head differs.
fn never_the(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    open: Parametric,
    closed: KType,
) -> bool {
    if closed == KType::NEVER {
        return !may_be_never(types, DeclaredType::Type(open));
    }
    if !is_subtype_of(types, scratch, closed, bound_above(types, scratch, open)) {
        return true;
    }
    match types.node(open) {
        TypeNode::Union { members } => members.iter().any(|member| {
            (types.concrete(member))
                .is_some_and(|member| !is_subtype_of(types, scratch, member, closed))
        }),
        node => {
            stable_head(&node).is_some_and(|head| stable_head(&types.node(closed)) != Some(head))
        }
    }
}

/// The head of `node` where a binding writes into it but never rebuilds it as another head: a
/// list, a dict, a record or a function type.
fn stable_head<H: TypeHandle>(node: &TypeNode<'_, H>) -> Option<u8> {
    match node {
        TypeNode::List { .. } => Some(0),
        TypeNode::Dict { .. } => Some(1),
        TypeNode::Record { .. } => Some(2),
        TypeNode::KFunction { .. } => Some(3),
        _ => None,
    }
}

/// Whether `ty`, which holds a variable, is a module at every binding or at none. A bare variable
/// is one where its term names a signature ([`signature_name`]), and none where its bound meets no
/// signature; a signature or its application is one; a union or a signature meet may come out one
/// or not; any other head is none.
fn module<'term>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    terms: &dyn Fn(DerivedId) -> Option<DerivedTerm<'term>>,
    ty: Parametric,
) -> Option<bool> {
    if bare_variable(types, DeclaredType::Type(ty)) {
        if signature_name(types, terms, ty) {
            return Some(true);
        }
        return variable_outside(types, scratch, ty, KType::EMPTY_SIGNATURE).then_some(false);
    }
    match types.node(ty) {
        TypeNode::Signature { .. } | TypeNode::SignatureApply { .. } => Some(true),
        TypeNode::Union { .. } | TypeNode::SignatureMeet { .. } => None,
        _ => Some(false),
    }
}

/// Whether `ty` is a derived name that is a signature at every binding, read off its term in
/// `terms`: a view's name ([`Rule::View`]), the type a `:Module` binder carries
/// ([`DerivedTerm::Carried`]), or a signature written over names ([`DerivedTerm::Signature`]).
pub fn signature_name<'term>(
    types: &Chain<'_, '_, '_, '_>,
    terms: &dyn Fn(DerivedId) -> Option<DerivedTerm<'term>>,
    ty: Parametric,
) -> bool {
    let TypeNode::Derived { term, .. } = types.node(ty) else {
        return false;
    };
    matches!(
        terms(term),
        Some(
            DerivedTerm::Output {
                rule: Rule::View,
                ..
            } | DerivedTerm::Signature { .. }
        )
    ) || terms(term).is_some_and(|term| term.carries_a_module())
}

/// Whether `end` is a lexical variable, a derived name or a projection whose declared bound lies
/// under `ceiling` in the order: every binding lies under the bound, so it lies under `ceiling`.
pub fn rigid_within(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    end: Parametric,
    ceiling: KType,
) -> bool {
    match types.node(end) {
        TypeNode::Lexical { bound, .. }
        | TypeNode::Derived { bound, .. }
        | TypeNode::Projection { bound, .. } => is_subtype_of(types, scratch, bound, ceiling),
        _ => false,
    }
}

/// Whether `ty` is a variable whose declared bound meets `other` at `Never` alone, so that no
/// binding of it lies under `other`: a binding lies under the bound, or is a view's carrier, which
/// lies under nothing but `Any` and a union holding it.
pub fn variable_outside(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: Parametric,
    other: KType,
) -> bool {
    (types.node(ty).rigid_bound())
        .is_some_and(|bound| meet(types, scratch, bound, other) == KType::NEVER)
}

/// Whether some binding makes `ty` `Never`: a variable may be bound to it, and a union is it where
/// every member is; any other head is never `Never`.
pub fn may_be_never(types: &Chain<'_, '_, '_, '_>, ty: DeclaredType<Parametric>) -> bool {
    let DeclaredType::Type(ty) = ty else {
        return false;
    };
    if let Some(closed) = types.concrete(ty) {
        return closed == KType::NEVER;
    }
    match types.node(ty) {
        TypeNode::Union { members } => members
            .iter()
            .all(|member| may_be_never(types, DeclaredType::Type(member))),
        node => is_variable(&node),
    }
}

/// Whether a binding decides the kind of `ty`: a variable, or a union holding one.
pub fn kind_varies(types: &Chain<'_, '_, '_, '_>, ty: Parametric) -> bool {
    bare_variable(types, DeclaredType::Type(ty))
        || (matches!(types.node(ty), TypeNode::Union { .. }) && types.concrete(ty).is_none())
}

/// Whether `ty` is a variable itself, not a type built over one.
pub fn bare_variable(types: &Chain<'_, '_, '_, '_>, ty: DeclaredType<Parametric>) -> bool {
    match ty {
        DeclaredType::Type(ty) => types.concrete(ty).is_none() && is_variable(&types.node(ty)),
        DeclaredType::Scheme(_) => false,
    }
}

/// Whether `node` is a variable's own node.
pub fn is_variable<H: TypeHandle>(node: &TypeNode<'_, H>) -> bool {
    matches!(
        node,
        TypeNode::Quantified { .. }
            | TypeNode::Lexical { .. }
            | TypeNode::Path { .. }
            | TypeNode::Derived { .. }
            | TypeNode::Parameter { .. }
            | TypeNode::Projection { .. }
    )
}
