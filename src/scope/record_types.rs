//! The **records type visitor**: every type a unit's record or a closing names, handed out once per
//! place it is named. What a walk interns in a chain that dies — a settlement's scratch chain, a
//! frame's region — is adopted where the records rest through it, so every handle a reader meets
//! resolves. A load handle costs the adopter a bit test, so the visitor does not tell them apart.
//!
//! Each record is destructured whole and each match lists every variant and field, so a
//! type-bearing field a record gains does not compile until it is visited.

use crate::type_lattice::{DeclaredType, Interval, KType, Parametric};
use crate::values::Retype;

use super::records::{AtRun, Born, Closing, Retypes, Typed, UnitFacts};
use super::shape::ShapeError;
use super::tree::{KnotNode, Leaf, NodeCapture, Tree, Unfolded, Within};
use super::typed::{
    Callable, Elaboration, InstanceRead, Kept, Narrowing, PathEntry, Point, Points, Registered,
    Resolution, Resolved, Static, StaticInstance, StaticType, UseStatics,
};

/// What the visitor hands each type to.
type Each<'e> = dyn FnMut(DeclaredType<Parametric>) + 'e;

impl UnitFacts<'_> {
    /// Hand `each` every type this record names.
    pub fn each_type(&self, each: &mut Each<'_>) {
        let UnitFacts {
            typed,
            born,
            paths,
            sites: _,
            named_variables: _,
        } = self;
        for (_, typed) in typed.iter() {
            typed_types(typed, each);
        }
        for (_, born) in born.iter() {
            born_types(born, each);
        }
        for (_, (entry, resolution)) in paths.iter() {
            let PathEntry {
                root: _,
                chain: _,
                label: _,
                level: _,
                through,
            } = entry;
            if let Some(through) = through {
                each((*through).into());
            }
            let Resolution {
                tree,
                resolved,
                lacks,
            } = resolution;
            tree_types(tree, each);
            if let Some(lacks) = lacks {
                each((*lacks).into());
            }
            match resolved {
                Resolved::Closed(ktype) => each((*ktype).into()),
                Resolved::Class(_) => {}
            }
        }
    }
}

impl Closing<'_> {
    /// Hand `each` every type this closing names.
    pub fn each_type(&self, each: &mut Each<'_>) {
        let Closing {
            class: _,
            summary,
            recursive: _,
            ran,
            refusal,
            overlap,
            retypes,
        } = self;
        let Retypes {
            parameters,
            returns,
        } = retypes;
        for (_, retype) in parameters.iter() {
            retype.each_named(each);
        }
        returns.each_named(each);
        if let Some(Some(summary)) = summary {
            tree_types(summary, each);
        }
        if let Some(ran) = ran {
            interval(*ran, each);
        }
        for error in refusal.iter().chain(overlap.iter()) {
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
        Static::Rigid {
            value,
            variables: _,
        } => each((*value).into()),
    }
}

fn registered<T: Copy + Into<Parametric>>(typed: &Registered<'_, T>, each: &mut Each<'_>) {
    let Registered {
        shape,
        quantifier_map: _,
        parameters: _,
    } = typed;
    each(declared(*shape));
}

