//! The walk's rule for a `MATCH`: its static type, each arm's `it`, the union clause's variants and
//! which arms a run can take.
//!
//! Each arm's guard is read as a type: a type guard's recorded expression, a value guard's scalar
//! type, a label's variant of the closed union clause, and none for `_`. Its `it` is the
//! scrutinee retyped to that type ([`retyped_to`]), as a parameter is retyped to its declared
//! type — a label's the variant's payload at the variant's representation — and `_`'s the
//! scrutinee as it is, or under `UNDER` a payload of a variant no label names. Each arm is typed as
//! an inline block entered with `it` at that interval, which the `MATCH` also keeps at the arm's
//! site for a walk rooted at the arm ([`entering_of`](super::Pass::entering_of)).
//!
//! The arms are then narrowed against the scrutinee's static type as
//! [static selection](super::super::select) narrows a keyworded use's candidates: an arm whose
//! guard can never admit is dropped, not refused, since coverage forces arms a narrow scrutinee
//! never reaches; one a guard that always admits outranks is dropped too, as is `_` beside such a
//! guard. No arm left refuses the load, and so do arms that all always admit with none ranking
//! first. Where every arm left always admits, the one they rank first is recorded as the arm the
//! run takes.
//! How guarded arms rank — a value outranks every type, and a type another it lies strictly under
//! — is laid in a table where every guard is closed and the arms fit one, which the run reads
//! instead of ranking; past a table's bound, both rank by brute force ([`first_ranked`]).
//!
//! The `MATCH` is its kept arms' static types spanned: the chosen arm's, or else the plain union
//! of their upper ends over the lattice meet of their lower ends where each is closed.
//!
//! See [README.md § Static types](../README.md#static-types).

use crate::elaborate::{arm_shapes, variant_label, variant_named, variants};
use crate::memory::{BumpVec, collect};
use crate::parse::KExpression;
use crate::scope::{Arms, BodyShape, Elimination, Guard, Scalar, ShapeError, Store, Table};
use crate::type_lattice::{Interval, KType, Parametric, Verdict, bound_above, fits, meet};
use crate::values::{Retype, representation, retyped_to, under, unknown};

use super::super::select::{first_ranked, guard_outranks};
use super::{Entering, Pass, never_satisfies};

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
        let shape = self.chain[level].shape;
        let (types, scratch) = (self.types, self.scratch);
        let scrutinee = self.part(level, &node.parts[1].value)?;
        let clause = match under_union {
            true => self.declared(level, &node.parts[3].value),
            false => None,
        };
        let union = clause
            .and_then(|clause| types.concrete(clause))
            .filter(|union| variants(types, *union).is_some());
        let arms: BumpVec<'_, &'graph BodyShape<'graph>> = {
            let mut arms = BumpVec::new_in(scratch);
            arms.extend(arm_shapes(shape, node));
            arms
        };
        let mut reads = BumpVec::with_capacity_in(arms.len(), scratch);
        for arm in arms.iter() {
            let guard = arm.arm().expect("an arm knows it is one").guard;
            let (read, retype) = self.read_arm(level, guard, scrutinee, under_union, union, &arms);
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
        if scrutinee.upper == KType::NEVER.into() {
            return Ok(Interval::point(KType::NEVER.into()));
        }
        let verdicts = self.verdicts(scrutinee, clause, &reads);
        let closed = (reads.iter()).all(|read| {
            matches!(read.guard, Guard::Default)
                || read
                    .admits
                    .and_then(|admits| types.concrete(admits))
                    .is_some()
        });
        let outranks = |a: usize, b: usize| outranks(types, scratch, &reads[a], &reads[b]);
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
        let scrutinee_named = bound_above(types, scratch, scrutinee.upper);
        if kept.is_empty() {
            return Err(ShapeError::NoAdmittingArm {
                scrutinee: scrutinee_named,
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
                            scrutinee: scrutinee_named,
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
        Ok(match chosen {
            Some(chosen) => typed[chosen as usize],
            None => {
                let mut kept_types = BumpVec::with_capacity_in(kept.len(), scratch);
                kept_types.extend(kept.iter().map(|(at, _)| typed[*at as usize]));
                spanned(types, scratch, &kept_types)
            }
        })
    }

    /// How the arm guarded by `guard` reads a scrutinee of the static type `scrutinee` — under a
    /// union clause where `under_union`, `union` where the load closed it — among `arms`, beside
    /// what its `it` is re-stamped at where the load closed the guard's type.
    fn read_arm(
        &self,
        level: usize,
        guard: Guard<'graph>,
        scrutinee: Interval,
        under_union: bool,
        union: Option<KType>,
        arms: &[&'graph BodyShape<'graph>],
    ) -> (Read<'graph>, Option<Retype>) {
        let (types, scratch) = (self.types, self.scratch);
        let retyped = |declared: Option<Parametric>| match declared {
            Some(declared) => (
                retyped_to(types, scratch, declared),
                types
                    .concrete(declared)
                    .map(|closed| Retype::of(types, scratch, closed)),
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
            Guard::Label(label) => {
                let variant = union.and_then(|union| variant_named(types, union, label));
                let payload = variant.and_then(|variant| representation(types, scratch, variant));
                (
                    variant.map(Parametric::from),
                    retyped(payload.map(Parametric::from)),
                )
            }
            Guard::Default => {
                let it = match (under_union, union) {
                    (false, _) => scrutinee,
                    (true, Some(union)) => self.uncovered_payload(union, arms),
                    (true, None) => unknown(),
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
    /// guard can admit no value of it, *always* where every value of it is admitted — a guard its
    /// upper end lies under, `_` under `WITH`, and `_` under a union `clause` its upper end lies
    /// under — and *maybe* otherwise. A value guard never always admits.
    fn verdicts(
        &self,
        scrutinee: Interval,
        clause: Option<Parametric>,
        reads: &[Read<'graph>],
    ) -> BumpVec<'p, Verdict> {
        let (types, scratch) = (self.types, self.scratch);
        let judge = |admits: Parametric, may_always: bool| {
            if never_satisfies(types, scratch, scrutinee.upper, admits).is_some() {
                Verdict::Never
            } else if may_always && fits(types, scratch, scrutinee.upper, admits) {
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
    types: &crate::values::Chain<'_, '_, '_, '_>,
    scratch: crate::memory::BumpAllocator<'_>,
    a: &Read<'_>,
    b: &Read<'_>,
) -> bool {
    let closed = |read: &Read<'_>| read.admits.and_then(|admits| types.concrete(admits));
    guard_outranks(types, scratch, (a.guard, closed(a)), (b.guard, closed(b)))
}

/// The static types `typed` of a `MATCH`'s kept arms spanned: the plain union of their upper ends,
/// over the lattice meet of their lower ends where every one is closed and `Never` otherwise.
fn spanned(
    types: &crate::values::Chain<'_, '_, '_, '_>,
    scratch: crate::memory::BumpAllocator<'_>,
    typed: &[Interval],
) -> Interval {
    let mut uppers = BumpVec::with_capacity_in(typed.len(), scratch);
    uppers.extend(typed.iter().map(|each| each.upper));
    let upper = types.union_of(scratch, &uppers);
    let mut lower = Some(KType::ANY);
    for each in typed {
        lower = lower
            .zip(types.concrete(each.lower))
            .map(|(met, lower)| meet(types, scratch, met, lower));
    }
    Interval {
        lower: lower.map_or(KType::NEVER.into(), Parametric::from),
        upper,
    }
}
