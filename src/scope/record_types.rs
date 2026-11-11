//! The **records type visitor**: every type a unit's record or a closing names, handed out once per
//! place it is named. What a walk interns in a chain that dies — a settlement's scratch chain, a
//! frame's region — is adopted where the records rest through it, so every handle a reader meets
//! resolves. A load handle costs the adopter a bit test, so the visitor does not tell them apart.
//!
//! Each record is destructured whole and each match lists every variant and field, so a
//! type-bearing field a record gains does not compile until it is visited.

use crate::memory::{Lays, resident};
use crate::type_lattice::{DeclaredType, Interval, KType, Parametric};
use crate::values::Retype;

use super::keyed::Keyed;
use super::records::{AtRun, Born, Closing, Restamp, Retypes, Typed, UnitFacts};
use super::shape::{Repeated, ShapeError};
use super::tree::{KnotNode, Leaf, NodeCapture, Tree, Unfolded, ViewRecord, Within};
use super::typed::{
    Callable, Elaboration, InstanceRead, Kept, Narrowing, PathEntry, Point, Points, Registered,
    Resolution, Resolved, Static, StaticInstance, StaticType, UseStatics,
};

/// What the visitor hands each type to.
pub(crate) type Each<'e> = dyn FnMut(DeclaredType<Parametric>) + 'e;

