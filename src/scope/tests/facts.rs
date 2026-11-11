//! The fact vocabulary: a frontier's order, its unknowns through the frontiers it read, and when a
//! binding wakes it.

use crate::memory::{Bump, BumpVec, Lays, program_storage};
use crate::type_lattice::KType;

use super::super::{Address, Binding, Bound, Cut, Frontier, Noted, Segment, Slot, Unknown};

// `scope` holds no heap outside its tests, so the flattening every test reads lives here.
impl<'a> Frontier<'a> {
    /// Its unknowns, flattened: its own and each read's past its cut, sorted and deduplicated.
    pub fn unknowns(self) -> Vec<Unknown> {
        // A chain of k reads is k deep, so the walk keeps its own stack. Each frontier is walked
        // again only at a wider cut than any it was walked at.
        let mut widest = std::collections::HashMap::<(usize, usize), Segment>::new();
        let (mut stack, mut unknowns) = (vec![(self, Segment(0))], Vec::new());
        while let Some((frontier, from)) = stack.pop() {
            match widest.get(&frontier.identity()) {
                Some(walked) if *walked <= from => continue,
                _ => _ = widest.insert(frontier.identity(), from),
            }
            unknowns.extend(
                frontier
                    .own_unknowns()
                    .filter(|unknown| unknown.segment >= from),
            );
            stack.extend((frontier.reads().iter()).map(|cut| (cut.frontier, cut.from.max(from))));
        }
        unknowns.sort_unstable();
        unknowns.dedup();
        unknowns
    }

    /// Its marks, flattened as [`unknowns`](Self::unknowns) flattens its unknowns.
    pub fn marks(self) -> Vec<Noted<'a>> {
        let mut widest = std::collections::HashMap::<(usize, usize), Segment>::new();
        let (mut stack, mut marks) = (vec![(self, Segment(0))], Vec::new());
        while let Some((frontier, from)) = stack.pop() {
            match widest.get(&frontier.identity()) {
                Some(walked) if *walked <= from => continue,
                _ => _ = widest.insert(frontier.identity(), from),
            }
            marks.extend((frontier.own().iter()).filter(|noted| noted.unknown.segment >= from));
            stack.extend((frontier.reads().iter()).map(|cut| (cut.frontier, cut.from.max(from))));
        }
        marks.dedup();
        marks
    }

    /// Whether `binding` binds some unknown here open: whether it re-runs the unit.
    pub fn meets_open(self, binding: Binding<'_>) -> bool {
        (self.marks().iter()).any(|noted| noted.is_open() && binding.bound(noted.unknown).is_some())
    }

    /// How the record stands, its marks flattened: it re-runs where a mark on a `checked` name
    /// is undecided or unkept, or one on a `witness` name is and none there is undecided of a
    /// holding kind; else it is read closed where it names what `chain` binds.
    pub fn wakes_flat(
        self,
        checked: Binding<'_>,
        witness: Binding<'_>,
        chain: Binding<'_>,
        keeps: &dyn Fn(&crate::scope::Answered<'_>) -> bool,
    ) -> crate::scope::Wake {
        let marks = self.marks();
        let open = |names: Binding<'_>| {
            (marks.iter()).any(|noted| match noted.mark {
                crate::scope::Mark::Open(open) => {
                    names.bound(noted.unknown).is_some()
                        && open.answered.is_none_or(|answered| !keeps(answered))
                }
                crate::scope::Mark::Named => false,
            })
        };
        let holds = (marks.iter()).any(|noted| match noted.mark {
            crate::scope::Mark::Open(open) => {
                witness.bound(noted.unknown).is_some()
                    && open.answered.is_none()
                    && open.kind.holds()
            }
            crate::scope::Mark::Named => false,
        });
        if open(checked) || (open(witness) && !holds) {
            crate::scope::Wake::Open
        } else if self.meets_binding(chain) {
            crate::scope::Wake::Named
        } else {
            crate::scope::Wake::Untouched
        }
    }

    /// Whether `binding` binds some unknown here: whether it wakes the unit's record.
    pub fn meets_binding(self, binding: Binding<'_>) -> bool {
        let unknowns = self.unknowns();
        (binding.entries().iter()).any(|(unknown, _)| unknowns.binary_search(unknown).is_ok())
    }
}

fn unknown(segment: u32, slot: u32) -> Unknown {
    Unknown {
        segment: Segment(segment),
        at: Address::Slot(Slot(slot)),
    }
}

/// `own` beside `reads`, laid through `lay`.
fn frontier<'a>(
    lay: impl Lays<'a>,
    scratch: &Bump,
    own: &[Unknown],
    reads: &[(Frontier<'a>, u32)],
) -> Frontier<'a> {
    let mut staged = BumpVec::new_in(scratch);
    staged.extend(own.iter().copied().map(Noted::named));
    let mut cuts = BumpVec::new_in(scratch);
    cuts.extend((reads.iter()).map(|(frontier, from)| Cut {
        frontier: *frontier,
        from: Segment(*from),
    }));
    Frontier::laid(lay, &mut staged, &cuts)
}

