//! The **questions** a door asks of a run-bound name, beside the answer a walk took
//! ([build/README.md § Resumption](../build/README.md#resumption)).
//!
//! A discrete door — one handing back a verdict, a bool, a stamp or a kind, never a type — leaves
//! a judgement it cannot decide rigidly as a [`Question`]: every input it judged, enough to judge
//! it again under another binding. A witnessing walk asks the closed door under its witness and
//! notes the [`Answered`] question in its open mark; a later binding **keeps** the question where
//! the closed door, asked with the binding written into the question's types, answers alike
//! ([`keeps`]). An [`Answer`] holds no type, so nothing a walk builds is built from a witness.
//!
//! Substitution commutes with a binding, so [`rebound`] lives here beside the questions it writes a
//! binding into; the walk's absorbing read shares it. `program` does not name `elaborate`, so the
//! closed re-ask lives in `scope`, and the never-satisfies door's pure core with it.

use crate::memory::{BumpAllocator, BumpVec, Lays, collect};
use crate::type_lattice::{
    DeclaredType, DerivedId, Interval, KKind, KType, Parametric, Side, TypeNode, Variable, Verdict,
    judge_by_class, meet, read_through, shape_slots,
};
use crate::values::digest::{DigestHasher, Tag};
use crate::values::{Chain, ContentDigest, Restamping};

use super::derived::{Binds, DerivedName, DerivedTerm, Spellings, handed};
use super::facts::{Binding, Bound, OpenKind};
use super::record_types::Closed;
use super::rule::Rule;
use super::runs::{ArmRunning, Run, Running, arms_run, never_admits, runs};
use super::shape::Shown;
use super::shapes::Shapes;
use super::typed::Resolved;

/// What a discrete door judged: enough to judge it again under another binding.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Question<'a> {
    /// The verdict `judge_by_class(shape, arguments)` gives; its intervals are the door's
    /// ([`Rule::Judged`]), so no answer holds a type.
    Verdict {
        shape: DeclaredType<Parametric>,
        arguments: &'a [Interval],
    },
    /// Whether no value of `value` can satisfy `wanted` ([`unsatisfiable`]).
    NeverSatisfies { value: Interval, wanted: Parametric },
    /// Whether `a` fits `b`.
    Fits {
        a: DeclaredType<Parametric>,
        b: DeclaredType<Parametric>,
    },
    /// Whether `value`'s lower end lies outside `bound`.
    LowerEndOutside {
        value: Interval,
        bound: DeclaredType<Parametric>,
    },
    /// Whether `a` and `b` are one type: the `same`, `is` and `exact` doors.
    Same {
        a: DeclaredType<Parametric>,
        b: DeclaredType<Parametric>,
    },
    /// The kind of the type above every instance of `ty`.
    Kind { ty: Parametric },
    /// Whether a binder declared at `ty` is a module binder: `ty` a signature or an application
    /// of one.
    Module { ty: Parametric },
    /// How a retype to `declared` re-stamps, once a binding closes it.
    Restamp { declared: Parametric },
    /// Whether `a` lies under `b` in the order, once a binding closes both: the order relates
    /// concrete types alone.
    Under { a: Parametric, b: Parametric },
    /// What the candidate at `candidate` of a use's `kept` does at its call ([`runs`]), the use in
    /// unfilled code where `unfilled` says so.
    Runs {
        kept: &'a [Running<'a>],
        candidate: u16,
        unfilled: bool,
    },
    /// What the arm at `arm` of a `MATCH` or `TRY` whose arms are `arms` does where a scrutinee
    /// of the static type `scrutinee` meets them, under the union `clause` ([`arms_run`]).
    Arms {
        scrutinee: Interval,
        clause: Option<Parametric>,
        arms: &'a [ArmRunning],
        arm: u16,
    },
    /// Which of `against`, the closed builtin overloads at a registration's key, the registration
    /// of the unquantified shape `registered` overlaps first ([`overlapping`]).
    Overlaps {
        registered: Parametric,
        against: &'a [KType],
    },
    /// Whether `rule`'s closed function answers over `operands`: a partial rule's, such as a
    /// stored solve's, which a step that admits no solve leaves without one.
    Defined {
        rule: Rule,
        operands: &'a [DeclaredType<Parametric>],
    },
    /// Whether `ty` is a function type, as an instance is wanted at one.
    IsFunction { ty: Parametric },
    /// Whether the registered shape `a` strictly outranks `b` at `class`: one bit of a use's
    /// ranking table, once a binding closes both.
    Outranks {
        a: DeclaredType<Parametric>,
        b: DeclaredType<Parametric>,
        class: u16,
    },
}

/// A closed door's answer, holding no type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Answer {
    Verdict(Verdict),
    Bool(bool),
    /// Some value of the static type may satisfy the wanted one.
    Satisfiable,
    /// No value of the static type can ever satisfy the wanted one.
    Never,
    Kind(KKind),
    Restamps(Restamping),
    Run(Run),
    /// The first overload a registration overlaps, by position, or `None`.
    Overlaps(Option<u16>),
}

