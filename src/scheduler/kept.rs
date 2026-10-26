//! What a step keeps of a value at its true home, and the values a step holds beside it.
//!
//! A carrier's home is the cell that lifted its value, not where the value's bytes are: a part of
//! a parameter, lifted in a frame, is homed in the frame. A [`Kept`] is a carrier put to rest at the
//! value's true home instead — the home of the carrier a frame was handed it in, or of the one a
//! child delivered it in — so the value can cross on from there, pinned free wherever that home is
//! an ancestor, rather than rebuilt from the cell that read it. A [`Held`] value carries the kept
//! part it is, or is a restamp of, where it has one; the veneer's finishes and the doors that pass
//! a value on read it ([`Step::finish`](super::Step::finish)). See
//! [README.md § What a step may name](README.md#what-a-step-may-name).

use crate::knot::{KValue, KValueFamily};
use crate::memory::Dormant;
use crate::values::FieldPath;

/// A value at rest at its true home: what a frame keeps of an argument, its callee or a binder's
/// delivered value. Opaque and brand-free; only the veneer redeems one.
#[derive(Clone, Copy)]
pub struct Kept<'graph>(pub(super) Dormant<'graph, KValueFamily>);

/// A kept value, or the part of it a path of record field labels reaches.
#[derive(Clone, Copy)]
pub struct KeptPart<'graph, 'cell> {
    pub kept: Kept<'graph>,
    pub path: FieldPath<'cell>,
}

/// A value a step holds, beside the kept part it is, or is a restamp of, where it has one.
///
/// **Invariant:** `value` is the part `kept` reaches along its path, or a restamp of that part —
/// the same storage seen at another type. Whatever builds one with a kept part upholds it; the
/// veneer checks it, in a debug build, where it redeems one.
#[derive(Clone, Copy)]
pub struct Held<'graph, 'cell> {
    pub value: KValue<'graph, 'cell>,
    pub kept: Option<KeptPart<'graph, 'cell>>,
}

impl<'graph, 'cell> Held<'graph, 'cell> {
    /// `value`, with no kept part: built or read where it is held.
    pub fn bare(value: KValue<'graph, 'cell>) -> Self {
        Held { value, kept: None }
    }

    /// `value`, a restamp of this held value's storage or something else built from it: it keeps
    /// this one's kept part only where it is one storage with it.
    pub fn becoming(self, value: KValue<'graph, 'cell>) -> Self {
        let kept = self.kept.filter(|_| value.same_storage(&self.value));
        Held { value, kept }
    }
}

impl<'graph, 'cell> From<KValue<'graph, 'cell>> for Held<'graph, 'cell> {
    fn from(value: KValue<'graph, 'cell>) -> Self {
        Held::bare(value)
    }
}
