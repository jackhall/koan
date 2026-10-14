//! A caching body's exits: where a value leaves a body whose types lie in a scratch tier, every
//! type it reaches is adopted into the region first, so nothing it reaches names that tier once
//! the scratch resets ([type tiers](../../design/type-tiers.md#the-scratch-branch-as-a-cache)).
//!
//! The body runner adopts at a frame's end and at a held frame's hand-off of its last statement;
//! the evaluator at each call into a frame, each `EVAL`, and where an evaluation owing a frame's
//! contract finishes. A value the frame received — a parameter, a closure binding, the callable it
//! runs — adopts its own memo alone, since its parts crossed into the region at the frame's birth;
//! so does an argument the load recorded as passing one on, or a member read of one
//! ([`BodyShape::received_at`](crate::scope::BodyShape::received_at)), which a call hands over.

use crate::knot::{KActivationView, KValue, KnottedFamily};
use crate::memory::{Bump, BumpVec};
use crate::type_lattice::{DeclaredType, KType};
use crate::values::{Chain, Types, adopt_value};

use super::record::Contract;

/// One exit of a caching body: the region chain `region` its values are adopted into, read from
/// `body`, the chain the body runs on, beside what the body's frame received.
pub struct Exit<'a, 'tier, 'x, 'graph, 'here, 'reg> {
    region: Types<'graph, 'here, 'a>,
    body: &'a Chain<'graph, 'tier, 'here, 'reg, 'a>,
    received: BumpVec<'x, KValue<'graph, 'here>>,
}

impl<'a, 'tier, 'x, 'graph, 'here, 'reg> Exit<'a, 'tier, 'x, 'graph, 'here, 'reg> {
    /// An exit from the body `view` reads through, whose chain is `body`, into `region`, where
    /// `passed` are values the body passes on that its frame received.
    pub fn new(
        region: Types<'graph, 'here, 'a>,
        body: &'a Chain<'graph, 'tier, 'here, 'reg, '_>,
        view: &KActivationView<'graph, 'here>,
        passed: &[KValue<'graph, 'here>],
        scratch: &'x Bump,
    ) -> Self {
        let mut received = BumpVec::new_in(scratch);
        view.received(&mut |value| received.push(value));
        received.extend_from_slice(passed);
        Exit {
            region,
            body,
            received,
        }
    }

    /// Adopt every type `value` reaches.
    pub fn value(&self, value: &KValue<'graph, 'here>) {
        adopt_value::<KnottedFamily>(&self.region, self.body, value, &self.received);
    }

    /// Adopt the type `kt`.
    pub fn handle(&self, kt: impl Into<DeclaredType<KType>>) {
        self.region.adopt_from(self.body, kt.into());
    }

    /// Adopt every type `contract` names.
    pub fn contract(&self, contract: &Contract) {
        if let Some(callee) = contract.callee {
            self.handle(callee);
        }
        self.handle(contract.returns);
        self.handle(contract.retype);
    }
}