/// A verdict answer's order: its index within the verdict's answers.
const VERDICTS: [Verdict; 3] = [Verdict::Never, Verdict::Always, Verdict::Maybe];

/// A kind answer's order.
const KINDS: [KKind; 5] = [
    KKind::ProperType,
    KKind::Signature,
    KKind::AnyType,
    KKind::NewType,
    KKind::TypeConstructor,
];

/// A run answer's order.
const RUNS: [Run; 3] = [Run::Never, Run::May, Run::Selected];

/// A restamp answer's order.
const RESTAMPINGS: [Restamping; 4] = [
    Restamping::Point,
    Restamping::Keeps,
    Restamping::Computed,
    Restamping::Fit,
];

/// Where `each` lies in `all`.
fn position<T: PartialEq>(all: &[T], each: T) -> Option<usize> {
    all.iter().position(|held| *held == each)
}

impl Answer {
    /// Its index among `question`'s answers, below the question's
    /// [`answer_count`](Question::answer_count). An answer of another question's kind is a bug.
    pub fn index_in(&self, question: &Question<'_>) -> u32 {
        let index = match (question, *self) {
            (Question::Verdict { .. }, Answer::Verdict(verdict)) => position(&VERDICTS, verdict),
            (Question::NeverSatisfies { .. }, Answer::Satisfiable) => Some(0),
            (Question::NeverSatisfies { .. }, Answer::Never) => Some(1),
            (
                Question::Fits { .. }
                | Question::LowerEndOutside { .. }
                | Question::Same { .. }
                | Question::Module { .. }
                | Question::Under { .. }
                | Question::Defined { .. }
                | Question::IsFunction { .. }
                | Question::Outranks { .. },
                Answer::Bool(holds),
            ) => Some(usize::from(holds)),
            (Question::Kind { .. }, Answer::Kind(kind)) => position(&KINDS, kind),
            (Question::Restamp { .. }, Answer::Restamps(how)) => position(&RESTAMPINGS, how),
            (Question::Runs { .. } | Question::Arms { .. }, Answer::Run(run)) => {
                position(&RUNS, run)
            }
            (Question::Overlaps { against, .. }, Answer::Overlaps(at)) => {
                Some(at.map_or(against.len(), usize::from))
            }
            _ => None,
        };
        index.expect("an answer is one of its question's") as u32
    }
}

/// A question beside the answer a walk took.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Answered<'a> {
    pub question: Question<'a>,
    pub answer: Answer,
}

impl<'a> Question<'a> {
    /// The kind of open mark this question leaves where no witness answers it.
    pub fn kind(&self) -> OpenKind {
        match self {
            Question::Verdict { .. } => OpenKind::Verdict,
            Question::NeverSatisfies { .. } => OpenKind::NeverSatisfies,
            Question::Fits { .. } => OpenKind::Fits,
            Question::LowerEndOutside { .. } => OpenKind::LowerEndOutside,
            Question::Same { .. } => OpenKind::Unequal,
            Question::Kind { .. } => OpenKind::Kind,
            Question::Module { .. } => OpenKind::ModuleBinder,
            Question::Restamp { .. } => OpenKind::Retype,
            Question::Under { .. } => OpenKind::Under,
            Question::Runs { .. } | Question::Arms { .. } | Question::Outranks { .. } => {
                OpenKind::Settles
            }
            Question::Overlaps { .. } => OpenKind::Overlap,
            Question::Defined {
                rule:
                    Rule::Member(_)
                    | Rule::Applied { .. }
                    | Rule::Pinned
                    | Rule::Needing
                    | Rule::Unbuilt
                    | Rule::Variant(_)
                    | Rule::Payload(_)
                    | Rule::Uncovered
                    | Rule::Covered
                    | Rule::FieldAbove(_)
                    | Rule::FieldUnder(_)
                    | Rule::Fielded
                    | Rule::SignatureValue(_)
                    | Rule::Application
                    | Rule::Wanted
                    | Rule::View
                    | Rule::LaidOut { .. }
                    | Rule::SliceBounds,
                ..
            } => OpenKind::Spells,
            // A solve's rules, and the rules a door hands back a derived name of without asking.
            Question::Defined {
                rule:
                    Rule::Solved { .. }
                    | Rule::Judged { .. }
                    | Rule::Constructed
                    | Rule::InstanceAt { .. }
                    | Rule::Construction { .. }
                    | Rule::Piece(_)
                    | Rule::Meet
                    | Rule::Retyped
                    | Rule::Fill { .. },
                ..
            } => OpenKind::Solve,
            Question::IsFunction { .. } => OpenKind::Instance,
        }
    }

    /// How many answers the closed door may give it: each [`Answer::index_in`] it lies below.
    pub fn answer_count(&self) -> u32 {
        match self {
            Question::Verdict { .. } => 3,
            Question::NeverSatisfies { .. }
            | Question::Fits { .. }
            | Question::LowerEndOutside { .. }
            | Question::Same { .. }
            | Question::Module { .. }
            | Question::Under { .. }
            | Question::Defined { .. }
            | Question::IsFunction { .. }
            | Question::Outranks { .. } => 2,
            Question::Kind { .. } => 5,
            Question::Restamp { .. } => 4,
            Question::Runs { .. } | Question::Arms { .. } => 3,
            Question::Overlaps { against, .. } => against.len() as u32 + 1,
        }
    }

