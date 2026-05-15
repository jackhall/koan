use crate::runtime::model::{Argument, ExpressionSignature, KObject, KType, SignatureElement, ReturnType};
use crate::runtime::model::types::UserTypeKind;
use crate::runtime::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, Scope, SchedulerHandle};
use crate::ast::{ExpressionPart, KExpression};

use super::{err, register_builtin_with_pre_run};

/// `LET <name> = <value:Any>` — copies the bound value into an arena-allocated `KObject`,
/// inserts it under `name`, and returns that same arena reference. Compound values recurse
/// through `KObject::deep_clone`.
///
/// Two overloads share this body, differing only in the `name` slot's `KType`: `Identifier`
/// (the original lowercase-name path) and `TypeExprRef` (so `LET ModuleName = (...)` can
/// bind a name that classifies as a Type token under the parser's token-classification rules).
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let value = match bundle.get("value") {
        Some(v) => v,
        None => return err(KError::new(KErrorKind::MissingArg("value".to_string()))),
    };
    // `type_for_types_map` is `Some(kt)` iff this call should route storage through
    // `register_type` (a Type-class LHS with an actual `KTypeValue(kt)` RHS).
    // `nominal_identity` is `Some(kt)` iff the RHS is a type-language carrier with a
    // recoverable nominal identity (`KModule` / `KSignature` / `StructType` /
    // `TaggedUnionType`); those route through `register_nominal` so the alias name
    // resolves both type-side (via `resolve_type`) and value-side (via `lookup`).
    // Only one of the two is `Some` at any time — they're mutually exclusive RHS shapes.
    let mut type_for_types_map: Option<KType> = None;
    let mut nominal_identity: Option<KType> = None;
    let name = match bundle.get("name") {
        Some(KObject::KString(s)) => s.clone(),
        // Stage-2 carrier: a Type-classed binder name not in `KType::from_name`'s
        // builtin table lands as a `TypeNameRef`. Parameterized shapes (`List<X>`,
        // function arrow forms) are rejected — the binder name must be a bare leaf.
        // The `TypeClassBindingExpectsType` blocklist runs the same shape as the
        // `KTypeValue` arm: non-type RHS rejected before storage routing.
        Some(KObject::TypeNameRef(t, _)) => match &t.params {
            crate::ast::TypeParams::List(_) | crate::ast::TypeParams::Function { .. } => {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "LET name must be a bare type name, got `{}`",
                    t.render(),
                ))));
            }
            crate::ast::TypeParams::None => {
                let resolved_name = t.name.clone();
                if matches!(
                    value.ktype(),
                    KType::Number | KType::Str | KType::Bool | KType::Null
                        | KType::List(_) | KType::Dict(_, _)
                ) {
                    return err(KError::new(KErrorKind::TypeClassBindingExpectsType {
                        name: resolved_name,
                        got: value.ktype(),
                    }));
                }
                if let KObject::KTypeValue(kt) = value {
                    type_for_types_map = Some(kt.clone());
                } else {
                    nominal_identity = derive_nominal_identity(value);
                }
                resolved_name
            }
        },
        // The `TypeExprRef` overload routes through `KTypeValue(kt)` post-refactor; only
        // leaf-named variants are valid binder names. Structural shapes (`List<X>`,
        // function types, `Mu` / `RecursiveRef`) are rejected as `ShapeError`.
        Some(KObject::KTypeValue(t)) => match t {
            KType::List(_)
            | KType::Dict(_, _)
            | KType::KFunction { .. }
            | KType::Mu { .. }
            | KType::RecursiveRef(_) => {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "LET name must be a bare type name, got `{}`",
                    t.render(),
                ))));
            }
            _ => {
                // Bind-time rejection for `LET <Type-class> = <non-type>`. Blocklist —
                // not `value.ktype() != KType::TypeExprRef` — because the type-language
                // carriers `KModule` / `KSignature` / `StructType` / `TaggedUnionType`
                // report `Module` / `Signature` / `Type`, not `TypeExprRef`, and shipped
                // `LET IntOrdAbstract = (IntOrd :| OrderedSig)` patterns in `ascribe.rs`
                // depend on those being accepted. The non-`KTypeValue` carriers continue
                // to write `data` via `bind_value` until their own storage migration.
                let resolved_name = t.name();
                if matches!(
                    value.ktype(),
                    KType::Number | KType::Str | KType::Bool | KType::Null
                        | KType::List(_) | KType::Dict(_, _)
                ) {
                    return err(KError::new(KErrorKind::TypeClassBindingExpectsType {
                        name: resolved_name,
                        got: value.ktype(),
                    }));
                }
                // Stage-1.7 storage flip: Type-class LHS + `KTypeValue(kt)` RHS routes
                // through `register_type` so the bound name lives in `bindings.types`
                // alongside builtin type names. The dispatch carrier returned below
                // stays a `KObject::KTypeValue(kt)` — only the storage location moves.
                if let KObject::KTypeValue(kt) = value {
                    type_for_types_map = Some(kt.clone());
                } else {
                    nominal_identity = derive_nominal_identity(value);
                }
                resolved_name
            }
        },
        Some(other) => {
            return err(KError::new(KErrorKind::TypeMismatch {
                arg: "name".to_string(),
                expected: "Identifier or TypeExprRef".to_string(),
                got: other.ktype().name(),
            }));
        }
        None => return err(KError::new(KErrorKind::MissingArg("name".to_string()))),
    };
    let cloned = value.deep_clone();
    let arena = scope.arena;
    let allocated: &'a KObject<'a> = arena.alloc_object(cloned);
    if let Some(kt) = type_for_types_map {
        // Infallible `register_type` matches the prior `bind_value` shape for shipped
        // call sites (placeholder-resolution catches name conflicts upstream before
        // the body runs). The returned `KObject::KTypeValue(kt)` carrier is preserved
        // so dispatch transport — `lift_kobject`, the `value_lookup`-TypeExprRef
        // synthesis site, downstream `KType::TypeExprRef`-typed slots — sees the
        // same shape as before the storage flip.
        scope.register_type(name, kt);
    } else if let Some(identity) = nominal_identity {
        // Aliasing dual-write: `LET P2 = Point` writes `bindings.types[P2]` carrying
        // the ORIGINAL carrier's identity (Point's `name`/`scope_id`), not a fresh
        // identity minted from the alias name. This is what makes
        // `(PICK x: P2)` and `(PICK x: Point)` dispatch to the same overload — aliasing
        // preserves type identity rather than introducing a new nominal type.
        if let Err(e) = scope.register_nominal(name, identity, allocated) {
            return err(e);
        }
    } else if let Err(e) = scope.bind_value(name, allocated) {
        return err(e);
    }
    BodyResult::Value(allocated)
}

