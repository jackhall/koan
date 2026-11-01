//! What a koan cell delivers to its consumer, as the substrate's two-family bundle: a value, or —
//! filed as a carrier — a [fault](super::README.md#faults).

use crate::knot::{KInner, KValue, KValueFamily};
use crate::memory::{Delivery, DropFree, covariant, reattachable};

/// Koan's delivery bundle: a value built through the consumer's own scratch writer, and a fill
/// filed at rest in the consumer's region. A scratch fill and a carrier fill of a value differ in
/// where the bytes are, never in what they are; a fault rides a carrier fill alone.
///
/// `pub(super)`: no step reaches a receipt slot, so nothing outside the scheduler names it.
pub(super) struct KDelivery;

impl<'graph> Delivery<'graph> for KDelivery {
    type Scratch = KValueFamily;
    type Carrier = FillFamily;
}

/// What a carrier fill holds: a value, or what lies inside a fault its producer ended with.
#[derive(Clone, Copy)]
pub(super) enum Fill<'graph, 'cell> {
    Value(KValue<'graph, 'cell>),
    Fault(KInner<'graph, 'cell>),
}

/// The family of [`Fill`].
pub(super) struct FillFamily;
reattachable!(FillFamily => Fill<'graph, 'cell>);
// A fill is redeemed into its consumer, so its family carries the covariance witness the crossing
// doors ask for.
covariant!(FillFamily);
impl DropFree for FillFamily {}
