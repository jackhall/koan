//! `admits_with` — the walk that admits a carried type into a declared position and, where the
//! position quantifies, collects what would solve it.
//!
//! Instead of binding a variable to the first argument it meets, the walk records **every**
//! argument type that reaches the variable as a lower contribution (covariant position) or an
//! upper one (contravariant). [`Collector::solve`] then takes, per variable, the maximum of the
//! lower contributions, else the minimum of the upper ones, else the declared bound. A contribution
//! set with no maximum is a failure, not a join: the solver never mints a union nobody wrote, and
//! admission cannot depend on the order the slots are read.
//!
//! So `(f _ :Elt _ :Elt)` admits `(1, 2)` with `Elt = Number` and `(1, (1 | "x"))` with
//! `Elt = (Number | Str)`, and rejects `(1, "x")`. A caller who wants mixed arguments writes the
//! union in the slot type or in the bound.

use super::handle::KType;
use super::node::TypeNode;
use super::order::is_subtype_of;
use super::registry::TypeRegistry;
use super::walk::Variance;
use super::walk::binary::{Arm, Lockstep, lockstep};

/// Why a carried type does not fill a declared position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnifyFailure {
    /// The two types disagree structurally, or a leaf position is not satisfied — the ordinary type
    /// mismatch.
    Mismatch,
    /// The lower contributions to a variable have no maximum among them. Since [`join`] is
    /// subsumption-or-union, "no maximum" and "the join is a union none of them wrote" are the same
    /// condition.
    ///
    /// [`join`]: super::lattice::join
    NoMaximum {
        index: usize,
        contributions: Vec<KType>,
    },
    /// The upper contributions to a variable have no minimum among them — the dual, which is what
    /// forbids an anonymous record or function.
    NoMinimum {
        index: usize,
        contributions: Vec<KType>,
    },
    /// A variable's lower solution does not lie under its upper one.
    Disagree {
        index: usize,
        lower: KType,
        upper: KType,
    },
}

/// What a quantified position's arguments contributed, per variable.
///
/// Cells grow on demand, so a walk that does not know the enclosing group's arity up front can
/// still collect; [`new`](Collector::new) takes the arity a call knows, so a variable no argument
/// reached is visible as bound-only.
#[derive(Clone, Debug, Default)]
pub struct Collector {
    lower: Vec<Vec<KType>>,
    upper: Vec<Vec<KType>>,
    /// Each variable's declared bound, recorded off the `Quantified` node when a contribution
    /// reached it.
    bounds: Vec<KType>,
}

impl Collector {
    /// One empty cell per quantifier — what a call collects its arguments into.
    pub fn new(arity: usize) -> Self {
        Collector {
            lower: vec![Vec::new(); arity],
            upper: vec![Vec::new(); arity],
            bounds: vec![KType::ANY; arity],
        }
    }

    /// Record that `carried` reached the `index`-th variable at `variance`.
    fn contribute(&mut self, index: usize, bound: KType, carried: KType, variance: Variance) {
        if self.lower.len() <= index {
            self.lower.resize(index + 1, Vec::new());
            self.upper.resize(index + 1, Vec::new());
            self.bounds.resize(index + 1, KType::ANY);
        }
        self.bounds[index] = bound;
        let cell = match variance {
            Variance::Co => &mut self.lower[index],
            Variance::Contra => &mut self.upper[index],
        };
        if !cell.contains(&carried) {
            cell.push(carried);
        }
    }

    /// How many variables this collector holds cells for.
    pub fn arity(&self) -> usize {
        self.bounds.len()
    }

    /// The lower and upper contributions to the `index`-th variable, in arrival order — what
    /// reached it at a covariant position and what reached it at a contravariant one. Empty slices
    /// for an index no argument reached.
    pub fn contributions(&self, index: usize) -> (&[KType], &[KType]) {
        fn cell(cells: &[Vec<KType>], index: usize) -> &[KType] {
            cells.get(index).map_or(&[], Vec::as_slice)
        }
        (cell(&self.lower, index), cell(&self.upper, index))
    }

    /// The bound recorded for the `index`-th variable, or [`KType::ANY`] for an index no
    /// contribution reached.
    pub fn bound(&self, index: usize) -> KType {
        self.bounds.get(index).copied().unwrap_or(KType::ANY)
    }

    /// The solution, in canonical quantifier order: per variable the maximum of its lower
    /// contributions, else the minimum of its upper ones, else its declared bound — checked to lie
    /// under both the upper side and the bound.
    ///
    /// Every solution is a contribution or a bound. Nothing here builds a type.
    pub fn solve(&self, types: &TypeRegistry) -> Result<Vec<KType>, UnifyFailure> {
        let mut solution = Vec::with_capacity(self.bounds.len());
        for index in 0..self.bounds.len() {
            let bound = self.bounds[index];
            let lower = extremum(types, &self.lower[index], Bound::Maximum).ok_or_else(|| {
                UnifyFailure::NoMaximum {
                    index,
                    contributions: self.lower[index].clone(),
                }
            })?;
            let upper = extremum(types, &self.upper[index], Bound::Minimum).ok_or_else(|| {
                UnifyFailure::NoMinimum {
                    index,
                    contributions: self.upper[index].clone(),
                }
            })?;
            if let (Some(lower), Some(upper)) = (lower, upper)
                && !is_subtype_of(types, lower, upper)
            {
                return Err(UnifyFailure::Disagree {
                    index,
                    lower,
                    upper,
                });
            }
            let solved = lower.or(upper).unwrap_or(bound);
            if !is_subtype_of(types, solved, bound) {
                return Err(UnifyFailure::Mismatch);
            }
            solution.push(solved);
        }
        Ok(solution)
    }
}

