use std::collections::HashMap;
use std::rc::Rc;

use crate::dispatch::arena::{CallArena, RuntimeArena};
use crate::dispatch::kobject::KObject;

/// Lift a KObject value out of the dying frame's arena into the destination arena.
/// Owned variants (Number, KString, Bool, Null) `deep_clone` cleanly because their
/// content is owned. `KObject::KFunction(&f, frame)` is the special case: `&f` may
/// point into the dying frame's arena (an escaping closure). If so, we carry a clone
/// of the dying frame's `Rc<CallArena>` in the lifted value's frame field, so the
/// arena stays alive past the slot's frame drop and the `&f` reference remains valid.
/// If the function lives in a longer-lived arena (run-root or another live frame), no
/// Rc is needed and the lifted value's frame field stays `None`.
///
/// `KObject::KFuture` is handled conservatively: any unanchored KFuture lifted from
/// the dying frame gets the dying-frame Rc attached, regardless of where its `function`
/// was defined. The KFuture's `bundle.args` and `parsed.parts`' `Future(&KObject)` refs
/// can independently point into the dying arena, and we have no per-descendant arena
/// tracking to tell us whether they do — anchoring unconditionally is safe and the
/// over-keep is theoretical until KFutures escape as values (they currently don't;
/// kept for the planned async features).
///
/// Pre-existing `Some(rc)` on the input value is preserved (the value is already
/// keeping some arena alive; we don't overwrite that with the current dying frame's).
///
/// Composite variants (`List`, `Dict`) recurse to find embedded closures that need an
/// Rc attach, but memoize via `needs_lift`: when no descendant needs lifting, the
/// payload's existing `Rc` is cloned instead of rebuilding the `Vec`/`HashMap`. This
/// makes a value's second-and-later lifts through a return chain O(N) walk + O(1)
/// rebuild for the unchanged composites — Koan's collection-immutability contract is
/// what makes the structural sharing safe.
///
/// Whole-tree fast path: if the dying arena has zero `KFunction`s allocated in it, no
/// descendant `&KFunction` can point into it (per `alloc_function`'s invariant). This
/// is sound *today* because KFutures don't escape as values — the only way a lifted
/// `v` could need anchoring under this condition is via a KFuture descendant, and
/// none exist in current usage. When KFutures begin escaping (planned async), this
/// gate must add a no-unanchored-KFuture-descendant clause; the slow path's KFuture
/// arm is already correct. The check is one O(1) emptiness query on the arena.
pub(super) fn lift_kobject<'b>(v: &KObject<'b>, dying_frame: &Rc<CallArena>) -> KObject<'b> {
    if dying_frame.arena().functions_is_empty() {
        return v.deep_clone();
    }
    match v {
        KObject::KFunction(f, existing) => {
            let new_frame = if existing.is_some() {
                existing.clone()
            } else {
                let dying_runtime: *const RuntimeArena = dying_frame.arena();
                let captured_runtime: *const RuntimeArena = f.captured_scope().arena;
                if std::ptr::eq(captured_runtime, dying_runtime) {
                    Some(Rc::clone(dying_frame))
                } else {
                    None
                }
            };
            KObject::KFunction(*f, new_frame)
        }
        KObject::KFuture(t, existing) => {
            let new_frame = existing.clone().or_else(|| Some(Rc::clone(dying_frame)));
            KObject::KFuture(t.deep_clone(), new_frame)
        }
        KObject::List(items) => {
            if items.iter().any(|x| needs_lift(x, dying_frame)) {
                let lifted: Vec<KObject<'b>> = items
                    .iter()
                    .map(|x| lift_kobject(x, dying_frame))
                    .collect();
                KObject::List(Rc::new(lifted))
            } else {
                KObject::List(Rc::clone(items))
            }
        }
        KObject::Dict(entries) => {
            if entries.values().any(|x| needs_lift(x, dying_frame)) {
                let lifted: HashMap<_, _> = entries
                    .iter()
                    .map(|(k, v)| (k.clone_box(), lift_kobject(v, dying_frame)))
                    .collect();
                KObject::Dict(Rc::new(lifted))
            } else {
                KObject::Dict(Rc::clone(entries))
            }
        }
        KObject::Tagged { tag, value } => {
            if needs_lift(value, dying_frame) {
                KObject::Tagged {
                    tag: tag.clone(),
                    value: Rc::new(lift_kobject(value, dying_frame)),
                }
            } else {
                KObject::Tagged {
                    tag: tag.clone(),
                    value: Rc::clone(value),
                }
            }
        }
        other => other.deep_clone(),
    }
}

/// True iff lifting `v` against `dying_frame` would attach an `Rc` to some descendant.
/// Drives both `lift_kobject`'s top-level fast-path skip and the per-composite rebuild
/// decision: when this returns false, the existing `Rc<Vec>`/`Rc<HashMap>` can be cloned
/// instead of allocating a fresh one. Walks composites recursively but bottoms out on
/// the first match (`any`-style).
///
/// `KFuture(_, None)` returns true unconditionally, mirroring `lift_kobject`'s
/// conservative anchor for KFutures — we can't cheaply tell whether the bundle/parsed
/// borrows reach into the dying arena, so we treat any unanchored KFuture as if they
/// might.
fn needs_lift<'b>(v: &KObject<'b>, dying_frame: &Rc<CallArena>) -> bool {
    match v {
        KObject::KFunction(_, Some(_)) => false,
        KObject::KFunction(f, None) => {
            let dying_runtime: *const RuntimeArena = dying_frame.arena();
            let captured_runtime: *const RuntimeArena = f.captured_scope().arena;
            std::ptr::eq(captured_runtime, dying_runtime)
        }
        KObject::KFuture(_, Some(_)) => false,
        KObject::KFuture(_, None) => true,
        KObject::List(items) => items.iter().any(|x| needs_lift(x, dying_frame)),
        KObject::Dict(entries) => entries.values().any(|x| needs_lift(x, dying_frame)),
        KObject::Tagged { value, .. } => needs_lift(value, dying_frame),
        _ => false,
    }
}
