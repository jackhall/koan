use crate::dispatch::runtime::{Frame, KError, KErrorKind};
use crate::dispatch::kfunction::NodeId;

use super::lift::lift_kobject;
use super::nodes::NodeOutput;
use super::scheduler::Scheduler;

impl<'a> Scheduler<'a> {
    /// Walk slots that returned `Done(Forward)` while owning a per-call frame; for each
    /// whose forward chain has resolved to a Value, lift the Value into the captured arena
    /// (the per-call scope's `outer.arena`) and drop the frame's slot-Rc. Called after
    /// every iteration of `execute`'s main loop.
    ///
    /// Reads the sidecar `frame_holding_slots` rather than scanning all `nodes`. Slots
    /// whose chain hasn't resolved yet stay in the sidecar for a future iteration; slots
    /// that finalize get removed.
    pub(super) fn finalize_ready_frames(&mut self) {
        let mut still_waiting: Vec<usize> = Vec::with_capacity(self.frame_holding_slots.len());
        for idx in std::mem::take(&mut self.frame_holding_slots) {
            if !self.is_result_ready(NodeId(idx)) {
                still_waiting.push(idx);
                continue;
            }
            // Capture the immediate `Forward` target before the lift overwrites
            // `results[idx]` with the terminal Value/Err. `Scheduler::free` recurses
            // through Forward chain links and dep trees, naturally stopping at any
            // still-live slot (queued, frame-holding, or freshly reused) via its
            // `nodes[i].is_some()` guard. Top-level dispatch slots are unreachable from
            // any chain (Forward only targets internal Binds), so they're never visited.
            let chain_target = match self.results[idx].as_ref() {
                Some(NodeOutput::Forward(t)) => Some(t.index()),
                _ => None,
            };
            match self.read_result(NodeId(idx)) {
                Ok(value) => {
                    let (dest, lifted_obj, function) = {
                        let node = self.nodes[idx].as_ref().unwrap();
                        let frame = node
                            .frame
                            .as_ref()
                            .expect("frame_holding_slot must own a frame");
                        let dest = node
                            .scope
                            .outer
                            .expect("per-call scope must have an outer (its captured scope)")
                            .arena;
                        let lifted_obj = lift_kobject(value, frame);
                        (dest, lifted_obj, node.function)
                    };
                    // Runtime return-type check: same enforcement as the direct Done(Value)
                    // path in `execute`. Forward-chain finalizers (a user-fn body that
                    // spawned a Bind for a sub-expression) land here instead.
                    let typecheck_failed = if let Some(f) = function {
                        let rt = &f.signature.return_type;
                        if !rt.matches_value(&lifted_obj) {
                            let err = KError::new(KErrorKind::TypeMismatch {
                                arg: "<return>".to_string(),
                                expected: rt.name(),
                                got: lifted_obj.ktype().name(),
                            })
                            .with_frame(Frame {
                                function: f.summarize(),
                                expression: f.summarize(),
                            });
                            self.results[idx] = Some(NodeOutput::Err(err));
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    if !typecheck_failed {
                        let lifted = dest.alloc_object(lifted_obj);
                        self.results[idx] = Some(NodeOutput::Value(lifted));
                    }
                }
                Err(e) => {
                    // Forward chain ended in an error. Append this slot's function
                    // frame (if any) so the trace records that the error happened
                    // inside this user-fn — non-tail-call chains, where the body
                    // forwards through a Bind, surface their function this way.
                    let owned = e.clone_for_propagation();
                    let with_frame = match self.nodes[idx].as_ref().unwrap().function {
                        Some(f) => owned.with_frame(Frame {
                            function: f.summarize(),
                            expression: f.summarize(),
                        }),
                        None => owned,
                    };
                    self.results[idx] = Some(NodeOutput::Err(with_frame));
                }
            }
            // Drop the slot's frame and clear the node. If the lifted value cloned an Rc,
            // the per-call arena lives on (closure escape); otherwise this is the last
            // strong reference and the arena frees.
            self.nodes[idx] = None;
            // Reclaim the now-collapsed Forward chain (and any dep sub-trees those links
            // own). `free` walks recursively and skips frame-holders/queued slots, so a
            // chain that dives into another in-flight user-fn call leaves that subtree
            // for that call's own finalize iteration.
            if let Some(t) = chain_target {
                self.free(t);
            }
        }
        self.frame_holding_slots = still_waiting;
    }
}
