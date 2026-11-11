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
    DeclaredType, Interval, KKind, KType, Parametric, Side, TypeNode, Variable, Verdict,
    judge_by_class, meet, read_through,
};
use crate::values::digest::{DigestHasher, Tag};
use crate::values::{Chain, ContentDigest, Restamping};

use super::facts::{Binding, Bound, OpenKind};
use super::record_types::Closed;
use super::shape::Shown;
use super::shapes::Shapes;

/// What a discrete door judged: enough to judge it again under another binding.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Question<'a> {
    /// `judge_by_class(shape, arguments)`, beside the rigid intervals the door handed back.
    Verdict {
        shape: DeclaredType<Parametric>,
        arguments: &'a [Interval],
        intervals: Option<&'a [Interval]>,
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
    /// Whether a retype at `declared` re-stamps at exactly `declared`.
    Retype { declared: Parametric },
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
}

/// A closed door's answer, holding no type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Answer {
    /// A verdict over the question's intervals.
    Verdict(Verdict),
    /// A verdict whose solve succeeded over no variable where the question's read none: the door
    /// hands back the empty intervals, which hold no type.
    Solved(Verdict),
    Bool(bool),
    /// Some value of the static type may satisfy the wanted one.
    Satisfiable,
    /// A retype re-stamps at exactly its declared type.
    Stamps,
    Kind(KKind),
    Restamps(Restamping),
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
            Question::Retype { .. } => OpenKind::Retype,
            Question::Same { .. } => OpenKind::Unequal,
            Question::Kind { .. } => OpenKind::Kind,
            Question::Module { .. } => OpenKind::ModuleBinder,
            Question::Restamp { .. } => OpenKind::Retype,
        }
    }

    /// Hand `each` every type the question names, interval ends and verdict intervals included.
    pub fn each_type(&self, each: &mut dyn FnMut(DeclaredType<Parametric>)) {
        let ends = |interval: &Interval, each: &mut dyn FnMut(DeclaredType<Parametric>)| {
            each(interval.lower.into());
            each(interval.upper.into());
        };
        match self {
            Question::Verdict {
                shape,
                arguments,
                intervals,
            } => {
                each(*shape);
                for argument in arguments.iter().chain(intervals.iter().copied().flatten()) {
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
            Question::Retype { declared: ty }
            | Question::Restamp { declared: ty }
            | Question::Kind { ty }
            | Question::Module { ty } => each((*ty).into()),
        }
    }

    /// This question with its slices laid again through `lay`.
    pub fn laid<'to>(&self, lay: impl Lays<'to>) -> Question<'to> {
        match *self {
            Question::Verdict {
                shape,
                arguments,
                intervals,
            } => Question::Verdict {
                shape,
                arguments: collect(lay, arguments.iter().copied()),
                intervals: intervals.map(|intervals| collect(lay, intervals.iter().copied())),
            },
            Question::NeverSatisfies { value, wanted } => {
                Question::NeverSatisfies { value, wanted }
            }
            Question::Fits { a, b } => Question::Fits { a, b },
            Question::LowerEndOutside { value, bound } => {
                Question::LowerEndOutside { value, bound }
            }
            Question::Retype { declared } => Question::Retype { declared },
            Question::Same { a, b } => Question::Same { a, b },
            Question::Kind { ty } => Question::Kind { ty },
            Question::Module { ty } => Question::Module { ty },
            Question::Restamp { declared } => Question::Restamp { declared },
        }
    }

    /// This question with the closed type `bound` gives each level written into every type it
    /// names, beside whether any was.
    pub fn substituted<'x>(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'x>,
        bound: &mut dyn FnMut(usize) -> Option<KType>,
    ) -> (Question<'x>, bool) {
        let mut changed = false;
        let mut declared = |ty: DeclaredType<Parametric>| match rebound(types, scratch, ty, bound) {
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
            Question::Verdict {
                shape,
                arguments,
                intervals,
            } => {
                let shape = declared(shape);
                let mut each = BumpVec::with_capacity_in(arguments.len(), scratch);
                each.extend(
                    arguments
                        .iter()
                        .map(|argument| interval(&mut declared, *argument)),
                );
                let arguments = collect(scratch, each.iter().copied());
                let intervals = intervals.map(|intervals| {
                    let mut each = BumpVec::with_capacity_in(intervals.len(), scratch);
                    each.extend(intervals.iter().map(|at| interval(&mut declared, *at)));
                    collect(scratch, each.iter().copied())
                });
                Question::Verdict {
                    shape,
                    arguments,
                    intervals,
                }
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
            Question::Retype { declared: ty } => Question::Retype {
                declared: parametric(&mut declared, ty),
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
        let variant = match self.question {
            Question::Verdict {
                arguments,
                intervals,
                ..
            } => {
                hasher.count(0).count(arguments.len());
                // A verdict's intervals are absent, or a run whose length follows.
                match intervals {
                    None => hasher.count(0),
                    Some(intervals) => hasher.count(1).count(intervals.len()),
                };
                None
            }
            Question::NeverSatisfies { .. } => Some(1),
            Question::Fits { .. } => Some(2),
            Question::LowerEndOutside { .. } => Some(3),
            Question::Retype { .. } => Some(4),
            Question::Same { .. } => Some(5),
            Question::Kind { .. } => Some(6),
            Question::Module { .. } => Some(7),
            Question::Restamp { .. } => Some(8),
        };
        variant.inspect(|variant| {
            hasher.count(*variant);
        });
        self.question
            .each_type(&mut |ty| _ = hasher.feed(ty.digest()));
        match self.answer {
            Answer::Verdict(verdict) => hasher.count(0).count(verdict_tag(verdict)),
            Answer::Bool(holds) => hasher.count(1).count(usize::from(holds)),
            Answer::Satisfiable => hasher.count(2),
            Answer::Stamps => hasher.count(3),
            Answer::Kind(kind) => hasher.count(4).feed(kind),
            Answer::Solved(verdict) => hasher.count(5).count(verdict_tag(verdict)),
            Answer::Restamps(how) => hasher.count(6).feed(how),
        };
        hasher.finished()
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
/// level each is homed at. Module binders bind no level.
pub struct Levels<'x> {
    dense: BumpVec<'x, Option<KType>>,
}

impl<'x> Levels<'x> {
    /// The levels `binding` binds over `shapes`, kept in `scratch`. A chain's levels are disjoint,
    /// so a level two unknowns are — a slot and a capture of it — is bound alike by both.
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

/// The closed door's answer to `question`: `None` where it cannot be written in the question's
/// rigid terms. A verdict other than *never* is written only where the intervals it hands back are
/// the question's, or empty where the question's solve read none; a never-satisfies refusal is worded from ends, and a retype not the point at
/// its declared type re-stamps at an end, so neither is written.
pub fn answer(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    question: &Question<'_>,
) -> Option<Answer> {
    match *question {
        Question::Verdict {
            shape,
            arguments,
            intervals,
        } => {
            let judged = judge_by_class(types, scratch, shape, arguments);
            match judged.verdict {
                Verdict::Never => Some(Answer::Verdict(Verdict::Never)),
                verdict if judged.intervals == intervals => Some(Answer::Verdict(verdict)),
                verdict if intervals.is_none() && judged.intervals == Some(&[]) => {
                    Some(Answer::Solved(verdict))
                }
                _ => None,
            }
        }
        Question::NeverSatisfies { value, wanted } => unsatisfiable(types, scratch, value, wanted)
            .is_none()
            .then_some(Answer::Satisfiable),
        Question::Fits { a, b } => Some(Answer::Bool(crate::type_lattice::fits(
            types, scratch, a, b,
        ))),
        Question::LowerEndOutside { value, bound } => Some(Answer::Bool(
            crate::type_lattice::lower_end_outside(types, scratch, value, bound),
        )),
        Question::Retype { declared } => (crate::values::retyped_to(types, scratch, declared)
            == Interval::point(declared))
        .then_some(Answer::Stamps),
        Question::Same { a, b } => Some(Answer::Bool(a == b)),
        Question::Kind { ty } => Some(Answer::Kind(
            crate::type_lattice::bound_above(types, scratch, ty).kind_of(types),
        )),
        Question::Module { ty } => types
            .concrete(ty)
            .map(|closed| Answer::Bool(is_module(types, closed))),
        Question::Restamp { declared } => (types.concrete(declared))
            .map(|closed| Answer::Restamps(Restamping::of(types, scratch, closed))),
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

/// Whether a binding whose levels are `levels` keeps `answered`: the closed door, asked with the
/// binding written into the question's types, answers alike.
pub fn keeps(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    answered: &Answered<'_>,
    levels: &Levels<'_>,
) -> bool {
    let (question, _) =
        (answered.question).substituted(types, scratch, &mut |level| levels.bound(level));
    answer(types, scratch, &question) == Some(answered.answer)
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

/// `ty` read under a binding: each lexical variable `bound` answers for, by its level, replaced by
/// its type. Substitution commutes with the binding, so this is no judgement.
pub fn rebound(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    ty: DeclaredType<Parametric>,
    bound: &mut dyn FnMut(usize) -> Option<KType>,
) -> Closed {
    match ty {
        DeclaredType::Type(typed) => {
            if types.concrete(typed).is_some() {
                return Closed::Same;
            }
            let mut changed = false;
            let read = read_through(types, scratch, typed, Side::Above, &mut |variable| {
                let Variable::Lexical { level, .. } = variable else {
                    return None;
                };
                let closed = bound(level)?;
                changed = true;
                Some(Interval::point(closed.into()))
            });
            match (changed, types.concrete(read)) {
                (false, _) => Closed::Same,
                (true, Some(closed)) => Closed::Type(DeclaredType::Type(closed)),
                (true, None) => Closed::Partial(DeclaredType::Type(read)),
            }
        }
        DeclaredType::Scheme(scheme) => {
            let mut levels = BumpVec::new_in(scratch);
            let mut open = false;
            for variable in free_variables(types, scratch, ty) {
                match variable {
                    Variable::Lexical { level, .. } => match bound(level) {
                        Some(closed) => levels.push((level, closed)),
                        None => open = true,
                    },
                    _ => open = true,
                }
            }
            if levels.is_empty() {
                return Closed::Same;
            }
            let top = levels.iter().map(|(level, _)| *level).max().unwrap_or(0);
            // A level `bound` leaves unbound keeps its variable.
            let mut dense = BumpVec::with_capacity_in(top + 1, scratch);
            dense.extend((0..=top).map(|at| {
                (levels.iter().find(|(level, _)| *level == at)).map(|(_, closed)| *closed)
            }));
            let closed = crate::type_lattice::substitute_levels(types, scratch, scheme, &dense);
            match open {
                false => Closed::Type(DeclaredType::Scheme(closed)),
                true => Closed::Partial(DeclaredType::Scheme(closed)),
            }
        }
    }
}

/// The free lexical and path variables `ty` holds, each once: what a mark reads off a type.
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
        if matches!(variable, Variable::Lexical { .. } | Variable::Path { .. })
            && !found.contains(&variable)
        {
            found.push(variable);
        }
        None
    });
    found
}