    /// The answer at `index` below [`answer_count`](Self::answer_count): what
    /// [`Answer::index_in`] inverts.
    pub fn nth_answer(&self, index: u32) -> Answer {
        debug_assert!(
            index < self.answer_count(),
            "an answer lies below its question's count"
        );
        let bool = || Answer::Bool(index == 1);
        match self {
            Question::Verdict { .. } => Answer::Verdict(VERDICTS[index as usize]),
            Question::NeverSatisfies { .. } => match index {
                0 => Answer::Satisfiable,
                _ => Answer::Never,
            },
            Question::Fits { .. }
            | Question::LowerEndOutside { .. }
            | Question::Same { .. }
            | Question::Module { .. }
            | Question::Under { .. }
            | Question::Defined { .. }
            | Question::IsFunction { .. }
            | Question::Outranks { .. } => bool(),
            Question::Kind { .. } => Answer::Kind(KINDS[index as usize]),
            Question::Restamp { .. } => Answer::Restamps(RESTAMPINGS[index as usize]),
            Question::Runs { .. } | Question::Arms { .. } => Answer::Run(RUNS[index as usize]),
            Question::Overlaps { against, .. } => {
                Answer::Overlaps(((index as usize) < against.len()).then_some(index as u16))
            }
        }
    }

    /// Hand `each` every type the question names, interval ends included.
    pub fn each_type(&self, each: &mut dyn FnMut(DeclaredType<Parametric>)) {
        let ends = |interval: &Interval, each: &mut dyn FnMut(DeclaredType<Parametric>)| {
            each(interval.lower.into());
            each(interval.upper.into());
        };
        match self {
            Question::Verdict { shape, arguments } => {
                each(*shape);
                for argument in arguments.iter() {
                    ends(argument, each);
                }
            }
            Question::NeverSatisfies { value, wanted } => {
                ends(value, each);
                each((*wanted).into());
            }
            Question::Fits { a, b } | Question::Same { a, b } => {
                each(*a);
                each(*b);
            }
            Question::LowerEndOutside { value, bound } => {
                ends(value, each);
                each(*bound);
            }
            Question::Restamp { declared: ty }
            | Question::Kind { ty }
            | Question::Module { ty } => each((*ty).into()),
            Question::Under { a, b } => {
                each((*a).into());
                each((*b).into());
            }
            Question::Runs { kept, .. } => {
                for running in kept.iter() {
                    each(running.shape);
                    for argument in running.arguments {
                        ends(argument, each);
                    }
                }
            }
            Question::Defined { operands, .. } => operands.iter().for_each(|ty| each(*ty)),
            Question::IsFunction { ty } => each((*ty).into()),
            Question::Outranks { a, b, .. } => {
                each(*a);
                each(*b);
            }
            Question::Overlaps {
                registered,
                against,
            } => {
                each((*registered).into());
                for builtin in against.iter() {
                    each(Parametric::from(*builtin).into());
                }
            }
            Question::Arms {
                scrutinee,
                clause,
                arms,
                ..
            } => {
                ends(scrutinee, each);
                let admits = arms.iter().filter_map(|arm| arm.admits);
                for ty in clause.iter().copied().chain(admits) {
                    each(ty.into());
                }
            }
        }
    }