impl UnitFacts<'_> {
    /// Hand `each` every type this record names.
    pub fn each_type(&self, each: &mut Each<'_>) {
        let UnitFacts {
            typed,
            born,
            paths,
            sites: _,
            named_variables: _,
            seeds,
        } = self;
        for (_, seed) in seeds.iter() {
            seed.each_type(each);
        }
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
                name: _,
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
        traced,
        by_name,
        routes: _,
        arms: _,
        retypes,
        trees,
        picks: _,
        contents,
        holding: _,
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
    for (_, traced) in traced.iter() {
        for (_, fixed) in traced.fixed.iter() {
            fixed.each_type(&mut |kt| each(kt.into()));
        }
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
        Tree::Digest(_, None) | Tree::Leaf(Leaf::Unnamed(_)) => {}
        Tree::Digest(_, Some(bare)) => {
            bare.signature
                .iter()
                .for_each(|ktype| each((*ktype).into()));
            if let Some(sealing) = bare.sealing {
                let ViewRecord {
                    application,
                    from,
                    to,
                    hidden,
                } = sealing.record;
                for ktype in [application, from, to, sealing.underlying_signature] {
                    each(ktype.into());
                }
                for (carrier, hides) in hidden {
                    each((*carrier).into());
                    each((*hides).into());
                }
            }
        }
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
            application,
            source,
            underlying,
            identity: _,
        } => {
            each((*application).into());
            tree_types(source, each);
            tree_types(underlying, each);
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
            arguments: _,
            lines: _,
            shown,
            against,
            at: _,
        } => {
            shown.iter().for_each(|shown| each(shown.named()));
            against.iter().for_each(|against| each(*against));
        }
        ShapeError::Ambiguous {
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
        } => {
            each(body.named());
            each(returns.into());
        }
        ShapeError::EvalNeverSatisfied {
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
            each(value.named());
            each(other.into());
        }
        ShapeError::CallNeverSatisfied {
            callee,
            arguments,
            parameters,
            at: _,
        } => {
            each(declared(callee));
            each(arguments.named());
            parameters
                .into_iter()
                .for_each(|parameters| each(parameters.into()));
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
        | ShapeError::NotAModule { value, at: _ } => each(value.named()),
        ShapeError::NotASignature {
            ascribed: value,
            at: _,
        }
        | ShapeError::ViewReveals {
            member: _,
            ascribed: value,
            at: _,
        } => ktype(value),
        ShapeError::NoAdmittingArm {
            scrutinee,
            union,
            at: _,
        } => {
            ktype(scrutinee);
            union.into_iter().for_each(ktype);
        }
        ShapeError::AmbiguousArms {
            count: _,
            scrutinee,
            at: _,
        } => ktype(scrutinee),
        ShapeError::NotUnion { clause, at: _ } => ktype(clause),
        ShapeError::Uncovered {
            union,
            variant,
            at: _,
        } => {
            ktype(union);
            ktype(variant);
        }
        ShapeError::RepeatedGuard {
            guard,
            first: _,
            at: _,
        } => {
            if let Repeated::Type(guard) = guard {
                each(guard.into())
            }
        }
        ShapeError::Type { error, at: _ } => elaboration_types(&error, each),
        ShapeError::UnderCall { call: _, refusal } => error_types(refusal, each),
        ShapeError::Rebind {
            name: _,
            first: _,
            second: _,
        }
        | ShapeError::ShadowsBuiltin { name: _, at: _ }
        | ShapeError::NotAGuard { at: _ }
        | ShapeError::NoVariant {
            label: _,
            union: _,
            at: _,
        }
        | ShapeError::LoneVariant { name: _, at: _ }
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
        | Elaboration::CycleWithoutNominal { name: _, site: _ }
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

impl Named for Restamp {
    fn each_named(&self, each: &mut Each<'_>) {
        each(self.at.into());
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
            name: _,
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

/// What a [`Closer`] makes of one type: the closed type a binding gives it, the type with only
/// some of its variables closed, or the type unchanged.
#[derive(Clone, Copy, Debug)]
pub enum Closed {
    Type(DeclaredType<KType>),
    Partial(DeclaredType<Parametric>),
    Same,
}

/// Closes the types a shared record's fact names under one binding: the walk's by substitution as
/// it absorbs, the run's by a probe of its chain's tables of closed types
/// ([`records`](super::records)).
pub trait Closer {
    fn close(&mut self, ty: DeclaredType<Parametric>) -> Closed;
}

/// A [`Closer`] that lays the slices it copies through [`lay`](Self::lay).
pub trait Laying<'l>: Closer {
    type Lay: Lays<'l>;

    fn lay(&self) -> Self::Lay;
}

/// A fact whose types a [`Closer`] maps without laying, beside the visitor's [`Named`]: the lists
/// are kept in step, so a type-bearing field the one reaches the other closes.
pub trait Close: Sized {
    fn closed(self, closer: &mut impl Closer) -> Self;
}

/// A fact holding a slice of types, which closing copies where some element changes.
pub trait CloseLaying<'l>: Sized {
    fn closed_laying(self, closer: &mut impl Laying<'l>) -> Self;
}

/// A fact whose rigid arm closes into its closed arm where every type it names closes: what a
/// [`Static`]'l rigid value and closed value are.
pub trait CloseInto: Sized {
    type Into;

    /// The closed value where every type closes, else the value with what closes closed.
    fn close_into(self, closer: &mut impl Closer) -> Result<Self::Into, Self>;
}

/// `ty` under `closer`: the closed type, or the type with what closes closed.
fn close_parametric(closer: &mut impl Closer, ty: Parametric) -> Result<KType, Parametric> {
    match closer.close(DeclaredType::Type(ty)) {
        Closed::Type(DeclaredType::Type(closed)) => Ok(closed),
        Closed::Partial(DeclaredType::Type(partial)) => Err(partial),
        Closed::Same => Err(ty),
        Closed::Type(DeclaredType::Scheme(_)) | Closed::Partial(DeclaredType::Scheme(_)) => {
            unreachable!("a type closes to a type")
        }
    }
}

/// `ty` under `closer`, read as a parametric type either way.
fn close_declared(
    closer: &mut impl Closer,
    ty: DeclaredType<Parametric>,
) -> DeclaredType<Parametric> {
    match closer.close(ty) {
        Closed::Type(closed) => declared(closed),
        Closed::Partial(partial) => partial,
        Closed::Same => ty,
    }
}

/// `ty` closed into a declared type over concrete handles where it closes whole.
fn declared_into(
    closer: &mut impl Closer,
    ty: DeclaredType<Parametric>,
) -> Result<DeclaredType<KType>, DeclaredType<Parametric>> {
    match closer.close(ty) {
        Closed::Type(closed) => Ok(closed),
        Closed::Partial(partial) => Err(partial),
        Closed::Same => Err(ty),
    }
}

/// `items` with each closed by `close`, laid again through the closer only where one changes: the
/// items before the first that changes are copied, and each is closed once.
fn close_slice<'l, T: Copy, C: Laying<'l>>(
    closer: &mut C,
    items: &'l [T],
    mut close: impl FnMut(T, &mut Counting<'_, C>) -> T,
) -> &'l [T] {
    let mut counting = Counting {
        closer,
        changed: false,
    };
    let mut first = None;
    for (at, item) in items.iter().enumerate() {
        let closed = close(*item, &mut counting);
        if counting.changed {
            first = Some((at, closed));
            break;
        }
    }
    let Some((first, closed)) = first else {
        return items;
    };
    let lay = counting.lay();
    let mut closed = Some(closed);
    lay.fill(items.len(), |at| match at.cmp(&first) {
        std::cmp::Ordering::Less => items[at],
        std::cmp::Ordering::Equal => closed.take().expect("the first change is laid once"),
        std::cmp::Ordering::Greater => close(items[at], &mut counting),
    })
}

/// `table` with each value closed by `close`, laid again only where one changes: the keys stay,
/// so it stays sorted.
fn close_keyed<'l, K: Ord + Copy, V: Copy, C: Laying<'l>>(
    closer: &mut C,
    table: Keyed<'l, (K, V)>,
    mut close: impl FnMut(V, &mut Counting<'_, C>) -> V,
) -> Keyed<'l, (K, V)> {
    let entries = close_slice(closer, table.entries(), |(key, value), closer| {
        (key, close(value, closer))
    });
    match std::ptr::eq(entries, table.entries()) {
        true => table,
        false => Keyed::from_sorted(entries),
    }
}

/// A closer noting whether it changed any type.
struct Counting<'c, C> {
    closer: &'c mut C,
    changed: bool,
}

impl<C: Closer> Closer for Counting<'_, C> {
    fn close(&mut self, ty: DeclaredType<Parametric>) -> Closed {
        let closed = self.closer.close(ty);
        self.changed |= !matches!(closed, Closed::Same);
        closed
    }
}

impl<'l, C: Laying<'l>> Laying<'l> for Counting<'_, C> {
    type Lay = C::Lay;

    fn lay(&self) -> C::Lay {
        self.closer.lay()
    }
}

impl Close for Parametric {
    fn closed(self, closer: &mut impl Closer) -> Self {
        close_parametric(closer, self).map_or_else(|partial| partial, Into::into)
    }
}

impl Close for Restamp {
    fn closed(self, closer: &mut impl Closer) -> Self {
        Restamp {
            at: self.at.closed(closer),
            ..self
        }
    }
}

impl<'l> CloseLaying<'l> for Retypes<'l> {
    fn closed_laying(self, closer: &mut impl Laying<'l>) -> Self {
        Retypes {
            parameters: close_slice(closer, self.parameters, |(name, restamp), closer| {
                (name, restamp.closed(closer))
            }),
            returns: self.returns.closed(closer),
        }
    }
}

