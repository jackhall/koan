//! The witnessed-transfer copy hook, the finalize sever, and their behavior tests.

use std::rc::Rc;

use crate::machine::core::{CallFrame, CarrierPin, CarrierWitness, RegionBrand};
use crate::machine::model::types::KType;
use crate::machine::model::values::{Carried, CarriedFamily, KObject};
use crate::witnessed::{erase_to_static, Witnessed};

/// The structural-copy callback a witnessed transfer's fold runs
/// ([`Sealed::transfer_into`](crate::witnessed::Sealed::transfer_into) /
/// `Scheduler::transfer_lifted`): copy a [`Carried`] into `dest`'s region at the fold brand. Only
/// the top-level node is re-allocated; the composite spine shares its `Rc` payloads
/// ([`deep_clone`](crate::machine::model::KObject::deep_clone)), and a `KFunction` / first-class
/// `Module` rides a bare borrow
/// preserved verbatim — kept alive by the witness set the transfer layer assembles, so this hook
/// owns only the copy, never a region anchor. It is not a delivery channel: dep terminals cross to
/// finishes as sealed carriers.
pub(in crate::machine::execute) fn copy_carried<'b>(
    value: Carried<'b>,
    dest: RegionBrand<'b>,
) -> Carried<'b> {
    match value {
        Carried::Object(v) => Carried::Object(dest.alloc_object(v.deep_clone())),
        Carried::Type(t) => Carried::Type(dest.alloc_ktype(t.clone())),
    }
}

/// The owned backing a [`sever_residence`] copies a top node into, before it becomes a
/// [`CarrierPin`]. Read out of the carrier brand-free (an `Rc` names no lifetime), so it escapes the
/// rank-2 `with` the branded `Carried` could not.
enum SeveredBacking {
    Object(Rc<KObject<'static>>),
    Type(Rc<KType<'static>>),
}

/// Sever a finalized terminal from its dying `producer` frame: copy the value's top node out of the
/// frame's region into an owned `Rc` backing, and re-seal the carrier witnessed by a pin holding that
/// backing — so the value outlives the frame with no fabricated lifetime.
///
/// The Done-boundary gate reaches here only when the carrier's **reach** does not cover `producer`
/// (the value borrows nothing into the frame it resides in), so copying the top node out loses no
/// borrow: a scalar owns everything, and an aggregate's spine is `Rc`-heap (shared by `deep_clone`)
/// with every interior borrow reaching a *foreign* region the carrier's reach — carried forward
/// verbatim — already pins. [`CarrierWitness::severed`] drops the frame's residence pin (nothing needs
/// it live now) and adds the backing, leaving reach untouched: a scalar thus seals with empty reach.
pub(in crate::machine::execute) fn sever_residence(
    carrier: Witnessed<CarriedFamily, CarrierWitness>,
    producer: &Rc<CallFrame>,
) -> Witnessed<CarriedFamily, CarrierWitness> {
    let old_witness = carrier.witness().clone();
    // Copy the top node out — a `deep_clone` of the object (its spine `Rc`s shared) or a `clone` of
    // the type — and erase it to its `'static` backing form, read brand-free out of the carrier.
    let backing = carrier.with(|carried| match carried {
        Carried::Object(object) => SeveredBacking::Object(Rc::new(erase_to_static::<
            KObject<'static>,
        >(object.deep_clone()))),
        Carried::Type(kt) => {
            SeveredBacking::Type(Rc::new(erase_to_static::<KType<'static>>((*kt).clone())))
        }
    });
    let producer_region = producer.region();
    match backing {
        SeveredBacking::Object(rc) => Witnessed::yoke_backing::<KObject<'static>>(
            rc,
            |rc| old_witness.severed(producer_region, CarrierPin::Object(rc)),
            |node| Carried::Object(node),
        ),
        SeveredBacking::Type(rc) => Witnessed::yoke_backing::<KType<'static>>(
            rc,
            |rc| old_witness.severed(producer_region, CarrierPin::Type(rc)),
            |node| Carried::Type(node),
        ),
    }
}

#[cfg(test)]
mod tests;
