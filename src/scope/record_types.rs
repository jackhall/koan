//! The **records type visitor**: every type a unit's record or a closing names, handed out once per
//! place it is named. What a walk interns in a chain that dies — a composition's walk's chain, a
//! frame's region — is adopted where the records rest through it, so every handle a reader meets
//! resolves. A load handle costs the adopter a bit test, so the visitor does not tell them apart.
//!
//! Each record is destructured whole and each match lists every variant and field, so a
//! type-bearing field a record gains does not compile until it is visited.

use crate::memory::{BumpAllocator, BumpVec, Lays, resident};
use crate::type_lattice::{DeclaredType, Interval, KType, Parametric, PathClass, Scheme};
use crate::values::{Chain, Retype};

use super::keyed::Keyed;
use super::records::{AtRun, Born, Closing, Restamp, Retypes, Typed, UnitFacts};
use super::shape::{NeverSatisfied, Repeated, ShapeError};
use super::site::{Passing, Selection};
use super::tree::{Identity, KnotNode, Leaf, NodeCapture, Tree, Unfolded, ViewRecord, Within};
use super::typed::{
    Callable, Elaboration, Holding, InstanceRead, Kept, MemberWanted, Narrowing, PathEntry, Point,
    Points, Rebounds, Registered, Resolution, Resolved, Static, StaticInstance, StaticType,
    UseStatics, rebuilt_scheme,
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
            named_variables: _,
            seeds,
            // A posed question is what a door asked, not a fact the record states: its types are
            // read where a binding answers it, never closed into the record.
            posed: _,
            // A site question's asks and payloads are written over names and applied where a
            // binding decides it, as a posed question is.
            questions: _,
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
                Resolved::Over(over) => each((*over).into()),
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
            // Written over names and applied where a binding decides them, as a unit's are.
            questions: _,
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
        rebounds,
    } = typed;
    each(declared(*shape));
    bounds(*rebounds, each);
}

fn bounds(rebounds: Rebounds<'_>, each: &mut Each<'_>) {
    for (_, bound) in rebounds.0 {
        each((*bound).into());
    }
}