impl Close for Interval {
    fn closed(self, closer: &mut impl Closer) -> Self {
        Interval {
            lower: self.lower.closed(closer),
            upper: self.upper.closed(closer),
        }
    }
}

impl Close for DeclaredType<Parametric> {
    fn closed(self, closer: &mut impl Closer) -> Self {
        close_declared(closer, self)
    }
}

impl Close for DeclaredType<Interval> {
    fn closed(self, closer: &mut impl Closer) -> Self {
        match self {
            DeclaredType::Type(typed) => DeclaredType::Type(typed.closed(closer)),
            DeclaredType::Scheme(scheme) => {
                match close_declared(closer, DeclaredType::Scheme(scheme)) {
                    DeclaredType::Scheme(scheme) => DeclaredType::Scheme(scheme),
                    DeclaredType::Type(_) => unreachable!("a scheme closes to a scheme"),
                }
            }
        }
    }
}

impl CloseInto for Parametric {
    type Into = KType;

    fn close_into(self, closer: &mut impl Closer) -> Result<KType, Self> {
        close_parametric(closer, self)
    }
}

impl<'l> CloseInto for Registered<'l, Parametric> {
    type Into = Registered<'l, KType>;

    fn close_into(self, closer: &mut impl Closer) -> Result<Self::Into, Self> {
        let Registered {
            shape,
            quantifier_map,
            parameters,
        } = self;
        match declared_into(closer, shape) {
            Ok(shape) => Ok(Registered {
                shape,
                quantifier_map,
                parameters,
            }),
            Err(shape) => Err(Registered {
                shape,
                quantifier_map,
                parameters,
            }),
        }
    }
}

impl<'l> CloseInto for Callable<'l, Parametric> {
    type Into = Callable<'l, KType>;

    fn close_into(self, closer: &mut impl Closer) -> Result<Self::Into, Self> {
        let Callable {
            ktype,
            quantifier_map,
            registered,
        } = self;
        let ktype = declared_into(closer, ktype);
        let registered = registered.map(|registered| registered.close_into(closer));
        match (ktype, registered) {
            (Ok(ktype), None) => Ok(Callable {
                ktype,
                quantifier_map,
                registered: None,
            }),
            (Ok(ktype), Some(Ok(registered))) => Ok(Callable {
                ktype,
                quantifier_map,
                registered: Some(registered),
            }),
            (ktype, registered) => Err(Callable {
                ktype: ktype.map_or_else(|partial| partial, declared),
                quantifier_map,
                registered: registered.map(|registered| match registered {
                    Ok(Registered {
                        shape,
                        quantifier_map,
                        parameters,
                    }) => Registered {
                        shape: declared(shape),
                        quantifier_map,
                        parameters,
                    },
                    Err(partial) => partial,
                }),
            }),
        }
    }
}

