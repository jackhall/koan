//! [`Component`] — a sealed strongly-connected component, laid once in the region and shared by its
//! members, and the one cycle the region holds.
//!
//! A recursive member's absolute schema names the member, so the member's node must exist before
//! the schema, yet the member has to reach its schema. [`Component::tie`] closes that loop in one
//! step: it lays the component holding its members as the digest read them, hands it to the seal,
//! which interns the members and rewrites each schema absolute against them, then repoints the
//! component at the rewritten table. A reader sees the absolute table from then on; inside the seal
//! the component reads relative. Either table is fully laid, so no read meets uninitialized memory
//! and none can fail.
//!
//! The table sits behind [`Tied`], a cell that erases its slice's lifetime so a node stays covariant
//! in `'run`, as a `Cell<&'run [T]>` would not. Its read is the crate's one `unsafe`; its soundness
//! rests on writes this module alone makes. See [identity.md](identity.md) § Recursive groups.

#![allow(unsafe_code)]

use std::cell::Cell;
use std::marker::PhantomData;

use crate::bump::BumpAllocator;

use super::digest::{ComponentMember, TypeDigest};

/// A sealed component: its digest, and each member's name, kind and schema in canonical order.
/// Nominally `pub` so a view can carry it.
pub struct Component<'run> {
    pub(super) digest: TypeDigest,
    members: Tied<'run, ComponentMember<'run>>,
}

const _: () = assert!(!std::mem::needs_drop::<Component<'static>>());

impl<'run> Component<'run> {
    /// Lay a component in `bump` over `relative`, its members as the digest read them, then
    /// repoint it at `absolute(component)`. Inside `absolute` the component reads `relative`, so
    /// the seal may intern its members, which read only names and kinds; every reader of the
    /// returned component reads what `absolute` answered.
    pub(super) fn tie(
        bump: BumpAllocator<'run>,
        digest: TypeDigest,
        relative: &'run [ComponentMember<'run>],
        absolute: impl FnOnce(&'run Self) -> &'run [ComponentMember<'run>],
    ) -> &'run Self {
        let component: &'run Self = bump.alloc(Self {
            digest,
            members: Tied::new(relative),
        });
        component.members.set(absolute(component));
        component
    }

    /// The members, in canonical order.
    pub(super) fn members(&self) -> &'run [ComponentMember<'run>] {
        self.members.get()
    }
}

/// A slice held by a lifetime-erased pointer, so its holder is covariant in `'run` and `T`.
///
/// **Soundness.** [`get`](Self::get) reads the slice as `&'run [T]`, which holds while every slice
/// the cell has held lives for `'run`. A covariant holder can be viewed at a shorter lifetime, so
/// a write through such a view could store a slice that dies first; hence the one write,
/// [`set`](Self::set), is private here, and its one caller, [`Component::tie`], writes once, into
/// the component it has just laid, a slice of that component's own `'run`.
struct Tied<'run, T> {
    slice: Cell<*const [()]>,
    held: PhantomData<&'run [T]>,
}

impl<'run, T> Tied<'run, T> {
    fn new(slice: &'run [T]) -> Self {
        Self {
            slice: Cell::new(slice as *const [T] as *const [()]),
            held: PhantomData,
        }
    }

    fn set(&self, slice: &'run [T]) {
        self.slice.set(slice as *const [T] as *const [()]);
    }

    fn get(&self) -> &'run [T] {
        // SAFETY: the pointer came from a `&'run [T]` in `new` or `set`, and the type's soundness
        // note holds every such slice to `'run`. A slice cast keeps its element count, and the
        // slice is only ever read.
        unsafe { &*(self.slice.get() as *const [T]) }
    }
}
