use std::rc::Rc;

use crate::machine::core::{
    CallArena, KError, KErrorKind, ResolveTypeExprOutcome, RuntimeArena, Scope,
};
use crate::machine::model::types::{
    elaborate_type_expr, DeferredReturn, ElabResult, Elaborator, KType, ReturnType,
    SignatureElement, UserTypeKind,
};
use crate::machine::model::values::KObject;

use super::argument_bundle::ArgumentBundle;
use super::body::{Body, BodyResult};
use super::scheduler_handle::SchedulerHandle;
use super::KFunction;

#[cfg(test)]
mod tests;

/// Resolution of a `ReturnType::Deferred` carrier at dispatch time. Exactly one
/// variant fires per call; the Combine consumes this to decide whether the
/// per-call return type is already known or must be read from `results[1]`.
enum PerCallReturnType {
    Ready(KType),
    /// Sub-Dispatch spawned that will produce a `KObject::KTypeValue` at the
    /// carried `NodeId`.
    Pending(crate::machine::NodeId),
}

impl<'a> KFunction<'a> {
    /// Run this function's body for an already-bound call. User-defined functions
    /// allocate a per-call child scope, bind parameters into it, and return a tail-call
    /// so the caller's slot is rewritten in place.
    ///
    /// Parameter references resolve against the per-call child scope at dispatch time;
    /// the same scope is the substrate for closure capture.
    ///
    /// Lifetime: the per-call `child` scope and `inner_arena` are re-anchored to `'a` —
    /// the outer slot-storage lifetime — by one consolidated `unsafe` block. The witness
    /// is the `Rc<CallArena>` (`frame`) moved into [`BodyResult::Tail`]: the slot stores
    /// both `frame` and the tailed expression at `'a`, so the heap-pinned arena outlives
    /// every `'a`-re-anchored read into it.
    pub fn invoke(
        &'a self,
        scope: &'a Scope<'a>,
        sched: &mut dyn SchedulerHandle<'a>,
        bundle: ArgumentBundle<'a>,
    ) -> BodyResult<'a> {
        match &self.body {
            Body::Builtin(f) => f(scope, sched, bundle),
            Body::UserDefined(expr) => {
                // `outer` is the FN's captured definition scope (lexical scoping).
                // Closure escapes whose captured scope lives in a per-call arena are
                // kept alive externally via the lifted `KFunction(&fn, Some(Rc))` on
                // the user-bound value.
                let outer = self.captured_scope();
                // Tail-reuse: when this invoke is the body of a TCO Replace step and
                // the previous slot's frame is uniquely owned (no closure / sub-slot
                // escaped a clone), reset it in place and reuse the shell instead of
                // allocating a new `CallArena`. Falls through to a fresh `CallArena`
                // for the first call and for any iteration whose previous frame
                // escaped. Re-link's the child scope's `outer` to the new FN's
                // captured scope, so this works across mutual tail calls between
                // user-fns whose captured scopes differ.
                let frame: Rc<CallArena> = sched
                    .try_take_reusable_frame_for_tail()
                    .and_then(|mut prev| prev.try_reset_for_tail(outer).then_some(prev))
                    .unwrap_or_else(|| CallArena::new(outer, None));
                // SAFETY (consolidated): both re-anchors below share one witness — `frame`
                // is moved into `BodyResult::Tail` below, whose slot-storage lifetime is
                // `'a`. The `Rc<CallArena>` heap-pins the per-call arena (and therefore
                // its scope) for as long as the slot lives, so claiming `'a` here is
                // exactly the receiver-bound-borrow → slot-storage-lifetime re-anchor that
                // `NodeStore::reinstall_with_frame` performs on the scheduler side after
                // a Replace.
                let (inner_arena, child): (&'a RuntimeArena, &'a Scope<'a>) = unsafe {
                    (
                        std::mem::transmute::<&RuntimeArena, &'a RuntimeArena>(frame.arena()),
                        std::mem::transmute::<&Scope<'_>, &'a Scope<'a>>(frame.scope()),
                    )
                };
                for (name, rc) in bundle.args.iter() {
                    let cloned = rc.deep_clone();
                    let allocated = inner_arena.alloc_object(cloned);
                    // Signature parser enforces parameter-name uniqueness; a rebind
                    // error here would mean an upstream invariant break.
                    let _ = child.bind_value(name.clone(), allocated);
                    // For type-denoting parameters, dual-write the per-call binding
                    // into `bindings.types` so the FN body's type-position references
                    // to the parameter resolve through `resolve_type`'s outer-chain walk.
                    if let Some(arg) = signature_argument_by_name(self, name) {
                        if arg.ktype.is_type_denoting() {
                            match type_identity_for(name, allocated, &arg.ktype, outer) {
                                Ok(Some(identity)) => {
                                    child.register_type(name.clone(), identity);
                                }
                                Ok(None) => {}
                                Err(e) => return BodyResult::Err(e),
                            }
                        }
                    }
                }
                let body_expr = expr.clone();

                // Deferred return-type path: the per-call return type isn't known
                // statically. `TypeExpr` is elaborated inline against `child`;
                // `Expression` is dispatched as a sub-slot whose `KTypeValue` result
                // is read by the Combine. The body itself runs as a sub-Dispatch
                // under the per-call frame; a Combine joins them and runs the
                // per-call return-type check.
                match &self.signature.return_type {
                    ReturnType::Resolved(_) => {
                        BodyResult::tail_with_frame(body_expr, frame, self)
                    }
                    ReturnType::Deferred(d) => {
                        let per_call_ret: PerCallReturnType = match d {
                            DeferredReturn::TypeExpr(te) => {
                                let mut el = Elaborator::new(child);
                                let kt = match elaborate_type_expr(&mut el, te) {
                                    ElabResult::Done(kt) => kt,
                                    // Park / Unbound here is a protocol break: the
                                    // parameter-name dual-write and the fn_def carrier
                                    // scan should jointly guarantee resolution. Assert
                                    // in debug; in release fall back to `Any` so the
                                    // body's own dispatch surfaces the real error.
                                    ElabResult::Park(_) => {
                                        debug_assert!(
                                            false,
                                            "deferred return-type TypeExpr parked at dispatch boundary",
                                        );
                                        KType::Any
                                    }
                                    ElabResult::Unbound(ref msg) => {
                                        debug_assert!(
                                            false,
                                            "deferred return-type TypeExpr unbound at dispatch boundary: {msg}",
                                        );
                                        KType::Any
                                    }
                                };
                                PerCallReturnType::Ready(kt)
                            }
                            DeferredReturn::Expression(e) => {
                                let cloned = e.clone();
                                let mut tid = None;
                                sched.with_active_frame(frame.clone(), &mut |s| {
                                    tid = Some(s.add_dispatch(cloned.clone(), child));
                                });
                                PerCallReturnType::Pending(tid.expect("type dispatch must spawn"))
                            }
                        };
                        let mut bid = None;
                        sched.with_active_frame(frame.clone(), &mut |s| {
                            bid = Some(s.add_dispatch(body_expr.clone(), child));
                        });
                        let body_id = bid.expect("body dispatch must spawn");

                        // Combine deps: body at [0], optional return-type sub-Dispatch at [1].
                        let mut deps = vec![body_id];
                        if let PerCallReturnType::Pending(t) = per_call_ret {
                            deps.push(t);
                        }
                        let function_summary = self.summarize();
                        let combine_id = sched.add_combine(deps, child, Box::new(move |_scope, _sched, results| {
                            let body_value: &KObject<'_> = results[0];
                            let per_call_ret: KType = match per_call_ret {
                                PerCallReturnType::Ready(kt) => kt,
                                PerCallReturnType::Pending(_) => match results.get(1).copied() {
                                    Some(KObject::KTypeValue(kt)) => kt.clone(),
                                    Some(other) => {
                                        return BodyResult::Err(KError::new(
                                            KErrorKind::ShapeError(format!(
                                                "FN deferred return-type expression \
                                                 produced a non-type {} value",
                                                other.ktype().name(),
                                            )),
                                        ));
                                    }
                                    None => KType::Any,
                                },
                            };
                            if !per_call_ret.matches_value(body_value) {
                                return BodyResult::Err(KError::new(
                                    KErrorKind::TypeMismatch {
                                        arg: "<return>".to_string(),
                                        expected: format!(
                                            "{} (per-call return type)",
                                            per_call_ret.name(),
                                        ),
                                        got: body_value.ktype().name(),
                                    },
                                ).with_frame(crate::machine::Frame::bare(
                                    function_summary.clone(),
                                    function_summary.clone(),
                                )));
                            }
                            BodyResult::Value(body_value)
                        }));
                        // Rc clones into each `with_active_frame` call above keep the
                        // per-call arena alive across sub-slot lifetimes; the FN's slot
                        // retains its own `frame` via `defer_to_lift`'s frame-stay-attached
                        // contract.
                        drop(frame);
                        BodyResult::DeferTo(combine_id)
                    }
                }
            }
        }
    }
}

