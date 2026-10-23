//! A first-class type as a value: the handle it names and the `OfKind` type it has — its kind's,
//! or one a retype to a kind above it stamped.

use super::Chain;
use crate::memory::{Writer, resident};
use crate::type_lattice::KType;

/// A type in value position. Its own type is the kind of the type it names, memoized at
/// construction, so a kind slot checks it with one lattice relation.
#[derive(Clone, Copy, Debug)]
pub struct TypeValue {
    handle: KType,
    ktype: KType,
}

impl TypeValue {
    /// The type value naming `handle`, typed `OfKind` of `handle`'s kind, which `types` reads and
    /// never interns.
    pub fn new<'cell>(
        writer: Writer<'cell, '_>,
        handle: KType,
        types: &Chain<'_, '_, '_, '_>,
    ) -> &'cell TypeValue {
        resident(
            writer,
            TypeValue {
                handle,
                ktype: KType::of_kind(handle.kind_of(types)),
            },
        )
    }

    /// The type this value names.
    pub fn handle(&self) -> KType {
        self.handle
    }

    /// This value's own type: `OfKind` of the named type's kind, or the kind a retype stamped.
    pub fn ktype(&self) -> KType {
        self.ktype
    }

    /// The type value naming the same type at `ktype`, a kind its own lies under, in `writer`'s
    /// region: what a retype to a kind hands on.
    pub fn with_type<'cell>(&self, writer: Writer<'cell, '_>, ktype: KType) -> &'cell TypeValue {
        resident(
            writer,
            TypeValue {
                handle: self.handle,
                ktype,
            },
        )
    }

    /// The same value written into another region.
    pub(crate) fn copied<'cell>(&self, writer: Writer<'cell, '_>) -> &'cell TypeValue {
        resident(writer, *self)
    }
}
