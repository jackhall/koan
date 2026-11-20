//! The walk's rule for a `MATCH`: its static type, each arm's `it`, the union clause's variants and
//! which arms a run can take. A `TRY`'s arms are read here too, as a `MATCH … UNDER Error`'s
//! ([catching.rs](catching.rs)).
//!
//! Each arm's guard is read as a type: a type guard's recorded expression, a value guard's scalar
//! type, a label's variant of the union clause, and none for `_`. A clause written over run-bound
//! names is read by its questions — whether each label names a variant, and whether the labels
//! cover it — and each variant, payload and `_`'s payload is the derived name a binding reads it
//! through ([`Rule::Variant`], [`Rule::Payload`], [`Rule::Uncovered`]). Its `it` is the
//! scrutinee retyped to that type ([`retyped_to`]), as a parameter is retyped to its declared
//! type — a label's the variant's payload at the variant's representation — and `_`'s the
//! scrutinee as it is, or under `UNDER` a payload of a variant no label names. Each arm is typed as
//! an inline block entered with `it` at that interval, which the `MATCH` also keeps at the arm's
//! site for a walk rooted at the arm ([`entering_of`](super::Pass::entering_of)).
//!
//! The arms are then narrowed against the scrutinee's static type as
//! [static selection](super::super::select) narrows a keyworded use's candidates, reading both
//! ends of the scrutinee's static type: an arm whose guard can never admit, or that the
//! scrutinee's lower end lies outside, is dropped, not refused, since coverage forces arms a
//! narrow scrutinee never reaches; one a guard that always admits outranks is dropped too, as is
//! `_` beside such a guard. No arm left refuses the load, and so do arms that all always admit
//! with none ranking first. Where every arm left always admits, the one they rank first is
//! recorded as the arm the run takes.
//! How guarded arms rank — a value outranks every type, and a type another it lies strictly under
//! — is laid in a table where every guard is closed and the arms fit one, which the run reads
//! instead of ranking; past a table's bound, both rank by brute force ([`first_ranked`]).
//!
//! The `MATCH` is the static types of the arms that may run spanned, as every binding agrees which
//! do ([`arms_run`]): the chosen arm's, or else the plain union of their upper ends over the meet
//! of their lower ends. Where a binding decides which run, each arm's type is a decided name on
//! [`Question::Arms`], so the `MATCH` is the same at load and under every binding.
//!
//! See [README.md § Static types](../README.md#static-types).

use crate::elaborate::judge::{self, Marks};
use crate::elaborate::{arm_shapes, variant_label, variant_named, variants};
use crate::memory::{BumpVec, collect};
use crate::parse::KExpression;
use crate::scope::{
    ArmRunning, Arms, BodyShape, Elimination, Guard, Question, Restamp, Rule, Run, Scalar,
    ShapeError, Store, Table, arms_run, first_ranked, guard_outranks,
};
use crate::type_lattice::{DeclaredType, Interval, KType, Parametric, Verdict};
use crate::values::{representation, under, unknown};

use super::{Entering, Pass};

/// One arm as the walk reads it: what admits a value to it, and the interval its `it` lies in.
#[derive(Clone, Copy)]
struct Read<'graph> {
    guard: Guard<'graph>,
    /// The type the guard admits by: `None` for `_`, and for a guard the load cannot read.
    admits: Option<Parametric>,
    it: Interval,
}

