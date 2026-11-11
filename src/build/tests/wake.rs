//! The wake memo: how a frontier stands under checked and witnessed names over a chain's binding,
//! answered once per frontier, agrees with the frontier's flattened marks
//! ([`Frontier::wakes_flat`]) over frontier DAGs with cuts, undecided and answered marks of
//! holding and discrete kinds alike.

use proptest::prelude::*;

use crate::memory::{Bump, BumpVec, program_storage, resident};
use crate::scope::{
    Address, Answer, Answered, Binding, Bound, Cut, Frontier, Mark, Noted, OpenKind, OpenMark,
    Question, Segment, Slot, Unknown,
};
use crate::values::{Nothing, Value};

use super::super::wake::Wakes;

/// A drawn mark: named, undecided of a holding kind or not, or answered by a question the
/// binding keeps or not.
#[derive(Clone, Copy, Debug)]
enum Drawing {
    Named,
    Undecided { holding: bool },
    Answered { kept: bool },
}

fn drawing() -> impl Strategy<Value = Drawing> {
    prop_oneof![
        Just(Drawing::Named),
        any::<bool>().prop_map(|holding| Drawing::Undecided { holding }),
        any::<bool>().prop_map(|kept| Drawing::Answered { kept }),
    ]
}

/// One frontier of a generated DAG: its own unknowns, as (segment, slot) beside its mark, and the
/// earlier frontiers it reads, each by index beside its cut's segment.
type Drawn = (Vec<((u32, u32), Drawing)>, Vec<(usize, u32)>);

fn unknown((segment, slot): (u32, u32)) -> Unknown {
    Unknown {
        segment: Segment(segment),
        at: Address::Slot(Slot(slot)),
    }
}

/// How a walk holds a drawn unknown: bound below the walk, bound and checked by it, or witnessed.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Role {
    Chain,
    Checked,
    Witnessed,
}

/// A bound unknown, as (segment, slot), beside its role.
type DrawnBound = ((u32, u32), Role);

/// The frontiers, then the bound unknowns.
fn drawn() -> impl Strategy<Value = (Vec<Drawn>, Vec<DrawnBound>)> {
    let pair = (0u32..6, 0u32..3);
    let frontier = (
        prop::collection::vec((pair.clone(), drawing()), 0..3),
        prop::collection::vec((any::<prop::sample::Index>(), 0u32..6), 0..3),
    );
    (
        prop::collection::vec(frontier, 1..8),
        prop::collection::vec(
            (
                pair,
                prop_oneof![
                    Just(Role::Chain),
                    Just(Role::Checked),
                    Just(Role::Witnessed)
                ],
            ),
            0..4,
        ),
    )
        .prop_map(|(frontiers, bound)| {
            // Each frontier reads only earlier ones, as an ended frontier is read.
            let frontiers = (frontiers.into_iter().enumerate())
                .map(|(at, (own, reads))| {
                    let reads = match at {
                        0 => Vec::new(),
                        _ => (reads.into_iter())
                            .map(|(index, from)| (index.index(at), from))
                            .collect(),
                    };
                    (own, reads)
                })
                .collect();
            (frontiers, bound)
        })
}

proptest! {
    #[test]
    fn the_wake_memo_stands_a_frontier_as_its_flattened_marks_meet_the_binding(
        (frontiers, bound) in drawn(),
    ) {
        let storage = program_storage();
        let writer = storage.brand().writer();
        let scratch = Bump::new();
        let (mut chain, mut checked, mut witness) = (
            BumpVec::new_in(&scratch),
            BumpVec::new_in(&scratch),
            BumpVec::new_in(&scratch),
        );
        let (mut seen, number) = (Vec::new(), Value::<Nothing>::Number(0.0).concrete_ktype());
        for (pair, role) in bound {
            if !seen.contains(&pair) {
                seen.push(pair);
                let bound = (unknown(pair), Bound::Type(number));
                match role {
                    Role::Witnessed => witness.push(bound),
                    Role::Chain => chain.push(bound),
                    Role::Checked => {
                        chain.push(bound);
                        checked.push(bound);
                    }
                }
            }
        }
        let (chain, checked, witness) = (
            Binding::laid(writer, &mut chain),
            Binding::laid(writer, &mut checked),
            Binding::laid(writer, &mut witness),
        );
        let judged = number.into();
        // A question the binding keeps answers `true`; one it does not, `false`.
        let answered = |kept| {
            resident(writer, Answered {
                question: Question::Kind { ty: number.into() },
                answer: Answer::Bool(kept),
            })
        };
        let keeps = |answered: &Answered<'_>| answered.answer == Answer::Bool(true);
        let mut laid: Vec<Frontier<'_>> = Vec::new();
        for (own, reads) in &frontiers {
            let mut noted = BumpVec::new_in(&scratch);
            noted.extend(own.iter().map(|(pair, drawing)| Noted {
                unknown: unknown(*pair),
                mark: match drawing {
                    Drawing::Named => Mark::Named,
                    Drawing::Undecided { holding } => {
                        let kind = match holding {
                            true => OpenKind::EndRead,
                            false => OpenKind::Verdict,
                        };
                        Mark::Open(OpenMark::undecided(kind, None, judged))
                    }
                    Drawing::Answered { kept } => Mark::Open(OpenMark {
                        answered: Some(answered(*kept)),
                        ..OpenMark::undecided(OpenKind::Kind, None, judged)
                    }),
                },
            }));
            let cuts: Vec<_> = (reads.iter())
                .map(|(index, from)| Cut { frontier: laid[*index], from: Segment(*from) })
                .collect();
            laid.push(Frontier::laid(writer, &mut noted, &cuts));
        }
        let wakes = Wakes::new(checked, witness, chain, &scratch);
        // Asked newest first, so a read is answered from the memo an earlier ask filled.
        for frontier in laid.iter().rev() {
            let expected = frontier.wakes_flat(checked, witness, chain, &keeps);
            prop_assert_eq!(wakes.wakes(*frontier, &keeps), expected);
        }
    }
}
