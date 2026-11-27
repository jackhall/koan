//! The walk's rule for a `MATCH`: its static type, each arm's `it`, the union clause's variants and
//! which arms a run can take. A `TRY`'s arms are read here too, as a `MATCH … UNDER Error`'s
//! ([catching.rs](catching.rs)).
//!
//! Each arm's guard is read as a type: a type guard's recorded expression, a value guard's scalar
//! type, a label's variant of the union clause, and none for `_`. A clause's variants are those
//! its text shows, a family applied at a run-bound name's among them, each written over the names
//! its arguments read; the labels and coverage were checked at load
//! ([`arms_under`](crate::elaborate::arms_under)). Its `it` is the
//! scrutinee retyped to that type ([`retyped_to`]), as a parameter is retyped to its declared
//! type — a label's the variant's payload at the variant's representation — and `_`'s the
//! scrutinee as it is, or under `UNDER` a payload of a variant no label names. Each arm is typed as
//! an inline block entered with `it` at that interval, which the `MATCH` also keeps at the arm's
//! site for a walk rooted at the arm ([`entering_of`](super::Pass::entering_of)), beside how the
//! run re-stamps `it`, which a binding decides as its restamp question does
//! ([`SiteRule::ArmRestamp`]).
//!
//! The arms are then narrowed against the scrutinee's static type as
//! [static selection](super::super::select) narrows a keyworded use's candidates, reading both
//! ends of the scrutinee's static type: an arm whose guard can never admit, or that the
//! scrutinee's lower end lies outside, is dropped, not refused, since coverage forces arms a
//! narrow scrutinee never reaches; one a guard that always admits outranks is dropped too, as is
//! `_` beside such a guard. No arm left refuses the load, and so do arms that all always admit
//! with none ranking first. Where every arm left always admits, the one they rank first is
//! recorded as the arm the run takes. Where a binding decides a verdict or a rank, the asks this
//! choice reads are laid with it, the choice a binding makes decided by them
//! ([`SiteRule::ArmsRun`]).
//! How guarded arms rank — a value outranks every type, and a type another it lies strictly under
//! — is laid in a table where every guard is closed and the arms fit one, which the run reads
//! instead of ranking; past a table's bound, both rank by brute force ([`first_ranked`]).
//!
//! The `MATCH` is the static types of the arms that may run spanned, as every binding agrees which
//! do ([`ArmChoice::runs`]): the chosen arm's, or else the plain union of their upper ends over the meet
//! of their lower ends. Where a binding decides which run, each arm's type is a decided name on
//! [`Question::Arms`], so the `MATCH` is the same at load and under every binding.
//!
//! See [README.md § Static types](../README.md#static-types).