/// Which end of a contribution set is being taken.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bound {
    Maximum,
    Minimum,
}

/// The one member of `contributions` every other member lies under (or over). `Ok(None)` for an
/// empty set, `Err`-worthy `None` when the set has no such member.
fn extremum(types: &TypeRegistry, contributions: &[KType], end: Bound) -> Option<Option<KType>> {
    if contributions.is_empty() {
        return Some(None);
    }
    contributions
        .iter()
        .find(|candidate| {
            contributions.iter().all(|other| match end {
                Bound::Maximum => is_subtype_of(types, *other, **candidate),
                Bound::Minimum => is_subtype_of(types, **candidate, *other),
            })
        })
        .map(|found| Some(*found))
}

/// Does `carried` fill the position `declared`, and what does it contribute to the variables there?
///
/// A declared type holding no free quantifier answers in one step through the ordinary order, which
/// is what keeps every unquantified slot off this walk entirely.
pub fn admits_with(
    types: &TypeRegistry,
    declared: KType,
    carried: KType,
    variance: Variance,
    collector: &mut Collector,
) -> Result<(), UnifyFailure> {
    let mut rules = Admits {
        collector: std::mem::take(collector),
    };
    let outcome = lockstep(types, declared, carried, variance, &mut rules);
    *collector = rules.collector;
    outcome
}

/// The declared members of a union in the order they are tried against one carried member: an exact
/// match first, then the members with nothing to solve, then the rest.
///
/// Binding is the last resort. A member that admits without touching a variable makes the stronger
/// claim, and trying a free variable first would let it swallow a member that matches exactly —
/// which is what would make `(Elt | Number)` fail to admit itself, since `Elt` would take the
/// `Number` contribution and leave the variable with two contributions and no maximum.
fn most_determined_first(
    types: &TypeRegistry,
    declared: &[KType],
    carried: KType,
) -> smallvec::SmallVec<[KType; 8]> {
    let mut order: smallvec::SmallVec<[KType; 8]> = smallvec::SmallVec::new();
    if declared.contains(&carried) {
        order.push(carried);
    }
    for member in declared {
        if *member != carried && !types.contains_quantified(*member) {
            order.push(*member);
        }
    }
    for member in declared {
        if *member != carried && types.contains_quantified(*member) {
            order.push(*member);
        }
    }
    order
}

/// The collecting [`Lockstep`] instance. It owns its collector so a declared-side union can try
/// each member on a clone and keep the first that admits.
struct Admits {
    collector: Collector,
}

type Admission = Result<(), UnifyFailure>;

impl Lockstep for Admits {
    type Out = Admission;

    fn enter(
        &mut self,
        types: &TypeRegistry,
        declared: KType,
        carried: KType,
        v: Variance,
    ) -> Option<Admission> {
        if !types.contains_quantified(declared) {
            let admits = match v {
                Variance::Co => is_subtype_of(types, carried, declared),
                Variance::Contra => is_subtype_of(types, declared, carried),
            };
            return Some(admits.then_some(()).ok_or(UnifyFailure::Mismatch));
        }
        let variable = types.with_node(declared, |node| match node {
            TypeNode::Quantified { index, bound } => Some((*index, *bound)),
            _ => None,
        });
        variable.map(|(index, bound)| {
            self.collector.contribute(index, bound, carried, v);
            Ok(())
        })
    }

    fn leaf(
        &mut self,
        types: &TypeRegistry,
        _declared: KType,
        carried: KType,
        v: Variance,
    ) -> Admission {
        // A deferred FN return is a per-call-elaborated placeholder: it admits nothing on its own,
        // and a return position carrying one has nothing yet to disagree with.
        let deferred = types.with_node(carried, |node| matches!(node, TypeNode::DeferredReturn(_)));
        if deferred && v == Variance::Co {
            return Ok(());
        }
        Err(UnifyFailure::Mismatch)
    }

    fn set_wise(
        &mut self,
        types: &TypeRegistry,
        declared: &[KType],
        carried: &[KType],
        v: Variance,
        recurse: &mut dyn FnMut(&mut Self, KType, KType, Variance) -> Admission,
    ) -> Admission {
        // Every carried member must be admitted by some declared member. A non-union side arrives
        // as a one-element slice, so this covers a union on either side and on both. The
        // declared-side choice is made on a clone, so a rejected member leaves no contribution
        // behind; contributions from every carried member accumulate in the one collector.
        for one in carried {
            let mut admitted = false;
            for option in most_determined_first(types, declared, *one) {
                let saved = self.collector.clone();
                match recurse(self, option, *one, v) {
                    Ok(()) => {
                        admitted = true;
                        break;
                    }
                    Err(_) => self.collector = saved,
                }
            }
            if !admitted {
                return Err(UnifyFailure::Mismatch);
            }
        }
        Ok(())
    }

    fn structural(
        &mut self,
        _types: &TypeRegistry,
        paired: &[Admission],
        arm: Arm<'_>,
    ) -> Admission {
        if let Some(failure) = paired.iter().find(|out| out.is_err()) {
            return failure.clone();
        }
        // The width verdict reads `a ≤ b`; a covariant position asks whether the *carried* side
        // lies under the declared one, so its leftovers go in the other order.
        let permitted = match arm.variance {
            Variance::Co => arm.width.permits(arm.only_b, arm.only_a),
            Variance::Contra => arm.width.permits(arm.only_a, arm.only_b),
        };
        permitted.then_some(()).ok_or(UnifyFailure::Mismatch)
    }

    fn short_circuits(&self, out: &Admission) -> bool {
        out.is_err()
    }
}
