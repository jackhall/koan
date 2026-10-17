//! A caching body's exits: where a value leaves a body whose types lie in a scratch tier, every
//! type it reaches is adopted into the region first, so nothing it reaches names that tier once
//! the scratch resets ([type tiers](../../design/type-tiers.md#the-scratch-branch-as-a-cache)).
//!
//! The body runner adopts at a frame's end and at a held frame's hand-off of its last statement;
//! the evaluator at each call into a frame, each `EVAL`, and where an evaluation owing a frame's
//! contract finishes. The walk stops at a part whose memo bits say its types already resolve
//! there, handing over nothing for it, and marks every value it walked settled after, so a value
//! handed on through a loop is walked once
//! ([type tiers](../../design/type-tiers.md#an-adoption-stops-at-a-settled-part)).

use crate::knot::KnottedFamily;
use crate::memory::BumpAllocator;
use crate::type_lattice::{DeclaredType, KType};
use crate::values::{Chain, Destination, Types, adopt_value};

use super::record::Contract;

/// One exit of a caching body: the region chain `region` its values are adopted into, read from
/// `body`, the chain the body runs on.
pub struct Exit<'a, 'tier, 'graph, 'here, 'reg> {
    region: Types<'graph, 'here, 'a>,
    body: &'a Chain<'graph, 'tier, 'here, 'reg, 'a>,
    /// Where each walk lays its stack, knot set and settled list: the step's scratch.
    scratch: BumpAllocator<'a>,
}

impl<'a, 'tier, 'graph, 'here, 'reg> Exit<'a, 'tier, 'graph, 'here, 'reg> {
    /// An exit from the body whose chain is `body` into `region`, walking in `scratch`.
    pub fn new(
        region: Types<'graph, 'here, 'a>,
        body: &'a Chain<'graph, 'tier, 'here, 'reg, '_>,
        scratch: BumpAllocator<'a>,
    ) -> Self {
        Exit {
            region,
            body,
            scratch,
        }
    }

    /// Adopt every type `value` reaches, stopping at a settled part, and mark what was walked
    /// settled.
    pub fn value(&self, value: &crate::knot::KValue<'graph, 'here>) {
        adopt_value::<KnottedFamily>(
            Destination::Region(&self.region),
            self.body,
            value,
            self.scratch,
        );
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
