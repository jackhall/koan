//! Koan's instantiation of the library's collapsed walking-carrier witness
//! ([`crate::witnessed::Carrier`]): `F = FrameStorage` (the per-call frame owner) and
//! `S = `[`SeveredBacking`] (koan's frame-free owned node backing). See
//! [design/witness-hosting.md § The carrier](../../../design/witness-hosting.md#the-carrier).

use std::rc::Rc;

use crate::machine::model::types::KType;
use crate::machine::model::values::{CarriedFamily, KObject};

use super::arena::FrameStorage;

/// The owned backing a finalize sever holds: a top node copied out of a dying frame into an owned
/// `Rc`, read through by the walking carrier's `Severed` arm. Transitional debt — deleted along
/// with the sever gate once the scheduler retains producer frames itself
/// ([Delivery-driven frame retention](../../../roadmap/scheduler_library/delivery-driven-frame-retention.md)).
#[derive(Clone)]
pub enum SeveredBacking {
    /// A severed object top node.
    Object(Rc<KObject<'static>>),
    /// A severed type top node.
    Type(Rc<KType<'static>>),
}

/// Koan's value-carrier witness: the library [`Carrier`](crate::witnessed::Carrier) generic over
/// koan's frame owner and severed backing. Every site that only *threads* this type as the `W`
/// witness parameter of `Witnessed<T, W>` / `Sealed<T, W>` is unaffected by this alias; a site that
/// constructs or inspects a carrier routes the library's `Carrier` surface directly.
pub type CarrierWitness = crate::witnessed::Carrier<FrameStorage, SeveredBacking>;

/// Koan's **delivery envelope**: the library [`Delivered`](crate::witnessed::Delivered) carrying a
/// [`CarrierWitness`]-witnessed value carrier paired with its retained [`FrameStorage`] owner. The
/// in-transit form of a value's liveness — from a scheduler pull (or a resident seal) to its
/// adoption — replacing the per-call-site `(Sealed, Option<Rc<FrameStorage>>)` pairing threaded at
/// consumer sites.
pub type DeliveredCarried = crate::witnessed::Delivered<CarriedFamily, CarrierWitness, FrameStorage>;