    /// This question with its slices laid again through `lay`.
    pub fn laid<'to>(&self, lay: impl Lays<'to>) -> Question<'to> {
        match *self {
            Question::Verdict { shape, arguments } => Question::Verdict {
                shape,
                arguments: collect(lay, arguments.iter().copied()),
            },
            Question::NeverSatisfies { value, wanted } => {
                Question::NeverSatisfies { value, wanted }
            }
            Question::Fits { a, b } => Question::Fits { a, b },
            Question::LowerEndOutside { value, bound } => {
                Question::LowerEndOutside { value, bound }
            }
            Question::Same { a, b } => Question::Same { a, b },
            Question::Kind { ty } => Question::Kind { ty },
            Question::Module { ty } => Question::Module { ty },
            Question::Restamp { declared } => Question::Restamp { declared },
            Question::Under { a, b } => Question::Under { a, b },
            Question::Runs {
                kept,
                candidate,
                unfilled,
            } => Question::Runs {
                kept: collect(lay, kept.iter().map(|running| running.laid(lay))),
                candidate,
                unfilled,
            },
            Question::Arms {
                scrutinee,
                clause,
                arms,
                arm,
            } => Question::Arms {
                scrutinee,
                clause,
                arms: collect(lay, arms.iter().copied()),
                arm,
            },
            Question::Overlaps {
                registered,
                against,
            } => Question::Overlaps {
                registered,
                against: collect(lay, against.iter().copied()),
            },
            Question::Defined { rule, operands } => Question::Defined {
                rule,
                operands: collect(lay, operands.iter().copied()),
            },
            Question::IsFunction { ty } => Question::IsFunction { ty },
            Question::Outranks { a, b, class } => Question::Outranks { a, b, class },
        }
    }

    /// This question with `binds` written into every type it names, beside whether any was.
    pub fn substituted<'x>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'x>,
        binds: &dyn Binds,
    ) -> (Question<'x>, bool) {
        let mut changed = false;
        let mut declared = |ty: DeclaredType<Parametric>| match rebound(types, scratch, ty, binds) {
            Closed::Same => ty,
            Closed::Type(closed) => {
                changed = true;
                closed.into()
            }
            Closed::Partial(partial) => {
                changed = true;
                partial
            }
        };
        let substituted = match *self {
            Question::Verdict { shape, arguments } => {
                let shape = declared(shape);
                let mut each = BumpVec::with_capacity_in(arguments.len(), scratch);
                each.extend(
                    arguments
                        .iter()
                        .map(|argument| interval(&mut declared, *argument)),
                );
                let arguments = collect(scratch, each.iter().copied());
                Question::Verdict { shape, arguments }
            }
            Question::NeverSatisfies { value, wanted } => Question::NeverSatisfies {
                value: interval(&mut declared, value),
                wanted: parametric(&mut declared, wanted),
            },
            Question::Fits { a, b } => Question::Fits {
                a: declared(a),
                b: declared(b),
            },
            Question::LowerEndOutside { value, bound } => Question::LowerEndOutside {
                value: interval(&mut declared, value),
                bound: declared(bound),
            },
            Question::Same { a, b } => Question::Same {
                a: declared(a),
                b: declared(b),
            },
            Question::Kind { ty } => Question::Kind {
                ty: parametric(&mut declared, ty),
            },
            Question::Module { ty } => Question::Module {
                ty: parametric(&mut declared, ty),
            },
            Question::Restamp { declared: ty } => Question::Restamp {
                declared: parametric(&mut declared, ty),
            },
            Question::Under { a, b } => Question::Under {
                a: parametric(&mut declared, a),
                b: parametric(&mut declared, b),
            },
            Question::Runs {
                kept,
                candidate,
                unfilled,
            } => {
                let mut each = BumpVec::with_capacity_in(kept.len(), scratch);
                for running in kept.iter() {
                    let mut arguments = BumpVec::with_capacity_in(running.arguments.len(), scratch);
                    arguments
                        .extend((running.arguments.iter()).map(|at| interval(&mut declared, *at)));
                    each.push(Running {
                        shape: declared(running.shape),
                        arguments: collect(scratch, arguments.iter().copied()),
                        ..*running
                    });
                }
                Question::Runs {
                    kept: collect(scratch, each.iter().copied()),
                    candidate,
                    unfilled,
                }
            }
            Question::Arms {
                scrutinee,
                clause,
                arms,
                arm,
            } => {
                let mut each = BumpVec::with_capacity_in(arms.len(), scratch);
                each.extend(arms.iter().map(|at| ArmRunning {
                    admits: at.admits.map(|ty| parametric(&mut declared, ty)),
                    ..*at
                }));
                Question::Arms {
                    scrutinee: interval(&mut declared, scrutinee),
                    clause: clause.map(|ty| parametric(&mut declared, ty)),
                    arms: collect(scratch, each.iter().copied()),
                    arm,
                }
            }
            Question::Overlaps {
                registered,
                against,
            } => Question::Overlaps {
                registered: parametric(&mut declared, registered),
                against: collect(scratch, against.iter().copied()),
            },
            Question::Defined { rule, operands } => {
                let mut each = BumpVec::with_capacity_in(operands.len(), scratch);
                each.extend(operands.iter().map(|operand| declared(*operand)));
                Question::Defined {
                    rule,
                    operands: collect(scratch, each.iter().copied()),
                }
            }
            Question::IsFunction { ty } => Question::IsFunction {
                ty: parametric(&mut declared, ty),
            },
            Question::Outranks { a, b, class } => Question::Outranks {
                a: declared(a),
                b: declared(b),
                class,
            },
        };
        (substituted, changed)
    }
}

/// What substitutes one type of a question.
type Substitute<'f> = dyn FnMut(DeclaredType<Parametric>) -> DeclaredType<Parametric> + 'f;

fn parametric(declared: &mut Substitute<'_>, ty: Parametric) -> Parametric {
    match declared(ty.into()) {
        DeclaredType::Type(ty) => ty,
        DeclaredType::Scheme(_) => unreachable!("a type substitutes to a type"),
    }
}

fn interval(declared: &mut Substitute<'_>, interval: Interval) -> Interval {
    Interval {
        lower: parametric(declared, interval.lower),
        upper: parametric(declared, interval.upper),
    }
}