fn callable<T: Copy + Into<Parametric>>(typed: &Callable<'_, T>, each: &mut Each<'_>) {
    let Callable {
        ktype,
        quantifier_map: _,
        registered: registered_as,
    } = typed;
    each(declared(*ktype));
    if let Some(registered_as) = registered_as {
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

fn instance_types(instance: &StaticInstance<'_>, each: &mut Each<'_>) {
    match instance {
        StaticInstance::Closed { solution, ktype } => {
            solution
                .iter()
                .for_each(|each_type| each((*each_type).into()));
            each((*ktype).into());
        }
        StaticInstance::Rigid {
            solution,
            ktype,
            variables: _,
        } => {
            solution.iter().for_each(|typed| each((*typed).into()));
            each((*ktype).into());
        }
    }
}

fn typed_types(typed: &Typed<'_>, each: &mut Each<'_>) {
    let Typed {
        declared,
        registered: registrations,
        expressions,
        parts,
        statements,
        binders,
        settled: _,
        instances,
        uses,
        selections,
        by_name,
        routes: _,
        retypes,
        trees,
        picks: _,
        contents,
        minting: _,
        signature,
        exits: _,
        closed: _,
        folded: _,
    } = typed;
    for (_, declared) in declared.iter() {
        static_type(declared, each);
    }
    for (_, entry) in registrations.iter() {
        match entry {
            Static::Unknown => {}
            Static::Closed(closed) => registered(closed, each),
            Static::Rigid {
                value,
                variables: _,
            } => registered(value, each),
        }
    }
    for (_, expression) in expressions.iter() {
        static_type(expression, each);
    }
    for (_, part) in parts.iter() {
        interval(*part, each);
    }
    for (_, statement) in statements.iter() {
        interval(*statement, each);
    }
    for (_, binder) in binders.iter() {
        match binder {
            DeclaredType::Type(typed) => interval(*typed, each),
            DeclaredType::Scheme(scheme) => each(DeclaredType::Scheme(*scheme)),
        }
    }
    for (_, read) in instances.iter() {
        match read {
            InstanceRead::Solved(solved) => instance_types(solved, each),
            InstanceRead::AsIs => {}
        }
    }
    for (_, statics) in uses.iter() {
        statics.each_named(each);
    }
    for (_, points) in selections.iter() {
        points_types(points, each);
    }
    for (_, contributions) in by_name.iter() {
        contributions
            .iter()
            .for_each(|typed| static_type(typed, each));
    }
    for (_, retype) in retypes.iter() {
        retype.each_named(each);
    }
    for (_, tree) in trees.iter() {
        tree_types(tree, each);
    }
    for (_, tree) in contents.iter() {
        tree_types(tree, each);
    }
    if let Some(signature) = signature {
        each((*signature).into());
    }
}

fn born_types(born: &Born<'_>, each: &mut Each<'_>) {
    let Born {
        callable: born_as,
        levels,
        instance,
        digestible: _,
    } = born;
    match born_as {
        Static::Unknown => {}
        Static::Closed(closed) => callable(closed, each),
        Static::Rigid {
            value,
            variables: _,
        } => callable(value, each),
    }
    levels.iter().for_each(|level| each((*level).into()));
    if let Some(instance) = instance {
        instance_types(instance, each);
    }
}

/// Every type `tree` names, its subtrees' included.
pub(super) fn tree_types(tree: &Tree<'_>, each: &mut Each<'_>) {
    match tree {
        Tree::Digest(_) | Tree::Leaf(Leaf::Unnamed(_)) => {}
        Tree::Body {
            body: _,
            captures,
            identity: _,
        } => captures.iter().for_each(|(_, at)| tree_types(at, each)),
        Tree::Knot {
            members,
            index: _,
            identity: _,
        } => {
            for member in *members {
                let KnotNode {
                    body: _,
                    instance,
                    captures,
                } = member;
                instance.iter().for_each(|ktype| each((*ktype).into()));
                for (_, capture) in *captures {
                    match capture {
                        NodeCapture::Fellow(_) => {}
                        NodeCapture::Tree(at) => tree_types(at, each),
                    }
                }
            }
        }
        Tree::View {
            operator: _,
            application,
            source,
            identity: _,
        } => {
            each((*application).into());
            tree_types(source, each);
        }
        Tree::Retyped {
            carried,
            source,
            identity: _,
        } => {
            each((*carried).into());
            tree_types(source, each);
        }
        Tree::Leaf(Leaf::Bound {
            binder: _,
            chain: _,
            within,
            identity: _,
        }) => match within {
            None => {}
            Some(Within::Body(captures)) => {
                captures.iter().for_each(|(_, at)| tree_types(at, each));
            }
            Some(Within::Call(Unfolded {
                callee: _,
                arguments,
                solution,
            })) => {
                arguments.iter().for_each(|(_, at)| tree_types(at, each));
                solution.iter().for_each(|(_, ktype)| each((*ktype).into()));
            }
        },
    }
}

fn error_types(error: &ShapeError<'_>, each: &mut Each<'_>) {
    let mut ktype = |ktype: KType| each(ktype.into());
    match *error {
        ShapeError::Unfixed {
            variables: _,
            wanted,
            at: _,
        } => wanted.into_iter().for_each(ktype),
        ShapeError::NoInstance {
            scheme,
            wanted,
            at: _,
        } => {
            ktype(wanted);
            each(DeclaredType::Scheme(scheme));
        }
        ShapeError::Inadmissible {
            form: _,
            index: _,
            slot,
            at: _,
        } => ktype(slot),
        ShapeError::Overlaps {
            key: _,
            builtin,
            at: _,
        } => ktype(builtin),
        ShapeError::NoAdmittingCandidate {
            key: _,
            arguments,
            at: _,
        }
        | ShapeError::Ambiguous {
            key: _,
            arguments,
            count: _,
            at: _,
        } => arguments.iter().for_each(|typed| each(*typed)),
        ShapeError::NoField {
            of,
            field: _,
            at: _,
        }
        | ShapeError::NoMember {
            of,
            member: _,
            at: _,
        } => ktype(of),
        ShapeError::ReturnNeverSatisfied {
            body,
            returns,
            at: _,
        }
        | ShapeError::EvalNeverSatisfied {
            code: body,
            returns,
            at: _,
        } => {
            ktype(body);
            ktype(returns);
        }
        ShapeError::AscriptionNeverSatisfied {
            value,
            ascribed: other,
            at: _,
        }
        | ShapeError::AnnotationNeverSatisfied {
            value,
            annotated: other,
            at: _,
        } => {
            ktype(value);
            ktype(other);
        }
        ShapeError::CallNeverSatisfied {
            callee,
            arguments,
            at: _,
        } => {
            each(declared(callee));
            each(arguments.into());
        }
        ShapeError::SpreadNeverAList { value, at: _ }
        | ShapeError::SpliceNeverCode { value, at: _ } => ktype(value),
        ShapeError::ReadNeverSatisfied {
            container,
            argument,
            at: _,
        } => {
            ktype(container);
            ktype(argument);
        }
        ShapeError::NotCode { value, at: _ }
        | ShapeError::BodyNotCode { value, at: _ }
        | ShapeError::NotAModule { value, at: _ }
        | ShapeError::NotASignature {
            ascribed: value,
            at: _,
        } => ktype(value),
        ShapeError::RepeatedGuard {
            guard,
            first: _,
            at: _,
        } => each(guard.into()),
        ShapeError::Type { error, at: _ } => elaboration_types(&error, each),
        ShapeError::Rebind {
            name: _,
            first: _,
            second: _,
        }
        | ShapeError::ShadowsBuiltin { name: _, at: _ }
        | ShapeError::Unbound {
            name: _,
            site: _,
            at: _,
        }
        | ShapeError::QuantifiedValue { at: _ }
        | ShapeError::AmbiguousInstance { key: _, at: _ }
        | ShapeError::NoInstanceAtCandidates { key: _, at: _ }
        | ShapeError::QuantifiedRead {
            name: _,
            site: _,
            at: _,
        }
        | ShapeError::EagerCycle {
            members: _,
            definitions: _,
            at: _,
        }
        | ShapeError::MarkOutsideQuote { at: _ }
        | ShapeError::Unsupported { form: _, at: _ }
        | ShapeError::Malformed { form: _, at: _ }
        | ShapeError::Unsurfaced { at: _, site: _ }
        | ShapeError::Unchained { symbol: _, at: _ }
        | ShapeError::MixedGroups {
            first: _,
            second: _,
            at: _,
        }
        | ShapeError::RedeclaresGroup { symbol: _, at: _ }
        | ShapeError::ResultOutsidePairwise { symbol: _, at: _ }
        | ShapeError::SpellsForm { symbol: _, at: _ }
        | ShapeError::Derived { symbol: _, at: _ }
        | ShapeError::Unquoted {
            form: _,
            part: _,
            at: _,
        }
        | ShapeError::DictDefault { site: _, at: _ }
        | ShapeError::ClosedBucket { key: _, at: _ }
        | ShapeError::NoKeyword { at: _ }
        | ShapeError::RankedDefinition { at: _ }
        | ShapeError::NestedBinder { at: _ }
        | ShapeError::RankingDisagrees { key: _, at: _ }
        | ShapeError::NoCandidate { key: _, at: _ }
        | ShapeError::Unlisted { name: _, at: _ }
        | ShapeError::ParameterOutsideBody { name: _, at: _ }
        | ShapeError::OpenPathInType { at: _ }
        | ShapeError::SpliceInKnot { at: _ }
        | ShapeError::UnpinnedMember {
            member: _,
            parameter: _,
            at: _,
        } => {}
    }
}

fn elaboration_types(error: &Elaboration, each: &mut Each<'_>) {
    match *error {
        Elaboration::NoSuchMember {
            owner,
            name: _,
            site: _,
        } => each(owner.into()),
        Elaboration::TiedUnion {
            members: (first, second),
            site: _,
        } => {
            each(first.into());
            each(second.into());
        }
        Elaboration::NotAType { name: _, site: _ }
        | Elaboration::Unsupported { site: _ }
        | Elaboration::Bound { site: _ }
        | Elaboration::Quantified { site: _ }
        | Elaboration::MeetOverVariable { site: _ }
        | Elaboration::Unknown { site: _ }
        | Elaboration::Unbound { name: _, site: _ } => {}
    }
}

/// A fact the run reads off its [`Facts`](super::Facts): every type it names, each of which the
/// reader makes resolve in the frame's chain.
pub trait Named {
    fn each_named(&self, each: &mut Each<'_>);
}

impl Named for KType {
    fn each_named(&self, each: &mut Each<'_>) {
        each((*self).into());
    }
}

impl Named for Retype {
    fn each_named(&self, each: &mut Each<'_>) {
        match self {
            Retype::Point(ktype) | Retype::Computed(ktype) | Retype::Fit(ktype) => {
                each((*ktype).into())
            }
            Retype::Keeps => {}
        }
    }
}

impl Named for Parametric {
    fn each_named(&self, each: &mut Each<'_>) {
        each((*self).into());
    }
}

impl<C: Named, R: Named> Named for Static<'_, C, R> {
    fn each_named(&self, each: &mut Each<'_>) {
        match self {
            Static::Unknown => {}
            Static::Closed(closed) => closed.each_named(each),
            Static::Rigid {
                value,
                variables: _,
            } => value.each_named(each),
        }
    }
}

impl Named for UseStatics<'_> {
    fn each_named(&self, each: &mut Each<'_>) {
        let UseStatics {
            narrowing,
            contributions,
        } = self;
        // A kept list's *always* candidates solve to closed points; the rest names candidates,
        // verdicts and a table of positions.
        match narrowing {
            Narrowing::Full | Narrowing::Selected(_) => {}
            Narrowing::Kept(Kept {
                entries: _,
                points,
                elimination: _,
            }) => points.iter().for_each(|points| points_types(points, each)),
        }
        contributions.each_named(each);
    }
}

impl Named for Retypes<'_> {
    fn each_named(&self, each: &mut Each<'_>) {
        let Retypes {
            parameters,
            returns,
        } = self;
        for (_, retype) in parameters.iter() {
            retype.each_named(each);
        }
        returns.each_named(each);
    }
}

