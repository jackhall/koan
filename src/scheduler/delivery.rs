//! What a koan cell delivers to its consumer, as the substrate's two-family bundle.

use crate::knot::KValueFamily;
use crate::memory::Delivery;

/// Koan's delivery bundle: a value built through the consumer's own scratch writer, and a value
/// carrier filed at rest in the consumer's region. Both halves are koan's value family — a scratch
/// fill and a carrier fill differ in where the bytes are, never in what they are.
///
/// `pub(super)`: no step reaches a receipt slot, so nothing outside the scheduler names it.
pub(super) struct KDelivery;

impl<'graph> Delivery<'graph> for KDelivery {
    type Scratch = KValueFamily;
    type Carrier = KValueFamily;
}