impl<'a> Answered<'a> {
    /// This answered question laid again through `lay`.
    pub fn laid<'to>(&self, lay: impl Lays<'to>) -> Answered<'to> {
        Answered {
            question: self.question.laid(lay),
            answer: self.answer,
        }
    }

    /// Its content digest, alike across two lays: the question's variant, a verdict's run
    /// lengths, every type it names, then the answer.
    pub fn digest(&self) -> ContentDigest {
        let mut hasher = DigestHasher::new(Tag::Answered);
        self.question.fed(&mut hasher);
        match self.answer {
            Answer::Verdict(verdict) => hasher.count(0).count(verdict_tag(verdict)),
            Answer::Bool(holds) => hasher.count(1).count(usize::from(holds)),
            Answer::Satisfiable => hasher.count(2),
            Answer::Kind(kind) => hasher.count(4).feed(kind),
            Answer::Restamps(how) => hasher.count(6).feed(how),
            Answer::Never => hasher.count(7),
            Answer::Run(run) => hasher.count(8).count(run as usize),
            Answer::Overlaps(at) => hasher
                .count(9)
                .count(at.map_or(0, |at| usize::from(at) + 1)),
        };
        hasher.finished()
    }
}

impl Question<'_> {
    /// Feed `hasher` the question's content, alike across two lays: its variant, a verdict's run
    /// lengths, then every type it names.
    pub fn fed(&self, hasher: &mut DigestHasher) {
        let variant = match *self {
            Question::Verdict { arguments, .. } => {
                hasher.count(0).count(arguments.len());
                None
            }
            Question::NeverSatisfies { .. } => Some(1),
            Question::Fits { .. } => Some(2),
            Question::LowerEndOutside { .. } => Some(3),
            Question::Same { .. } => Some(5),
            Question::Kind { .. } => Some(6),
            Question::Module { .. } => Some(7),
            Question::Restamp { .. } => Some(8),
            Question::Under { .. } => Some(9),
            Question::Runs {
                kept,
                candidate,
                unfilled,
            } => {
                hasher
                    .count(10)
                    .count(kept.len())
                    .count(usize::from(candidate));
                hasher.count(usize::from(unfilled));
                for running in kept {
                    hasher.count(running.arguments.len());
                    hasher.count(usize::from(running.builtin) | usize::from(running.one) << 1);
                    hasher.count(running.surfacing as usize);
                    hasher
                        .count(running.pinned as usize)
                        .count((running.pinned >> 32) as usize);
                }
                None
            }
            Question::Arms {
                clause, arms, arm, ..
            } => {
                hasher.count(11).count(arms.len()).count(usize::from(arm));
                hasher.count(usize::from(clause.is_some()));
                for each in arms {
                    hasher
                        .count(each.guard as usize)
                        .count(usize::from(each.admits.is_some()));
                }
                None
            }
            Question::Overlaps { against, .. } => {
                hasher.count(12).count(against.len());
                None
            }
            Question::Defined { rule, operands } => {
                hasher.count(13).feed(rule).count(operands.len());
                None
            }
            Question::IsFunction { .. } => Some(14),
            Question::Outranks { class, .. } => {
                hasher.count(15).count(usize::from(class));
                None
            }
        };
        variant.inspect(|variant| {
            hasher.count(*variant);
        });
        self.each_type(&mut |ty| _ = hasher.feed(ty.digest()));
    }
}

fn verdict_tag(verdict: Verdict) -> usize {
    match verdict {
        Verdict::Never => 0,
        Verdict::Always => 1,
        Verdict::Maybe => 2,
    }
}

/// The closed type a binding gives each lexical level it binds: a binding's type names, by the
/// level each is homed at. Module binders bind no level. A derived name is no level: a reader gives
/// one its term with the levels written in ([`Binds`]).
pub struct Levels<'x> {
    dense: BumpVec<'x, Option<KType>>,
}

impl<'x> Levels<'x> {
    /// The levels `binding` binds over `shapes`, kept in `scratch`. A chain's levels are
    /// disjoint, so a level two unknowns are — a slot and a capture of it — is bound alike by both.
    pub fn of(scratch: BumpAllocator<'x>, shapes: &Shapes<'_>, binding: Binding<'_>) -> Self {
        let mut dense = BumpVec::new_in(scratch);
        for (unknown, bound) in binding.entries() {
            let Bound::Type(closed) = bound else {
                continue;
            };
            let Some(level) = shapes.level(unknown.segment, unknown.at) else {
                continue;
            };
            if dense.len() <= level {
                dense.resize(level + 1, None);
            }
            debug_assert!(
                dense[level].is_none_or(|held| held == *closed),
                "a level is bound alike by each unknown it is: a slot and its captures"
            );
            dense[level] = Some(*closed);
        }
        Levels { dense }
    }

    /// The levels `bound`, each beside its closed type: a test's binding, with no shapes.
    #[cfg(test)]
    pub(crate) fn from_bound(scratch: BumpAllocator<'x>, bound: &[(usize, KType)]) -> Self {
        let mut dense = BumpVec::new_in(scratch);
        for (level, closed) in bound {
            if dense.len() <= *level {
                dense.resize(level + 1, None);
            }
            dense[*level] = Some(*closed);
        }
        Levels { dense }
    }

