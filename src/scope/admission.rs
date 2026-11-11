//! A layer's **admission**: the conditions on a binding, up to its event, under which the layer's
//! decision holds. A decision memo finds a decision by its layer's admission
//! ([`memo`](super::memo)). A binding's layer admits by identity: each name binds the content the
//! layer was laid under. A witnessed layer admits by its questions: a binding meets each
//! [`Keeps`](Condition::Keeps) where the closed door, asked with the binding written in, answers
//! alike ([build/README.md § Resumption](../build/README.md#resumption)).

use crate::memory::{Lays, collect};

use super::build_layer::BuildLayer;
use super::facts::{Binding, Bound, Unknown};
use super::question::Answered;
use super::relay::Relay;

/// What the layers `below` bind `unknown` to, innermost first.
fn bound_below<'a>(below: Option<&BuildLayer<'a>>, unknown: Unknown) -> Option<Bound<'a>> {
    let mut at = below;
    while let Some(layer) = at {
        if let Some(bound) = layer.binding().bound(unknown) {
            return Some(bound);
        }
        at = layer.parent();
    }
    None
}

/// One condition a binding meets for a decision to hold.
#[derive(Clone, Copy, Debug)]
pub enum Condition<'a> {
    /// The binding gives `unknown` the same meaning as `bound`.
    Binds(Unknown, Bound<'a>),
    /// The binding keeps the answer a witnessing walk took to a question.
    Keeps(&'a Answered<'a>),
}

/// How many of a witnessed layer's questions a binding keeps.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Keeping {
    All,
    /// Some, but not all: how many.
    Some(usize),
    None,
}

/// The conditions a binding meets for a layer's decision to hold.
#[derive(Clone, Copy, Debug)]
pub struct Admission<'a> {
    conditions: &'a [Condition<'a>],
}

impl<'a> Admission<'a> {
    /// The admission of a layer that binds nothing.
    pub const EMPTY: Admission<'static> = Admission { conditions: &[] };

    /// The admission of a layer laid binding `own`: that each name binds what it bound.
    pub fn binding(lay: impl Lays<'a>, own: Binding<'a>) -> Self {
        if own.is_empty() {
            return Admission { conditions: &[] };
        }
        let conditions =
            (own.entries().iter()).map(|(unknown, bound)| Condition::Binds(*unknown, *bound));
        Admission {
            conditions: collect(lay, conditions),
        }
    }

    /// The admission of a witnessed layer whose walk took `answered`: that a binding keeps each,
    /// each held once.
    pub fn keeping(lay: impl Lays<'a>, answered: &[&'a Answered<'a>]) -> Self {
        // Held once each by address, in the order taken: `seen` sorted for the search.
        let mut kept: smallvec::SmallVec<[&'a Answered<'a>; 16]> = smallvec::SmallVec::new();
        let mut seen: smallvec::SmallVec<[usize; 16]> = smallvec::SmallVec::new();
        for each in answered {
            let at = std::ptr::from_ref(*each).addr();
            if let Err(slot) = seen.binary_search(&at) {
                seen.insert(slot, at);
                kept.push(each);
            }
        }
        if kept.is_empty() {
            return Admission { conditions: &[] };
        }
        Admission {
            conditions: collect(lay, kept.iter().map(|answered| Condition::Keeps(answered))),
        }
    }

    /// Whether an event binding `own` over the layers `below`, its cell's earlier events' and its
    /// enclosers', meets every identity condition: that it binds exactly the names the conditions
    /// on its own names name, each alike, and `below` binds each other name a condition names
    /// alike.
    pub fn admits(&self, own: &[(Unknown, Bound<'_>)], below: Option<&BuildLayer<'_>>) -> bool {
        let mut owned = 0;
        let each = (self.conditions.iter()).all(|condition| match condition {
            Condition::Binds(unknown, bound) => {
                match own.iter().find(|(held, _)| held == unknown) {
                    Some((_, held)) => {
                        owned += 1;
                        held.same_as(bound)
                    }
                    None => bound_below(below, *unknown).is_some_and(|held| held.same_as(bound)),
                }
            }
            Condition::Keeps(_) => false,
        });
        each && owned == own.len()
    }

    /// How many of its questions a binding keeps, where `keeps` says whether it keeps one: every
    /// one where the layer asked none.
    pub fn kept(&self, keeps: &mut dyn FnMut(&Answered<'_>) -> bool) -> Keeping {
        let (mut asked, mut held) = (0, 0);
        for condition in self.conditions {
            if let Condition::Keeps(answered) = condition {
                asked += 1;
                held += usize::from(keeps(answered));
            }
        }
        match held {
            _ if held == asked => Keeping::All,
            0 => Keeping::None,
            held => Keeping::Some(held),
        }
    }

    /// Hand `each` every question it keeps.
    pub fn each_kept(&self, each: &mut dyn FnMut(&'a Answered<'a>)) {
        for condition in self.conditions {
            if let Condition::Keeps(answered) = condition {
                each(answered);
            }
        }
    }

    /// These conditions laid again by `relay`.
    pub(super) fn relaid<'to, L: Lays<'to>>(
        &self,
        lay: L,
        relay: &mut Relay<'to, '_, '_, '_, L>,
    ) -> Admission<'to> {
        if self.conditions.is_empty() {
            return Admission { conditions: &[] };
        }
        let mut conditions: smallvec::SmallVec<[Condition<'to>; 16]> = smallvec::SmallVec::new();
        for condition in self.conditions {
            conditions.push(match condition {
                Condition::Binds(unknown, bound) => Condition::Binds(*unknown, relay.bound(bound)),
                Condition::Keeps(answered) => Condition::Keeps(relay.answered(answered)),
            });
        }
        Admission {
            conditions: collect(lay, conditions.iter().copied()),
        }
    }

    /// The conditions.
    pub fn conditions(&self) -> &'a [Condition<'a>] {
        self.conditions
    }
}
