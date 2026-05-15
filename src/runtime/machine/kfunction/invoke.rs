use std::rc::Rc;

use crate::ast::{ExpressionPart, KExpression};

use crate::runtime::machine::core::{CallArena, RuntimeArena, Scope};
use crate::runtime::model::types::{KType, SignatureElement, UserTypeKind};
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
                BodyResult::tail_with_frame(substituted, frame, self)
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
        let child = arena.alloc_scope(crate::runtime::machine::Scope::child_under_named(
            scope,
            "MODULE Foo".into(),
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
        let child = arena.alloc_scope(crate::runtime::machine::Scope::child_under_named(
            scope,
            "MODULE Bar".into(),
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
