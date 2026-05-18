use std::rc::Rc;

use crate::machine::model::ast::{ExpressionPart, KExpression};

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
/// variant fires per call — replacing the prior `(Option<KType>, Option<NodeId>)`
/// pair whose "exactly one Some" invariant lived only as control flow. The
/// Combine consumes this to decide whether the per-call return type is already
/// known or must be read from `results[1]`.
enum PerCallReturnType {
    /// TypeExpr arm: elaborated against the per-call scope, ready to compare
    /// against the body value as soon as the Combine fires.
    Ready(KType),
    /// Expression arm: a sub-Dispatch has been spawned that will produce a
    /// `KObject::KTypeValue` at the carried `NodeId`. The Combine reads it
    /// from `results[1]`.
    Pending(crate::machine::NodeId),
}

impl<'a> KFunction<'a> {
    /// Run this function's body for an already-bound call. Builtins call straight through;
    /// user-defined functions allocate a per-call child scope, bind parameters into it,
    /// substitute parameter Identifiers in a body clone with `Future(value)`, and return a
    /// tail-call so the caller's slot is rewritten in place.
    ///
    /// The child scope and substitution are complementary: substitution covers parameter
    /// references in typed-slot positions (`(PRINT x)` needs `x` as a `Future(KString)`),
    /// the child scope covers Identifier-slot lookups (`(x)` parens-wrapped) and is the
    /// substrate for closure capture.
    ///
    /// Lifetime shape: the per-call `child` scope and `inner_arena` are re-anchored to `'a`
    /// — the outer slot-storage lifetime — by one consolidated `unsafe` block. The witness
    /// is the `Rc<CallArena>` (`frame`) that this function moves into the
    /// [`BodyResult::Tail`] payload: the slot stores both `frame` and the tailed expression
    /// at `'a`, so the heap-pinned arena outlives every `'a`-re-anchored read into it.
    pub fn invoke(
        &'a self,
        scope: &'a Scope<'a>,
        sched: &mut dyn SchedulerHandle<'a>,
        bundle: ArgumentBundle<'a>,
    ) -> BodyResult<'a> {
        match &self.body {
            Body::Builtin(f) => f(scope, sched, bundle),
            Body::UserDefined(expr) => {
                // Per-call frame whose arena owns the child scope, parameter clones, and
                // substituted-body allocations. `outer` is the FN's captured definition
                // scope (lexical scoping). Closure escapes whose captured scope lives in a
                // per-call arena are kept alive externally via the lifted
                // `KFunction(&fn, Some(Rc))` on the user-bound value.
                let outer = self.captured_scope();
                let frame: Rc<CallArena> = CallArena::new(outer, None);
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
                    // The signature parser enforces parameter-name uniqueness upstream, so
                    // `bind_value`'s rebind error here would indicate a signature-parser
                    // invariant break rather than a recoverable case.
                    let _ = child.bind_value(name.clone(), allocated);
                    // Module-system functor-params Stage A: for parameters whose declared
                    // `KType` is type-denoting (signature-bound module, signature value,
                    // type value, type-expr-ref, or any-module wildcard), dual-write the
                    // per-call binding into `bindings.types` on the same child scope. This
                    // is what lets a FN body's type-position references to the parameter
                    // (`Er` in a return-type or pin slot) resolve through `resolve_type`'s
                    // outer-chain walk — the per-call scope is the body's lexical parent.
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
                let substituted = substitute_params(expr.clone(), &bundle, inner_arena);

                // Module-system functor-params Stage B: deferred return-type path.
                // For `Resolved(_)`, the existing tail-call path applies — slot inherits
                // `self` as `function`, lift-time check at `execute.rs:54` fires against
                // the static `KType` (and the `ReturnType::is_resolved` gate there is
                // satisfied by construction).
                //
                // For `Deferred(_)`, the per-call return type isn't known statically:
                //
                //  * `Deferred(TypeExpr)` — elaborate inline against `child` (where
                //    Stage A's dual-write has installed parameter-name → KType
                //    identities). The elaborator can park on a placeholder only if a
                //    *new* outstanding type-binding existed at call time, which is
                //    pathological; default to `KType::Any` on park/unbound so the
                //    check doesn't reject (the body's own value-side dispatch will
                //    have already surfaced the real error).
                //  * `Deferred(Expression)` — sub-Dispatch the captured parens-form
                //    expression against `child`, then read its `KTypeValue` result.
                //
                // The body itself is dispatched as a sub-Dispatch under the per-call
                // frame (via `with_active_frame`) so the frame's `Rc` propagates to
                // both the body and the type-elab sub-slots. A `Combine` joins them;
                // the finish closure runs the slot check against the per-call type.
                // The FN's slot DeferTo's the Combine, so its terminal becomes the
                // Combine's terminal — same shape as MODULE/SIG body wrap-up.
                match &self.signature.return_type {
                    ReturnType::Resolved(_) => {
                        BodyResult::tail_with_frame(substituted, frame, self)
                    }
                    ReturnType::Deferred(d) => {
                        // Resolve the per-call return-type carrier. Exactly one
                        // `PerCallReturnType` variant fires per call; the Combine
                        // below consumes it to decide whether the per-call return
                        // type is known eagerly or must be read from `results[1]`.
                        let per_call_ret: PerCallReturnType = match d {
                            DeferredReturn::TypeExpr(te) => {
                                let mut el = Elaborator::new(child);
                                let kt = match elaborate_type_expr(&mut el, te) {
                                    ElabResult::Done(kt) => kt,
                                    // Park / Unbound at the dispatch boundary is a
                                    // protocol break: Stage A's dual-write installs
                                    // every parameter the carrier could reference on
                                    // `bindings.types`, and the parameter-name scan in
                                    // `fn_def.rs` only picks `Deferred(TypeExpr)` when
                                    // the carrier has a parameter-leaf reference. Either
                                    // arm here means a regression in one of those
                                    // invariants — debug-assert to catch it in tests,
                                    // fall back to `Any` in release so the body's own
                                    // value-side problems still surface normally.
                                    ElabResult::Park(_) => {
                                        debug_assert!(
                                            false,
                                            "deferred return-type TypeExpr parked at dispatch \
                                             boundary — Stage A dual-write should have made \
                                             every parameter-leaf reference resolvable",
                                        );
                                        KType::Any
                                    }
                                    ElabResult::Unbound(ref msg) => {
                                        debug_assert!(
                                            false,
                                            "deferred return-type TypeExpr unbound at dispatch \
                                             boundary: {msg} — fn_def parameter-name scan \
                                             should have prevented this carrier",
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
                        // Spawn the body Dispatch under the per-call frame.
                        let mut bid = None;
                        sched.with_active_frame(frame.clone(), &mut |s| {
                            bid = Some(s.add_dispatch(substituted.clone(), child));
                        });
                        let body_id = bid.expect("body dispatch must spawn");

                        // Build the Combine's dep list: body first, then optional
                        // return-type sub-Dispatch. Closure reads `results[0]` for
                        // the body and `results[1]` for the type when Pending.
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
                                // Surface per-call return-type rejection with the
                                // documented "per-call return type" wording the
                                // Stage B tests pin.
                                return BodyResult::Err(KError::new(
                                    KErrorKind::TypeMismatch {
                                        arg: "<return>".to_string(),
                                        expected: format!(
                                            "{} (per-call return type)",
                                            per_call_ret.name(),
                                        ),
                                        got: body_value.ktype().name(),
                                    },
                                ).with_frame(crate::machine::Frame {
                                    function: function_summary.clone(),
                                    expression: function_summary.clone(),
                                }));
                            }
                            BodyResult::Value(body_value)
                        }));
                        // Suppress unused-variable warning when `frame` would otherwise
                        // be dropped here; the Rc clones into each `with_active_frame`
                        // call above keep the per-call arena alive across the body and
                        // type-elab sub-slot lifetimes. The FN's slot itself retains
                        // its own `frame` via `defer_to_lift`'s frame-stay-attached
                        // contract, so the final Rc reference for the duration of
                        // this slot's run is the slot's `prev_frame`.
                        drop(frame);
                        BodyResult::DeferTo(combine_id)
                    }
                }
            }
        }
    }
}