    /// The closed type bound at `level`, where one is.
    pub fn bound(&self, level: usize) -> Option<KType> {
        self.dense.get(level).copied().flatten()
    }
}

/// A binding's levels alone name no derived name: a name they meet is left as it is.
impl Binds for Levels<'_> {
    fn level(&self, _: &Chain<'_, '_, '_, '_>, level: usize) -> Option<KType> {
        self.bound(level)
    }
}

/// A store's binding: `levels`, beside a witness's where a witnessing walk asks a question again,
/// and each derived name's term and name through the store, which lays a term no table holds.
pub struct StoreBinds<'q, 'l, S> {
    levels: &'q Levels<'l>,
    store: &'q S,
    witness: Option<Witness<'q>>,
}

/// A witnessing walk's witness, beside how a level finds its unknown.
type Witness<'q> = (
    Binding<'q>,
    &'q dyn Fn(usize) -> Option<super::facts::Unknown>,
);

impl<'q, 'l, S> StoreBinds<'q, 'l, S> {
    /// `store`'s binding at `levels`.
    pub fn of(levels: &'q Levels<'l>, store: &'q S) -> Self {
        StoreBinds {
            levels,
            store,
            witness: None,
        }
    }

    /// This binding with `witness` written in, each level's unknown found by `unknown_of`.
    pub fn witnessing(
        self,
        witness: Binding<'q>,
        unknown_of: &'q dyn Fn(usize) -> Option<super::facts::Unknown>,
    ) -> Self {
        StoreBinds {
            witness: Some((witness, unknown_of)),
            ..self
        }
    }
}

impl<'o, S: super::facts::Store<'o>> Binds for StoreBinds<'_, '_, S> {
    fn level(&self, _: &Chain<'_, '_, '_, '_>, level: usize) -> Option<KType> {
        if let Some((witness, unknown_of)) = self.witness
            && let Some(unknown) = unknown_of(level)
        {
            return match witness.bound(unknown)? {
                Bound::Type(closed) => Some(closed),
                Bound::Module { .. } => None,
            };
        }
        self.levels.bound(level)
    }

    fn term(&self, id: DerivedId) -> Option<DerivedTerm<'_>> {
        self.store.derived_name(id).map(|name| name.term)
    }

    fn name(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        _: BumpAllocator<'_>,
        term: DerivedTerm<'_>,
    ) -> Option<Parametric> {
        Some(self.store.derived(types, term).ty)
    }
}

/// A binding's levels read over the derived names a reader's layers laid, `names` finding each by
/// its id: what a question is judged again under, and a table closed by, outside a walk. A term
/// no table holds is named through `spellings` and handed to `laid`, which keeps it where what the
/// reader closes rests.
pub struct LevelsOver<'q, 'l> {
    pub levels: &'q Levels<'l>,
    pub names: &'q dyn Fn(DerivedId) -> Option<DerivedName<'q>>,
    pub spellings: Spellings<'q>,
    pub laid: &'q dyn Fn(DerivedName<'_>),
}

impl Binds for LevelsOver<'_, '_> {
    fn level(&self, _: &Chain<'_, '_, '_, '_>, level: usize) -> Option<KType> {
        self.levels.bound(level)
    }

    fn term(&self, id: DerivedId) -> Option<DerivedTerm<'_>> {
        (self.names)(id).map(|name| name.term)
    }

    fn name(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
        term: DerivedTerm<'_>,
    ) -> Option<Parametric> {
        let id = term.id();
        if let Some(held) = (self.names)(id) {
            return Some(held.ty);
        }
        let name = self.spellings.named((types, scratch), term);
        (self.laid)(name);
        Some(name.ty)
    }
}

