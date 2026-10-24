//! The **records type visitor**: every type a unit's record or a closing names, handed out once per
//! place it is named. What a walk interns in a chain that dies — a settlement's scratch chain, a
//! frame's region — is adopted where the records rest through it, so every handle a reader meets
//! resolves. A load handle costs the adopter a bit test, so the visitor does not tell them apart.
//!
//! Each match below lists every variant, so a type-bearing field a record gains does not compile
//! until it is visited.

use crate::type_lattice::{DeclaredType, Interval, KType, Parametric};

use super::records::{Born, Closing, Typed, UnitFacts};
use super::shape::ShapeError;
use super::tree::{Leaf, NodeCapture, Tree, Within};
use super::typed::{
    Callable, Elaboration, InstanceRead, Registered, Resolved, Static, StaticSolution, StaticType,
};

/// What the visitor hands each type to.
type Each<'e> = dyn FnMut(DeclaredType<Parametric>) + 'e;

impl UnitFacts<'_> {
    /// Hand `each` every type this record names.
    pub fn each_type(&self, each: &mut Each<'_>) {
        for (_, typed) in self.typed.iter() {
            typed_types(typed, each);
        }
        for (_, born) in self.born.iter() {
            born_types(born, each);
        }
        for (_, (entry, resolution)) in self.paths.iter() {
            if let Some(through) = entry.through {
                each(through.into());
            }
            tree_types(&resolution.tree, each);
            match resolution.resolved {
                Resolved::Closed(ktype) => each(ktype.into()),
                Resolved::Class(_) => {}
            }
        }
    }
}

impl Closing<'_> {
    /// Hand `each` every type this closing names.
    pub fn each_type(&self, each: &mut Each<'_>) {
        if let Some(Some(summary)) = &self.summary {
            tree_types(summary, each);
        }
        if let Some(ran) = self.ran {
            interval(ran, each);
        }
        for error in self.refusal.iter().chain(self.overlap.iter()) {
            error_types(error, each);
        }
    }
}

fn interval(typed: Interval<Parametric>, each: &mut Each<'_>) {
    each(typed.lower.into());
    each(typed.upper.into());
}

fn static_type(typed: &StaticType<'_>, each: &mut Each<'_>) {
    match typed {
        Static::Unknown => {}
        Static::Closed(ktype) => each((*ktype).into()),
        Static::Rigid { value, .. } => each((*value).into()),
    }
}

fn registered<T: Copy + Into<Parametric>>(typed: &Registered<'_, T>, each: &mut Each<'_>) {
    each(declared(typed.shape));
}

fn callable<T: Copy + Into<Parametric>>(typed: &Callable<'_, T>, each: &mut Each<'_>) {
    each(declared(typed.ktype));
    if let Some(registered_as) = &typed.registered {
        registered(registered_as, each);
    }
}

/// `typed` as a declared type over parametric handles.
fn declared<T: Into<Parametric>>(typed: DeclaredType<T>) -> DeclaredType<Parametric> {
    match typed {
        DeclaredType::Type(typed) => DeclaredType::Type(typed.into()),
        DeclaredType::Scheme(scheme) => DeclaredType::Scheme(scheme),
    }
}

fn solution(solved: &StaticSolution<'_>, each: &mut Each<'_>) {
    match solved {
        StaticSolution::Closed(types) => types.iter().for_each(|ktype| each((*ktype).into())),
        StaticSolution::Rigid { value, .. } => value.iter().for_each(|typed| each((*typed).into())),
    }
}

fn typed_types(typed: &Typed<'_>, each: &mut Each<'_>) {
    for (_, declared) in typed.declared.iter() {
        static_type(declared, each);
    }
    for (_, entry) in typed.registered.iter() {
        match entry {
            Static::Unknown => {}
            Static::Closed(closed) => registered(closed, each),
            Static::Rigid { value, .. } => registered(value, each),
        }
    }
    for (_, expression) in typed.expressions.iter() {
        static_type(expression, each);
    }
    for (_, part) in typed.parts.iter() {
        interval(*part, each);
    }
    for (_, statement) in typed.statements.iter() {
        interval(*statement, each);
    }
    for (_, binder) in typed.binders.iter() {
        match binder {
            DeclaredType::Type(typed) => interval(*typed, each),
            DeclaredType::Scheme(scheme) => each(DeclaredType::Scheme(*scheme)),
        }
    }
    for (_, read) in typed.instances.iter() {
        match read {
            InstanceRead::Solved(solved) => solution(solved, each),
            InstanceRead::AsIs => {}
        }
    }
    // A use's narrowing names candidates and verdicts, no type.
    for (_, statics) in typed.uses.iter() {
        for contribution in statics.contributions {
            static_type(contribution, each);
        }
    }
    for (_, points) in typed.selections.iter() {
        points
            .iter()
            .flatten()
            .for_each(|ktype| each((*ktype).into()));
    }
    for (_, contributions) in typed.by_name.iter() {
        contributions
            .iter()
            .for_each(|typed| static_type(typed, each));
    }
    for (_, tree) in typed.trees.iter() {
        tree_types(tree, each);
    }
    for (_, tree) in typed.contents.iter() {
        tree_types(tree, each);
    }
    if let Some(signature) = typed.signature {
        each(signature.into());
    }
}

fn born_types(born: &Born<'_>, each: &mut Each<'_>) {
    match &born.callable {
        Static::Unknown => {}
        Static::Closed(closed) => callable(closed, each),
        Static::Rigid { value, .. } => callable(value, each),
    }
    born.levels.iter().for_each(|level| each((*level).into()));
    if let Some(instance) = &born.instance {
        solution(instance, each);
    }
}

