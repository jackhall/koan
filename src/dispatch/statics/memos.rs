//! Where the load places a content digest's memos: the slots that keep one, each `:|` that reads
//! them, and each call that hands one on
//! ([where a digest is demanded](../../../design/modules.md#a-module-is-its-content)).
//!
//! A body-born module's content is its code over what it captures, computed where a demand first
//! reaches it. A `:|` over a module binder read bare is such a demand, and each capture its content
//! composes that the module read from a slot of the view's own activation chain is a value that
//! activation may digest again — a second view over another module capturing it, or a later hop
//! of a loop that passes it on. So that slot is **digested**: it keeps a memo beside its value,
//! which the `:|` fills and reads ([`ViewSlots`]). A call that passes a digested slot bare hands
//! its memo ([`Handoff`]) to the parameter it binds, which the callee keeps where it is digested
//! too. A slot is marked when its `:|` is typed, so a hand-off is recorded only for a call typed
//! after it: the under-approximation loses a saving, never soundness, since a memo is only ever
//! filled from the value it sits beside.

use crate::memory::{BumpVec, collect};
use crate::parse::ExpressionPart;
use crate::scope::{
    CaptureSlot, CaptureSource, Coordinate, Handoff, ShapeKind, Site, Slot, Target, ViewSlots,
};

use super::{Form, Pass, of_part};

impl<'p, 'graph: 'p, 'tier: 'p> Pass<'p, '_, 'graph, 'tier, '_, '_> {
    /// Record where the captures of the module the `:|` at `site` in the shape at `level` views
    /// keep their memos, where its `operand` is a module binder of this activation chain read
    /// bare, and mark each capture's source slot digested.
    pub(super) fn view_slots(
        &mut self,
        level: usize,
        site: Site,
        operand: &'graph ExpressionPart<'graph>,
    ) {
        if self.unfilled {
            return;
        }
        let Some((hops, slot)) = self.local_read(level, operand) else {
            return;
        };
        let born = level - hops as usize;
        let Some(body) =
            (self.chain[born].shape.births(slot)).filter(|body| body.kind() == ShapeKind::Module)
        else {
            return;
        };
        let mut captures = BumpVec::new_in(self.scratch);
        for (index, capture) in body.captures().iter().enumerate() {
            let CaptureSource::Read(Coordinate::Activation {
                hops: out,
                target: Target::Local(source),
            }) = capture.source
            else {
                continue;
            };
            if !body.composes(CaptureSlot(index as u32)) {
                continue;
            }
            let Some(at) = born.checked_sub(out as usize) else {
                continue;
            };
            captures.push((
                index as u32,
                Coordinate::Activation {
                    hops: (level - at) as u32,
                    target: Target::Local(source),
                },
            ));
            self.chain[at].digested.push(source);
        }
        if !captures.is_empty() {
            let captures = collect(self.scratch, captures.iter().copied());
            self.chain[level].view_slots.push((
                site,
                ViewSlots {
                    body: body.segment(),
                    captures,
                },
            ));
        }
    }

    /// The coordinate of the digested slot `part`, read in the shape at `level`, names bare.
    pub(super) fn digested_read(
        &self,
        level: usize,
        part: &'graph ExpressionPart<'graph>,
    ) -> Option<Coordinate> {
        let (hops, slot) = self.local_read(level, part)?;
        let at = level - hops as usize;
        self.chain[at]
            .digested
            .contains(&slot)
            .then_some(Coordinate::Activation {
                hops,
                target: Target::Local(slot),
            })
    }

    /// Record the `handed` arguments of the call keyed by `site` in the shape at `level`.
    pub(super) fn handoffs(&mut self, level: usize, site: Site, handed: &[Handoff]) {
        if !self.unfilled && !handed.is_empty() {
            let handed = collect(self.scratch, handed.iter().copied());
            self.chain[level].handoffs.push((site, handed));
        }
    }

    /// Where `part`, read in the shape at `level`, names a slot of this activation chain bare: the
    /// hops out to it and the slot.
    fn local_read(
        &self,
        level: usize,
        part: &'graph ExpressionPart<'graph>,
    ) -> Option<(u32, Slot)> {
        let shape = self.chain[level].shape;
        let Form::Name(name) = of_part(shape, part) else {
            return None;
        };
        match shape.mention(Site::of(name))?.coordinate {
            Coordinate::Activation {
                hops,
                target: Target::Local(slot),
            } if (hops as usize) <= level => Some((hops, slot)),
            _ => None,
        }
    }
}
