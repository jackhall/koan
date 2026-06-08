use crate::machine::model::ArgValue;
use std::rc::Rc;

use super::body::split_body_statements;
use crate::machine::core::{
    assemble_body_chain, BindingIndex, CallArena, KError, KErrorKind, LexicalFrame, Scope,
};
use crate::machine::model::types::{
    elaborate_type_expr, DeferredReturn, ElabResult, Elaborator, KType, ReturnType,
    SignatureElement,
};
use crate::machine::model::values::Carried;
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
        sched: &mut dyn SchedulerHandle<'a, 'a>,
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
                // The per-call re-anchor is concentrated in `with_anchored_child`: parameters
                // (values whose type carries the caller's `'a`) allocate into the frame arena
                // and bind into the frame's child scope inside the closure, so the seed itself
                // fabricates no `&'a`.
                let bind_params =
                    frame.with_anchored_child(|inner_arena, child| -> Result<(), KError> {
                        for (name, arg_val) in bundle.args.iter() {
                            // FN parameters bind at idx 0; the body's statements sit at idx >= 1,
                            // so the strict `idx < cutoff` rule makes the parameters visible to
                            // the body (same as MATCH / TRY `it`).
                            let param_index = BindingIndex::value(0);
                            match arg_val {
                                // A value parameter writes `bindings.data`.
                                ArgValue::Object(rc) => {
                                    let mut cloned = rc.deep_clone();
                                    // Splice-time element check + stamp for parameterized
                                    // carriers (`:(LIST OF T)`, `:(MAP K -> V)`, `:(Result T E)`).
                                    // Dispatch-time admission is shape-only and cannot do
                                    // content-recursive element checking, so it lands here,
                                    // symmetric with the return boundary in the Deferred Combine.
                                    if let Some(arg) = signature_argument_by_name(self, name) {
                                        if is_parameterized_carrier(&arg.ktype) {
                                            if !arg.ktype.matches_value(&cloned) {
                                                return Err(KError::new(
                                                    KErrorKind::TypeMismatch {
                                                        arg: name.clone(),
                                                        expected: arg.ktype.name(),
                                                        got: cloned.ktype().name(),
                                                    },
                                                )
                                                .with_frame(crate::machine::Frame::bare(
                                                    self.summarize(),
                                                    self.summarize(),
                                                )));
                                            }
                                            cloned = cloned.stamp_type(&arg.ktype);
                                        }
                                    }
                                    let allocated = inner_arena.alloc_object(cloned);
                                    // Signature parser enforces parameter-name uniqueness.
                                    let _ = child.bind_value(name.clone(), allocated, param_index);
                                }
                                // A type-denoting parameter writes ONLY into `bindings.types`;
                                // ATTR-on-type projects through that carrier for `Er.pure(x)`-style
                                // references.
                                ArgValue::Type(kt) => match type_identity_for(name, kt, outer) {
                                    Ok(Some(identity)) => {
                                        child.register_type(name.clone(), identity, param_index);
                                    }
                                    Ok(None) => {}
                                    Err(e) => return Err(e),
                                },
                            }
                        }
                        Ok(())
                    });
                if let Err(e) = bind_params {
                    return BodyResult::Err(e);
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
                            let body_scope_id = frame.scope_for_bind().id;
                            let body_chain_parent = assemble_body_chain(
                                frame.scope_for_bind(),
                                call_site_chain.clone(),
                                0,
                            )
                            .parent
                            .clone();
                            let mut stmts = body_statements;
                            let last = stmts.pop().expect("n >= 2");
                            for (i, stmt) in stmts.into_iter().enumerate() {
                                let chain = LexicalFrame::push(
                                    body_chain_parent.clone(),
                                    body_scope_id,
                                    i + 1,
                                );
                                sched.with_active_frame(frame.clone(), &mut |s| {
                                    s.add_dispatch_with_chain_in_frame(stmt.clone(), chain.clone());
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
                                // Resolved at call time against the per-call `child` (the same
                                // re-anchor `with_anchored_child` concentrates for params): the
                                // params bind at index 0 (visible) and every outer type the
                                // signature named is already finalized. A deferred return type
                                // references a parameter (that is why it deferred), so there is
                                // no lexical-forward-reference case to gate here.
                                let kt = frame.with_anchored_child(|_arena, child| {
                                    let mut el = Elaborator::new(child);
                                    match elaborate_type_expr(&mut el, te) {
                                        ElabResult::Done(kt) => kt,
                                        // Park / Unbound here is a protocol break: the
                                        // parameter-name install and the fn_def carrier scan
                                        // should jointly guarantee resolution. In release fall
                                        // back to `Any` so the body's own dispatch surfaces the
                                        // real error.
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
                                    }
                                });
                                PerCallReturnType::Ready(kt)
                            }
                            DeferredReturn::Expression(e) => {
                                let cloned = e.clone();
                                let mut tid = None;
                                sched.with_active_frame(frame.clone(), &mut |s| {
                                    tid = Some(s.add_dispatch_in_frame(cloned.clone()));
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
                        let body_scope_id = frame.scope_for_bind().id;
                        let body_chain_parent =
                            assemble_body_chain(frame.scope_for_bind(), call_site_chain.clone(), 0)
                                .parent
                                .clone();
                        let mut body_ids: Vec<crate::machine::core::kfunction::NodeId> =
                            Vec::with_capacity(n);
                        for (i, stmt) in body_statements.into_iter().enumerate() {
                            // Body statements sit at idx 1..N; idx 0 is reserved for params.
                            let idx = i + 1;
                            let chain =
                                LexicalFrame::push(body_chain_parent.clone(), body_scope_id, idx);
                            let mut bid = None;
                            sched.with_active_frame(frame.clone(), &mut |s| {
                                bid = Some(
                                    s.add_dispatch_with_chain_in_frame(stmt.clone(), chain.clone()),
                                );
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
                        // The Combine reads `child` (in the per-call arena) when it runs, but its
                        // deps — the body and return-type sub-Dispatches — drop their arena Rc
                        // clones on Done, so without its own clone the arena would free before the
                        // Combine runs. `with_active_frame` stamps the Combine node's `frame` with a
                        // per-call-arena Rc, keeping `child` live until the Combine completes.
                        let mut combine_slot: Option<crate::machine::core::kfunction::NodeId> =
                            None;
                        let mut pending = Some((deps, per_call_ret, function_summary));
                        sched.with_active_frame(frame.clone(), &mut |s| {
                            let (deps, per_call_ret, function_summary) =
                                pending.take().expect("with_active_frame body runs once");
                            combine_slot = Some(s.add_combine_in_frame(
                                deps,
                                vec![],
                                Box::new(move |_scope, _sched, results| {
                                    let body_carried = results[body_terminal_idx];
                                    let per_call_ret: KType<'_> = match per_call_ret {
                                        PerCallReturnType::Ready(kt) => kt,
                                        PerCallReturnType::Pending(_) => {
                                            match results.get(body_terminal_idx + 1).copied() {
                                                Some(Carried::Type(kt)) => kt.clone(),
                                                Some(Carried::Object(other)) => {
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
                                    let mismatch = |got: String| {
                                        BodyResult::Err(
                                            KError::new(KErrorKind::TypeMismatch {
                                                arg: "<return>".to_string(),
                                                expected: format!(
                                                    "{} (per-call return type)",
                                                    per_call_ret.name(),
                                                ),
                                                got,
                                            })
                                            .with_frame(crate::machine::Frame::bare(
                                                function_summary.clone(),
                                                function_summary.clone(),
                                            )),
                                        )
                                    };
                                    match body_carried {
                                        // A type-valued body (e.g. a functor returning a module,
                                        // or `(Er)` returning a type-class param) rides the type
                                        // channel: validate the type against the declared return
                                        // slot and pass it through without value stamping.
                                        Carried::Type(body_type) => {
                                            if !per_call_ret.matches_type(body_type) {
                                                return mismatch(body_type.name());
                                            }
                                            BodyResult::ktype(
                                                _scope.arena.alloc_ktype(body_type.clone()),
                                            )
                                        }
                                        Carried::Object(body_value) => {
                                            if !per_call_ret.matches_value(body_value) {
                                                return mismatch(body_value.ktype().name());
                                            }
                                            // Stamp the carrier to the resolved per-call return
                                            // type at the return boundary, symmetric with the
                                            // param-bind stamp above.
                                            let stamped =
                                                body_value.deep_clone().stamp_type(&per_call_ret);
                                            BodyResult::value(_scope.arena.alloc_object(stamped))
                                        }
                                    }
                                }),
                            ));
                        });
                        let combine_id = combine_slot.expect("combine must spawn");
                        // The Combine node now carries its own per-call-arena Rc (stamped above),
                        // so this local clone is no longer load-bearing.
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

/// Compute the per-call type-language identity for a type-denoting parameter, given the
/// type it received in the value channel's `Type` arm. Returns the `KType` to register in
/// the per-call scope's `bindings.types`.
///
/// A [`KType::Unresolved`] transient (a bare user name the synchronous bind seam couldn't
/// lower) is elaborated against `definition_scope` — where every type the signature names is
/// already finalized, so no lexical-order gating applies. Every other type is its own identity.
///
/// `Ok(None)`: the name is unbound — skip the type-side install; the body's value-side
/// dispatch surfaces the real error.
/// `Err(TypeIdentityPendingAtDispatch)`: an `Unresolved` name elaborates against
/// `definition_scope` but the result still references a pending-finalize type.
pub(crate) fn type_identity_for<'a>(
    param_name: &str,
    kt: &KType<'a>,
    definition_scope: &'a Scope<'a>,
) -> Result<Option<KType<'a>>, KError> {
    match kt {
        KType::Unresolved(t) => match definition_scope.resolve_type_expr(t, None) {
            ResolveTypeExprOutcome::Done(resolved) => Ok(Some(resolved.clone())),
            ResolveTypeExprOutcome::Park(pending_on) => {
                Err(KError::new(KErrorKind::TypeIdentityPendingAtDispatch {
                    param: param_name.to_string(),
                    surface: t.render(),
                    pending_on,
                }))
            }
            ResolveTypeExprOutcome::Unbound(_) => Ok(None),
        },
        other => Ok(Some(other.clone())),
    }
}