impl Named for PathEntry<'_> {
    fn each_named(&self, each: &mut Each<'_>) {
        let PathEntry {
            root: _,
            chain: _,
            label: _,
            level: _,
            through,
        } = self;
        through.each_named(each);
    }
}

impl Named for Resolution<'_> {
    fn each_named(&self, each: &mut Each<'_>) {
        let Resolution {
            tree,
            resolved,
            lacks,
        } = self;
        tree_types(tree, each);
        lacks.each_named(each);
        match resolved {
            Resolved::Closed(ktype) => each((*ktype).into()),
            Resolved::Class(_) => {}
        }
    }
}

impl<T: Named> Named for DeclaredType<T> {
    fn each_named(&self, each: &mut Each<'_>) {
        match self {
            DeclaredType::Type(typed) => typed.each_named(each),
            DeclaredType::Scheme(scheme) => each(DeclaredType::Scheme(*scheme)),
        }
    }
}

impl<T: Named> Named for [T] {
    fn each_named(&self, each: &mut Each<'_>) {
        self.iter().for_each(|named| named.each_named(each));
    }
}

impl<A: Named, B: Named> Named for (A, B) {
    fn each_named(&self, each: &mut Each<'_>) {
        self.0.each_named(each);
        self.1.each_named(each);
    }
}

