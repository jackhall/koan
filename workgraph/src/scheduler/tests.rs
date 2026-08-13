//! The scheduler's white-box slates and the fixtures they share.
//!
//! Every fixture here names only stand-in types — a `Cart`-backed anchor, a boxed `dyn FnOnce`
//! continuation — never a koan type, so the slates exercise the generic scheduler on its own terms.
//!
//! - [`continuation`] — the owned-tier continuation slot under Miri (tree borrows).
//! - [`edges`] — the edge slab's alloc/release recycling and the install door's branches.

use std::rc::Rc;

use super::{Anchor, Workload};
use crate::witnessed::doctest_fixture::Cart;
use crate::witnessed::reattachable;

mod continuation;
mod edges;

/// A lifetime-free `Reattachable` family for the trivial test value.
struct U32Value;
/// The trivial extern operand a step with nothing to zip passes to the owned tier's one open verb.
struct UnitOperand;
reattachable! {
    U32Value => u32,
    UnitOperand => (),
}

/// The stored-continuation shape: a boxed `dyn FnOnce` over a captured region borrow — a **fat**
/// pointer whose `At<'static>` needs drop, so it rests only on the owned tier.
struct DynContinuation;
reattachable!(droppable DynContinuation => Box<dyn FnOnce() -> u32 + 'r>);

/// The per-slot memory anchor: an `Rc<Cart>` whose backing `Vec` is the region a continuation
/// borrows into. Sealing the continuation against `Rc<TestAnchor>` therefore transitively pins that
/// backing for the seal's whole dormant life.
struct TestAnchor(Rc<Cart>);

impl Anchor for TestAnchor {
    type Owner = Cart;
    fn owner(&self) -> &Rc<Self::Owner> {
        &self.0
    }
}

struct TestWorkload;
impl Workload for TestWorkload {
    type Value = U32Value;
    type Error = ();
    type Frame = TestAnchor;
    type Continuation = DynContinuation;
}
