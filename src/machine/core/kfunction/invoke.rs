use std::rc::Rc;

use crate::machine::core::{
    assemble_body_chain, BindingIndex, CallArena, KError, KErrorKind, LexicalFrame,
    ResolveTypeExprOutcome, RuntimeArena, Scope,
};
use super::body::split_body_statements;
use crate::machine::model::types::{
    elaborate_type_expr, DeferredReturn, ElabResult, Elaborator, KType, ReturnType,
    SignatureElement,
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
enum PerCallReturnType<'a> {
    Ready(KType<'a>),
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
                    let mut cloned = rc.deep_clone();
                    // Splice-time argument element check + stamp for parameterized
                    // carrier slots (`:(List T)`, `:(Dict K V)`, `:(Result T E)`).
                    // `bundle.args` holds evaluated values here — only `KExpression`
                    // slots stay lazy by design (see `argument_bundle.rs`) — so this
                    // bind loop is a valid value boundary, symmetric with the return
                    // boundary in `execute.rs` / the Deferred Combine below. The
                    // dispatch-time shape-only admission (`Argument::matches`) cannot
                    // do the content-recursive element check, so it lands here.
                    if let Some(arg) = signature_argument_by_name(self, name) {
                        if is_parameterized_carrier(&arg.ktype) {
                            if !arg.ktype.matches_value(&cloned) {
                                return BodyResult::Err(KError::new(
                                    KErrorKind::TypeMismatch {
                                        arg: name.clone(),
                                        expected: arg.ktype.name(),
                                        got: cloned.ktype().name(),
                                    },
                                ).with_frame(crate::machine::Frame::bare(
                                    self.summarize(),
                                    self.summarize(),
                                )));
                            }
                            // Coarsen the carrier to exactly the declared slot type,
                            // mirroring the return-boundary stamp.
                            cloned = cloned.stamp_type(&arg.ktype);
                        }
                    }
                    let allocated = inner_arena.alloc(cloned);
                    // Post-collapse: for type-denoting parameters, write ONLY into
                    // `bindings.types`. ATTR-on-type (the `KTypeValue(KType::Module
                    // { .. })` / `KType::Signature(_)` arms in `attr.rs`) projects
                    // through the type-side carrier for `Er.pure(x)`-style references,
                    // dissolving the previous paired install into `bindings.data` that
                    // forced the `read_result` race at scheduler/node_store.rs:165-171.
                    //
                    // Value-typed parameters (Number, List<T>, struct types, ...)
                    // continue to write `bindings.data` via `bind_value` — they are
                    // not type-denoting and have no `bindings.types` storage to use.
                    let is_type_denoting = signature_argument_by_name(self, name)
                        .map(|a| a.ktype.is_type_denoting())
                        .unwrap_or(false);
                    // FN parameters are bound *before* the body block's first
                    // statement runs. Tag with the nominal-binder carve-out so the
                    // body's visibility test sees them — same logic the MATCH `it`
                    // binding uses. Lexically they sit "before" any in-body
                    // statement; the carve-out is what makes "before the block's
                    // first statement" work under the chain-cutoff rule.
                    let param_index =
                        BindingIndex { idx: 0, nominal_binder: true };
                    if !is_type_denoting {
                        // Signature parser enforces parameter-name uniqueness; a
                        // rebind error here would mean an upstream invariant break.
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
                // Multi-statement bodies (`((s_0) (s_1) ... (s_{N-1}))`) split into
                // N statements: the first N-1 run as sibling sub-slots in the body
                // scope (chain indices 1..N-1), and the FN's slot tail-replaces into
                // the last one (chain index N). Single-statement bodies take the
                // length-1 path through the same code.
                let body_statements = split_body_statements(body_expr);
                let n = body_statements.len();

                // Deferred return-type path: the per-call return type isn't known
                // statically. `TypeExpr` is elaborated inline against `child`;
                // `Expression` is dispatched as a sub-slot whose `KTypeValue` result
                // is read by the Combine. The body itself runs as a sub-Dispatch
                // under the per-call frame; a Combine joins them and runs the
                // per-call return-type check.
                match &self.signature.return_type {
                    ReturnType::Resolved(_) => {
                        // For multi-statement bodies, submit the first N-1 statements
                        // as siblings via the assembled body chain at indices 1..N-1,
                        // then tail-replace into the last statement at index N.
                        if n >= 2 {
                            let call_site_chain = sched
                                .current_lexical_chain()
                                .expect("FN invoke runs inside an enter_block / active_chain");
                            let body_chain_parent =
                                assemble_body_chain(child, call_site_chain.clone(), 0).parent.clone();
                            let mut stmts = body_statements;
                            let last = stmts.pop().expect("n >= 2");
                            for (i, stmt) in stmts.into_iter().enumerate() {
                                let chain = LexicalFrame::push(
                                    body_chain_parent.clone(),
                                    child.id,
                                    i + 1,
                                );
                                sched.with_active_frame(frame.clone(), &mut |s| {
                                    s.add_dispatch_with_chain(stmt.clone(), child, chain.clone());
                                });
                            }
                            BodyResult::tail_with_frame_at_index(last, frame, self, n)
                        } else {
                            let only = body_statements.into_iter().next()
                                .expect("split_body_statements always yields >= 1");
                            BodyResult::tail_with_frame(only, frame, self)
                        }
                    }
                    ReturnType::Deferred(d) => {
                        let per_call_ret: PerCallReturnType<'a> = match d {
                            DeferredReturn::TypeExpr(te) => {
                                let mut el = Elaborator::new(child);
                                let kt = match elaborate_type_expr(&mut el, te) {
                                    ElabResult::Done(kt) => kt,
                                    // Park / Unbound here is a protocol break: the
                                    // parameter-name install and the fn_def carrier
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
                        // FN body runs in its own lexical block (the per-call child
                        // scope). Assemble the body chain from the call-site chain and
                        // the FN's lexical outer walk so chain depth tracks lexical
                        // nesting, not call depth. For multi-statement bodies each
                        // statement gets its own chain index `1..=N` so the strict
                        // `b.idx < c` predicate orders siblings correctly; single-
                        // statement bodies stay at index 0 (today's shape).
                        let call_site_chain = sched
                            .current_lexical_chain()
                            .expect("FN invoke runs inside an enter_block / active_chain");
                        let body_chain_parent =
                            assemble_body_chain(child, call_site_chain.clone(), 0).parent.clone();
                        let mut body_ids: Vec<crate::machine::core::kfunction::NodeId> =
                            Vec::with_capacity(n);
                        for (i, stmt) in body_statements.into_iter().enumerate() {
                            // Single-statement body keeps index 0 (matches the
                            // assemble_body_chain shape today); multi-statement uses
                            // `i + 1` so siblings see each other under the gate.
                            let idx = if n == 1 { 0 } else { i + 1 };
                            let chain = LexicalFrame::push(
                                body_chain_parent.clone(),
                                child.id,
                                idx,
                            );
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
                        // sub-Dispatch at [N]. The finish reads `results[body_terminal_idx]`
                        // as the body value (last statement) and `results[N]` (if present)
                        // as the per-call return type carrier.
                        let mut deps = body_ids;
                        if let PerCallReturnType::Pending(t) = per_call_ret {
                            deps.push(t);
                        }
                        let function_summary = self.summarize();
                        let combine_id = sched.add_combine(deps, vec![], child, Box::new(move |_scope, _sched, results| {
                            let body_value: &KObject<'_> = results[body_terminal_idx];
                            let per_call_ret: KType<'_> = match per_call_ret {
                                PerCallReturnType::Ready(kt) => kt,
                                PerCallReturnType::Pending(_) => match results.get(body_terminal_idx + 1).copied() {
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
                            // Phase 3 ascription stamping at the per-call return boundary:
                            // re-tag the parameterized carrier to exactly the resolved
                            // per-call return type (coarsening included).
                            let stamped = body_value.deep_clone().stamp_type(&per_call_ret);
                            BodyResult::Value(_scope.arena.alloc(stamped))
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

/// True iff a declared slot type is one of the parameterized carrier types whose
/// `matches_value` does content-recursive element/payload checking — the only slots
/// where dispatch-time shape-only admission (`Argument::matches`) leaves an element
/// check undone. `KExpression` (the lazy-slot type) is never one of these, so gating
/// on this predicate also keeps the unevaluated `Expression`-slot path untouched.
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
/// Post-collapse the identity-bearing module/signature variants live in `KType` itself,
/// so a signature-typed parameter mints `KType::Module { module: m, frame }` (carrying
/// the per-call frame anchor the argument arrived with) — and `ATTR`'s `KTypeValue
/// (KType::Module { .. })` arm projects from it for `Er.pure(x)`-style references.
///
/// | Declared `KType`               | Bound `KObject`                                  | Identity                                              |
/// | ------------------------------ | ------------------------------------------------ | ----------------------------------------------------- |
/// | `SatisfiesSignature { .. }`    | `KTypeValue(KType::Module { module, frame })`    | `KType::Module { module, frame }` (same carrier)      |
/// | `AnyModule`                    | `KTypeValue(KType::Module { module, frame })`    | same                                                  |
/// | `AnySignature`                 | `KTypeValue(KType::Signature(s))`                | `KType::Signature(s)` (same carrier)                  |
/// | `Type`                         | `KTypeValue(kt)`                                 | `kt.clone()`                                          |
/// | `TypeExprRef`                  | `KTypeValue(kt)`                                 | `kt.clone()`                                          |
/// | `TypeExprRef`                  | `TypeNameRef(t)`                                 | elaborated via `definition_scope.resolve_type_expr`   |
///
/// `Ok(None)` means the carrier shape didn't match any row (programming error
/// downstream of an `is_type_denoting`/`matches` disagreement; skip the type-side install).
///
/// `Err(TypeIdentityPendingAtDispatch)` fires when a `TypeNameRef` elaborates against
/// `definition_scope` (the FN's captured lexical environment) but the result still
/// references a pending-finalize type.
pub(crate) fn type_identity_for<'a>(
    param_name: &str,
    obj: &KObject<'a>,
    declared: &KType<'a>,
    definition_scope: &'a Scope<'a>,
) -> Result<Option<KType<'a>>, KError> {
    match declared {
        // Signature-typed parameter: mint the module identity directly so ATTR-on-type
        // projects `Er.pure(x)` against the carried `&Module`.
        KType::SatisfiesSignature { .. } => Ok(match obj {
            KObject::KTypeValue(kt @ KType::Module { .. }) => Some(kt.clone()),
            _ => None,
        }),
        // `:Module` slot wildcard: same as `SatisfiesSignature` for the carrier-extract.
        KType::AnyModule => Ok(match obj {
            KObject::KTypeValue(kt @ KType::Module { .. }) => Some(kt.clone()),
            _ => None,
        }),
        // `:Signature` slot wildcard: pass the carried signature carrier through.
        KType::AnySignature => Ok(match obj {
            KObject::KTypeValue(kt @ KType::Signature(_)) => Some(kt.clone()),
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
                // Unbound: skip the type-side install; the body's own value-side
                // dispatch will surface the real error.
                ResolveTypeExprOutcome::Unbound(_) => Ok(None),
            },
            _ => Ok(None),
        },
        // Non-type-denoting variants: caller already gated, unreachable on the happy path.
        _ => Ok(None),
    }
}