/// A rigid value closes into its closed arm where every type it names closes; its variables stay
/// as written where only some do, since the run refuses a rigid fact whatever it lists. A closed
/// value is closed already.
impl<C, R: CloseInto<Into = C>> Close for Static<'_, C, R> {
    fn closed(self, closer: &mut impl Closer) -> Self {
        match self {
            Static::Rigid { value, variables } => match value.close_into(closer) {
                Ok(closed) => Static::Closed(closed),
                Err(value) => Static::Rigid { value, variables },
            },
            Static::Unknown | Static::Closed(_) => self,
        }
    }
}

impl Close for Point {
    fn closed(self, closer: &mut impl Closer) -> Self {
        match self {
            Point::Rigid(point) => match close_parametric(closer, point) {
                Ok(closed) => Point::Closed(closed),
                Err(partial) => Point::Rigid(partial),
            },
            Point::Closed(_) | Point::Carried(_) => self,
        }
    }
}

impl Close for PathEntry<'_> {
    fn closed(self, closer: &mut impl Closer) -> Self {
        PathEntry {
            through: self.through.map(|through| through.closed(closer)),
            ..self
        }
    }
}

impl<T: Close> Close for Option<T> {
    fn closed(self, closer: &mut impl Closer) -> Self {
        self.map(|value| value.closed(closer))
    }
}

impl<A: Close, B: Close> Close for (A, B) {
    fn closed(self, closer: &mut impl Closer) -> Self {
        (self.0.closed(closer), self.1.closed(closer))
    }
}

/// What names only closed types: a resolution, a content tree, a retype table. A refusal that
/// names a variable is marked open where the walk keeps it, so it is never read closed.
macro_rules! closes_as_is {
    ($($fact:ty),* $(,)?) => {
        $(impl Close for $fact {
            fn closed(self, _: &mut impl Closer) -> Self {
                self
            }
        })*
    };
}

closes_as_is!(
    KType,
    Retype,
    Resolution<'_>,
    Tree<'_>,
    &Tree<'_>,
    &ShapeError<'_>,
    Elaboration,
);

impl<'l> CloseLaying<'l> for Points<'l> {
    fn closed_laying(self, closer: &mut impl Laying<'l>) -> Self {
        Points {
            points: close_slice(closer, self.points, |point, closer| point.closed(closer)),
            variables: self.variables,
        }
    }
}

impl<'l> CloseLaying<'l> for StaticInstance<'l> {
    fn closed_laying(self, closer: &mut impl Laying<'l>) -> Self {
        let StaticInstance::Rigid {
            solution,
            ktype,
            variables,
        } = self
        else {
            return self;
        };
        let closes = (solution.iter()).all(|each| close_parametric(closer, *each).is_ok());
        match close_parametric(closer, ktype) {
            Ok(ktype) if closes => StaticInstance::Closed {
                solution: closer.lay().fill(solution.len(), |at| {
                    close_parametric(closer, solution[at]).expect("every point closes")
                }),
                ktype,
            },
            ktype => StaticInstance::Rigid {
                solution: close_slice(closer, solution, |each, closer| each.closed(closer)),
                ktype: ktype.map_or_else(|partial| partial, Into::into),
                variables,
            },
        }
    }
}

impl<'l> CloseLaying<'l> for InstanceRead<'l> {
    fn closed_laying(self, closer: &mut impl Laying<'l>) -> Self {
        match self {
            InstanceRead::Solved(solved) => InstanceRead::Solved(solved.closed_laying(closer)),
            InstanceRead::AsIs => InstanceRead::AsIs,
        }
    }
}

impl<'l> CloseLaying<'l> for UseStatics<'l> {
    fn closed_laying(self, closer: &mut impl Laying<'l>) -> Self {
        let UseStatics {
            narrowing,
            contributions,
        } = self;
        let narrowing = match narrowing {
            Narrowing::Full | Narrowing::Selected(_) => narrowing,
            Narrowing::Kept(kept) => {
                let points = close_slice(closer, kept.points, |points, closer| {
                    points.closed_laying(closer)
                });
                match std::ptr::addr_eq(points, kept.points) {
                    true => narrowing,
                    false => Narrowing::Kept(crate::memory::resident(
                        closer.lay(),
                        Kept { points, ..*kept },
                    )),
                }
            }
        };
        UseStatics {
            narrowing,
            contributions: close_slice(closer, contributions, |each, closer| each.closed(closer)),
        }
    }
}