impl<'p, 'o: 'p, 'graph: 'o + 'p, 'tier: 'o, S: Store<'o>>
    Pass<'p, 'o, '_, 'graph, 'tier, '_, '_, S>
{
    /// `MATCH <scrutinee> WITH <arms>`, or, `under`, `MATCH <scrutinee> UNDER <union> WITH
    /// <arms>`: each arm typed with its `it`, the arms narrowed, and the kept arms' types spanned.
    pub(super) fn matched(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        under_union: bool,
    ) -> Result<Interval, ShapeError<'o>> {
        let types = self.types;
        let scrutinee = self.part(level, &node.parts[1].value)?;
        let clause = match under_union {
            true => self.declared(level, &node.parts[3].value),
            false => None,
        };
        // A clause over run-bound names is read by its questions ([`Named`]); one over a path is
        // read again where its module binder's content holds, and no binding closes any other.
        let union = clause
            .filter(|clause| {
                judge::names_held(types, self.scratch, *clause) != judge::Names::RunBound
            })
            .and_then(|clause| judge::static_of(types, clause))
            .filter(|union| variants(types, *union).is_some());
        self.armed(level, node, scrutinee, clause, union, under_union)
    }

    /// The arms of the `MATCH` or `TRY` `node` over a scrutinee of the static type `scrutinee`,
    /// under the union `clause` where `under_union` (`union` where the load closed it): each arm
    /// typed with its `it`, the arms narrowed, and the kept arms' types spanned.
    pub(super) fn armed(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        scrutinee: Interval,
        clause: Option<Parametric>,
        union: Option<KType>,
        under_union: bool,
    ) -> Result<Interval, ShapeError<'o>> {
        let shape = self.chain[level].shape;
        let (types, scratch) = (self.types, self.scratch);
        let arms: BumpVec<'_, &'graph BodyShape<'graph>> = {
            let mut arms = BumpVec::new_in(scratch);
            arms.extend(arm_shapes(shape, node));
            arms
        };
        let named = match (under_union, union, clause) {
            (true, None, Some(clause))
                if judge::names_held(types, scratch, clause) == judge::Names::RunBound =>
            {
                Some(self.named(node, clause, &arms)?)
            }
            _ => None,
        };
        let clause_read = match (union, named) {
            (Some(union), _) => Clause::Closed(union),
            (None, Some(named)) => Clause::Named(named),
            (None, None) => Clause::Unread,
        };
        let mut reads = BumpVec::with_capacity_in(arms.len(), scratch);
        for arm in arms.iter() {
            let guard = arm.arm().expect("an arm knows it is one").guard;
            let (read, retype) =
                self.read_arm(level, guard, scrutinee, under_union, clause_read, &arms);
            // The arm's site keeps its `it`'s interval and what the run re-stamps `it` at.
            let site = self.arm_site(level, arm);
            self.record_part(level, site, read.it);
            if let Some(retype) = retype {
                self.record_retype(level, site, retype);
            }
            reads.push(read);
        }
        // Each arm typed as a block entered with its `it`.
        let tag = self.innermost();
        let mut typed = BumpVec::with_capacity_in(arms.len(), scratch);
        for (arm, read) in arms.iter().zip(reads.iter()) {
            let last = |pass: &mut Self, at: usize| Ok(pass.last_statement(at));
            let (last, popped) = self.inline(level, arm, Entering::Arm(read.it, tag), last)?;
            self.fold(level, &popped);
            typed.push(last.unwrap_or_else(unknown));
        }
        // A scrutinee that never arrives takes no arm.
        if self.never_arrives(&[scrutinee.upper]) {
            return Ok(Interval::point(KType::NEVER.into()));
        }
        let verdicts = self.verdicts(scrutinee, clause, &reads);
        let marks: &dyn Marks = &*self;
        // How each pair of arms ranks, decided where a binding does not decide it: the load ranks
        // only where every pair is decided, and leaves the rest to the run.
        let mut ranks = BumpVec::with_capacity_in(reads.len() * reads.len(), scratch);
        for a in 0..reads.len() {
            for b in 0..reads.len() {
                ranks.push(match a == b {
                    true => Some(false),
                    false => outranks(marks, types, scratch, &reads[a], &reads[b]),
                });
            }
        }
        let closed = ranks.iter().all(Option::is_some);
        let outranks = |a: usize, b: usize| ranks[a * reads.len() + b] == Some(true);
        let mut kept = BumpVec::with_capacity_in(reads.len(), scratch);
        for (at, verdict) in verdicts.iter().enumerate() {
            let dropped = match verdict {
                Verdict::Never => true,
                // An arm an always one outranks never runs, nor `_` beside a guarded always one.
                Verdict::Maybe | Verdict::Always => (0..reads.len()).any(|always| {
                    verdicts[always] == Verdict::Always
                        && !matches!(reads[always].guard, Guard::Default)
                        && (matches!(reads[at].guard, Guard::Default)
                            || (*verdict == Verdict::Maybe && closed && outranks(always, at)))
                }),
            };
            if !dropped {
                kept.push((at as u16, *verdict));
            }
        }
        if kept.is_empty() {
            return Err(ShapeError::NoAdmittingArm {
                scrutinee: scrutinee.upper,
                union,
                at: node.source,
            });
        }
        let ranking = match closed {
            true => Table::laid(scratch, reads.len(), 1, |_, a, b| outranks(a, b), |_| false)
                .map_or(Elimination::Open, Elimination::Table),
            false => Elimination::Open,
        };
        let chosen = match &kept[..] {
            [(only, Verdict::Always)] => Some(*only),
            [_] => None,
            _ if (kept.iter()).all(|(_, verdict)| *verdict == Verdict::Always) => {
                let mut all = BumpVec::with_capacity_in(kept.len(), scratch);
                all.extend(kept.iter().map(|(at, _)| usize::from(*at)));
                let decided = match ranking {
                    // A table lays at most `Table::MOST` arms, so each position is one bit.
                    Elimination::Table(table) => {
                        Some(table.decide((all.iter()).fold(0u64, |mask, at| mask | 1 << at)))
                    }
                    Elimination::Open if closed => Some(first_ranked(&all, outranks)),
                    // A rigid guard among them: ranked where its binding is known.
                    Elimination::Open => None,
                };
                match decided {
                    Some(Ok(winner)) => Some(winner as u16),
                    Some(Err(count)) => {
                        return Err(ShapeError::AmbiguousArms {
                            count,
                            scrutinee: scrutinee.upper,
                            at: node.source,
                        });
                    }
                    None => None,
                }
            }
            _ => None,
        };
        if !self.unfilled {
            let arms = Arms {
                kept: collect(self.lay, kept.iter().copied()),
                ranking: match ranking {
                    Elimination::Table(table) => Elimination::Table(table.relaid(self.lay)),
                    Elimination::Open => Elimination::Open,
                },
                chosen,
            };
            self.record_arms(level, self.node_site(level, node), arms);
        }
        let armed = self.arms_type(scrutinee, clause, &reads, &typed);
        Ok(self.arriving(&[scrutinee.upper], armed))
    }

    /// The type of arms read as `reads` and typed as `typed`, over a scrutinee of the static type
    /// `scrutinee` under the union `clause`: each arm's type on what it does as every binding
    /// agrees ([`arms_run`]) — closed where they do, else a decided name per arm on
    /// [`Question::Arms`] — the plain union of the upper ends of those that may run, over the meet
    /// of their lower ends, so it is the same at load and under every binding.
    fn arms_type(
        &self,
        scrutinee: Interval,
        clause: Option<Parametric>,
        reads: &[Read<'graph>],
        typed: &[Interval],
    ) -> Interval {
        let (types, scratch) = (self.types, self.scratch);
        let (never, any) = (Parametric::from(KType::NEVER), Parametric::from(KType::ANY));
        let arms = collect(
            scratch,
            (reads.iter()).map(|read| ArmRunning {
                guard: read.guard.kind(),
                admits: read.admits,
            }),
        );
        let answers = arms_run(types, scratch, scrutinee, clause, arms);
        let (mut lower, mut uppers) = (any, BumpVec::with_capacity_in(typed.len(), scratch));
        for (at, each) in typed.iter().enumerate() {
            let (low, up) = match &answers {
                Some(answers) => match answers[at] {
                    Run::Never => (any, never),
                    Run::May | Run::Selected => (each.lower, each.upper),
                },
                None => {
                    let question = Question::Arms {
                        scrutinee,
                        clause,
                        arms,
                        arm: at as u16,
                    };
                    let decided = |arms: &[Parametric], undecided| {
                        judge::decided(self, types, scratch, (question, arms), undecided)
                    };
                    (
                        decided(&[any, each.lower, each.lower], never),
                        decided(&[never, each.upper, each.upper], each.upper),
                    )
                }
            };
            lower = judge::met(self, types, scratch, lower, low).unwrap_or(never);
            uppers.push(up);
        }
        Interval {
            lower,
            upper: types.union_of(scratch, &uppers),
        }
    }

    /// The union clause `clause` of the `MATCH … UNDER` `node`, written over run-bound names,
    /// read by its questions: whether each label among `arms` names a variant of it, and whether
    /// the labelled variants cover it or, beside a `_` arm, whether it is a union at all
    /// ([`Rule::Variant`], [`Rule::Covered`], [`Rule::Uncovered`]). A binding answering no refuses,
    /// and the closed walk words the refusal. Each labelled variant is the derived name a binding
    /// reads it through.
    fn named(
        &self,
        node: &'graph KExpression<'graph>,
        clause: Parametric,
        arms: &[&'graph BodyShape<'graph>],
    ) -> Result<Named<'p>, ShapeError<'o>> {
        let (types, scratch) = (self.types, self.scratch);
        let refused = ShapeError::NotUnion {
            clause,
            at: node.source,
        };
        let operand = [DeclaredType::Type(clause)];
        let mut operands = BumpVec::with_capacity_in(arms.len() + 1, scratch);
        operands.push(DeclaredType::Type(clause));
        let mut defaulted = false;
        for arm in arms {
            match arm.arm().expect("an arm knows it is one").guard {
                Guard::Label(label) => {
                    let rule = Rule::Variant(label);
                    if judge::defined(self, types, scratch, rule, &operand).0 == Some(false) {
                        return Err(refused);
                    }
                    let variant = judge::ruled_one(self, types, scratch, rule, &operand);
                    operands.push(DeclaredType::Type(variant.unwrap_or(KType::ANY.into())));
                }
                Guard::Default => defaulted = true,
                Guard::Type(_) | Guard::Value(_) => {}
            }
        }
        let rule = if defaulted {
            Rule::Uncovered
        } else {
            Rule::Covered
        };
        if judge::defined(self, types, scratch, rule, &operands).0 == Some(false) {
            return Err(refused);
        }
        Ok(Named {
            clause,
            operands: operands.leak(),
        })
    }

    /// How the arm guarded by `guard` reads a scrutinee of the static type `scrutinee` — under a
    /// union clause where `under_union`, read as `clause` — among `arms`, beside what its `it` is
    /// re-stamped at where the load knows the guard's type.
    fn read_arm(
        &self,
        level: usize,
        guard: Guard<'graph>,
        scrutinee: Interval,
        under_union: bool,
        clause: Clause<'p>,
        arms: &[&'graph BodyShape<'graph>],
    ) -> (Read<'graph>, Option<Restamp>) {
        let (types, scratch) = (self.types, self.scratch);
        let retyped = |declared: Option<Parametric>| match declared {
            Some(declared) => (
                judge::retyped(self, types, scratch, declared),
                judge::restamp(self, types, scratch, declared),
            ),
            None => (unknown(), None),
        };
        let (admits, (it, retype)) = match guard {
            Guard::Type(quote) => {
                let admits = self.declared(level, quote);
                (admits, retyped(admits))
            }
            Guard::Value(scalar) => {
                let admits: Parametric = scalar_type(scalar).into();
                (Some(admits), (Interval::point(admits), None))
            }
            Guard::Label(label) => match clause {
                Clause::Closed(union) => {
                    let variant = variant_named(types, union, label);
                    let payload =
                        variant.and_then(|variant| representation(types, scratch, variant));
                    (
                        variant.map(Parametric::from),
                        retyped(payload.map(Parametric::from)),
                    )
                }
                // Over names, the variant and its payload a binding reads.
                Clause::Named(named) => {
                    let operand = [DeclaredType::Type(named.clause)];
                    let read = |rule| judge::ruled_one(self, types, scratch, rule, &operand);
                    (
                        read(Rule::Variant(label)),
                        retyped(read(Rule::Payload(label))),
                    )
                }
                Clause::Unread => (None, retyped(None)),
            },
            Guard::Default => {
                let it = match (under_union, clause) {
                    (false, _) => scrutinee,
                    (true, Clause::Closed(union)) => self.uncovered_payload(union, arms),
                    (true, Clause::Named(named)) => {
                        let operands = (Rule::Uncovered, named.operands);
                        let undecided = [KType::NEVER.into(), KType::ANY.into()];
                        match judge::ruled(self, types, scratch, operands, &undecided) {
                            Some([lower, upper]) => Interval {
                                lower: *lower,
                                upper: *upper,
                            },
                            _ => unknown(),
                        }
                    }
                    (true, Clause::Unread) => unknown(),
                };
                (None, (it, None))
            }
        };
        (Read { guard, admits, it }, retype)
    }

    /// The payload a `_` arm under the union `union` binds: at most the union of the
    /// representations of the variants no label among `arms` names.
    fn uncovered_payload(&self, union: KType, arms: &[&'graph BodyShape<'graph>]) -> Interval {
        let (types, scratch) = (self.types, self.scratch);
        let labels = |variant: KType| {
            let label = variant_label(types, variant);
            arms.iter().any(|arm| {
                matches!(arm.arm().map(|arm| arm.guard), Some(Guard::Label(named))
                    if Some(named) == label)
            })
        };
        let mut payloads = BumpVec::new_in(scratch);
        for variant in variants(types, union).into_iter().flatten() {
            if labels(variant) {
                continue;
            }
            match representation(types, scratch, variant) {
                Some(payload) => payloads.push(payload),
                None => return unknown(),
            }
        }
        under(types.union_of(scratch, &payloads).into())
    }

    /// Each arm's verdict against a scrutinee of the static type `scrutinee`: *never* where its
    /// guard can admit no value of it or the scrutinee's lower end lies outside the guard,
    /// *always* where every value of it is admitted — a guard its upper end lies under, `_` under
    /// `WITH`, and `_` under a union `clause` its upper end lies under — and *maybe* otherwise. A
    /// value guard never always admits.
    fn verdicts(
        &self,
        scrutinee: Interval,
        clause: Option<Parametric>,
        reads: &[Read<'graph>],
    ) -> BumpVec<'p, Verdict> {
        let (types, scratch) = (self.types, self.scratch);
        let judge = |admits: Parametric, may_always: bool| {
            if judge::never_satisfied(self, types, scratch, scrutinee, admits) {
                Verdict::Never
            } else if may_always && judge::fits(self, types, scratch, scrutinee.upper, admits) {
                Verdict::Always
            } else {
                Verdict::Maybe
            }
        };
        let mut verdicts = BumpVec::with_capacity_in(reads.len(), scratch);
        verdicts.extend(reads.iter().map(|read| match (read.guard, read.admits) {
            (Guard::Default, _) => match clause {
                Some(clause) => judge(clause, true),
                None => Verdict::Always,
            },
            (Guard::Value(_), Some(admits)) => judge(admits, false),
            (_, Some(admits)) => judge(admits, true),
            (_, None) => Verdict::Maybe,
        }));
        verdicts
    }

    /// The site of the arm `arm` in the shape at `level`.
    fn arm_site(&self, level: usize, arm: &BodyShape<'graph>) -> crate::scope::Site {
        let shape = self.chain[level].shape;
        (shape.nested_shapes().iter())
            .find_map(|(site, nested)| (nested.segment() == arm.segment()).then_some(*site))
            .expect("an arm is nested in its shape")
    }
}

