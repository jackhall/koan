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
//! A crossing re-lays a component another half holds with [`Component::absolute`], over a table
//! that is already absolute, so it ties nothing.
//!
//! The table sits behind [`Tied`], an atomic pointer beside a fixed length that erases its slice's
//! lifetime, so a node stays covariant in `'run`, as a `Cell<&'run [T]>` would not, and a sealed
//! component a frozen load holds can be read from any thread. Its read is the crate's one `unsafe`; its soundness
//! rests on writes this module alone makes. See [identity.md](identity.md) § Recursive groups.

#![allow(unsafe_code)]

use std::marker::PhantomData;
use std::sync::atomic::{AtomicPtr, Ordering};

use crate::bump::LaysRuns;

use super::digest::{ComponentMember, TypeDigest};
use super::tiers::lay_one;

/// A sealed component: its digest, and each member's name, kind and schema in canonical order.
/// Nominally `pub` so a view can carry it.
pub struct Component<'run> {
    pub(super) digest: TypeDigest,
    members: Tied<'run, ComponentMember<'run>>,
}

const _: () = assert!(!std::mem::needs_drop::<Component<'static>>());

impl<'run> Component<'run> {
    /// Lay a component through `lay` over `relative`, its members as the digest read them, then
    /// repoint it at `absolute(component)`. Inside `absolute` the component reads `relative`, so
    /// the seal may intern its members, which read only names and kinds; every reader of the
    /// returned component reads what `absolute` answered.
    pub(super) fn tie(
        lay: impl LaysRuns<'run>,
        digest: TypeDigest,
        relative: &'run [ComponentMember<'run>],
        absolute: impl FnOnce(&'run Self) -> &'run [ComponentMember<'run>],
    ) -> &'run Self {
        let component: &'run Self = lay_one(
            lay,
            Self {
                digest,
                members: Tied::new(relative),
            },
        );
        component.members.set(absolute(component));
        component
    }

    /// Lay a component through `lay` over `members`, whose schemas are already absolute: how a
    /// crossing re-lays a sealed component another half holds. Nothing reads the component before
    /// it is laid, so it needs no tie.
    pub(super) fn absolute(
        lay: impl LaysRuns<'run>,
        digest: TypeDigest,
        members: &'run [ComponentMember<'run>],
    ) -> &'run Self {
        lay_one(
            lay,
            Self {
                digest,
                members: Tied::new(members),
            },
        )
    }

    /// The members, in canonical order.
    pub(super) fn members(&self) -> &'run [ComponentMember<'run>] {
        self.members.get()
    }
}

/// A slice held by a lifetime-erased pointer beside a fixed length, so its holder is covariant in
/// `'run` and `T`, and `Sync` where `T` is.
///
/// **Soundness.** [`get`](Self::get) reads the slice as `&'run [T]`, which holds while every slice
/// the cell has held lives for `'run` and has the cell's length. A covariant holder can be viewed at
/// a shorter lifetime, so a write through such a view could store a slice that dies first; hence the
/// one write, [`set`](Self::set), is private here, and its one caller, [`Component::tie`], writes
/// once, into the component it has just laid, a slice of that component's own `'run` and of the
/// same length. The write is relaxed: every reader on another thread is spawned after the load's
/// last write, and a spawn synchronizes.
struct Tied<'run, T> {
    ptr: AtomicPtr<()>,
    len: usize,
    held: PhantomData<&'run [T]>,
}

impl<'run, T> Tied<'run, T> {
    fn new(slice: &'run [T]) -> Self {
        Self {
            ptr: AtomicPtr::new(slice.as_ptr() as *mut ()),
            len: slice.len(),
            held: PhantomData,
        }
    }

    fn set(&self, slice: &'run [T]) {
        assert_eq!(slice.len(), self.len, "a tie keeps its table's length");
        self.ptr.store(slice.as_ptr() as *mut (), Ordering::Relaxed);
    }

    fn get(&self) -> &'run [T] {
        // SAFETY: the pointer came from a `&'run [T]` of `self.len` elements in `new` or `set`,
        // and the type's soundness note holds every such slice to `'run`. The slice is only read.
        unsafe { std::slice::from_raw_parts(self.ptr.load(Ordering::Relaxed) as *const T, self.len) }
    }
}
