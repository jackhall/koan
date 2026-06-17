//! Generic run-lifetime storage substrate. Owns the erase-store machinery (the irreducible
//! `unsafe`), the cycle-redirect `escape` pointer, and an address membership side-table — and
//! names no workload type. A [`StorageProfile`] injects its storage families via [`Stored`]; the single
//! [`StorageFrame::alloc`] engine runs the cycle gate uniformly. The gate is unbypassable because
//! [`StorageFrame::storage`] is private and `alloc` is the only path that reaches it — no `&Arena`
//! ever escapes, so no `Stored` impl can route a value around the redirect.
//!
//! The Koan instantiation (`RuntimeArena = StorageFrame<KoanStorageProfile>`, the family `Stored` impls,
//! the cycle-gate walkers) lives in [`super::arena`]. See
//! [memory-model.md § Arena lifetime erasure](../../../design/memory-model.md#arena-lifetime-erasure)
//! for the transmute soundness argument and
//! [per-call-arena-protocol.md § Cycle gate](../../../design/per-call-arena-protocol.md#cycle-gate-on-alloc_object)
//! for the redirect `alloc` enforces.

use std::cell::RefCell;
use std::mem::ManuallyDrop;
use std::ptr::NonNull;

use typed_arena::Arena;

use super::reattach::pin_deref;

/// A workload's declaration of what a [`StorageFrame`] stores for it. `Storage` is the bundle of
/// typed sub-arenas the frame owns; the workload's [`Stored`] impls project each family out of it.
pub trait StorageProfile {
    type Storage: Default;
}

/// Per-family storage policy, implemented by the workload. Keyed on the stored type's `'static`
/// form (`At<'static> == Self`); a live value enters the engine as `At<'a>`. One trait carries
/// every storage-safety answer for a family — which sub-arena it lands in, whether it would
/// self-cycle, and any post-store side effect — so [`StorageFrame::alloc`] reasons about the
/// gate-erase-store sequence once instead of forking it per type.
///
/// Not sealed: this is the workload's extension point. Unbypassability comes from elsewhere — the
/// engine is the only path to the private [`StorageFrame::storage`], so an impl can supply policy
/// but cannot route a value around the cycle gate.
pub trait Stored<W: StorageProfile>: Sized + 'static {
    /// The lifetime family of the stored type. `At<'static>` is `Self`. Because the engine keys on
    /// the `'static` form, the live and stored forms are both projections of this one GAT and
    /// cannot name different constructors — a wrong binding fails to compile in the safe wrapper.
    type At<'a>;
    /// Project this family's sub-arena out of the workload storage bundle. This return type is the
    /// binding chokepoint: storing `At<'static>` into `Arena<Self::At<'static>>` only type-checks
    /// when the family is wired to the matching sub-arena.
    fn sub_arena(storage: &W::Storage) -> &Arena<Self::At<'static>>;
    /// True iff any descendant of `value` carries an anchor back to the frame at `self_ptr` — i.e.
    /// storing it there would form a self-referential cycle. Families that hold no anchor return
    /// `false` as a deliberate declaration.
    fn anchors_to(value: &Self::At<'_>, self_ptr: *const StorageFrame<W>) -> bool;
    /// Post-store hook, run inside the engine on the *final* storing frame (after any escape
    /// redirect). Default no-op; a family overrides it to record the stored address for
    /// [`StorageFrame::owns_addr`] membership queries.
    fn record_local(_frame: &StorageFrame<W>, _stored: &Self::At<'static>) {}
}

/// Lifetime-erase a stored value's live form to its `'static` form by moving it through a union. A
/// generic `mem::transmute::<K::At<'a>, K::At<'static>>` will not compile — the compiler cannot
/// prove the two GAT projections share a size — so the move-through-union form stands in, with a
/// `const` assert restoring the size check `transmute` would emit.
fn erase_store<'a, W: StorageProfile, K: Stored<W>>(value: K::At<'a>) -> K::At<'static> {
    const { assert!(size_of::<K::At<'a>>() == size_of::<K::At<'static>>()) };
    union Erase<A, B> {
        live: ManuallyDrop<A>,
        stored: ManuallyDrop<B>,
    }
    let e = Erase::<K::At<'a>, K::At<'static>> {
        live: ManuallyDrop::new(value),
    };
    // SAFETY: `At<'a>` and `At<'static>` share layout — a lifetime never changes a type's size or
    // representation. The value is moved into the union once and exactly one `ManuallyDrop` field
    // is read out, so a single drop runs (no leak, no double-free).
    ManuallyDrop::into_inner(unsafe { std::ptr::read(&e.stored) })
}

