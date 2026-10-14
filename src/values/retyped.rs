//! A knot member's header: the member beside the type it is retyped to.
//!
//! A record is retyped by writing a new header over its shared runs ([`Record::with_type`]); a knot
//! member has none, since its type lives in the node every holder shares. [`Retyped`] is that
//! header: the bare member and the type it shows, the persistent form of the [`Seen`] a read
//! carries. It holds the member, not a value, so a retype of a retyped value is one header over
//! the same member. See [README.md § The type memo and `satisfies`](README.md#the-type-memo-and-satisfies).
//!
//! [`Record::with_type`]: super::Record
//! [`Seen`]: super::Seen

use crate::memory::{Writer, resident};
use crate::type_lattice::{DeclaredType, KType};

use super::Knotted;

/// A bare knot member beside the type it is retyped to: never its node's own type, and a scheme
/// only for a callable.
#[derive(Clone, Copy, Debug)]
pub struct Retyped<X> {
    member: X,
    ktype: DeclaredType<KType>,
}

impl<X: Knotted> Retyped<X> {
    /// `member` retyped to `ktype`, laid down in `writer`'s region.
    pub fn of<'cell>(
        writer: Writer<'cell, '_>,
        member: X,
        ktype: DeclaredType<KType>,
    ) -> &'cell Self
    where
        X: 'cell,
    {
        debug_assert!(
            member.ktype() != ktype,
            "a header retypes a member to a type other than its own"
        );
        resident(writer, Retyped { member, ktype })
    }

    /// The bare member.
    pub fn member(&self) -> X {
        self.member
    }

    /// The type the member is retyped to.
    pub fn ktype(&self) -> DeclaredType<KType> {
        self.ktype
    }
}