#[test]
fn a_frontier_lays_its_own_unknowns_sorted_and_each_read_once_at_its_widest_cut() {
    let storage = program_storage();
    let writer = storage.brand().writer();
    let scratch = Bump::new();
    let read = frontier(writer, &scratch, &[unknown(4, 0)], &[]);
    let other = frontier(writer, &scratch, &[unknown(5, 0)], &[]);
    let laid = frontier(
        writer,
        &scratch,
        &[unknown(2, 0), unknown(1, 3), unknown(2, 0), unknown(1, 1)],
        &[(read, 3), (Frontier::EMPTY, 0), (other, 0), (read, 1)],
    );
    assert_eq!(
        laid.own_unknowns().collect::<Vec<_>>(),
        [unknown(1, 1), unknown(1, 3), unknown(2, 0)]
    );
    // In the order first read, whatever the address each read frontier is laid at.
    let reads: Vec<_> = (laid.reads().iter())
        .map(|cut| (cut.frontier.identity(), cut.from))
        .collect();
    assert_eq!(
        reads,
        [
            (read.identity(), Segment(1)),
            (other.identity(), Segment(0))
        ]
    );
    assert_eq!(laid.top(), Some(Segment(5)));
}

/// A read whose cut keeps none of its frontier's unknowns is never laid, so a frontier is empty
/// exactly where it holds no unknown.
#[test]
fn a_frontier_reading_only_past_its_cuts_is_empty() {
    let storage = program_storage();
    let writer = storage.brand().writer();
    let scratch = Bump::new();
    let read = frontier(writer, &scratch, &[unknown(1, 0), unknown(4, 0)], &[]);
    let narrowed = frontier(writer, &scratch, &[], &[(read, 2)]);
    assert_eq!(narrowed.unknowns(), [unknown(4, 0)]);
    // The cut at 5 keeps nothing of `read`, nor of `narrowed`, whose latest unknown is at 4.
    let past = frontier(writer, &scratch, &[], &[(read, 5), (narrowed, 5)]);
    assert!(past.is_empty() && past.unknowns().is_empty() && past.top().is_none());
}

#[test]
fn a_frontier_s_unknowns_are_its_own_and_each_read_s_past_its_cut() {
    let storage = program_storage();
    let writer = storage.brand().writer();
    let scratch = Bump::new();
    let a = frontier(
        writer,
        &scratch,
        &[unknown(1, 0), unknown(4, 0), unknown(9, 0)],
        &[],
    );
    let b = frontier(writer, &scratch, &[unknown(9, 1)], &[(a, 4)]);
    // A diamond: `c` reads `a` both through `b`'s cut and at a wider one of its own.
    let c = frontier(writer, &scratch, &[], &[(b, 9), (a, 1)]);
    assert_eq!(b.unknowns(), [unknown(4, 0), unknown(9, 0), unknown(9, 1)]);
    assert_eq!(
        c.unknowns(),
        [unknown(1, 0), unknown(4, 0), unknown(9, 0), unknown(9, 1)]
    );
    // A cut through `b` narrows the cut `b` read `a` at.
    let d = frontier(writer, &scratch, &[], &[(b, 9)]);
    assert_eq!(d.unknowns(), [unknown(9, 0), unknown(9, 1)]);
}

#[test]
fn a_binding_wakes_exactly_the_frontiers_naming_what_it_binds() {
    let storage = program_storage();
    let writer = storage.brand().writer();
    let scratch = Bump::new();
    let named = frontier(writer, &scratch, &[unknown(1, 3), unknown(2, 0)], &[]);
    let read = |from| frontier(writer, &scratch, &[], &[(named, from)]);
    let laid = |bound: &[(Unknown, KType)]| {
        let mut held = BumpVec::new_in(&scratch);
        held.extend(
            bound
                .iter()
                .map(|(unknown, typed)| (*unknown, Bound::Type(*typed))),
        );
        Binding::laid(writer, &mut held)
    };
    let binding = laid(&[(unknown(2, 0), KType::NUMBER), (unknown(0, 0), KType::STR)]);
    assert!(named.meets_binding(binding));
    assert!(!Frontier::EMPTY.meets_binding(binding));
    let apart = laid(&[(unknown(1, 2), KType::NUMBER)]);
    assert!(!named.meets_binding(apart));
    // Only a read's unknown meets the binding, and only while the cut keeps it.
    assert!(read(2).meets_binding(binding));
    assert!(!read(3).meets_binding(binding));
    assert!(
        matches!(binding.bound(unknown(0, 0)), Some(Bound::Type(typed)) if typed == KType::STR)
    );
    assert!(binding.bound(unknown(1, 3)).is_none());
    assert!(!binding.extends(apart));
    assert!(binding.extends(Binding::EMPTY));
}