/// What the load reads of a `MATCH … UNDER`'s union clause.
#[derive(Clone, Copy)]
enum Clause<'p> {
    /// A union the load closed.
    Closed(KType),
    /// A clause written over run-bound names, read by its questions.
    Named(Named<'p>),
    /// A clause the load does not read: none, or one it leaves to the run.
    Unread,
}

/// A union clause written over run-bound names ([`Pass::named`]): the clause, and the operands of
/// the rules over it, the clause beside each labelled variant's derived name.
#[derive(Clone, Copy)]
struct Named<'p> {
    clause: Parametric,
    operands: &'p [DeclaredType<Parametric>],
}

/// The type a value guard's scalar is.
fn scalar_type(scalar: Scalar<'_>) -> KType {
    match scalar {
        Scalar::Number(_) => KType::NUMBER,
        Scalar::Str(_) => KType::STR,
        Scalar::Bool(_) => KType::BOOL,
        Scalar::Null => KType::NULL,
    }
}

/// Whether the arm read `a` strictly outranks `b` ([`guard_outranks`]).
fn outranks(
    marks: &dyn Marks,
    types: &crate::values::Chain<'_, '_, '_, '_>,
    scratch: crate::memory::BumpAllocator<'_>,
    a: &Read<'_>,
    b: &Read<'_>,
) -> Option<bool> {
    let (Guard::Type(_), Guard::Type(_), Some(x), Some(y)) = (a.guard, b.guard, a.admits, b.admits)
    else {
        // No other pair ranks by its types.
        return Some(guard_outranks(
            types,
            scratch,
            (a.guard.kind(), None),
            (b.guard.kind(), None),
        ));
    };
    if let (Some(x), Some(y)) = (judge::static_of(types, x), judge::static_of(types, y)) {
        return Some(guard_outranks(
            types,
            scratch,
            (a.guard.kind(), Some(x)),
            (b.guard.kind(), Some(y)),
        ));
    }
    // A type guard outranks one it lies strictly under: the order both ways.
    match judge::under_decided(marks, types, scratch, x, y)? {
        false => Some(false),
        true => Some(!judge::under_decided(marks, types, scratch, y, x)?),
    }
}

/// The static types `typed` spanned — a `TRY`'s body's beside its arms': the plain union of their
/// upper ends, over the meet of their lower ends through the meet rule ([`judge::met`]), so it is
/// the same at load and under every binding.
pub(super) fn spanned(
    marks: &dyn Marks,
    types: &crate::values::Chain<'_, '_, '_, '_>,
    scratch: crate::memory::BumpAllocator<'_>,
    typed: &[Interval],
) -> Interval {
    let mut uppers = BumpVec::with_capacity_in(typed.len(), scratch);
    uppers.extend(typed.iter().map(|each| each.upper));
    let upper = types.union_of(scratch, &uppers);
    let never = Parametric::from(KType::NEVER);
    let mut lower = Parametric::from(KType::ANY);
    for each in typed {
        lower = judge::met(marks, types, scratch, lower, each.lower).unwrap_or(never);
    }
    Interval { lower, upper }
}