/// A born callable's own group levels are how its body names its group: they stay as laid.
impl<'l> CloseLaying<'l> for Born<'l> {
    fn closed_laying(self, closer: &mut impl Laying<'l>) -> Self {
        let Born {
            callable,
            levels,
            instance,
            digestible,
        } = self;
        Born {
            callable: callable.closed(closer),
            levels,
            instance: instance.map(|instance| instance.closed_laying(closer)),
            digestible,
        }
    }
}

impl<'l> CloseLaying<'l> for &'l [StaticType<'l>] {
    fn closed_laying(self, closer: &mut impl Laying<'l>) -> Self {
        close_slice(closer, self, |each, closer| each.closed(closer))
    }
}

impl<'l, T: CloseLaying<'l>> CloseLaying<'l> for Option<T> {
    fn closed_laying(self, closer: &mut impl Laying<'l>) -> Self {
        self.map(|value| value.closed_laying(closer))
    }
}

/// A body's facts closed table by table, each laid again only where one of its values changes. A
/// route, an arm set's verdicts and a pick name no type: a verdict a binding could change was
/// marked open where it was judged, so its record re-runs instead.
impl<'l> CloseLaying<'l> for Typed<'l> {
    fn closed_laying(self, closer: &mut impl Laying<'l>) -> Self {
        let Typed {
            declared,
            registered,
            expressions,
            parts,
            statements,
            binders,
            settled,
            instances,
            uses,
            selections,
            traced,
            by_name,
            routes,
            arms,
            retypes,
            trees,
            picks,
            contents,
            holding,
            minting,
            signature,
            exits,
            closed,
            folded,
        } = self;
        Typed {
            declared: close_keyed(closer, declared, |typed, closer| typed.closed(closer)),
            registered: close_keyed(closer, registered, |typed, closer| typed.closed(closer)),
            expressions: close_keyed(closer, expressions, |typed, closer| typed.closed(closer)),
            parts: close_keyed(closer, parts, |typed, closer| typed.closed(closer)),
            statements: close_keyed(closer, statements, |typed, closer| typed.closed(closer)),
            binders: close_keyed(closer, binders, |typed, closer| typed.closed(closer)),
            settled,
            instances: close_keyed(closer, instances, |read, closer| read.closed_laying(closer)),
            uses: close_keyed(closer, uses, |statics, closer| {
                statics.closed_laying(closer)
            }),
            selections: close_keyed(closer, selections, |points, closer| {
                points.closed_laying(closer)
            }),
            // A traced call's content is closed: the trace keys it.
            traced,
            by_name: close_keyed(closer, by_name, |typed, closer| typed.closed_laying(closer)),
            routes,
            arms,
            retypes,
            trees,
            picks,
            contents,
            holding,
            minting,
            signature,
            exits,
            closed,
            folded,
        }
    }
}

/// A unit's record closed, laid again only where some fact changes.
impl<'l> CloseLaying<'l> for &'l UnitFacts<'l> {
    fn closed_laying(self, closer: &mut impl Laying<'l>) -> Self {
        let UnitFacts {
            typed,
            born,
            paths,
            sites,
            named_variables,
            seeds,
        } = *self;
        let closed = UnitFacts {
            typed: close_keyed(closer, typed, |typed, closer| typed.closed_laying(closer)),
            born: close_keyed(closer, born, |born, closer| born.closed_laying(closer)),
            paths: close_keyed(closer, paths, |path, closer| path.closed(closer)),
            sites,
            named_variables,
            seeds,
        };
        let same = std::ptr::eq(closed.typed.entries(), typed.entries())
            && std::ptr::eq(closed.born.entries(), born.entries())
            && std::ptr::eq(closed.paths.entries(), paths.entries());
        match same {
            true => self,
            false => resident(closer.lay(), closed),
        }
    }
}

/// A closing record closed: only a quote's code's unfilled type names a type a binding closes;
/// a summary is a content tree, and a refusal naming a variable is marked open where it is kept.
impl<'l> CloseLaying<'l> for &'l Closing<'l> {
    fn closed_laying(self, closer: &mut impl Laying<'l>) -> Self {
        let Some(ran) = self.ran else {
            return self;
        };
        let mut counting = Counting {
            closer,
            changed: false,
        };
        let closed = ran.closed(&mut counting);
        match counting.changed {
            false => self,
            true => resident(
                counting.lay(),
                Closing {
                    ran: Some(closed),
                    ..*self
                },
            ),
        }
    }
}