/// Run-lifetime allocation frame. Lives for one program run (or one per-call frame). Sub-arenas
/// store `K::At<'static>` (phantom); each [`alloc`](Self::alloc) re-anchors to the caller's `'a`
/// on the way out.
pub struct StorageFrame<W: StorageProfile> {
    /// The workload's typed sub-arena bundle. PRIVATE and never exposed by reference: the only
    /// path in is [`alloc`](Self::alloc), which runs the cycle gate, so the gate is unbypassable
    /// by construction.
    storage: W::Storage,
    /// Stable addresses of values a family opts to record (via [`Stored::record_local`]), backing
    /// [`owns_addr`](Self::owns_addr). `usize` rather than `*const _` keeps the field
    /// lifetime-erased and `Send`/`Sync`-neutral.
    membership: RefCell<Vec<usize>>,
    /// Redirect target for the cycle gate. `None` on a run-root frame. Stable for `self`'s
    /// lifetime: the per-call frame heap-pins the outer via `Rc` and the outer outlives this inner
    /// per the lexical-scoping invariant. `NonNull` because a `Some` escape is always a live frame
    /// address, never null.
    escape: Option<NonNull<StorageFrame<W>>>,
}

impl<W: StorageProfile> StorageFrame<W> {
    pub fn new() -> Self {
        Self {
            storage: W::Storage::default(),
            membership: RefCell::new(Vec::new()),
            escape: None,
        }
    }

    /// `alloc` will redirect self-cyclic values to `escape`; see the cycle gate in [`alloc`](Self::alloc).
    pub fn with_escape(escape: NonNull<StorageFrame<W>>) -> Self {
        Self {
            storage: W::Storage::default(),
            membership: RefCell::new(Vec::new()),
            escape: Some(escape),
        }
    }

    /// Number of values stored in family `K`'s sub-arena. Read-only; exposes no `&Arena`, so it
    /// cannot be used to bypass the gate.
    pub(crate) fn family_len<K: Stored<W>>(&self) -> usize {
        K::sub_arena(&self.storage).len()
    }

    /// Whether `addr` was recorded by a prior [`Stored::record_local`] on this frame.
    pub(crate) fn owns_addr(&self, addr: usize) -> bool {
        self.membership.borrow().contains(&addr)
    }

    /// Record `addr` into the membership side-table. Called by a family's
    /// [`Stored::record_local`]; the only writer.
    pub(crate) fn record_addr(&self, addr: usize) {
        self.membership.borrow_mut().push(addr);
    }

    /// Single allocator engine for any family `K`. Runs the cycle gate — a value that would
    /// self-cycle (its [`Stored::anchors_to`] is true for `self`) redirects to the escape frame —
    /// then erases the live form to `'static`, stores it in the family's sub-arena, fires
    /// [`Stored::record_local`] on the final storing frame, and re-anchors the store to `'a`. The
    /// sole store path: `storage` is private, so this gate cannot be skipped.
    ///
    /// SAFETY of the `escape_ptr.as_ref()`: `escape_ptr` was set by the frame constructor to an
    /// outer frame's address that outlives `self` (the per-call frame nests inside it, heap-pinned
    /// by `Rc`). So `'a` (bounded by `&self`) is a valid lifetime to attach to the dereferenced
    /// escape pointer.
    pub(crate) fn alloc<'a, K: Stored<W>>(&'a self, value: K::At<'a>) -> &'a K::At<'a> {
        if let Some(escape_ptr) = self.escape {
            if K::anchors_to(&value, self as *const StorageFrame<W>) {
                let escape_ref: &'a StorageFrame<W> = unsafe { pin_deref(escape_ptr.as_ptr()) };
                return escape_ref.alloc::<K>(value);
            }
        }
        let stored: &'a mut K::At<'static> =
            K::sub_arena(&self.storage).alloc(erase_store::<W, K>(value));
        let p: *const K::At<'static> = stored;
        // The post-store hook fires on the final storing frame (this one, after any redirect
        // above), so a recorded address tracks its true owner.
        K::record_local(self, unsafe { pin_deref(p) });
        // SAFETY: `At<'static>`/`At<'a>` share layout; re-anchor the `'static` store to the
        // frame-bounded `'a`. The returned `&'a` cannot outlive `&'a self`, so no `'static`-claiming
        // reference escapes the frame's own borrow.
        //
        // The `'static` → `'a` cast only changes the lifetime parameter, which clippy can't see, so
        // it reads as a no-op cast despite being load-bearing.
        #[allow(clippy::unnecessary_cast)]
        unsafe {
            &*(p as *const K::At<'a>)
        }
    }
}

impl<W: StorageProfile> Default for StorageFrame<W> {
    fn default() -> Self {
        Self::new()
    }
}
