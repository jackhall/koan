use std::rc::Rc;

use crate::ast::{ExpressionPart, KExpression};

use crate::runtime::machine::core::{CallArena, KError, KErrorKind, RuntimeArena, Scope};
use crate::runtime::model::types::{
    elaborate_type_expr, DeferredReturn, ElabResult, Elaborator, KType, ReturnType,
    SignatureElement, UserTypeKind,
};
use crate::runtime::model::values::KObject;

use super::argument_bundle::ArgumentBundle;
use super::body::{Body, BodyResult};
use super::scheduler_handle::SchedulerHandle;
use super::KFunction;

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
                            if let Some(identity) =
                                type_identity_for(allocated, &arg.ktype, outer)
                            {
                                child.register_type(name.clone(), identity);
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
                        // Resolve the per-call return-type carrier into either a
                        // ready `KType` (TypeExpr arm) or a pending sub-Dispatch
                        // (Expression arm). The closure captures only what it
                        // needs — no live borrow of `self.signature` survives the
                        // sub-Dispatch spawning below.
                        let body_id;
                        let typ_id_opt: Option<crate::runtime::machine::NodeId>;
                        let inline_typ: Option<KType>;
                        match d {
                            DeferredReturn::TypeExpr(te) => {
                                let mut el = Elaborator::new(child);
                                inline_typ = match elaborate_type_expr(&mut el, te) {
                                    ElabResult::Done(kt) => Some(kt),
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
                                        Some(KType::Any)
                                    }
                                    ElabResult::Unbound(ref msg) => {
                                        debug_assert!(
                                            false,
                                            "deferred return-type TypeExpr unbound at dispatch \
                                             boundary: {msg} — fn_def parameter-name scan \
                                             should have prevented this carrier",
                                        );
                                        Some(KType::Any)
                                    }
                                };
                                typ_id_opt = None;
                            }
                            DeferredReturn::Expression(e) => {
                                inline_typ = None;
                                let cloned = e.clone();
                                let mut tid = None;
                                sched.with_active_frame(frame.clone(), &mut |s| {
                                    tid = Some(s.add_dispatch(cloned.clone(), child));
                                });
                                typ_id_opt = tid;
                            }
                        }
                        // Spawn the body Dispatch under the per-call frame.
                        let mut bid = None;
                        sched.with_active_frame(frame.clone(), &mut |s| {
                            bid = Some(s.add_dispatch(substituted.clone(), child));
                        });
                        body_id = bid.expect("body dispatch must spawn");

                        // Build the Combine's dep list: body first, then optional
                        // return-type sub-Dispatch. Closure reads `results[0]` for
                        // the body and `results[1]` for the type when present.
                        let mut deps = vec![body_id];
                        if let Some(t) = typ_id_opt {
                            deps.push(t);
                        }
                        let function_summary = self.summarize();
                        let combine_id = sched.add_combine(deps, child, Box::new(move |_scope, _sched, results| {
                            let body_value: &KObject<'_> = results[0];
                            let per_call_ret: KType = match inline_typ {
                                Some(kt) => kt,
                                None => match results.get(1).copied() {
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
                                ).with_frame(crate::runtime::machine::Frame {
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
) -> Option<&'a crate::runtime::model::types::Argument> {
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
/// | `TypeExprRef`                  | `TypeNameRef(t, _)`    | elaborated via `definition_scope`                     |
///
/// Returns `None` when the carrier shape doesn't match any of the table rows —
/// the dispatcher's `matches_value` filter already ran before this site, so a `None`
/// here indicates an `is_type_denoting`/`matches` disagreement (programming error,
/// not user error). Production behavior: skip the dual-write silently; tests can
/// debug-assert if they want stricter coverage.
///
/// `definition_scope` is the FN's captured (definition-time) scope, used to elaborate
/// a `TypeNameRef` carrier — the carrier's `TypeExpr` references type-side bindings
/// from the definition's lexical environment, not the call site's.
pub(crate) fn type_identity_for<'a>(
    obj: &KObject<'a>,
    declared: &KType,
    definition_scope: &'a Scope<'a>,
) -> Option<KType> {
    match declared {
        KType::SignatureBound { .. } => match obj {
            KObject::KModule(m, _) => Some(KType::UserType {
                kind: UserTypeKind::Module,
                scope_id: m.scope_id(),
                name: m.path.clone(),
            }),
            _ => None,
        },
        KType::AnyUserType { kind: UserTypeKind::Module } => match obj {
            KObject::KModule(m, _) => Some(KType::UserType {
                kind: UserTypeKind::Module,
                scope_id: m.scope_id(),
                name: m.path.clone(),
            }),
            _ => None,
        },
        KType::Signature => match obj {
            KObject::KSignature(s) => Some(KType::SignatureBound {
                sig_id: s.sig_id(),
                sig_path: s.path.clone(),
                pinned_slots: Vec::new(),
            }),
            _ => None,
        },
        KType::Type => match obj {
            KObject::KTypeValue(kt) => Some(kt.clone()),
            _ => None,
        },
        KType::TypeExprRef => match obj {
            KObject::KTypeValue(kt) => Some(kt.clone()),
            KObject::TypeNameRef(t, _) => {
                use crate::runtime::model::types::{elaborate_type_expr, ElabResult, Elaborator};
                let mut el = Elaborator::new(definition_scope);
                match elaborate_type_expr(&mut el, t) {
                    ElabResult::Done(kt) => Some(kt),
                    // The carrier landed in `bindings.data` at definition time as part
                    // of the FN-def Combine finish — re-elaborating against the FN's
                    // captured scope ought to succeed. A `Park` or `Unbound` here means
                    // the scope shape changed between definition and call, which would
                    // already have surfaced upstream; skip the dual-write.
                    ElabResult::Park(_) | ElabResult::Unbound(_) => None,
                }
            }
            _ => None,
        },
        // Any other variant is non-type-denoting; caller already gated, so this is
        // unreachable on the happy path.
        _ => None,
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

#[cfg(test)]
mod tests {
    //! Direct unit coverage for the Stage A `type_identity_for` helper. The
    //! end-to-end coverage of the dual-write itself lives in
    //! [`crate::runtime::builtins::fn_def::tests::module_stage2`]; these tests
    //! pin the per-row mapping in isolation without the surrounding scheduler.

    use super::*;
    use crate::runtime::builtins::default_scope;
    use crate::runtime::machine::core::RuntimeArena;
    use crate::runtime::model::types::UserTypeKind;
    use crate::runtime::model::values::{Module, Signature};

    /// `SignatureBound`-declared parameter bound to a `KModule` yields a
    /// `UserType { kind: Module, scope_id, name }` identity.
    #[test]
    fn type_identity_for_signature_bound_yields_module_user_type() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let child = arena.alloc_scope(crate::runtime::machine::Scope::child_under_module(
            scope,
            "Foo".into(),
        ));
        let module = arena.alloc_module(Module::new("Foo".into(), child));
        let obj = arena.alloc_object(KObject::KModule(module, None));
        let declared = KType::SignatureBound {
            sig_id: 42,
            sig_path: "OrderedSig".into(),
            pinned_slots: Vec::new(),
        };
        let identity = type_identity_for(obj, &declared, scope).expect("module identity expected");
        assert_eq!(
            identity,
            KType::UserType {
                kind: UserTypeKind::Module,
                scope_id: module.scope_id(),
                name: "Foo".into(),
            },
        );
    }

    /// `AnyUserType { kind: Module }`-declared parameter bound to a `KModule`
    /// yields the same `UserType { kind: Module, .. }` identity. Mirrors the
    /// `SignatureBound` arm.
    #[test]
    fn type_identity_for_any_module_yields_module_user_type() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let child = arena.alloc_scope(crate::runtime::machine::Scope::child_under_module(
            scope,
            "Bar".into(),
        ));
        let module = arena.alloc_module(Module::new("Bar".into(), child));
        let obj = arena.alloc_object(KObject::KModule(module, None));
        let declared = KType::AnyUserType { kind: UserTypeKind::Module };
        let identity = type_identity_for(obj, &declared, scope).expect("module identity expected");
        assert_eq!(
            identity,
            KType::UserType {
                kind: UserTypeKind::Module,
                scope_id: module.scope_id(),
                name: "Bar".into(),
            },
        );
    }

    /// `Signature`-declared parameter bound to a `KSignature` yields a bare
    /// `SignatureBound { sig_id, sig_path, pinned_slots: [] }` identity.
    #[test]
    fn type_identity_for_signature_yields_signature_bound() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let sig = arena.alloc_signature(Signature::new("OrderedSig".into(), scope));
        let obj = arena.alloc_object(KObject::KSignature(sig));
        let declared = KType::Signature;
        let identity = type_identity_for(obj, &declared, scope).expect("signature identity expected");
        assert_eq!(
            identity,
            KType::SignatureBound {
                sig_id: sig.sig_id(),
                sig_path: "OrderedSig".into(),
                pinned_slots: Vec::new(),
            },
        );
    }

    /// `Type`-declared parameter bound to a `KTypeValue(kt)` yields `kt.clone()`.
    #[test]
    fn type_identity_for_type_yields_inner_ktype() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let inner = KType::List(Box::new(KType::Number));
        let obj = arena.alloc_object(KObject::KTypeValue(inner.clone()));
        let declared = KType::Type;
        let identity = type_identity_for(obj, &declared, scope).expect("type identity expected");
        assert_eq!(identity, inner);
    }

    /// `TypeExprRef`-declared parameter bound to a `KTypeValue(kt)` yields
    /// `kt.clone()` (the same arm as `Type`, since the carrier is the same).
    #[test]
    fn type_identity_for_type_expr_ref_kt_carrier_yields_inner_ktype() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let inner = KType::Number;
        let obj = arena.alloc_object(KObject::KTypeValue(inner.clone()));
        let declared = KType::TypeExprRef;
        let identity = type_identity_for(obj, &declared, scope).expect("type identity expected");
        assert_eq!(identity, inner);
    }

    /// Mismatched carrier for a type-denoting declared `KType` returns `None` —
    /// the dispatcher's `matches_value` filter already gated, so this path
    /// indicates an `is_type_denoting` / `matches_value` disagreement (skip the
    /// dual-write rather than panic).
    #[test]
    fn type_identity_for_carrier_mismatch_returns_none() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let obj = arena.alloc_object(KObject::Number(1.0));
        let declared = KType::SignatureBound {
            sig_id: 1,
            sig_path: "OrderedSig".into(),
            pinned_slots: Vec::new(),
        };
        assert!(type_identity_for(obj, &declared, scope).is_none());
    }
}