/// Recover the nominal identity (a `KType::UserType` or `KType::SignatureBound`) carried
/// by a type-language value `obj`. Returns `Some(identity)` for the four shapes that came
/// from a STRUCT / UNION / MODULE / SIG declaration (or an alias of one); `None` for
/// every other carrier shape — those keep flowing through `Scope::bind_value` and never
/// dual-write to `bindings.types`.
///
/// The identity preserves the ORIGINAL declaration's `name` / `scope_id` rather than the
/// alias's binder name, so `LET P2 = Point` makes `P2` resolve to the same `UserType`
/// that `Point` carries.
fn derive_nominal_identity(obj: &KObject<'_>) -> Option<KType> {
    match obj {
        KObject::KModule(m, _) => Some(KType::UserType {
            kind: UserTypeKind::Module,
            scope_id: m.scope_id(),
            name: m.path.clone(),
        }),
        KObject::KSignature(s) => Some(KType::SignatureBound {
            sig_id: s.sig_id(),
            sig_path: s.path.clone(),
            // A bare SIG alias (`LET S2 = OrderedSig`) carries no sharing constraints.
            pinned_slots: Vec::new(),
        }),
        KObject::StructType { name, scope_id, .. } => Some(KType::UserType {
            kind: UserTypeKind::Struct,
            scope_id: *scope_id,
            name: name.clone(),
        }),
        KObject::TaggedUnionType { name, scope_id, .. } => Some(KType::UserType {
            kind: UserTypeKind::Tagged,
            scope_id: *scope_id,
            name: name.clone(),
        }),
        _ => None,
    }
}