/// Look up the `Argument` element on `f.signature` whose `name` matches `param_name`.
/// `bundle.args` is keyed by `name` rather than position, so this is the indirection
/// from a bundle iteration back to the declared parameter type.
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
/// is type-denoting (caller already gated on `KType::is_type_denoting`). Returns the
/// `KType` to register in the per-call scope's `bindings.types` for `param_name`.
///
/// Mapping table (matches the plan):
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
/// Returns `Ok(None)` when the carrier shape doesn't match any of the table rows —
/// the dispatcher's `matches_value` filter already ran before this site, so this case
/// indicates an `is_type_denoting`/`matches` disagreement (programming error,
/// not user error). Production behavior: skip the dual-write silently; tests can
/// debug-assert if they want stricter coverage.
///
/// Returns `Err(KError::TypeIdentityPendingAtDispatch)` when a `TypeNameRef`
/// carrier elaborates against `definition_scope` but the result references a
/// type that is still pending finalization. Surfaces the precise context
/// (parameter, surface form, pending finalize-node) instead of silently
/// skipping the dual-write — replaces the legacy silent-on-Park fallback.
///
/// `definition_scope` is the FN's captured (definition-time) scope, used to elaborate
/// a `TypeNameRef` carrier — the carrier's `TypeExpr` references type-side bindings
/// from the definition's lexical environment, not the call site's.
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
                // Unbound: the carrier was constructed at definition time but the
                // name no longer resolves here. The body's own value-side dispatch
                // will surface the real error; skip the dual-write rather than
                // erroring here to preserve the legacy unbound-at-dispatch shape.
                ResolveTypeExprOutcome::Unbound(_) => Ok(None),
            },
            _ => Ok(None),
        },
        // Any other variant is non-type-denoting; caller already gated, so this is
        // unreachable on the happy path.
        _ => Ok(None),
    }
}

