//! A first-class type as a value: the handle it names and the `OfKind` type it has.

use crate::memory::Writer;
use crate::type_lattice::{KType, TypeRegistry};

use super::resident;

/// A type in value position. Its own type is the kind of the type it names, memoized at
/// construction, so a kind slot checks it with one lattice relation.
#[derive(Clone, Copy, Debug)]
pub struct TypeValue {
    handle: KType,
    ktype: KType,
}

impl TypeValue {
    /// The type value naming `handle`, typed `OfKind` of `handle`'s kind.
    pub fn new<'cell>(
        writer: Writer<'cell>,
        handle: KType,
        types: &TypeRegistry<'_>,
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

    /// This value's own type: `OfKind` of the named type's kind.
    pub fn ktype(&self) -> KType {
        self.ktype
    }

    /// The same value written into another region.
    pub(crate) fn copied<'cell>(&self, writer: Writer<'cell>) -> &'cell TypeValue {
        resident(writer, *self)
    }
}
