//! A signature type as a **set of applications**, and the order and the meet over two such sets.
//!
//! Three nodes are signature types: a [`TypeNode::Signature`], a [`TypeNode::SignatureApply`] and a
//! [`TypeNode::SignatureMeet`]. Each reads as a set of [`Application`]s — a declared signature with
//! some of its head parameters pinned — through [`applications`]: a signature is the one
//! application pinning nothing, except the empty signature, which is the empty set; an application
//! is itself; a meet is its members. The order ([`applications_under`]), the meet
//! ([`canonical_applications`], what [`TypeRegistry::signature_meet`] interns) and *fits* (in
//! [`sig_relations`](super::sig_relations)) all read signature types through here and nothing
//! else, so none of them solves: two applications compare by their pins alone.

use crate::bump::{BumpAllocator, BumpVec};
use crate::symbols::{BinderSymbol, Symbol};

use super::handle::{Handle, TypeHandle, wrap};
use super::node::TypeNode;
use super::record::Record;
use super::registry::TypeRegistry;

/// One application: `signature`, a [`TypeNode::Signature`], with `pins` fixing some of its head
/// parameters by name. Empty pins for the signature on its own.
#[derive(Clone, Copy, Debug)]
pub(super) struct Application<'run> {
    pub(super) signature: Handle,
    pub(super) pins: &'run [(BinderSymbol, Handle)],
}

impl Application<'_> {
    /// The handle this application interns to: the signature itself for no pins.
    pub(super) fn handle(self, types: &TypeRegistry<'_>, scratch: BumpAllocator<'_>) -> Handle {
        types.signature_apply(scratch, wrap(self.signature), self.pins)
    }

    /// The type `self` pins the parameter named `name` to, if it pins it.
    pub(super) fn pin(self, name: Symbol) -> Option<Handle> {
        Record::<Handle>::over(self.pins).get(name)
    }
}

/// Whether `kt` is a signature type — one of the three nodes [`applications`] reads.
pub(super) fn is_signature_type(types: &TypeRegistry<'_>, kt: Handle) -> bool {
    matches!(
        types.node(kt),
        TypeNode::Signature { .. }
            | TypeNode::SignatureApply { .. }
            | TypeNode::SignatureMeet { .. }
    )
}

/// The applications `kt` holds, or `None` where `kt` is no signature type. The empty signature
/// holds none.
pub(super) fn applications<'run, 's>(
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'s>,
    kt: Handle,
) -> Option<BumpVec<'s, Application<'run>>> {
    let mut set = BumpVec::new_in(scratch);
    let mut one = |kt: Handle| match types.node(kt) {
        TypeNode::Signature { .. } if kt == Handle::EMPTY_SIGNATURE => true,
        TypeNode::Signature { .. } => {
            set.push(Application {
                signature: kt,
                pins: &[],
            });
            true
        }
        TypeNode::SignatureApply { signature, pins } => {
            set.push(Application {
                signature: signature.raw(),
                pins: pins.raw(),
            });
            true
        }
        _ => false,
    };
    match types.node(kt) {
        TypeNode::SignatureMeet { members } => {
            for member in members.iter() {
                let read = one(*member);
                debug_assert!(read, "a meet's members are applications");
            }
        }
        _ if !one(kt) => return None,
        _ => {}
    }
    Some(set)
}

/// R-5 for two applications: `lower` lies under `upper` when both apply one signature and `lower`'s
/// pins include `upper`'s, each at an equal type. Two different pins of one parameter are
/// unordered, whatever their types' own order.
pub(super) fn application_under(lower: Application<'_>, upper: Application<'_>) -> bool {
    lower.signature == upper.signature
        && upper
            .pins
            .iter()
            .all(|(name, pinned)| lower.pin(name.symbol()) == Some(*pinned))
}

/// R-5 for two sets: `lower` lies under `upper` when each application of `upper` lies above some
/// application of `lower`. The empty set — `Module` — is the top.
pub(super) fn applications_under(lower: &[Application<'_>], upper: &[Application<'_>]) -> bool {
    upper
        .iter()
        .all(|u| lower.iter().any(|l| application_under(*l, *u)))
}

/// The meet of the signature types `members` as the handles of its applications: every application
/// any member holds, less each one lying above another — of two equal, the first — sorted by
/// handle. What [`TypeRegistry::signature_meet`] interns, and exact: it is the greatest set lying
/// under every member.
pub(super) fn canonical_applications<'s>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'s>,
    members: &[Handle],
) -> BumpVec<'s, Handle> {
    let mut all = BumpVec::new_in(scratch);
    for member in members {
        let set = applications(types, scratch, *member)
            .expect("a signature meet is over signature types");
        all.extend_from_slice(&set);
    }
    let mut kept = BumpVec::with_capacity_in(all.len(), scratch);
    for (index, application) in all.iter().enumerate() {
        let above_another = all.iter().enumerate().any(|(other, peer)| {
            other != index
                && application_under(*peer, *application)
                && (other < index || !application_under(*application, *peer))
        });
        if !above_another {
            kept.push(application.handle(types, scratch));
        }
    }
    kept.sort_unstable();
    kept.dedup();
    kept
}
