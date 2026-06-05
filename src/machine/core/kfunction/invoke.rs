use std::rc::Rc;

use super::body::split_body_statements;
use crate::machine::core::{
    assemble_body_chain, BindingIndex, CallArena, KError, KErrorKind, LexicalFrame, RuntimeArena,
    Scope,
};
use crate::machine::model::types::{
    elaborate_type_expr, DeferredReturn, ElabResult, Elaborator, KType, ReturnType,
    SignatureElement,
};
use crate::machine::model::values::KObject;
use crate::machine::ResolveTypeExprOutcome;

use super::argument_bundle::ArgumentBundle;
use super::body::{Body, BodyResult};
use super::scheduler_handle::SchedulerHandle;
use super::KFunction;

#[cfg(test)]
mod tests;

/// Resolution of a `ReturnType::Deferred` carrier at dispatch time. The Combine
/// consumes this to decide whether the per-call return type is already known or
/// must be read from the sub-Dispatch's result.
enum PerCallReturnType<'a> {
    Ready(KType<'a>),
    Pending(crate::machine::NodeId),
}

impl<'a> KFunction<'a> {
    /// Run this function's body for an already-bound call. User-defined functions
    /// allocate a per-call child scope, bind parameters into it, and return a tail-call
    /// so the caller's slot is rewritten in place. The per-call child scope is the
    /// substrate for closure capture.
    pub fn invoke(
        &'a self,
        scope: &'a Scope<'a>,
        sched: &mut dyn SchedulerHandle<'a>,
        bundle: ArgumentBundle<'a>,
    ) -> BodyResult<'a> {
        match &self.body {
            Body::Builtin(f) => f(scope, sched, bundle),
            Body::UserDefined(expr) => {
                let outer = self.captured_scope();
                // Tail-reuse: when this invoke is the body of a TCO Replace step and the
                // previous slot's frame is uniquely owned, reset it in place and reuse
                // the shell. `try_reset_for_tail` relinks the child scope's `outer` so
                // this works across mutual tail calls between fns with different
                // captured scopes.
                let frame: Rc<CallArena> = sched
                    .try_take_reusable_frame_for_tail()
                    .and_then(|mut prev| prev.try_reset_for_tail(outer).then_some(prev))
                    .unwrap_or_else(|| CallArena::new(outer, None));
                let (inner_arena, child): (&'a RuntimeArena, &'a Scope<'a>) =
                    frame.anchored_parts();
                for (name, rc) in bundle.args.iter() {
                    let mut cloned = rc.deep_clone();
                    // Splice-time element check + stamp for parameterized carriers
                    // (`:(LIST OF T)`, `:(MAP K -> V)`, `:(Result T E)`). Dispatch-time
                    // admission is shape-only and cannot do content-recursive element
                    // checking, so it lands here, symmetric with the return boundary
                    // in the Deferred Combine below.
                    if let Some(arg) = signature_argument_by_name(self, name) {
                        if is_parameterized_carrier(&arg.ktype) {
                            if !arg.ktype.matches_value(&cloned) {
                                return BodyResult::Err(
                                    KError::new(KErrorKind::TypeMismatch {
                                        arg: name.clone(),
                                        expected: arg.ktype.name(),
                                        got: cloned.ktype().name(),
                                    })
                                    .with_frame(
                                        crate::machine::Frame::bare(
                                            self.summarize(),
                                            self.summarize(),
                                        ),
                                    ),
                                );
                            }
                            cloned = cloned.stamp_type(&arg.ktype);
                        }
                    }
                    let allocated = inner_arena.alloc_object(cloned);
                    // Type-denoting parameters write ONLY into `bindings.types`;
                    // ATTR-on-type projects through that carrier for `Er.pure(x)`-style
                    // references. Value-typed parameters write `bindings.data`.
                    let is_type_denoting = signature_argument_by_name(self, name)
                        .map(|a| a.ktype.is_type_denoting())
                        .unwrap_or(false);
                    // FN parameters bind at idx 0; the body's statements sit at idx >= 1,
                    // so the strict `idx < cutoff` rule makes the parameters visible to the
                    // body (same as MATCH / TRY `it`).
                    let param_index = BindingIndex::value(0);
                    if !is_type_denoting {
                        // Signature parser enforces parameter-name uniqueness.
                        let _ = child.bind_value(name.clone(), allocated, param_index);
                    }
                    if let Some(arg) = signature_argument_by_name(self, name) {
                        if arg.ktype.is_type_denoting() {
                            match type_identity_for(name, allocated, &arg.ktype, outer) {
                                Ok(Some(identity)) => {
                                    child.register_type(name.clone(), identity, param_index);
                                }
                                Ok(None) => {}
                                Err(e) => return BodyResult::Err(e),
                            }
                        }
                    }
                }
                let body_expr = expr.clone();
                // Multi-statement bodies split into N statements: the first N-1 run as
                // sibling sub-slots at chain indices 1..N-1, the FN's slot tail-replaces
                // into the last at index N.
                let body_statements = split_body_statements(body_expr);
                let n = body_statements.len();

                match &self.signature.return_type {
                    ReturnType::Resolved(_) => {
                        if n >= 2 {
                            let call_site_chain = sched
                                .current_lexical_chain()
                                .expect("FN invoke runs inside an enter_block / active_chain");
                            let body_chain_parent =
                                assemble_body_chain(child, call_site_chain.clone(), 0)
                                    .parent
                                    .clone();
                            let mut stmts = body_statements;
                            let last = stmts.pop().expect("n >= 2");
                            for (i, stmt) in stmts.into_iter().enumerate() {
                                let chain =
                                    LexicalFrame::push(body_chain_parent.clone(), child.id, i + 1);
                                sched.with_active_frame(frame.clone(), &mut |s| {
                                    s.add_dispatch_with_chain(stmt.clone(), child, chain.clone());
                                });
                            }
                            BodyResult::tail_with_frame_at_index(last, frame, self, n)
                        } else {
                            let only = body_statements
                                .into_iter()
                                .next()
                                .expect("split_body_statements always yields >= 1");
                            BodyResult::tail_with_frame(only, frame, self)
                        }
                    }
                    // Deferred return type: `TypeExpr` is elaborated inline against
                    // `child`; `Expression` is dispatched as a sub-slot whose
                    // `KTypeValue` result is joined by the Combine.
                    ReturnType::Deferred(d) => {
                        let per_call_ret: PerCallReturnType<'a> = match d {
                            DeferredReturn::TypeExpr(te) => {
                                let mut el = Elaborator::new(child);
                                let kt = match elaborate_type_expr(&mut el, te) {
                                    ElabResult::Done(kt) => kt,
                                    // Park / Unbound here is a protocol break: the
                                    // parameter-name install and the fn_def carrier scan
                                    // should jointly guarantee resolution. In release
                                    // fall back to `Any` so the body's own dispatch
                                    // surfaces the real error.
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
                        // Assemble the body chain from the call-site chain plus the
                        // FN's lexical outer walk so chain depth tracks lexical nesting,
                        // not call depth. Multi-statement bodies use indices `1..=N` so
                        // the strict `b.idx < c` sibling-order predicate works.
                        let call_site_chain = sched
                            .current_lexical_chain()
                            .expect("FN invoke runs inside an enter_block / active_chain");
                        let body_chain_parent =
                            assemble_body_chain(child, call_site_chain.clone(), 0)
                                .parent
                                .clone();
                        let mut body_ids: Vec<crate::machine::core::kfunction::NodeId> =
                            Vec::with_capacity(n);
                        for (i, stmt) in body_statements.into_iter().enumerate() {
                            // Body statements sit at idx 1..N; idx 0 is reserved for params.
                            let idx = i + 1;
                            let chain =
                                LexicalFrame::push(body_chain_parent.clone(), child.id, idx);
                            let mut bid = None;
                            sched.with_active_frame(frame.clone(), &mut |s| {
                                bid = Some(s.add_dispatch_with_chain(
                                    stmt.clone(),
                                    child,
                                    chain.clone(),
                                ));
                            });
                            body_ids.push(bid.expect("body dispatch must spawn"));
                        }
                        let body_terminal_idx = body_ids.len() - 1;

                        // Combine deps: body statements at [0..N], optional return-type
                        // sub-Dispatch at [N]. Finish reads `results[body_terminal_idx]`
                        // as the body value and `results[N]` (if present) as the per-call
                        // return type carrier.
                        let mut deps = body_ids;
                        if let PerCallReturnType::Pending(t) = per_call_ret {
                            deps.push(t);
                        }
                        let function_summary = self.summarize();
                        let combine_id =
                            sched.add_combine(
                                deps,
                                vec![],
                                child,
                                Box::new(move |_scope, _sched, results| {
                                    let body_value: &KObject<'_> = results[body_terminal_idx];
                                    let per_call_ret: KType<'_> = match per_call_ret {
                                        PerCallReturnType::Ready(kt) => kt,
                                        PerCallReturnType::Pending(_) => {
                                            match results.get(body_terminal_idx + 1).copied() {
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
                                            }
                                        }
                                    };
                                    if !per_call_ret.matches_value(body_value) {
                                        return BodyResult::Err(
                                            KError::new(KErrorKind::TypeMismatch {
                                                arg: "<return>".to_string(),
                                                expected: format!(
                                                    "{} (per-call return type)",
                                                    per_call_ret.name(),
                                                ),
                                                got: body_value.ktype().name(),
                                            })
                                            .with_frame(crate::machine::Frame::bare(
                                                function_summary.clone(),
                                                function_summary.clone(),
                                            )),
                                        );
                                    }
                                    // Stamp the carrier to the resolved per-call return type
                                    // at the return boundary, symmetric with the param-bind
                                    // stamp above.
                                    let stamped = body_value.deep_clone().stamp_type(&per_call_ret);
                                    BodyResult::Value(_scope.arena.alloc_object(stamped))
                                }),
                            );
                        // Rc clones inside `with_active_frame` above keep the per-call
                        // arena alive across sub-slot lifetimes; the FN's slot retains
                        // its own `frame` via `defer_to_lift`'s frame-stay-attached
                        // contract.
                        drop(frame);
                        BodyResult::DeferTo(combine_id)
                    }
                }
            }
        }
    }
}

/// True iff `ktype` is a parameterized carrier whose `matches_value` does
/// content-recursive element checking — the only slots where dispatch-time
/// shape-only admission leaves an element check undone. `KExpression` is never
/// one of these, so gating here keeps the unevaluated-expression slot path untouched.
fn is_parameterized_carrier(ktype: &KType) -> bool {
    matches!(
        ktype,
        KType::List(_) | KType::Dict(_, _) | KType::ConstructorApply { .. }
    )
}

/// Indirection from a bundle iteration (keyed by `name`) back to the declared
/// parameter on `f.signature`.
fn signature_argument_by_name<'a>(
    f: &'a KFunction<'a>,
    param_name: &str,
) -> Option<&'a crate::machine::model::types::Argument<'a>> {
    f.signature.elements.iter().find_map(|el| match el {
        SignatureElement::Argument(a) if a.name == param_name => Some(a),
        _ => None,
    })
}