fn callable<T: Copy + Into<Parametric>>(typed: &Callable<'_, T>, each: &mut Each<'_>) {
    let Callable {
        ktype,
        quantifier_map: _,
        registered: registered_as,
        rebounds,
    } = typed;
    each(declared(*ktype));
    bounds(*rebounds, each);
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
        minting: _,
        held: _,
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
            Holding::Type(typed) => interval(*typed, each),
            Holding::Scheme(scheme, rebounds) => {
                each(DeclaredType::Scheme(*scheme));
                bounds(*rebounds, each);
            }
        }
    }
    for (_, read) in instances.iter() {
        read.each_named(each);
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
        for (_, passed) in traced.passes.iter() {
            match passed {
                Passing::Fixed(fixed) => fixed.each_type(&mut |kt| each(kt.into())),
                Passing::Type(ty) => each((*ty).into()),
                Passing::Carried(typed) => {
                    each(typed.lower.into());
                    each(typed.upper.into());
                }
                Passing::Point(_) => {}
            }
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

/// Every type a use an answer selects among names: each of its selection's asks'.
pub(super) fn undecided_types(selection: &Selection<'_>, each: &mut Each<'_>) {
    for ask in selection.asks {
        ask.each_type(each);
    }
}

/// Every type `tree` names, its subtrees' included.
pub(super) fn tree_types(tree: &Tree<'_>, each: &mut Each<'_>) {
    match tree {
        Tree::TypeValue { named, .. } => each((*named).into()),
        Tree::Leaf(Leaf::Call { call, .. }) => {
            undecided_types(&call.selection, each);
            call.arguments.iter().for_each(|at| tree_types(at, each));
        }
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
        } => (wanted.into_iter()).for_each(|wanted| each(wanted.into())),
        ShapeError::NoInstance {
            scheme,
            rebounds,
            wanted,
            at: _,
        } => {
            each(wanted.into());
            each(DeclaredType::Scheme(scheme));
            bounds(rebounds, each);
        }
        ShapeError::MemberUninstanced {
            name: _,
            carried,
            wanted,
            at: _,
        } => {
            each(carried.into());
            (wanted.into_iter()).for_each(|wanted| each(wanted.into()));
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
            listed,
            members,
            shapes,
            at: _,
        } => {
            for argument in arguments {
                each(argument.lower.into());
                each(argument.upper.into());
            }
            (listed.iter().chain(shapes)).for_each(|ty| each(*ty));
            (members.iter()).for_each(|(_, _, carried)| each((*carried).into()));
        }
        ShapeError::Ambiguous {
            key: _,
            arguments,
            members,
            count: _,
            at: _,
        } => {
            arguments.iter().for_each(|typed| each(*typed));
            (members.iter()).for_each(|(_, _, carried)| each((*carried).into()));
        }
        ShapeError::NoField {
            of,
            written: _,
            field: _,
            at: _,
        }
        | ShapeError::NoMember {
            of,
            member: _,
            at: _,
        } => each(of.into()),
        ShapeError::ReturnNeverSatisfied { refused, at: _ }
        | ShapeError::EvalNeverSatisfied { refused, at: _ }
        | ShapeError::AscriptionNeverSatisfied { refused, at: _ }
        | ShapeError::AnnotationNeverSatisfied { refused, at: _ }
        | ShapeError::NotCode { refused, at: _ }
        | ShapeError::BodyNotCode { refused, at: _ }
        | ShapeError::NotAModule { refused, at: _ } => refused.each_type(each),
        ShapeError::CallNeverSatisfied {
            callee,
            rebounds,
            payload,
            parameters,
            at: _,
        } => {
            each(callee);
            bounds(rebounds, each);
            each(payload.lower.into());
            each(payload.upper.into());
            each(parameters.into());
        }
        ShapeError::SpreadNeverAList { value, at: _ }
        | ShapeError::SpliceNeverCode { value, at: _ } => each(value.into()),
        ShapeError::ReadNeverSatisfied {
            container,
            argument,
            member,
            at: _,
        } => {
            each(container.into());
            each(argument.into());
            (member.iter()).for_each(|(_, carried)| each((*carried).into()));
        }
        ShapeError::NotASignature { ascribed, at: _ } => each(ascribed.into()),
        ShapeError::ViewReveals {
            member: _,
            ascribed: value,
            at: _,
        } => ktype(value),
        ShapeError::NoAdmittingArm {
            scrutinee,
            union,
            at: _,
        } => {
            each(scrutinee.into());
            (union.into_iter()).for_each(|union| each(union.into()));
        }
        ShapeError::AmbiguousArms {
            count: _,
            scrutinee,
            at: _,
        } => each(scrutinee.into()),
        ShapeError::NotUnion {
            clause,
            written: _,
            at: _,
        } => each(clause.into()),
        ShapeError::Uncovered {
            union,
            variant,
            at: _,
        } => {
            each(union.into());
            each(variant.into());
        }
        ShapeError::NoVariant {
            label: _,
            union,
            at: _,
        } => each(union.into()),
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
        | ShapeError::Grows { name: _, at: _ }
        | ShapeError::NotAGuard { at: _ }
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
        Elaboration::Unlicensed {
            name,
            written: _,
            spelling: _,
            site: _,
        } => each(name.into()),
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
        | Elaboration::TooWide {
            width: _,
            count: _,
            site: _,
        }
        | Elaboration::CycleWithoutNominal { name: _, site: _ }
        | Elaboration::Unknown {
            site: _,
            unwalked: _,
        }
        | Elaboration::BoundOverRunBound { site: _ }
        | Elaboration::OpenAtRun { site: _ }
        | Elaboration::Unresolved { site: _, path: _ }
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
            undecided,
        } = self;
        if let Some(undecided) = undecided {
            undecided_types(undecided, each);
        }
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
            Resolved::Over(over) => each((*over).into()),
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

impl Named for Holding<'_> {
    fn each_named(&self, each: &mut Each<'_>) {
        match self {
            Holding::Type(typed) => typed.each_named(each),
            Holding::Scheme(scheme, rebounds) => {
                each(DeclaredType::Scheme(*scheme));
                bounds(*rebounds, each);
            }
        }
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
            InstanceRead::Member {
                name: _,
                carried,
                wanted,
            } => {
                each((*carried).into());
                (wanted.at().iter()).for_each(|wanted| each((*wanted).into()));
            }
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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Closed {
    Type(DeclaredType<KType>),
    Partial(DeclaredType<Parametric>),
    Same,
}

/// Closes the types a shared record's fact names under one binding: a frame's, by a probe of its
/// chain's tables of closed types ([`records`](super::records)), or a refusal's as it is worded.
pub trait Closer {
    fn close(&mut self, ty: DeclaredType<Parametric>) -> Closed;

    /// `ty` as a closed type, where it is one: what a type [`close`](Self::close) leaves as it is
    /// is closed already when it is concrete.
    fn concrete(&self, ty: Parametric) -> Option<KType>;

    /// `scheme` with each of `rebounds`, closed, written into its group, closed
    /// ([`rebuilt_closed`]): `None` where a bound or the scheme's body does not close.
    fn rebuilt(&mut self, scheme: Scheme, rebounds: Rebounds<'_>) -> Option<DeclaredType<KType>>;

    /// What the binding makes the variable of the path class `class` ([`Binds::class`](super::Binds::class)): module
    /// substitution, which only the walk does.
    fn class(&mut self, class: PathClass) -> Option<Resolved> {
        let _ = class;
        None
    }
}

/// A [`Closer`] that lays the slices it copies through [`lay`](Self::lay).
pub trait Laying<'l>: Closer {
    type Lay: Lays<'l>;

    fn lay(&self) -> Self::Lay;

    /// The tree the binding gives the open tree of `identity`, where it changes it: tree
    /// substitution, which only the walk does.
    fn tree(&mut self, identity: Identity) -> Option<&'l Tree<'l>> {
        let _ = identity;
        None
    }
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
        Closed::Same => closer.concrete(ty).ok_or(ty),
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
/// `scheme` with each of `rebounds`, closed by `close`, written into its group
/// ([`rebuilt_scheme`]), closed: `None` where a bound or the scheme's body does not close. Every
/// [`Closer`]'s [`rebuilt`](Closer::rebuilt).
pub fn rebuilt_closed(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    scheme: Scheme,
    rebounds: Rebounds<'_>,
    close: &mut dyn FnMut(DeclaredType<Parametric>) -> Closed,
) -> Option<DeclaredType<KType>> {
    let mut bounds = BumpVec::with_capacity_in(rebounds.0.len(), scratch);
    for (at, bound) in rebounds.0 {
        let closed = match close((*bound).into()) {
            Closed::Type(DeclaredType::Type(closed)) => closed,
            Closed::Same => types.concrete(*bound)?,
            _ => return None,
        };
        bounds.push((usize::from(*at), closed));
    }
    types.concrete_declared(rebuilt_scheme(types, scratch, scheme.into(), &bounds)?)
}

/// `ty` closed beside the bounds `rebounds` its group writes over names: where there are none, as
/// [`declared_into`]; else the scheme closed as far as `closer` closes it, rebuilt once every bound
/// and its body close. A shape beside open bounds stays open, its bounds as written.
fn rebound_into(
    closer: &mut impl Closer,
    ty: DeclaredType<Parametric>,
    rebounds: Rebounds<'_>,
) -> Result<DeclaredType<KType>, DeclaredType<Parametric>> {
    if rebounds.is_empty() {
        return declared_into(closer, ty);
    }
    match close_declared(closer, ty) {
        DeclaredType::Scheme(scheme) => {
            (closer.rebuilt(scheme, rebounds)).ok_or(DeclaredType::Scheme(scheme))
        }
        partial => Err(partial),
    }
}

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

    fn concrete(&self, ty: Parametric) -> Option<KType> {
        self.closer.concrete(ty)
    }

    fn rebuilt(&mut self, scheme: Scheme, rebounds: Rebounds<'_>) -> Option<DeclaredType<KType>> {
        let rebuilt = self.closer.rebuilt(scheme, rebounds);
        self.changed |= rebuilt.is_some();
        rebuilt
    }

    fn class(&mut self, class: PathClass) -> Option<Resolved> {
        let closed = self.closer.class(class);
        self.changed |= closed.is_some();
        closed
    }
}

impl<'l, C: Laying<'l>> Laying<'l> for Counting<'_, C> {
    type Lay = C::Lay;

    fn lay(&self) -> C::Lay {
        self.closer.lay()
    }

    fn tree(&mut self, identity: Identity) -> Option<&'l Tree<'l>> {
        let tree = self.closer.tree(identity);
        self.changed |= tree.is_some();
        tree
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

impl Close for Holding<'_> {
    fn closed(self, closer: &mut impl Closer) -> Self {
        match self {
            Holding::Type(typed) => Holding::Type(typed.closed(closer)),
            Holding::Scheme(scheme, rebounds) => {
                let scheme = match close_declared(closer, DeclaredType::Scheme(scheme)) {
                    DeclaredType::Scheme(scheme) => scheme,
                    DeclaredType::Type(_) => unreachable!("a scheme closes to a scheme"),
                };
                // A scheme beside bounds over names is rebuilt once they all close.
                let rebuilt = match rebounds.is_empty() {
                    true => None,
                    false => closer.rebuilt(scheme, rebounds),
                };
                match rebuilt {
                    Some(DeclaredType::Scheme(rebuilt)) => Holding::scheme(rebuilt),
                    _ => Holding::Scheme(scheme, rebounds),
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
            rebounds,
        } = self;
        match rebound_into(closer, shape, rebounds) {
            Ok(shape) => Ok(Registered {
                shape,
                quantifier_map,
                parameters,
                rebounds: Rebounds::NONE,
            }),
            Err(shape) => Err(Registered {
                shape,
                quantifier_map,
                parameters,
                rebounds,
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
            rebounds,
        } = self;
        let ktype = rebound_into(closer, ktype, rebounds);
        let registered = registered.map(|registered| registered.close_into(closer));
        match (ktype, registered) {
            (Ok(ktype), None) => Ok(Callable {
                ktype,
                quantifier_map,
                registered: None,
                rebounds: Rebounds::NONE,
            }),
            (Ok(ktype), Some(Ok(registered))) => Ok(Callable {
                ktype,
                quantifier_map,
                registered: Some(registered),
                rebounds: Rebounds::NONE,
            }),
            (ktype, registered) => Err(Callable {
                ktype: ktype.map_or_else(|partial| partial, declared),
                quantifier_map,
                registered: registered.map(|registered| match registered {
                    Ok(Registered {
                        shape,
                        quantifier_map,
                        parameters,
                        rebounds: _,
                    }) => Registered {
                        shape: declared(shape),
                        quantifier_map,
                        parameters,
                        rebounds: Rebounds::NONE,
                    },
                    Err(partial) => partial,
                }),
                // A type that closed keeps no bound; one that did not keeps them as written.
                rebounds: match ktype {
                    Ok(_) => Rebounds::NONE,
                    Err(_) => rebounds,
                },
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

/// What names only closed types: a content tree, a retype table.
macro_rules! closes_as_is {
    ($($fact:ty),* $(,)?) => {
        $(impl Close for $fact {
            fn closed(self, _: &mut impl Closer) -> Self {
                self
            }
        })*
    };
}

/// A resolution to a class closes where the binding binds the module binder the class's paths
/// read through, to the path read off the module it carries, and falls in another class where the
/// binding is written into its application or tree; a type over names closes as any type does. Its
/// tree is what the walk resolved it off, which a binding of a parameter leaves a leaf.
impl Close for Resolution<'_> {
    fn closed(self, closer: &mut impl Closer) -> Self {
        let resolved = match self.resolved {
            Resolved::Class(class) => closer.class(class).unwrap_or(self.resolved),
            Resolved::Over(over) => match closer.close(DeclaredType::Type(over)) {
                Closed::Type(DeclaredType::Type(closed)) => Resolved::Closed(closed),
                Closed::Partial(DeclaredType::Type(partial)) => Resolved::Over(partial),
                _ => self.resolved,
            },
            Resolved::Closed(_) => self.resolved,
        };
        Resolution {
            resolved,
            lacks: self.lacks.map(|lacks| lacks.closed(closer)),
            ..self
        }
    }
}

closes_as_is!(KType, Retype, Tree<'_>, &Tree<'_>, Elaboration,);

impl Close for NeverSatisfied {
    fn closed(self, closer: &mut impl Closer) -> Self {
        NeverSatisfied {
            value: self.value.closed(closer),
            wanted: self.wanted.closed(closer),
        }
    }
}

/// A refusal closed, laid again only where some type it judged changes: a quote's code's, written
/// over the code's captures, which a binding of a capture's source closes as a fresh load reads
/// them. An elaboration's names are closed.
impl<'l> CloseLaying<'l> for &'l ShapeError<'l> {
    fn closed_laying(self, closer: &mut impl Laying<'l>) -> Self {
        let mut counting = Counting {
            closer,
            changed: false,
        };
        let error = closed_error(*self, &mut counting);
        match counting.changed {
            false => self,
            true => resident(counting.lay(), error),
        }
    }
}

/// `ty` closed beside the bounds `rebounds` its group writes over names: rebuilt with them written
/// in once every bound and its body close, its bounds then none ([`rebound_into`]); else each
/// closed as far as `closer` closes it.
fn closed_rebound<'l, C: Laying<'l>>(
    closer: &mut Counting<'_, C>,
    ty: DeclaredType<Parametric>,
    rebounds: Rebounds<'l>,
) -> (DeclaredType<Parametric>, Rebounds<'l>) {
    if rebounds.is_empty() {
        return (ty.closed(closer), rebounds);
    }
    match rebound_into(closer, ty, rebounds) {
        Ok(rebuilt) => (rebuilt.into(), Rebounds::NONE),
        Err(partial) => {
            let closed = close_slice(closer, rebounds.0, |(at, bound), closer| {
                (at, bound.closed(closer))
            });
            (partial, Rebounds(closed))
        }
    }
}

/// `error` with every type it judged closed, a refusal under a call's laid again where it changes:
/// one closer type all the way down, so the descent instantiates once.
fn closed_error<'l, C: Laying<'l>>(
    mut error: ShapeError<'l>,
    closer: &mut Counting<'_, C>,
) -> ShapeError<'l> {
    {
        match &mut error {
            ShapeError::Unfixed { wanted, .. } => *wanted = wanted.closed(closer),
            ShapeError::MemberUninstanced {
                carried, wanted, ..
            } => {
                *carried = carried.closed(closer);
                *wanted = wanted.map(|wanted| wanted.closed(closer));
            }
            ShapeError::NoInstance {
                scheme,
                rebounds,
                wanted,
                ..
            } => {
                *wanted = wanted.closed(closer);
                let closed = closed_rebound(closer, (*scheme).into(), *rebounds);
                if let (DeclaredType::Scheme(closed), rebound) = closed {
                    (*scheme, *rebounds) = (closed, rebound);
                }
            }
            ShapeError::NoAdmittingCandidate {
                arguments,
                listed,
                members,
                shapes,
                ..
            } => {
                *arguments = close_slice(closer, arguments, |typed, closer| typed.closed(closer));
                *listed = close_slice(closer, listed, |ty, closer| ty.closed(closer));
                *members = close_slice(closer, members, |(at, name, carried), closer| {
                    (at, name, carried.closed(closer))
                });
                *shapes = close_slice(closer, shapes, |ty, closer| ty.closed(closer));
            }
            ShapeError::Ambiguous {
                arguments, members, ..
            } => {
                *arguments = close_slice(closer, arguments, |ty, closer| ty.closed(closer));
                *members = close_slice(closer, members, |(at, name, carried), closer| {
                    (at, name, carried.closed(closer))
                });
            }
            ShapeError::NoField { of, .. } | ShapeError::NoMember { of, .. } => {
                *of = of.closed(closer)
            }
            ShapeError::ReturnNeverSatisfied { refused, .. }
            | ShapeError::EvalNeverSatisfied { refused, .. }
            | ShapeError::AscriptionNeverSatisfied { refused, .. }
            | ShapeError::AnnotationNeverSatisfied { refused, .. }
            | ShapeError::NotCode { refused, .. }
            | ShapeError::BodyNotCode { refused, .. }
            | ShapeError::NotAModule { refused, .. } => *refused = refused.closed(closer),
            ShapeError::CallNeverSatisfied {
                callee,
                rebounds,
                payload,
                parameters,
                ..
            } => {
                (*callee, *rebounds) = closed_rebound(closer, *callee, *rebounds);
                *payload = payload.closed(closer);
                *parameters = parameters.closed(closer);
            }
            ShapeError::SpreadNeverAList { value, .. }
            | ShapeError::SpliceNeverCode { value, .. } => *value = value.closed(closer),
            ShapeError::ReadNeverSatisfied {
                container,
                argument,
                member,
                ..
            } => {
                *container = container.closed(closer);
                *argument = argument.closed(closer);
                *member = member.map(|(name, carried)| (name, carried.closed(closer)));
            }
            ShapeError::NoAdmittingArm {
                scrutinee, union, ..
            } => {
                *scrutinee = scrutinee.closed(closer);
                if let Some(union) = union {
                    *union = union.closed(closer);
                }
            }
            ShapeError::AmbiguousArms { scrutinee, .. } => *scrutinee = scrutinee.closed(closer),
            ShapeError::NotUnion { clause, .. } => *clause = clause.closed(closer),
            ShapeError::NoVariant { union, .. } => *union = union.closed(closer),
            ShapeError::Uncovered { union, variant, .. } => {
                *union = union.closed(closer);
                *variant = variant.closed(closer);
            }
            ShapeError::RepeatedGuard {
                guard: Repeated::Type(guard),
                ..
            } => *guard = guard.closed(closer),
            ShapeError::UnderCall { refusal, .. } => {
                let before = std::mem::replace(&mut closer.changed, false);
                let inner = closed_error(**refusal, closer);
                if closer.changed {
                    *refusal = resident(closer.lay(), inner);
                }
                closer.changed |= before;
            }
            _ => {}
        }
    }
    error
}

impl<'l> CloseLaying<'l> for Parametric {
    fn closed_laying(self, closer: &mut impl Laying<'l>) -> Self {
        self.closed(closer)
    }
}

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
            InstanceRead::Member {
                name,
                carried,
                wanted,
            } => InstanceRead::Member {
                name,
                carried: carried.closed(closer),
                wanted: match wanted {
                    MemberWanted::At(wanted) => MemberWanted::At(wanted.closed(closer)),
                    wanted => wanted,
                },
            },
        }
    }
}

impl<'l> CloseLaying<'l> for Selection<'l> {
    fn closed_laying(self, closer: &mut impl Laying<'l>) -> Self {
        let asks = close_slice(closer, self.asks, |ask, closer| {
            let lay = closer.lay();
            ask.mapped(lay, &mut |ty| ty.closed(closer))
        });
        Selection { asks, ..self }
    }
}

impl<'l> CloseLaying<'l> for UseStatics<'l> {
    fn closed_laying(self, closer: &mut impl Laying<'l>) -> Self {
        let UseStatics {
            narrowing,
            contributions,
            undecided,
        } = self;
        let undecided = undecided.map(|undecided| {
            let closed = undecided.closed_laying(closer);
            match std::ptr::addr_eq(closed.asks, undecided.asks) {
                true => undecided,
                false => crate::memory::resident(closer.lay(), closed),
            }
        });
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
            undecided,
        }
    }
}

/// A born callable's own group levels are how its body names its group: they stay as laid. Its
/// type closes as far as the binding closes it, a scheme rebuilt once every bound over names
/// closes.
impl<'l> CloseLaying<'l> for Born<'l> {
    fn closed_laying(self, closer: &mut impl Laying<'l>) -> Self {
        let Born {
            callable,
            levels,
            instance,
        } = self;
        Born {
            callable: callable.closed(closer),
            levels,
            instance: instance.map(|instance| instance.closed_laying(closer)),
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
/// marked open where it was judged, so a stored question decides it instead.
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
            minting,
            held,
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
            // A traced call's content is closed: the binding graph maps it.
            traced,
            by_name: close_keyed(closer, by_name, |typed, closer| typed.closed_laying(closer)),
            routes,
            arms,
            retypes,
            trees: close_keyed(closer, trees, |tree, closer| {
                substituted_tree(closer, *tree).unwrap_or(tree)
            }),
            picks,
            contents: close_keyed(closer, contents, |tree, closer| {
                substituted_tree(closer, tree).map_or(tree, |tree| *tree)
            }),
            minting,
            held,
            signature: signature.map(|signature| {
                match closer.close(DeclaredType::Type(signature)) {
                    Closed::Type(DeclaredType::Type(closed)) => closed.into(),
                    Closed::Partial(DeclaredType::Type(partial)) => partial,
                    _ => signature,
                }
            }),
            exits,
            closed,
            folded,
        }
    }
}

/// The tree the closer's binding gives `tree`, where `tree` is open and the binding changes it.
fn substituted_tree<'l>(closer: &mut impl Laying<'l>, tree: Tree<'l>) -> Option<&'l Tree<'l>> {
    match tree.identity() {
        identity @ Identity::Open(_) => closer.tree(identity),
        Identity::Closed(_) | Identity::Unnamed => None,
    }
}

/// A unit's record closed, laid again only where some fact changes.
impl<'l> CloseLaying<'l> for &'l UnitFacts<'l> {
    fn closed_laying(self, closer: &mut impl Laying<'l>) -> Self {
        let UnitFacts {
            typed,
            born,
            paths,
            named_variables,
            seeds,
            posed,
            questions,
        } = *self;
        let closed = UnitFacts {
            typed: close_keyed(closer, typed, |typed, closer| typed.closed_laying(closer)),
            born: close_keyed(closer, born, |born, closer| born.closed_laying(closer)),
            paths: close_keyed(closer, paths, |path, closer| path.closed(closer)),
            named_variables,
            seeds,
            posed,
            questions,
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

/// A closing record closed: a quote's code's unfilled type and its refusal name types a binding
/// closes; a summary is a content tree, and an overlap names a builtin's closed type.
impl<'l> CloseLaying<'l> for &'l Closing<'l> {
    fn closed_laying(self, closer: &mut impl Laying<'l>) -> Self {
        if self.ran.is_none() && self.refusal.is_none() {
            return self;
        }
        let mut counting = Counting {
            closer,
            changed: false,
        };
        let ran = self.ran.closed(&mut counting);
        let refusal = (self.refusal).map(|refusal| refusal.closed_laying(&mut counting));
        match counting.changed {
            false => self,
            true => resident(
                counting.lay(),
                Closing {
                    ran,
                    refusal,
                    ..*self
                },
            ),
        }
    }
}
