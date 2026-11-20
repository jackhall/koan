//! The **wake memo**: how a record stands under a binding, answered once per frontier. A
//! frontier's unknowns are its own and each read's past its cut, transitively
//! ([the store](README.md#the-store)), and frontiers share structure, so the memo keeps, per
//! frontier by identity, the latest segment among the marks of each sort it reaches. A read's cut
//! keeps the unknowns at segments at least its own, so a read reaches a mark where that segment
//! lies past its cut. Frontiers read only ended ones, so they form a DAG; a chain of k reads is k
//! deep, so the post-order keeps its own stack, as the folds' summary does.
//!
//! Each mark on a **checked** name — one the walk binds closed and judges the marks of — counts
//! **open** where it is undecided, or answered by a question the binding does not keep. Each on a
//! **witnessed** name — one a witnessing walk leaves rigid and asks its doors about — counts open
//! likewise, and **holds** where it is undecided and of a kind that holds
//! ([`OpenKind::holds`](crate::scope::OpenKind::holds)). A record **re-runs** where it reaches an
//! open mark on a checked name, or one on a witnessed name and no holding one: a held record
//! stands rigid, for the closing walk to re-run. Else it is read **closed** where it reaches any
//! mark on a name the chain's binding binds, and stands untouched otherwise ([`Wake`]).

use std::cell::Cell;

use crate::memory::{BumpAllocator, BumpBackedMap, BumpVec, bump_table};
use crate::scope::{Answered, Binding, Frontier, Mark, Segment, Wake};

/// The latest segment among the marks of each sort a frontier reaches: `None` where it reaches
/// none of that sort.
#[derive(Clone, Copy, Default)]
struct Reach {
    /// An open mark on a checked name.
    open: Option<Segment>,
    /// An open mark on a witnessed name.
    witnessed: Option<Segment>,
    /// An undecided mark of a holding kind on a witnessed name.
    holds: Option<Segment>,
    /// Any mark on a name the chain's binding binds.
    chain: Option<Segment>,
}

/// How a frontier stands under a walk's checked and witnessed names over a chain's binding,
/// memoized per frontier.
pub struct Wakes<'o, 'p> {
    /// The names the walk binds closed and judges the marks of.
    checked: Binding<'o>,
    /// The names a witnessing walk leaves rigid: empty in a closing walk.
    witness: Binding<'o>,
    /// Everything the walk binds closed: its base's and the checked names.
    chain: Binding<'o>,
    scratch: BumpAllocator<'p>,
    /// What each frontier reaches, by identity. Taken out while an ask fills it.
    latest: Cell<BumpBackedMap<'p, (usize, usize), Reach>>,
    /// Whether the binding keeps each answered question, by where it rests.
    kept: Cell<BumpBackedMap<'p, usize, bool>>,
}

impl<'o, 'p> Wakes<'o, 'p> {
    /// The memo of how records stand under `checked` and `witness` over `chain`, kept in
    /// `scratch`. `chain` binds every name `checked` does, and none `witness` does.
    pub fn new(
        checked: Binding<'o>,
        witness: Binding<'o>,
        chain: Binding<'o>,
        scratch: BumpAllocator<'p>,
    ) -> Self {
        debug_assert!(
            (witness.entries().iter()).all(|(unknown, _)| chain.bound(*unknown).is_none()),
            "a witnessed name is left unbound"
        );
        Wakes {
            checked,
            witness,
            chain,
            scratch,
            latest: Cell::new(bump_table(scratch)),
            kept: Cell::new(bump_table(scratch)),
        }
    }

    /// How `frontier` stands, where `keeps` says whether the binding keeps an answered question:
    /// re-run, read closed, or untouched.
    pub fn wakes(&self, frontier: Frontier<'_>, keeps: &dyn Fn(&Answered<'_>) -> bool) -> Wake {
        let reach = self.latest(frontier, keeps);
        let witnessed = reach.witnessed.is_some() && reach.holds.is_none();
        if reach.open.is_some() || witnessed {
            Wake::Open
        } else if reach.chain.is_some() {
            Wake::Named
        } else {
            Wake::Untouched
        }
    }

    /// Whether `frontier` reaches an open mark on a checked name.
    pub fn checks_open(
        &self,
        frontier: Frontier<'_>,
        keeps: &dyn Fn(&Answered<'_>) -> bool,
    ) -> bool {
        self.latest(frontier, keeps).open.is_some()
    }

    /// Whether the walk checks `unknown`.
    #[cfg(test)]
    pub fn checks(&self, unknown: crate::scope::Unknown) -> bool {
        self.checked.bound(unknown).is_some()
    }

    /// Whether `keeps` keeps `answered`, asked once per question.
    fn kept(&self, answered: &Answered<'_>, keeps: &dyn Fn(&Answered<'_>) -> bool) -> bool {
        let at = std::ptr::from_ref(answered).addr();
        let mut kept = self.kept.replace(bump_table(self.scratch));
        let held = *kept.entry(at).or_insert_with(|| keeps(answered));
        self.kept.set(kept);
        held
    }

    /// What `frontier` reaches.
    fn latest(&self, frontier: Frontier<'_>, keeps: &dyn Fn(&Answered<'_>) -> bool) -> Reach {
        if frontier.is_empty() || (self.chain.is_empty() && self.witness.is_empty()) {
            return Reach::default();
        }
        let mut latest = self.latest.replace(bump_table(self.scratch));
        let mut stack = BumpVec::new_in(self.scratch);
        stack.push((frontier, false));
        while let Some((at, expanded)) = stack.pop() {
            if latest.contains_key(&at.identity()) {
                continue;
            }
            if !expanded {
                stack.push((at, true));
                stack.extend((at.reads().iter()).map(|cut| (cut.frontier, false)));
                continue;
            }
            let mut reach = Reach::default();
            for noted in at.own() {
                let segment = Some(noted.unknown.segment);
                let (checked, witnessed) = (
                    self.checked.bound(noted.unknown).is_some(),
                    self.witness.bound(noted.unknown).is_some(),
                );
                if let Mark::Open(open) = noted.mark
                    && (checked || witnessed)
                {
                    let undecided = open.answered.is_none();
                    let unkept =
                        (open.answered).is_some_and(|answered| !self.kept(answered, keeps));
                    if undecided || unkept {
                        match checked {
                            true => reach.open = reach.open.max(segment),
                            false => reach.witnessed = reach.witnessed.max(segment),
                        }
                    }
                    if witnessed && undecided && open.kind.holds() {
                        reach.holds = reach.holds.max(segment);
                    }
                }
                if self.chain.bound(noted.unknown).is_some() {
                    reach.chain = reach.chain.max(segment);
                }
            }
            for cut in at.reads() {
                let read = latest[&cut.frontier.identity()];
                let past = |segment: Option<Segment>| segment.filter(|at| *at >= cut.from);
                reach.open = reach.open.max(past(read.open));
                reach.witnessed = reach.witnessed.max(past(read.witnessed));
                reach.holds = reach.holds.max(past(read.holds));
                reach.chain = reach.chain.max(past(read.chain));
            }
            latest.insert(at.identity(), reach);
        }
        let found = latest[&frontier.identity()];
        self.latest.set(latest);
        found
    }
}
