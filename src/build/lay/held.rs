//! **Holders** and **results**: what a call of the text gives the binder holding its value.
//!
//! A `LET` binder whose whole right-hand side is a call is a **holder**: a bind site of its own,
//! which owns the questions reading it, whatever gives it its type. Where an answer through the
//! call gives the type, the holder's **arrival** — an answered name typed through each of the
//! call's edges, or a closed term where the text selects a callee whose result is closed — is what
//! an **onward call** on the holder's statement gives the holder's carried name, so the evaluation
//! that follows the call makes the holder's instance under it. Where none does, the run binds the
//! holder as the value lands ([the binding graph](../../../design/binding-graph.md)).
//!
//! A part held in place — a call or an `EVAL` written inline, whose value's type is its holder's
//! name ([`Address::Held`]) — is a holder too, with a statement of its own in its owning body, laid
//! before its unit's in the order the run binds it: within a unit, a holder after every holder
//! inside it, and after every holder to its left. Its unit's calls inside it are laid at its
//! statement.
//!
//! A callable's **result** is what its value carries to a caller: a closed term where its summary
//! is a `MODULE` body the text closes, retyped as its frame retypes its value, or an answered name
//! of the callable typed through the calls its last statement makes, one edge per call, where its
//! frame keeps its value's own type. A result asked while it is being asked, around a recursion,
//! carries nothing.

use bindgraph::Site as GraphSite;

use super::{Fold, GraphName, GraphTerm, NameKind, Returned};
use crate::memory::{BumpVec, Lays};
use crate::scope::{
    Address, NameAddress, Position, Segment, ShapeKind, Site, SiteRule, Tree, Unknown,
};
use crate::type_lattice::{DeclaredType, Interval, KType};
use crate::values::{Restamping, retype_target_as};

impl<'g, 'x, L: Lays<'g>> Fold<'_, 'g, 'x, '_, '_, '_, '_, L> {
    /// Lays each holder's arrival and onward call: run once every call is laid.
    pub(super) fn lay_holders(&mut self) {
        let mut holders = BumpVec::new_in(self.scratch);
        for (&(unknown, kind), &name) in self.named.iter() {
            if kind == NameKind::Carried && self.holds_a_call(unknown) {
                holders.push((unknown, name));
            }
        }
        holders.sort_unstable_by_key(|(_, name)| name.index());
        for (unknown, holder) in holders.iter().copied() {
            let shape = self.shapes.shape_at(unknown.segment);
            // A part held in place holds the value of the node it is.
            let site = match unknown.at {
                Address::Slot(slot) => shape.held_site(slot),
                Address::Held(site) => shape.held_node(site),
                Address::Capture(_) => None,
            };
            let Some(site) = site else {
                continue;
            };
            let upper = self.names[holder.index()].upper;
            let address = NameAddress::Arrival(unknown);
            let Some(arrival) = self.through(address, (unknown.segment, site), upper) else {
                continue;
            };
            let term = match arrival {
                Returned::Fixed(term) => term,
                Returned::Name(name) => self.term(GraphTerm::Name(name)),
            };
            let at = self.bound_at(unknown);
            self.text.arrives(at, vec![(holder, term)]);
            self.calls.push(super::LaidCall {
                callee: at.body(),
                written: None,
                held: None,
            });
        }
    }

    /// Whether `unknown` is a holder binding at a statement: a `LET` binder of a statement, or a
    /// part held in place.
    fn holds_a_call(&self, unknown: Unknown) -> bool {
        let shape = self.shapes.shape_at(unknown.segment);
        let held = match unknown.at {
            Address::Slot(slot) => {
                shape.position_of(slot) != Position::PARAMETER && shape.held_site(slot).is_some()
            }
            Address::Held(_) => true,
            Address::Capture(_) => false,
        };
        held && matches!(self.bound_at(unknown), GraphSite::Statement(..))
    }

    /// What the calls laid at `at` give what holds their value: the closed term of the one call the
    /// text selects, or an answered name addressed `address`, bounded by `upper`, typed through
    /// each call whose callee's result carries something. `None` where none does.
    fn through(
        &mut self,
        address: NameAddress,
        at: (Segment, Site),
        upper: KType,
    ) -> Option<Returned> {
        let mut laid = BumpVec::new_in(self.scratch);
        laid.extend((self.calls.iter()).filter_map(|call| call.held.filter(|held| held.at == at)));
        let mut typing = BumpVec::new_in(self.scratch);
        for held in laid.iter() {
            if let Some(returned) = self.returned(held.into) {
                typing.push((held.edge, held.on, returned));
            }
        }
        if let [(_, None, Returned::Fixed(term))] = typing.as_slice() {
            return Some(Returned::Fixed(*term));
        }
        // A call the text selects beside others is no call an answer takes.
        typing.retain(|(_, on, returned)| on.is_some() || matches!(returned, Returned::Name(_)));
        if typing.is_empty() {
            return None;
        }
        let name = self.text.answered_name();
        self.names.push(GraphName {
            address,
            kind: NameKind::Carried,
            upper,
        });
        for (edge, on, returned) in typing.iter().copied() {
            match (returned, on) {
                (Returned::Name(takes), _) => {
                    self.text.answers_through(edge, name, takes);
                }
                (Returned::Fixed(term), Some((question, arm))) => {
                    self.text.answers(question, arm, name, term);
                }
                (Returned::Fixed(_), None) => unreachable!("kept only on an arm"),
            }
        }
        Some(Returned::Name(name))
    }