/// The closed door's answer to `question`: `None` where it cannot be written in the question's
/// rigid terms. A retype not the point at its declared type re-stamps at an end, so it is not
/// written.
pub fn answer(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    question: &Question<'_>,
) -> Option<Answer> {
    match *question {
        Question::Verdict { shape, arguments } => Some(Answer::Verdict(
            judge_by_class(types, scratch, shape, arguments).verdict,
        )),
        Question::NeverSatisfies { value, wanted } => {
            Some(match unsatisfiable(types, scratch, value, wanted) {
                Some(_) => Answer::Never,
                None => Answer::Satisfiable,
            })
        }
        Question::Fits { a, b } => Some(Answer::Bool(crate::type_lattice::fits(
            types, scratch, a, b,
        ))),
        Question::LowerEndOutside { value, bound } => Some(Answer::Bool(
            crate::type_lattice::lower_end_outside(types, scratch, value, bound),
        )),
        Question::Same { a, b } => Some(Answer::Bool(a == b)),
        Question::Kind { ty } => Some(Answer::Kind(
            crate::type_lattice::bound_above(types, scratch, ty).kind_of(types),
        )),
        Question::Module { ty } => types
            .concrete(ty)
            .map(|closed| Answer::Bool(is_module(types, closed))),
        Question::Restamp { declared } => (types.concrete(declared))
            .map(|closed| Answer::Restamps(Restamping::of(types, scratch, closed))),
        Question::Under { a, b } => (types.concrete(a))
            .zip(types.concrete(b))
            .map(|(a, b)| Answer::Bool(crate::type_lattice::is_subtype_of(types, scratch, a, b))),
        Question::Runs {
            kept,
            candidate,
            unfilled,
        } => match runs(types, scratch, kept, unfilled) {
            Some(answers) => answers
                .get(usize::from(candidate))
                .copied()
                .map(Answer::Run),
            // A candidate that admits at no binding never runs, whatever the rest do.
            None => (kept.get(usize::from(candidate)))
                .filter(|each| never_admits(types, scratch, each))
                .map(|_| Answer::Run(Run::Never)),
        },
        Question::Arms {
            scrutinee,
            clause,
            arms,
            arm,
        } => arms_run(types, scratch, scrutinee, clause, arms)
            .and_then(|answers| answers.get(usize::from(arm)).copied())
            .map(Answer::Run),
        Question::Overlaps {
            registered,
            against,
        } => (types.concrete(registered)).map(|registered| {
            Answer::Overlaps(overlapping(types, scratch, registered, against).map(|at| at as u16))
        }),
        Question::Defined { rule, operands } => {
            let mut closed = BumpVec::with_capacity_in(operands.len(), scratch);
            closed.extend(operands.iter().map_while(|ty| types.concrete_declared(*ty)));
            (closed.len() == operands.len())
                .then(|| Answer::Bool(rule.closed(types, scratch, &closed).is_some()))
        }
        Question::IsFunction { ty } => (types.concrete(ty))
            .map(|ty| Answer::Bool(matches!(types.node(ty), TypeNode::KFunction { .. }))),
        Question::Outranks { a, b, class } => (types.concrete_declared(a))
            .zip(types.concrete_declared(b))
            .map(|(a, b)| {
                Answer::Bool(crate::type_lattice::outranks(
                    types,
                    scratch,
                    a.into(),
                    b.into(),
                    class,
                ))
            }),
    }
}

/// Whether a binder declared at the closed type `ty` is a module binder: `ty` a signature, or an
/// application of one.
pub fn is_module(types: &Chain<'_, '_, '_, '_>, ty: KType) -> bool {
    match types.node(ty) {
        TypeNode::Signature { .. } => true,
        TypeNode::SignatureApply { signature, .. } => {
            matches!(types.node(signature), TypeNode::Signature { .. })
        }
        _ => false,
    }
}

/// Whether `binds` keeps `answered`: the closed door, asked with the binding written into the
/// question's types, answers alike.
pub fn keeps(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    answered: &Answered<'_>,
    binds: &dyn Binds,
) -> bool {
    let (question, _) = (answered.question).substituted(types, scratch, binds);
    answer(types, scratch, &question) == Some(answered.answer)
}

/// The first of `against`, the closed builtin overloads at a key, by position, that the closed
/// unquantified `registered` overlaps there: every slot pair meets above `Never`.
pub fn overlapping(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    registered: KType,
    against: &[KType],
) -> Option<usize> {
    against.iter().position(|builtin| {
        (shape_slots(registered, types).zip(shape_slots(*builtin, types)))
            .all(|(own, theirs)| meet(types, scratch, own, theirs) != KType::NEVER)
    })
}

/// Whether the unquantified `registered`, written over names, overlaps none of `against` at every
/// binding: for each, some slot's bound already meets that overload's slot at `Never`.
pub fn never_overlaps(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    registered: Parametric,
    against: &[KType],
) -> bool {
    against.iter().all(|builtin| {
        (shape_slots(registered, types).zip(shape_slots(*builtin, types))).any(
            |(own, theirs): (Parametric, KType)| {
                let bound = crate::type_lattice::bound_above(types, scratch, own);
                meet(types, scratch, bound, theirs) == KType::NEVER
            },
        )
    })
}

/// `value` shown by the end that decides and `wanted` read through its bound, where no value of
/// the static type `value` can ever satisfy `wanted`: its upper end, read through its bound, meets
/// `wanted` at `Never`, or else its lower end, read below its variables, lies outside it. `None`
/// where one might. The never-satisfies door's judgement, before it marks.
pub fn unsatisfiable(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    value: Interval,
    wanted: Parametric,
) -> Option<(Shown, KType)> {
    let (upper, bound) = (
        crate::type_lattice::bound_above(types, scratch, value.upper),
        crate::type_lattice::bound_above(types, scratch, wanted),
    );
    match meet(types, scratch, upper, bound) == KType::NEVER {
        true => Some((upper.into(), bound)),
        false => crate::type_lattice::lower_end_outside(types, scratch, value, bound).then(|| {
            let value = Interval {
                upper: upper.into(),
                ..value
            };
            (Shown::at_least(types, value), bound)
        }),
    }
}