/// Dispatch-time placeholder extractor for LET. Both overloads (`LET <name:Identifier> = ...`
/// and `LET <name:TypeExprRef> = ...`) put the bound name at `parts[1]`; pull it out
/// structurally without dispatching anything. Returns `None` on shape mismatch (the body
/// will surface a structured error later).
pub(crate) fn pre_run(expr: &KExpression<'_>) -> Option<String> {
    match expr.parts.get(1)? {
        ExpressionPart::Identifier(s) => Some(s.clone()),
        ExpressionPart::Type(t) => Some(t.name.clone()),
        _ => None,
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin_with_pre_run(
        scope,
        "LET",
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Any),
            elements: vec![
                SignatureElement::Keyword("LET".into()),
                SignatureElement::Argument(Argument { name: "name".into(),  ktype: KType::Identifier }),
                SignatureElement::Keyword("=".into()),
                SignatureElement::Argument(Argument { name: "value".into(), ktype: KType::Any }),
            ],
        },
        body,
        Some(pre_run),
    );
    register_builtin_with_pre_run(
        scope,
        "LET",
        ExpressionSignature {
            return_type: ReturnType::Resolved(KType::Any),
            elements: vec![
                SignatureElement::Keyword("LET".into()),
                SignatureElement::Argument(Argument { name: "name".into(),  ktype: KType::TypeExprRef }),
                SignatureElement::Keyword("=".into()),
                SignatureElement::Argument(Argument { name: "value".into(), ktype: KType::Any }),
            ],
        },
        body,
        Some(pre_run),
    );
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::rc::Rc;

    use super::body;
    use crate::runtime::builtins::default_scope;
    use crate::runtime::builtins::test_support::run_root_bare;
    use crate::runtime::model::KObject;
    use crate::runtime::machine::{ArgumentBundle, BodyResult};
    use crate::runtime::machine::execute::Scheduler;
    use crate::ast::{ExpressionPart, KExpression, KLiteral};

    #[test]
    fn let_inserts_binding_into_scope() {
        use crate::runtime::machine::RuntimeArena;
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let mut sched = Scheduler::new();
        let mut args = HashMap::new();
        args.insert("name".to_string(), Rc::new(KObject::KString("x".into())));
        args.insert("value".to_string(), Rc::new(KObject::Number(42.0)));

        let result = body(scope, &mut sched, ArgumentBundle { args });

        let value = match result {
            BodyResult::Value(v) => v,
            BodyResult::Tail { .. } => panic!("LET should not produce a Tail"),
            BodyResult::DeferTo(_) => panic!("LET should not produce a DeferTo"),
            BodyResult::Err(e) => panic!("LET errored unexpectedly: {e}"),
        };
        assert!(matches!(value, KObject::Number(n) if *n == 42.0));
        let data = scope.bindings().data();
        let entry = data.get("x").expect("expected binding 'x'");
        assert!(matches!(entry, KObject::Number(n) if *n == 42.0));
    }

    /// Smoke test for LET's pre_run extractor: structural extraction of `parts[1]`
    /// returns the bound name without requiring sub-dispatches.
    #[test]
    fn pre_run_extracts_let_name() {
        use crate::parse::parse;
        let mut exprs = parse("LET hello = 1").expect("parse should succeed");
        let expr = exprs.remove(0);
        let name = super::pre_run(&expr);
        assert_eq!(name.as_deref(), Some("hello"));
    }

    /// End-to-end install-then-clear: dispatch `LET x = 1` through the scheduler. The
    /// pre_run hook installs `placeholders["x"] = NodeId(...)` before the body runs;
    /// after the body finalizes via `bind_value`, the placeholder is removed.
    #[test]
    fn pre_run_install_then_body_finalize_clears_placeholder() {
        use crate::runtime::machine::RuntimeArena;
        use crate::runtime::machine::execute::Scheduler;
        use crate::runtime::builtins::default_scope;
        use crate::parse::parse;
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let exprs = parse("LET hello = 1").unwrap();
        for e in exprs { sched.add_dispatch(e, scope); }
        sched.execute().unwrap();
        // After execute, placeholders should not contain "hello" — bind_value cleared it.
        assert!(scope.bindings().placeholders().get("hello").is_none());
        assert!(matches!(scope.lookup("hello"), Some(KObject::Number(n)) if *n == 1.0));
    }

    /// Phase 3: `LET T = T` is a trivially cyclic alias — the RHS references the binder
    /// itself. The dispatcher detects the placeholder-points-at-self condition and
    /// surfaces a structured `ShapeError` rather than parking the sub-Dispatch on its own
    /// ancestor (which would deadlock).
    #[test]
    fn let_t_cycle_errors() {
        use crate::runtime::machine::RuntimeArena;
        use crate::runtime::machine::execute::Scheduler;
        use crate::runtime::machine::KErrorKind;
        use crate::runtime::builtins::default_scope;
        use crate::parse::parse;
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let exprs = parse("LET Ty = Ty").unwrap();
        let mut ids = Vec::new();
        for e in exprs {
            ids.push(sched.add_dispatch(e, scope));
        }
        sched.execute().expect("execute does not surface per-slot errors");
        let res = sched.read_result(ids[0]);
        match res {
            Err(e) => assert!(
                matches!(&e.kind, KErrorKind::ShapeError(msg) if msg.contains("cycle")),
                "expected ShapeError mentioning cycle, got {e}",
            ),
            Ok(v) => panic!("expected cycle error, got value {:?}", v.ktype()),
        }
    }

    #[test]
    fn dispatch_let_expression() {
        use crate::runtime::machine::RuntimeArena;
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Keyword("LET".into()),
                ExpressionPart::Identifier("x".into()),
                ExpressionPart::Keyword("=".into()),
                ExpressionPart::Literal(KLiteral::Number(42.0)),
            ],
        };

        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(expr, scope);
        sched.execute().unwrap();

        assert!(matches!(sched.read(id), KObject::Number(n) if *n == 42.0));
        let data = scope.bindings().data();
        let entry = data.get("x").expect("expected binding 'x'");
        assert!(matches!(entry, KObject::Number(n) if *n == 42.0));
    }

    /// Stage 1.6: `LET Foo = 1` — Type-class LHS with a non-type RHS. The bind-time
    /// check fires before the value reaches storage, producing a structured
    /// `TypeClassBindingExpectsType` rather than the downstream `UnboundName` /
    /// `ShapeError` the old "bind silently" path eventually surfaced.
    #[test]
    fn let_type_class_with_non_type_value_errors() {
        use crate::runtime::machine::RuntimeArena;
        use crate::runtime::machine::KErrorKind;
        use crate::runtime::model::KType;
        use crate::parse::parse;
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let exprs = parse("LET Foo = 1").unwrap();
        let mut ids = Vec::new();
        for e in exprs {
            ids.push(sched.add_dispatch(e, scope));
        }
        sched.execute().expect("execute does not surface per-slot errors");
        let res = sched.read_result(ids[0]);
        match res {
            Err(e) => assert!(
                matches!(
                    &e.kind,
                    KErrorKind::TypeClassBindingExpectsType { name, got }
                        if name == "Foo" && matches!(got, KType::Number),
                ),
                "expected TypeClassBindingExpectsType {{ name: \"Foo\", got: Number }}, got {e}",
            ),
            Ok(v) => panic!("expected bind-time error, got value {:?}", v.ktype()),
        }
    }

    /// Stage 1.7: `LET Foo = Number` — Type-class LHS with a type RHS. Storage now
    /// lives in `bindings.types` (via `register_type`), reachable through
    /// `Scope::resolve_type`. Regression guard that the blocklist doesn't reject the
    /// good case and that the storage flip lands on the right map.
    #[test]
    fn let_type_class_with_type_value_still_binds() {
        use crate::runtime::machine::RuntimeArena;
        use crate::runtime::model::KType;
        use crate::parse::parse;
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let exprs = parse("LET Foo = Number").unwrap();
        let mut ids = Vec::new();
        for e in exprs {
            ids.push(sched.add_dispatch(e, scope));
        }
        sched.execute().expect("execute does not surface per-slot errors");
        let res = sched.read_result(ids[0]);
        assert!(res.is_ok(), "expected bind to succeed, got {:?}", res.err());
        let kt = scope
            .resolve_type("Foo")
            .expect("expected type binding 'Foo' in bindings.types");
        assert_eq!(*kt, KType::Number, "expected Number, got {:?}", kt);
    }

    /// Stage 1.6: `LET foo = 1` (lowercase, Identifier overload) is untouched by
    /// the new check — it doesn't go through the `KTypeValue(_)` arm at all.
    #[test]
    fn let_identifier_lhs_with_non_type_still_binds() {
        use crate::runtime::machine::RuntimeArena;
        use crate::parse::parse;
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let exprs = parse("LET foo = 1").unwrap();
        let mut ids = Vec::new();
        for e in exprs {
            ids.push(sched.add_dispatch(e, scope));
        }
        sched.execute().expect("execute does not surface per-slot errors");
        let res = sched.read_result(ids[0]);
        assert!(res.is_ok(), "expected bind to succeed, got {:?}", res.err());
        let data = scope.bindings().data();
        let entry = data.get("foo").expect("expected binding 'foo'");
        assert!(
            matches!(entry, KObject::Number(n) if *n == 1.0),
            "expected Number(1.0), got {:?}",
            entry.ktype(),
        );
    }

    /// Stage 1.6: `LET List<Number> = 1` — parameterized binder name is rejected by
    /// the structural-shape check, which fires before the primitive blocklist.
    /// Regression guard for ordering.
    #[test]
    fn let_parameterized_type_lhs_still_shape_errors() {
        use crate::runtime::machine::RuntimeArena;
        use crate::runtime::machine::KErrorKind;
        use crate::parse::parse;
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let exprs = parse("LET List<Number> = 1").unwrap();
        let mut ids = Vec::new();
        for e in exprs {
            ids.push(sched.add_dispatch(e, scope));
        }
        sched.execute().expect("execute does not surface per-slot errors");
        let res = sched.read_result(ids[0]);
        match res {
            Err(e) => assert!(
                matches!(&e.kind, KErrorKind::ShapeError(_)),
                "expected ShapeError, got {e}",
            ),
            Ok(v) => panic!("expected shape error, got value {:?}", v.ktype()),
        }
    }

    /// Stage 3.1 dual-write: `LET IntOrdA = (IntOrd :| OrderedSig)` writes the alias
    /// into `bindings.types` (via `register_nominal`) AND `bindings.data` at the same
    /// scope. The identity preserves the ORIGINAL module's `(scope_id, path)` rather
    /// than minting a fresh nominal — aliasing is type-equivalent.
    #[test]
    fn let_type_class_with_module_carrier_dual_writes() {
        use crate::runtime::machine::RuntimeArena;
        use crate::runtime::model::types::UserTypeKind;
        use crate::runtime::model::KType;
        use crate::runtime::builtins::test_support::run;
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        run(
            scope,
            "MODULE IntOrd = (LET compare = 0)\n\
             SIG OrderedSig = (LET compare = 0)\n\
             LET IntOrdA = (IntOrd :| OrderedSig)",
        );
        let types = scope.bindings().types();
        let kt = types
            .get("IntOrdA")
            .expect("IntOrdA should be in bindings.types");
        assert!(matches!(
            **kt,
            KType::UserType { kind: UserTypeKind::Module, .. }
        ));
        drop(types);
        let data = scope.bindings().data();
        let obj = data
            .get("IntOrdA")
            .expect("IntOrdA should be in bindings.data");
        assert!(matches!(obj, KObject::KModule(_, _)));
    }

    /// Stage 3.1 aliasing-preserves-identity: `LET Pt = Point` writes a `types[Pt]`
    /// entry that equals `types[Point]` field-wise — `Pt` and `Point` lower to the
    /// same `UserType` (same kind, scope_id, name="Point"). The alias binder name
    /// `Pt` is for value-side lookup only; the type identity stays Point's. Token
    /// classification requires the binder to carry at least one lowercase letter
    /// to read as a type-class name.
    #[test]
    fn let_aliases_struct_preserves_type_identity() {
        use crate::runtime::machine::RuntimeArena;
        use crate::runtime::model::KType;
        use crate::runtime::builtins::test_support::run;
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        run(
            scope,
            "STRUCT Point = (x: Number, y: Number)\n\
             LET Pt = Point",
        );
        let types = scope.bindings().types();
        let pt: &KType = types
            .get("Pt")
            .copied()
            .expect("Pt should be in bindings.types after alias");
        let point: &KType = types
            .get("Point")
            .copied()
            .expect("Point should be in bindings.types");
        assert_eq!(*pt, *point, "alias must preserve type identity field-wise");
    }

    /// Stage 1.6: `LET Foo = "hello"` — confirms the blocklist covers `Str`, not just
    /// `Number`. Same diagnostic shape as the Number case.
    #[test]
    fn let_type_class_with_string_value_errors() {
        use crate::runtime::machine::RuntimeArena;
        use crate::runtime::machine::KErrorKind;
        use crate::runtime::model::KType;
        use crate::parse::parse;
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let exprs = parse("LET Foo = \"hello\"").unwrap();
        let mut ids = Vec::new();
        for e in exprs {
            ids.push(sched.add_dispatch(e, scope));
        }
        sched.execute().expect("execute does not surface per-slot errors");
        let res = sched.read_result(ids[0]);
        match res {
            Err(e) => assert!(
                matches!(
                    &e.kind,
                    KErrorKind::TypeClassBindingExpectsType { name, got }
                        if name == "Foo" && matches!(got, KType::Str),
                ),
                "expected TypeClassBindingExpectsType {{ name: \"Foo\", got: Str }}, got {e}",
            ),
            Ok(v) => panic!("expected bind-time error, got value {:?}", v.ktype()),
        }
    }
}