/// Replace every `Identifier(name)` in `expr` whose name is in `bundle.args` with a
/// `Future(value)` allocated in `arena`. Recurses into nested `Expression`, `ListLiteral`,
/// and `DictLiteral` parts; other parts pass through unchanged.
pub(crate) fn substitute_params<'a>(
    expr: KExpression<'a>,
    bundle: &ArgumentBundle<'a>,
    arena: &'a RuntimeArena,
) -> KExpression<'a> {
    KExpression {
        parts: expr
            .parts
            .into_iter()
            .map(|p| substitute_part(p, bundle, arena))
            .collect(),
    }
}

fn substitute_part<'a>(
    part: ExpressionPart<'a>,
    bundle: &ArgumentBundle<'a>,
    arena: &'a RuntimeArena,
) -> ExpressionPart<'a> {
    match part {
        ExpressionPart::Identifier(name) => match bundle.get(&name) {
            Some(value) => {
                let allocated: &'a KObject<'a> = arena.alloc_object(value.deep_clone());
                ExpressionPart::Future(allocated)
            }
            None => ExpressionPart::Identifier(name),
        },
        ExpressionPart::Expression(boxed) => {
            ExpressionPart::Expression(Box::new(substitute_params(*boxed, bundle, arena)))
        }
        ExpressionPart::ListLiteral(items) => ExpressionPart::ListLiteral(
            items
                .into_iter()
                .map(|p| substitute_part(p, bundle, arena))
                .collect(),
        ),
        ExpressionPart::DictLiteral(pairs) => ExpressionPart::DictLiteral(
            pairs
                .into_iter()
                .map(|(k, v)| {
                    (
                        substitute_part(k, bundle, arena),
                        substitute_part(v, bundle, arena),
                    )
                })
                .collect(),
        ),
        other => other,
    }
}
