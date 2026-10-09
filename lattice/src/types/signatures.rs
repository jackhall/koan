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

use super::handle::Handle;
use super::node::TypeNode;
use super::registry::TypeRegistry;
use super::stored::Linked;

/// One application: `signature`, a [`TypeNode::Signature`], with `pins` fixing some of its head
/// parameters by name. Empty pins for the signature on its own. The pins live in `'p`: the region
/// for an application read off a node, a scratch for one a caller assembles.
#[derive(Clone, Copy, Debug)]
pub(super) struct Application<'p, 'run> {
    pub(super) signature: Linked<'run>,
    pub(super) pins: &'p [(BinderSymbol, Linked<'run>)],
}

impl<'run> Application<'_, 'run> {
    /// The node this application interns to: the signature itself for no pins.
    pub(super) fn node(
        self,
        types: &TypeRegistry<'run>,
        scratch: BumpAllocator<'_>,
    ) -> Linked<'run> {
        types.signature_apply(scratch, self.signature, self.pins)
    }

    /// The type `self` pins the parameter named `name` to, if it pins it.
    pub(super) fn pin(self, name: Symbol) -> Option<Linked<'run>> {
        self.pins
            .iter()
            .find(|(key, _)| key.symbol() == name)
            .map(|(_, pinned)| *pinned)
    }
}

/// Whether `kt` is a signature type — one of the three nodes [`applications`] reads.
pub(super) fn is_signature_type(kt: Linked<'_>) -> bool {
    matches!(
        kt.view(),
        TypeNode::Signature { .. }
            | TypeNode::SignatureApply { .. }
            | TypeNode::SignatureMeet { .. }
    )
}

/// The applications `kt` holds, or `None` where `kt` is no signature type. The empty signature
/// holds none.
pub(super) fn applications<'run, 's>(
    scratch: BumpAllocator<'s>,
    kt: Linked<'run>,
) -> Option<BumpVec<'s, Application<'run, 'run>>> {
    let mut set = BumpVec::new_in(scratch);
    let mut one = |kt: Linked<'run>| match kt.view() {
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
                signature,
                pins: pins.raw(),
            });
            true
        }
        _ => false,
    };
    match kt.view() {
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
pub(super) fn application_under<'run>(
    lower: Application<'_, 'run>,
    upper: Application<'_, 'run>,
) -> bool {
    lower.signature == upper.signature
        && upper
            .pins
            .iter()
            .all(|(name, pinned)| lower.pin(name.symbol()) == Some(*pinned))
}

/// R-5 for two sets: `lower` lies under `upper` when each application of `upper` lies above some
/// application of `lower`. The empty set — `Module` — is the top.
pub(super) fn applications_under<'run>(
    lower: &[Application<'_, 'run>],
    upper: &[Application<'_, 'run>],
) -> bool {
    upper
        .iter()
        .all(|u| lower.iter().any(|l| application_under(*l, *u)))
}

/// The meet of the signature types `members` as the nodes of its applications: every application
/// any member holds, less each one lying above another — of two equal, the first — sorted by
/// handle. What [`TypeRegistry::signature_meet`] interns, and exact: it is the greatest set lying
/// under every member.
pub(super) fn canonical_applications<'s, 'run>(
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'s>,
    members: &[Linked<'run>],
) -> BumpVec<'s, Linked<'run>> {
    let mut all = BumpVec::new_in(scratch);
    for member in members {
        let set = applications(scratch, member).expect("a signature meet is over signature types");
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
            kept.push(application.node(types, scratch));
        }
    }
    kept.sort_unstable();
    kept.dedup();
    kept
}