    /// What `callee`'s result carries to its callers, asked once.
    fn returned(&mut self, callee: Segment) -> Option<Returned> {
        match self.returned.get(&callee) {
            Some(Asked::Done(held)) => return *held,
            Some(Asked::Asking) => return None,
            None => {}
        }
        self.returned.insert(callee, Asked::Asking);
        let returned = self.returns_of(callee);
        self.returned.insert(callee, Asked::Done(returned));
        returned
    }

    /// What `callee`'s result carries: its summary's closed `MODULE` body, retyped as its frame
    /// retypes its value, or what its body yields where its frame keeps its value's own type.
    fn returns_of(&mut self, callee: Segment) -> Option<Returned> {
        let shape = self.shapes.shape_at(callee);
        if shape.kind() != ShapeKind::Callable {
            return None;
        }
        let (closing, _) = self.load.closing(callee)?;
        let restamp = closing.retypes.returns;
        if let Some(Some(Tree::Body {
            body, captures: [], ..
        })) = closing.summary
        {
            let unit = self.shapes.birth_unit(body)?;
            let (facts, _) = self.load.unit(unit)?;
            let (_, typed) = facts.typed.get(body)?;
            let own = DeclaredType::Type(self.types.concrete(typed.signature?)?);
            let carried = match restamp {
                None => own,
                Some(restamp) => {
                    let declared = self.types.concrete(restamp.at)?;
                    let retype = restamp.retype(self.types);
                    retype_target_as(self.types, self.scratch, own, declared, retype)
                }
            };
            let DeclaredType::Type(carried) = carried else {
                return None;
            };
            return Some(Returned::Fixed(self.term(GraphTerm::Carried {
                typed: Interval::point(carried.into()),
                mentions: &[],
                variables: &[],
            })));
        }
        if restamp.is_some_and(|restamp| restamp.how != Restamping::Keeps) {
            return None;
        }
        match self.yields(callee, callee)? {
            Yield::Returns(returned) => Some(returned),
            Yield::Recurs => None,
        }
    }

    /// What body `segment`, in `asking`'s body, yields: what its last statement's calls give, or
    /// where that statement is a `MATCH`, what its arms yield — on each arm of its arms question
    /// where it has one, else the one thing every arm but those returning `asking`'s own result
    /// yields. A tail call into `asking` alone returns `asking`'s own result.
    fn yields(&mut self, segment: Segment, asking: Segment) -> Option<Yield> {
        let shape = self.shapes.shape_at(segment);
        let tail = shape.tail_site()?;
        let address = NameAddress::Returned(segment);
        if !self.is_match(segment, tail) {
            let calls = (self.calls.iter()).filter_map(|call| call.held);
            let mut into = calls.filter(|held| held.at == (segment, tail)).peekable();
            if segment != asking && into.peek().is_some() && into.all(|held| held.into == asking) {
                return Some(Yield::Recurs);
            }
            return (self.through(address, (segment, tail), KType::ANY)).map(Yield::Returns);
        }
        let last = shape.body().len() - 1;
        let mut arms = BumpVec::new_in(self.scratch);
        for (_, nested) in shape.nested_in(last) {
            if nested.arm().is_some() {
                arms.push(nested.segment());
            }
        }
        let decides = (self.sites.iter())
            .find(|(key, _)| *key == (segment, tail, SiteRule::ArmsRun))
            .map(|(_, question)| *question);
        let Some(question) = decides else {
            // A value chooses the arm: the result is what every arm yields, the arms returning
            // the result itself aside.
            let mut yielded = None;
            for arm in arms.iter().copied() {
                match self.yields(arm, asking)? {
                    Yield::Recurs => {}
                    Yield::Returns(returned) => match yielded {
                        None => yielded = Some(returned),
                        Some(held) if same(held, returned) => {}
                        Some(_) => return None,
                    },
                }
            }
            return yielded.map(Yield::Returns);
        };
        let mut typing = BumpVec::new_in(self.scratch);
        for (at, arm) in arms.iter().enumerate() {
            if let Some(Yield::Returns(Returned::Fixed(term))) = self.yields(*arm, asking) {
                typing.push((at as u32 + 1, term));
            }
        }
        if typing.is_empty() {
            return None;
        }
        let name = self.text.answered_name();
        self.names.push(GraphName {
            address,
            kind: NameKind::Carried,
            upper: KType::ANY,
        });
        for (arm, term) in typing.iter().copied() {
            self.text.answers(question, arm, name, term);
        }
        Some(Yield::Returns(Returned::Name(name)))
    }

    /// Whether the node at `site` of `segment` is a `MATCH`: the walk recorded its arms.
    fn is_match(&self, segment: Segment, site: Site) -> bool {
        (self.load.own_units()).any(|(_, (facts, _))| {
            (facts.typed.get(segment)).is_some_and(|(_, typed)| typed.arms.get(site).is_some())
        })
    }
}

/// Whether a callable's result is being asked, or what it carries once asked.
#[derive(Clone, Copy)]
pub(super) enum Asked {
    Asking,
    Done(Option<Returned>),
}

/// What a body yields its callable: what its value carries, or the callable's own result, which a
/// tail call back into it returns.
#[derive(Clone, Copy)]
enum Yield {
    Returns(Returned),
    Recurs,
}

/// Whether two results are one: one term, or one name.
fn same(a: Returned, b: Returned) -> bool {
    match (a, b) {
        (Returned::Fixed(a), Returned::Fixed(b)) => a == b,
        (Returned::Name(a), Returned::Name(b)) => a == b,
        _ => false,
    }
}
