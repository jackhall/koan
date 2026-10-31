//! What a step keeps of a value at its true home, and the values a step holds beside it.
//!
//! A carrier's home is the cell that lifted its value, not where the value's bytes are: a part of
//! a parameter, lifted in a frame, is homed in the frame. A [`Kept`] is a carrier put to rest at the
//! value's true home instead — the home of the carrier a frame was handed it in, or of the one a
//! child delivered it in — so the value can cross on from there, pinned free wherever that home is
//! an ancestor, rather than rebuilt from the cell that read it. A [`Held`] value carries the kept
//! part it is, or is a restamp of, where it has one; the veneer's finishes and the doors that pass
//! a value on read it ([`Step::finish`](super::Step::finish)). See
//! [README.md § Kept and held values](README.md#kept-and-held-values).

use crate::knot::module::layout;
use crate::knot::{KValue, KValueFamily};
use crate::memory::{Active, Dormant, Ready, Writer};
use crate::scheduler::action::Context;
use crate::scheduler::continuation::StepBundle;
use crate::values::{PartPath, PathSegment};

/// A value at rest at its true home: what a frame keeps of an argument, its callee or a binder's
/// delivered value. Opaque and brand-free; only the veneer redeems one.
#[derive(Clone, Copy)]
pub struct Kept<'graph>(pub(super) Dormant<'graph, KValueFamily>);

/// A kept value, or the part of it a path of segments reaches: a record field, a function's
/// capture, a module's member or a tagged value's payload at each step.
#[derive(Clone, Copy)]
pub struct KeptPart<'graph, 'cell> {
    pub kept: Kept<'graph>,
    pub path: PartPath<'cell>,
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

impl<'graph> KeptPart<'graph, '_> {
    /// This part's kept value redeemed into the step `context` runs, and projected along its path,
    /// each segment read as [`Value::along`](crate::values::Value::along),
    /// [`Value::captured`](crate::values::Value::captured), [`layout::stored_member`] or
    /// [`Value::untagged`](crate::values::Value::untagged) reads it:
    /// the carrier of the storage this part names, at its true home. `None` where it does not
    /// redeem or the path reaches nothing.
    pub(super) fn redeemed<'step, 'here, 'scratch, 'lent, B: StepBundle<'graph>>(
        &self,
        context: &Context<'graph, 'step, 'here, 'scratch, 'lent, B>,
    ) -> Option<Ready<'graph, 'step, KValueFamily>>
    where
        'graph: 'step + 'here + 'lent,
        'here: 'scratch,
        'lent: 'scratch,
    {
        let carrier = context.redeem(self.kept.0).ok()?;
        let path = self.path;
        if path.is_empty() {
            return Some(carrier);
        }
        context.project::<KValueFamily, KValueFamily>(&carrier, |value, _| {
            let mut at = value.value();
            for segment in path {
                at = match *segment {
                    PathSegment::Label(label) => at.along(label)?,
                    PathSegment::Capture(slot) => at.captured(slot)?,
                    PathSegment::Member(slot) => layout::stored_member(at, slot as usize)?,
                    PathSegment::Payload => at.untagged()?,
                };
            }
            Some(Active::new(at))
        })
    }
}

impl<'graph, 'cell> KeptPart<'graph, 'cell> {
    /// This part, one segment further down, the path laid through `writer`.
    pub fn extended(self, writer: Writer<'cell, '_>, segment: PathSegment) -> Self {
        let path = self.path;
        KeptPart {
            kept: self.kept,
            path: writer.fill(path.len() + 1, |at| {
                path.get(at).copied().unwrap_or(segment)
            }),
        }
    }
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