use crate::elaborate::judge::{self, Derives, Marks};
use crate::elaborate::{arm_shapes, variant_label, variant_named, variants};
use crate::memory::{BumpVec, Lays, collect, resident};
use crate::parse::KExpression;
use crate::scope::{
    ArmChoice, Arms, ArmsLayout, AskedBy, BodyShape, Elimination, Guard, GuardAsks, GuardKind,
    Payload, Question, Rule, Run, Scalar, ShapeError, SiteAnswer, SiteRule, Store, Table,
    first_ranked, guard_outranks,
};
use crate::symbols::Symbol;
use crate::type_lattice::{DeclaredType, Interval, KType, Parametric, Verdict};
use crate::values::{payload_of, under, unknown};

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
        let scrutinee = self.part(level, &node.parts[1].value)?;
        let clause = match under_union {
            true => self.declared(level, &node.parts[3].value),
            false => None,
        };
        self.armed(level, node, scrutinee, clause, under_union)
    }

    /// The arms of the `MATCH` or `TRY` `node` over a scrutinee of the static type `scrutinee`,
    /// under the union `clause` where `under_union`: each arm typed with its `it`, the arms
    /// narrowed, and the kept arms' types spanned.
    pub(super) fn armed(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        scrutinee: Interval,
        clause: Option<Parametric>,
        under_union: bool,
    ) -> Result<Interval, ShapeError<'o>> {
        let shape = self.chain[level].shape;
        let (types, scratch) = (self.types, self.scratch);
        let arms: BumpVec<'_, &'graph BodyShape<'graph>> = {
            let mut arms = BumpVec::new_in(scratch);
            arms.extend(arm_shapes(shape, node));
            arms
        };
        // A clause that shows no variants was refused at load.
        let clause_read = match clause.filter(|_| under_union) {
            Some(union) if variants(types, union).is_some() => Clause::Shown(union),
            Some(union) if self.shown_variants(union).is_some() => Clause::Declared(union),
            _ => Clause::Unread,
        };
        let mut reads = BumpVec::with_capacity_in(arms.len(), scratch);
        for arm in arms.iter() {
            let guard = arm.arm().expect("an arm knows it is one").guard;
            let (read, restamped) =
                self.read_arm(level, guard, scrutinee, under_union, clause_read, &arms);
            // The arm's site keeps its `it`'s interval and what the run re-stamps `it` at.
            let site = self.arm_site(level, arm);
            self.record_part(level, site, read.it);
            if let Some(declared) = restamped {
                if let Some(retype) = judge::restamp(self, types, scratch, declared) {
                    self.record_retype(level, site, retype);
                }
                let mut asks = BumpVec::with_capacity_in(1, scratch);
                self.lay_restamp(level, (site, SiteRule::ArmRestamp), &mut asks, declared);
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
        let refused = ShapeError::NoAdmittingArm {
            scrutinee: scrutinee.upper,
            union: clause_read.union(),
            at: node.source,
        };
        if kept.is_empty() {
            return Err(refused);
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
        // Past 64 arms, a bit each, a binding's choice is the run's.
        let choice = (reads.len() <= 64).then(|| self.arm_choice(scrutinee, clause, &reads));
        if let Some(choice) = choice {
            let taken = match chosen {
                Some(chosen) => SiteAnswer::Arm(chosen + 1),
                None => SiteAnswer::Kept {
                    kept: (kept.iter()).fold(0, |mask, (at, _)| mask | 1 << at),
                    always: (kept.iter())
                        .filter(|(_, verdict)| *verdict == Verdict::Always)
                        .fold(0, |mask, (at, _)| mask | 1 << at),
                    ranked: closed,
                },
            };
            self.lay_arms(level, node, (choice, scrutinee.upper), refused, taken);
        }
        let runs = |at: usize| match chosen {
            Some(chosen) if usize::from(chosen) == at => Run::Selected,
            Some(_) => Run::Never,
            None if kept.iter().any(|(kept, _)| usize::from(*kept) == at) => Run::May,
            None => Run::Never,
        };
        let mut walked = BumpVec::with_capacity_in(reads.len(), scratch);
        walked.extend((0..reads.len()).map(runs));
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
        let armed = self.arms_type(choice, &walked, &typed);
        Ok(self.arriving(&[scrutinee.upper], armed))
    }

    /// Which of the arms read as `reads` run over a scrutinee of the static type `scrutinee`
    /// under the union `clause`, as a value: the asks the walk's verdicts and ranks read, in the
    /// walk's order, each arm by where its asks sit ([`ArmChoice`]). At most 64 arms, a bit each.
    fn arm_choice(
        &self,
        scrutinee: Interval,
        clause: Option<Parametric>,
        reads: &[Read<'graph>],
    ) -> ArmChoice<'p> {
        let (scratch, count) = (self.scratch, reads.len());
        let mut asks = BumpVec::new_in(scratch);
        self.ask_arrives(&mut asks, &[scrutinee.upper]);
        let arriving = asks.len() as u16;
        let mut guards = BumpVec::with_capacity_in(count, scratch);
        let mut typed = BumpVec::with_capacity_in(count, scratch);
        for read in reads {
            let guard = read.guard.kind();
            let (admits, may_always) = match (read.guard, read.admits) {
                (Guard::Default, _) => (clause, true),
                (Guard::Value(_), admits) => (admits, false),
                (_, admits) => (admits, true),
            };
            let mut never = None;
            let mut always = None;
            if let Some(wanted) = admits {
                never = Some(asks.len() as u16);
                asks.push(Question::NeverSatisfies {
                    value: scrutinee,
                    wanted,
                });
                if may_always {
                    always = Some(asks.len() as u16);
                    asks.push(Question::Fits {
                        a: scrutinee.upper.into(),
                        b: wanted.into(),
                    });
                }
            }
            let rank = match (guard, read.admits) {
                (GuardKind::Type, Some(admits)) => {
                    typed.push(admits);
                    Some(typed.len() as u16 - 1)
                }
                _ => None,
            };
            guards.push(GuardAsks {
                guard,
                never,
                always,
                rank,
            });
        }
        let layout = ArmsLayout {
            count: count as u16,
            arriving,
            ranks: asks.len() as u16,
            ranked: typed.len() as u16,
        };
        for a in typed.iter() {
            for b in typed.iter().filter(|b| !std::ptr::eq(*b, a)) {
                asks.push(Question::Under { a: *a, b: *b });
            }
        }
        ArmChoice {
            asks: asks.leak(),
            layout,
            guards: guards.leak(),
        }
    }

    /// Lay `choice`, which arms of the `MATCH` or `TRY` `node` of the shape at `level` run over a
    /// scrutinee whose upper end is `scrutinee` ([`SiteRule::ArmsRun`]), beside `taken`, the
    /// walk's outcome, and the refusals of a binding that leaves it no arm, `refused`, or several
    /// ranked alike.
    fn lay_arms(
        &mut self,
        level: usize,
        node: &'graph KExpression<'graph>,
        (choice, scrutinee): (ArmChoice<'p>, Parametric),
        refused: ShapeError<'o>,
        taken: SiteAnswer,
    ) {
        let (count, guards) = (choice.guards.len(), choice.guards);
        let site = self.node_site(level, node);
        let arms = |pass: &mut Self| {
            let lay = pass.lay;
            lay.fill(count + 2 + count + 1, |at| match at.checked_sub(1) {
                None => Payload::Arms(choice.layout),
                Some(at) if at < count => Payload::Guard(guards[at]),
                Some(at) if at == count => Payload::Refusal(resident(lay, refused)),
                Some(at) => Payload::Refusal(resident(
                    lay,
                    ShapeError::AmbiguousArms {
                        count: at - count - 1,
                        scrutinee,
                        at: node.source,
                    },
                )),
            })
        };
        let site_rule = (site, SiteRule::ArmsRun);
        self.lay_taken(level, site_rule, choice.asks, arms, Some(taken));
    }

    /// The type of arms typed as `typed`: each arm's type on what it does as every binding agrees
    /// (`choice`'s [`runs`](ArmChoice::runs)) — closed where they do, else a decided name per arm
    /// on [`Question::Arms`] — the plain union of the upper ends of those that may run, over the
    /// meet of their lower ends, so it is the same at load and under every binding. Past 64 arms,
    /// with no choice, each does what the walk took, `walked`, which holds at every binding.
    fn arms_type(
        &self,
        choice: Option<ArmChoice<'p>>,
        walked: &[Run],
        typed: &[Interval],
    ) -> Interval {
        let (types, scratch) = (self.types, self.scratch);
        let (never, any) = (Parametric::from(KType::NEVER), Parametric::from(KType::ANY));
        let rigid = choice.map(|choice| {
            let mut rigid = BumpVec::with_capacity_in(choice.asks.len(), scratch);
            rigid.extend((choice.asks.iter()).map(|ask| judge::rigid(self, types, scratch, ask)));
            rigid
        });
        let (mut lower, mut uppers) = (any, BumpVec::with_capacity_in(typed.len(), scratch));
        for (at, each) in typed.iter().enumerate() {
            let run = match (choice, &rigid) {
                (Some(choice), Some(rigid)) => {
                    choice.runs(at as u16, &mut AskedBy(|ask: u16| rigid[usize::from(ask)]))
                }
                _ => Some(walked[at]),
            };
            let (low, up) = match (run, choice) {
                (Some(Run::Never), _) => (any, never),
                (Some(Run::May | Run::Selected), _) | (None, None) => (each.lower, each.upper),
                (None, Some(choice)) => {
                    let question = Question::Arms {
                        choice,
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

    /// How the arm guarded by `guard` reads a scrutinee of the static type `scrutinee` — under a
    /// union clause where `under_union`, read as `clause` — among `arms`, beside the type its `it`
    /// is re-stamped at where the load knows the guard's type.
    fn read_arm(
        &self,
        level: usize,
        guard: Guard<'graph>,
        scrutinee: Interval,
        under_union: bool,
        clause: Clause,
        arms: &[&'graph BodyShape<'graph>],
    ) -> (Read<'graph>, Option<Parametric>) {
        let (types, scratch) = (self.types, self.scratch);
        let retyped = |declared: Option<Parametric>| match declared {
            Some(declared) => (
                judge::retyped(self, types, scratch, declared),
                Some(declared),
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
                Clause::Shown(union) => {
                    let variant = variant_named(types, union, label);
                    let payload = variant.and_then(|variant| payload_of(types, scratch, variant));
                    (variant, retyped(payload))
                }
                // A union declared over names: the variant and its payload a binding seals.
                Clause::Declared(union) => {
                    let operand = [DeclaredType::Type(union)];
                    let read = |rule| {
                        let Ok(read) = judge::ruled_one(self, types, scratch, rule, &operand);
                        read
                    };
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
                    (true, Clause::Shown(union)) => self.uncovered_payload(union, arms),
                    (true, Clause::Declared(union)) => self.uncovered_declared(union, arms),
                    (true, Clause::Unread) => unknown(),
                };
                (None, (it, None))
            }
        };
        (Read { guard, admits, it }, retype)
    }

    /// The payload a `_` arm under the union `union` binds: at most the union of the
    /// representations of the variants no label among `arms` names.
    fn uncovered_payload(&self, union: Parametric, arms: &[&'graph BodyShape<'graph>]) -> Interval {
        let (types, scratch) = (self.types, self.scratch);
        let labels = |variant: Parametric| {
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
            match payload_of(types, scratch, variant) {
                Some(payload) => payloads.push(payload),
                None => return unknown(),
            }
        }
        under(types.union_of(scratch, &payloads))
    }

    /// The payload a `_` arm under `union`, a union declared over run-bound names, binds: the
    /// derived name a binding reads the variants no label among `arms` names through
    /// ([`Rule::Uncovered`]).
    fn uncovered_declared(
        &self,
        union: Parametric,
        arms: &[&'graph BodyShape<'graph>],
    ) -> Interval {
        let (types, scratch) = (self.types, self.scratch);
        let mut operands = BumpVec::with_capacity_in(arms.len() + 1, scratch);
        operands.push(DeclaredType::Type(union));
        for arm in arms {
            if let Some(Guard::Label(label)) = arm.arm().map(|arm| arm.guard) {
                let operand = [DeclaredType::Type(union)];
                let Ok(variant) =
                    judge::ruled_one(self, types, scratch, Rule::Variant(label), &operand);
                operands.push(DeclaredType::Type(variant.unwrap_or(KType::ANY.into())));
            }
        }
        let undecided = [KType::NEVER.into(), KType::ANY.into()];
        match judge::ruled(
            self,
            types,
            scratch,
            (Rule::Uncovered, &operands),
            &undecided,
        ) {
            Some([lower, upper]) => Interval {
                lower: *lower,
                upper: *upper,
            },
            _ => unknown(),
        }
    }

    /// The variants the `MATCH … UNDER` clause `clause` shows, each beside its label: a union's
    /// members, each written over the names it reads, or a union declared over run-bound names'
    /// variants, each the derived name a binding seals it at. `None` where it shows none, as a
    /// bare run-bound name does.
    pub(super) fn shown_variants(
        &self,
        clause: Parametric,
    ) -> Option<BumpVec<'p, (Option<Symbol>, Parametric)>> {
        let (types, scratch) = (self.types, self.scratch);
        if let Some(members) = variants(types, clause) {
            let mut shown = BumpVec::new_in(scratch);
            shown.extend(members.map(|member| (variant_label(types, member), member)));
            return Some(shown);
        }
        let declared = judge::declared(self, types, clause)?;
        let mut shown = BumpVec::new_in(scratch);
        for label in declared.variants()? {
            let operand = [DeclaredType::Type(clause)];
            let Ok(variant) =
                judge::ruled_one(self, types, scratch, Rule::Variant(label), &operand);
            shown.push((Some(label), variant.unwrap_or(KType::ANY.into())));
        }
        Some(shown)
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
enum Clause {
    /// A union, its variants those its text shows, each written over the names it reads.
    Shown(Parametric),
    /// A union declared over run-bound names, its variants its declaration's.
    Declared(Parametric),
    /// A clause the load does not read: none, or one past a quote's chain.
    Unread,
}

impl Clause {
    /// The union clause a refusal names.
    fn union(self) -> Option<Parametric> {
        match self {
            Clause::Shown(union) | Clause::Declared(union) => Some(union),
            Clause::Unread => None,
        }
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
pub(super) fn spanned<U>(
    marks: &dyn Derives<Unwalked = U>,
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