impl Named for Interval {
    fn each_named(&self, each: &mut Each<'_>) {
        interval(*self, each);
    }
}

impl<T: Copy + Into<Parametric>> Named for Callable<'_, T> {
    fn each_named(&self, each: &mut Each<'_>) {
        callable(self, each);
    }
}

impl<T: Copy + Into<Parametric>> Named for Registered<'_, T> {
    fn each_named(&self, each: &mut Each<'_>) {
        registered(self, each);
    }
}

impl Named for StaticInstance<'_> {
    fn each_named(&self, each: &mut Each<'_>) {
        instance_types(self, each);
    }
}

impl Named for InstanceRead<'_> {
    fn each_named(&self, each: &mut Each<'_>) {
        match self {
            InstanceRead::Solved(solved) => instance_types(solved, each),
            InstanceRead::AsIs => {}
        }
    }
}

impl Named for Points<'_> {
    fn each_named(&self, each: &mut Each<'_>) {
        points_types(self, each);
    }
}

impl Named for Tree<'_> {
    fn each_named(&self, each: &mut Each<'_>) {
        tree_types(self, each);
    }
}

impl Named for ShapeError<'_> {
    fn each_named(&self, each: &mut Each<'_>) {
        error_types(self, each);
    }
}

impl Named for Elaboration {
    fn each_named(&self, each: &mut Each<'_>) {
        elaboration_types(self, each);
    }
}

impl<T: Named + ?Sized> Named for &T {
    fn each_named(&self, each: &mut Each<'_>) {
        (**self).each_named(each);
    }
}

impl<T: Named> Named for Option<T> {
    fn each_named(&self, each: &mut Each<'_>) {
        if let Some(value) = self {
            value.each_named(each);
        }
    }
}

impl<T: Named, E: Named> Named for Result<T, E> {
    fn each_named(&self, each: &mut Each<'_>) {
        match self {
            Ok(value) => value.each_named(each),
            Err(error) => error.each_named(each),
        }
    }
}

impl<T: Named> Named for AtRun<T> {
    fn each_named(&self, each: &mut Each<'_>) {
        match self {
            AtRun::Closed(value) => value.each_named(each),
            AtRun::Refused(refused) => refused.each_named(each),
            AtRun::Open => {}
        }
    }
}

/// Hand `each` every type `points` name.
fn points_types(points: &Points<'_>, each: &mut dyn FnMut(DeclaredType<Parametric>)) {
    for point in points.points {
        match point {
            Point::Closed(point) => each((*point).into()),
            Point::Rigid(point) => each((*point).into()),
            Point::Carried(_) => {}
        }
    }
}