/// `ty` read under a binding: each lexical variable `binds` binds replaced by its type, each path
/// class's variable whose module binder it binds by the path's type off that module, and each
/// derived name whose term reads one by what [`handed`] gives that term with the binding written
/// in — a type, or the name of the term it leaves. Substitution commutes with the binding, so this
/// is no judgement.
pub fn rebound(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: DeclaredType<Parametric>,
    binds: &dyn Binds,
) -> Closed {
    let mut changed = false;
    let read = match ty {
        DeclaredType::Type(typed) => {
            if types.concrete(typed).is_some() {
                return Closed::Same;
            }
            let read = read_through(types, scratch, typed, Side::Above, &mut |variable| {
                let written = bound_variable(types, scratch, variable, binds)?;
                changed = true;
                Some(Interval::point(written))
            });
            DeclaredType::Type(read)
        }
        DeclaredType::Scheme(scheme) => {
            let mut dense = BumpVec::new_in(scratch);
            for variable in free_variables(types, scratch, ty) {
                if let Variable::Lexical { level, .. } = variable
                    && let Some(closed) = binds.level(types, level)
                {
                    if dense.len() <= level {
                        dense.resize(level + 1, None);
                    }
                    dense[level] = Some(closed);
                }
            }
            let mut scheme = crate::type_lattice::substitute_levels(types, scratch, scheme, &dense);
            changed |= !dense.is_empty();
            scheme = crate::type_lattice::substitute_derived(types, scratch, scheme, &mut |id| {
                let written = bound_derived(types, scratch, id, binds)?;
                changed = true;
                Some(written)
            });
            scheme = crate::type_lattice::substitute_paths(
                types,
                scratch,
                scheme,
                &mut |class, name| {
                    // A path variable's bound is `Any`: what a path reads is its class's alone.
                    let written =
                        path_bound(types, binds.class(types, scratch, class)?, name, KType::ANY);
                    changed = true;
                    Some(written)
                },
            );
            DeclaredType::Scheme(scheme)
        }
    };
    // A scheme is concrete as it is, so it is closed only where the binding left no name in it.
    let open =
        matches!(read, DeclaredType::Scheme(_)) && !free_variables(types, scratch, read).is_empty();
    match (changed, types.concrete_declared(read)) {
        (false, _) => Closed::Same,
        (true, Some(closed)) if !open => Closed::Type(closed),
        (true, _) => Closed::Partial(read),
    }
}

/// What `binds` writes in for `variable`: a bound level's closed type, a derived name's term
/// handed again with the binding written in, or the type a bound module binder gives a path
/// class's variable. `None` where it writes nothing.
fn bound_variable(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    variable: Variable,
    binds: &dyn Binds,
) -> Option<Parametric> {
    match variable {
        Variable::Lexical { level, .. } => binds.level(types, level).map(Parametric::from),
        Variable::Derived { term, .. } => bound_derived(types, scratch, term, binds),
        Variable::Path { class, name, bound } => {
            let resolved = binds.class(types, scratch, class)?;
            Some(path_bound(types, resolved, name, bound))
        }
        _ => None,
    }
}

/// The type `resolved`, what a binding makes a path variable named `name` with bound `bound`,
/// writes in: its closed type, or the variable of the class it renames it to.
fn path_bound(
    types: &Chain<'_, '_, '_, '_>,
    resolved: Resolved,
    name: crate::symbols::TypeSymbol,
    bound: KType,
) -> Parametric {
    match resolved {
        Resolved::Closed(closed) => closed.into(),
        Resolved::Over(over) => over,
        Resolved::Class(class) => types.path_variable(class, name, bound),
    }
}

/// What `binds` gives the derived name `id`: its term, with the binding written in, handed again.
/// `None` where no table `binds` reads holds the term, or the binding writes nothing into it.
fn bound_derived(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    id: DerivedId,
    binds: &dyn Binds,
) -> Option<Parametric> {
    let term = binds.term(id)?;
    if let DerivedTerm::Carried { binder, .. } = term {
        return binds.carried(binder).map(Parametric::from);
    }
    let (term, changed) = term.substituted(types, scratch, binds);
    if !changed {
        return None;
    }
    handed(types, scratch, term, &mut |term| {
        binds.name(types, scratch, term)
    })
}

/// The free lexical and path variables and derived names `ty` holds, each once: what a mark reads
/// off a type.
pub fn free_variables<'x>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    ty: DeclaredType<Parametric>,
) -> BumpVec<'x, Variable> {
    let body = match ty {
        DeclaredType::Type(ty) => ty,
        DeclaredType::Scheme(scheme) => {
            let group = DeclaredType::<Parametric>::Scheme(scheme).view(types).group;
            crate::type_lattice::instantiate_quantified(types, scratch, scheme, group.bounds)
        }
    };
    let mut found = BumpVec::new_in(scratch);
    if types.concrete(body).is_some() {
        return found;
    }
    read_through(types, scratch, body, Side::Above, &mut |variable| {
        if matches!(
            variable,
            Variable::Lexical { .. } | Variable::Path { .. } | Variable::Derived { .. }
        ) && !found.contains(&variable)
        {
            found.push(variable);
        }
        None
    });
    found
}