/// Indirection from a bundle iteration (keyed by `name`) back to the declared
/// parameter on `f.signature`.
fn signature_argument_by_name<'a>(
    f: &'a KFunction<'a>,
    param_name: &str,
) -> Option<&'a crate::machine::model::types::Argument> {
    f.signature.elements.iter().find_map(|el| match el {
        SignatureElement::Argument(a) if a.name == param_name => Some(a),
        _ => None,
    })
}

/// Compute the per-call type-language identity for a parameter whose declared `KType`
/// is type-denoting (caller gates on `KType::is_type_denoting`). Returns the `KType`
/// to register in the per-call scope's `bindings.types`.
///
/// | Declared `KType`               | Bound `KObject`        | Identity                                              |
/// | ------------------------------ | ---------------------- | ----------------------------------------------------- |
/// | `SignatureBound { .. }`        | `KModule(m, _)`        | `m.ktype()` — `UserType { kind: Module, .. }`         |
/// | `AnyUserType { kind: Module }` | `KModule(m, _)`        | same                                                  |
/// | `Signature`                    | `KSignature(s)`        | `SignatureBound { sig_id: s.sig_id(), sig_path, .. }` |
/// | `Type`                         | `KTypeValue(kt)`       | `kt.clone()`                                          |
/// | `TypeExprRef`                  | `KTypeValue(kt)`       | `kt.clone()`                                          |
/// | `TypeExprRef`                  | `TypeNameRef(t)`       | elaborated via `definition_scope.resolve_type_expr`   |
///
/// `Ok(None)` means the carrier shape didn't match any row (programming error
/// downstream of an `is_type_denoting`/`matches` disagreement; skip the dual-write).
///
/// `Err(TypeIdentityPendingAtDispatch)` fires when a `TypeNameRef` elaborates against
/// `definition_scope` (the FN's captured lexical environment) but the result still
/// references a pending-finalize type.
pub(crate) fn type_identity_for<'a>(
    param_name: &str,
    obj: &KObject<'a>,
    declared: &KType,
    definition_scope: &'a Scope<'a>,
) -> Result<Option<KType>, KError> {
    match declared {
        KType::SignatureBound { .. } => Ok(match obj {
            KObject::KModule(m, _) => Some(KType::UserType {
                kind: UserTypeKind::Module,
                scope_id: m.scope_id(),
                name: m.path.clone(),
            }),
            _ => None,
        }),
        KType::AnyUserType { kind: UserTypeKind::Module } => Ok(match obj {
            KObject::KModule(m, _) => Some(KType::UserType {
                kind: UserTypeKind::Module,
                scope_id: m.scope_id(),
                name: m.path.clone(),
            }),
            _ => None,
        }),
        KType::Signature => Ok(match obj {
            KObject::KSignature(s) => Some(KType::SignatureBound {
                sig_id: s.sig_id(),
                sig_path: s.path.clone(),
                pinned_slots: Vec::new(),
            }),
            _ => None,
        }),
        KType::Type => Ok(match obj {
            KObject::KTypeValue(kt) => Some(kt.clone()),
            _ => None,
        }),
        KType::TypeExprRef => match obj {
            KObject::KTypeValue(kt) => Ok(Some(kt.clone())),
            KObject::TypeNameRef(t) => match definition_scope.resolve_type_expr(t) {
                ResolveTypeExprOutcome::Done(kt) => Ok(Some(kt.clone())),
                ResolveTypeExprOutcome::Park(pending_on) => {
                    Err(KError::new(KErrorKind::TypeIdentityPendingAtDispatch {
                        param: param_name.to_string(),
                        surface: t.render(),
                        pending_on,
                    }))
                }
                // Unbound: skip the dual-write; the body's own value-side dispatch
                // will surface the real error.
                ResolveTypeExprOutcome::Unbound(_) => Ok(None),
            },
            _ => Ok(None),
        },
        // Non-type-denoting variants: caller already gated, unreachable on the happy path.
        _ => Ok(None),
    }
}