/// Every type `tree` names, its subtrees' included.
pub(super) fn tree_types(tree: &Tree<'_>, each: &mut Each<'_>) {
    match tree {
        Tree::Digest(_) | Tree::Leaf(Leaf::Unnamed(_)) => {}
        Tree::Body { captures, .. } => captures.iter().for_each(|(_, at)| tree_types(at, each)),
        Tree::Knot { members, .. } => {
            for member in *members {
                member
                    .instance
                    .iter()
                    .for_each(|ktype| each((*ktype).into()));
                for (_, capture) in member.captures {
                    match capture {
                        NodeCapture::Fellow(_) => {}
                        NodeCapture::Tree(at) => tree_types(at, each),
                    }
                }
            }
        }
        Tree::View {
            application,
            source,
            ..
        } => {
            each((*application).into());
            tree_types(source, each);
        }
        Tree::Retyped {
            carried, source, ..
        } => {
            each((*carried).into());
            tree_types(source, each);
        }
        Tree::Leaf(Leaf::Bound { within, .. }) => match within {
            None => {}
            Some(Within::Body(captures)) => {
                captures.iter().for_each(|(_, at)| tree_types(at, each));
            }
            Some(Within::Call(unfolded)) => {
                unfolded
                    .arguments
                    .iter()
                    .for_each(|(_, at)| tree_types(at, each));
                unfolded
                    .solution
                    .iter()
                    .for_each(|(_, ktype)| each((*ktype).into()));
            }
        },
    }
}

fn error_types(error: &ShapeError<'_>, each: &mut Each<'_>) {
    let mut ktype = |ktype: KType| each(ktype.into());
    match *error {
        ShapeError::Unfixed { wanted, .. } => wanted.into_iter().for_each(ktype),
        ShapeError::NoInstance { scheme, wanted, .. } => {
            ktype(wanted);
            each(DeclaredType::Scheme(scheme));
        }
        ShapeError::Inadmissible { slot, .. } => ktype(slot),
        ShapeError::Overlaps { builtin, .. } => ktype(builtin),
        ShapeError::NoAdmittingCandidate { arguments, .. }
        | ShapeError::Ambiguous { arguments, .. } => {
            arguments.iter().for_each(|typed| each(*typed))
        }
        ShapeError::NoField { of, .. } | ShapeError::NoMember { of, .. } => ktype(of),
        ShapeError::ReturnNeverSatisfied { body, returns, .. } => {
            ktype(body);
            ktype(returns);
        }
        ShapeError::AscriptionNeverSatisfied {
            value, ascribed, ..
        } => {
            ktype(value);
            ktype(ascribed);
        }
        ShapeError::AnnotationNeverSatisfied {
            value, annotated, ..
        } => {
            ktype(value);
            ktype(annotated);
        }
        ShapeError::CallNeverSatisfied {
            callee, arguments, ..
        } => {
            each(declared(callee));
            each(arguments.into());
        }
        ShapeError::NotCode { value, .. } | ShapeError::NotAModule { value, .. } => ktype(value),
        ShapeError::NotASignature { ascribed, .. } => ktype(ascribed),
        ShapeError::EvalNeverSatisfied { code, returns, .. } => {
            ktype(code);
            ktype(returns);
        }
        ShapeError::Rebind { .. }
        | ShapeError::ShadowsBuiltin { .. }
        | ShapeError::Unbound { .. }
        | ShapeError::QuantifiedValue { .. }
        | ShapeError::AmbiguousInstance { .. }
        | ShapeError::NoInstanceAtCandidates { .. }
        | ShapeError::QuantifiedRead { .. }
        | ShapeError::EagerCycle { .. }
        | ShapeError::MarkOutsideQuote { .. }
        | ShapeError::Unsupported { .. }
        | ShapeError::Malformed { .. }
        | ShapeError::Unsurfaced { .. }
        | ShapeError::Unchained { .. }
        | ShapeError::MixedGroups { .. }
        | ShapeError::RedeclaresGroup { .. }
        | ShapeError::ResultOutsidePairwise { .. }
        | ShapeError::SpellsForm { .. }
        | ShapeError::Derived { .. }
        | ShapeError::Unquoted { .. }
        | ShapeError::DictDefault { .. }
        | ShapeError::ClosedBucket { .. }
        | ShapeError::NoKeyword { .. }
        | ShapeError::RankedDefinition { .. }
        | ShapeError::NestedBinder { .. }
        | ShapeError::RankingDisagrees { .. }
        | ShapeError::NoCandidate { .. }
        | ShapeError::Unlisted { .. }
        | ShapeError::OpenPathInType { .. }
        | ShapeError::UnpinnedMember { .. } => {}
        ShapeError::RepeatedGuard { guard, .. } => each(guard.into()),
        ShapeError::Type { error, .. } => match error {
            Elaboration::NoSuchMember { owner, .. } => each(owner.into()),
            Elaboration::TiedUnion {
                members: (first, second),
                ..
            } => {
                each(first.into());
                each(second.into());
            }
            Elaboration::NotAType { .. }
            | Elaboration::Unsupported { .. }
            | Elaboration::Bound { .. }
            | Elaboration::Quantified { .. }
            | Elaboration::MeetOverVariable { .. }
            | Elaboration::Unknown { .. } => {}
        },
    }
}