/// Compute the per-call type-language identity for a parameter whose declared `KType`
/// is type-denoting (caller gates on `KType::is_type_denoting`). Returns the `KType`
/// to register in the per-call scope's `bindings.types`.
///
/// | Declared `KType`               | Bound `KObject`                                  | Identity                                              |
/// | ------------------------------ | ------------------------------------------------ | ----------------------------------------------------- |
/// | `Signature { .. }` (slot)      | `KTypeValue(KType::Module { module, frame })`    | `KType::Module { module, frame }` (same carrier)      |
/// | `AnyModule`                    | `KTypeValue(KType::Module { module, frame })`    | same                                                  |
/// | `AnySignature`                 | `KTypeValue(KType::Signature { .. })`            | `KType::Signature { .. }` (same carrier)              |
/// | `Type`                         | `KTypeValue(kt)`                                 | `kt.clone()`                                          |
/// | `TypeExprRef`                  | `KTypeValue(kt)`                                 | `kt.clone()`                                          |
/// | `TypeExprRef`                  | `TypeNameRef(t)`                                 | elaborated via `definition_scope.resolve_type_expr`   |
///
/// `Ok(None)`: carrier shape didn't match any row; skip the type-side install.
/// `Err(TypeIdentityPendingAtDispatch)`: a `TypeNameRef` elaborates against
/// `definition_scope` but the result still references a pending-finalize type.
pub(crate) fn type_identity_for<'a>(
    param_name: &str,
    obj: &KObject<'a>,
    declared: &KType<'a>,
    definition_scope: &'a Scope<'a>,
) -> Result<Option<KType<'a>>, KError> {
    match declared {
        KType::Signature { .. } => Ok(match obj {
            KObject::KTypeValue(kt @ KType::Module { .. }) => Some(kt.clone()),
            _ => None,
        }),
        KType::AnyModule => Ok(match obj {
            KObject::KTypeValue(kt @ KType::Module { .. }) => Some(kt.clone()),
            _ => None,
        }),
        KType::AnySignature => Ok(match obj {
            KObject::KTypeValue(kt @ KType::Signature { .. }) => Some(kt.clone()),
            _ => None,
        }),
        KType::Type => Ok(match obj {
            KObject::KTypeValue(kt) => Some(kt.clone()),
            _ => None,
        }),
        KType::TypeExprRef => match obj {
            KObject::KTypeValue(kt) => Ok(Some(kt.clone())),
            // Resolved against the definition scope at call time, where every type the
            // signature names is already finalized — no lexical-order gating applies.
            KObject::TypeNameRef(t) => match definition_scope.resolve_type_expr(t, None) {
                ResolveTypeExprOutcome::Done(kt) => Ok(Some(kt.clone())),
                ResolveTypeExprOutcome::Park(pending_on) => {
                    Err(KError::new(KErrorKind::TypeIdentityPendingAtDispatch {
                        param: param_name.to_string(),
                        surface: t.render(),
                        pending_on,
                    }))
                }
                // Unbound: skip the type-side install; the body's value-side
                // dispatch will surface the real error.
                ResolveTypeExprOutcome::Unbound(_) => Ok(None),
            },
            _ => Ok(None),
        },
        _ => Ok(None),
    }
}
