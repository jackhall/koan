//! `VAL <name:Identifier> : <ty:TypeExprRef>` — SIG-body-only declarator for value
//! slots whose declared type is recorded explicitly. See
//! [design/typing/modules.md § Structures and signatures](../../design/typing/modules.md#structures-and-signatures).
//!
//! Gate: outside a SIG body the body returns `ShapeError`. The gate pivots on the
//! first non-`Anonymous` [`ScopeKind`]: `Sig` admits, `Module` rejects.
//!
//! Storage: `bindings.data[name] = KObject::KTypeValue(declared_kt)`. Inside a SIG
//! decl_scope this carrier means "value slot whose declared type is `kt`" rather
//! than "name bound to a type value"; the disambiguation is by scope context.
//!
//! Type resolution dispatches on the `ty` carrier shape:
//! - Builtin-leaf `KTypeValue(kt)` and bare-leaf `TypeNameRef` — sub-Dispatch a
//!   single-part `[Type(te)]` expression so a SIG-local `LET <name> = ...` shadow
//!   wins over the builtin-table fallback. See [`schedule_type_resolve`] for the
//!   ownership rationale.
//! - Structural `KTypeValue(kt)` (`KFunction`, `List`, `Dict`, `UserType`, ...) —
//!   lifted directly. Inner builtin names don't re-bind against decl_scope
//!   shadowing; full type-shape checking will revisit when modular implicits land.
//! - Parameterized / function-shaped `TypeNameRef` — synchronous elaboration
//!   against decl_scope first; on park, sub-Dispatch each referenced leaf and
//!   re-elaborate in the Combine finish.

use crate::machine::core::ResolveTypeExprOutcome;
use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression, TypeExpr, TypeParams};
use crate::machine::{
    ArgumentBundle, BodyResult, CombineFinish, KError, KErrorKind, NodeId, Scope,
    SchedulerHandle,
};
use crate::machine::model::{KObject, KType};

use super::{arg, err, kw, register_builtin_with_pre_run, sig};

/// Sub-dispatch `[Type(te)]` against `decl_scope`.
///
/// Avoids calling `elaborate_type_expr` directly: a `Placeholder` resolution would
/// surface a producer NodeId that, if fed into `add_combine`, installs an OWNED
/// edge to the SIG-sibling LET. The SIG outer Combine already owns that LET, and
/// two combines cascade-freeing the same producer is a double-free. Sub-dispatching
/// routes through `value_lookup::body_type_expr`, whose replay-park installs a
/// Notify (not Owned) edge — the SIG outer Combine stays the sole owner.
fn schedule_type_resolve<'a>(
    sched: &mut dyn SchedulerHandle<'a>,
    decl_scope: &'a Scope<'a>,
    te: &TypeExpr,
) -> crate::machine::NodeId {
    let expr = KExpression::new(vec![Spanned::bare(ExpressionPart::Type(te.clone()))]);
    sched.add_dispatch(expr, decl_scope)
}

fn typeexpr_from_carrier<'a>(obj: &KObject<'a>) -> Result<CarrierForm<'a>, KError> {
    match obj {
        KObject::KTypeValue(kt) => match kt {
            KType::Number
            | KType::Str
            | KType::Bool
            | KType::Null
            | KType::Type
            | KType::AnySignature
            | KType::AnyModule
            | KType::Any
            | KType::Identifier
            | KType::KExpression
            | KType::TypeExprRef => Ok(CarrierForm::Leaf(TypeExpr::leaf(kt.name()))),
            _ => Ok(CarrierForm::Direct(kt.clone())),
        },
        KObject::TypeNameRef(te) => Ok(CarrierForm::Raw(te.clone())),
        other => Err(KError::new(KErrorKind::TypeMismatch {
            arg: "ty".to_string(),
            expected: "TypeExprRef".to_string(),
            got: other.ktype().name(),
        })),
    }
}

enum CarrierForm<'a> {
    /// Builtin leaf synthesized from `kt.name()`; re-elaborated against decl_scope
    /// so a SIG-local shadow wins over the builtin table.
    Leaf(TypeExpr),
    /// Parser-preserved `TypeExpr` from a `TypeNameRef` carrier.
    Raw(TypeExpr),
    /// Structural carrier accepted as-is; inner names are not re-bound.
    Direct(KType<'a>),
}

pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    if !scope.is_in_sig_body() {
        return err(KError::new(KErrorKind::ShapeError(
            "VAL is only valid inside a SIG body — use LET for value bindings in \
             modules and run-root scope"
                .to_string(),
        )));
    }

    let name = match bundle.get("name") {
        Some(KObject::KString(s)) => s.clone(),
        Some(other) => {
            return err(KError::new(KErrorKind::TypeMismatch {
                arg: "name".to_string(),
                expected: "Identifier".to_string(),
                got: other.ktype().name(),
            }));
        }
        None => return err(KError::new(KErrorKind::MissingArg("name".to_string()))),
    };

    // Defense-in-depth against a future routing change that lets a Type-classified
    // token reach the name slot; abstract-type members must use `LET`.
    if super::ascribe::is_abstract_type_name(&name) {
        return err(KError::new(KErrorKind::ShapeError(format!(
            "VAL slot name `{name}` classifies as a Type token; abstract-type members \
             must use `LET {name} = <Type>` instead of `VAL`",
        ))));
    }

    let ty_obj = match bundle.get("ty") {
        Some(o) => o,
        None => return err(KError::new(KErrorKind::MissingArg("ty".to_string()))),
    };
    let carrier = match typeexpr_from_carrier(ty_obj) {
        Ok(c) => c,
        Err(e) => return err(e),
    };

    match carrier {
        CarrierForm::Direct(kt) => finalize_val(scope, name, kt),
        CarrierForm::Leaf(te) => {
            let resolve_id = schedule_type_resolve(sched, scope, &te);
            defer_val_via_combine(scope, sched, name, te, resolve_id)
        }
        CarrierForm::Raw(te) => {
            if matches!(te.params, TypeParams::None) {
                let resolve_id = schedule_type_resolve(sched, scope, &te);
                return defer_val_via_combine(scope, sched, name, te, resolve_id);
            }
            match scope.resolve_type_expr(&te) {
                ResolveTypeExprOutcome::Done(kt) => finalize_val(scope, name, kt.clone()),
                ResolveTypeExprOutcome::Park(_) => {
                    let leaves = collect_leaf_names(&te);
                    let mut dep_ids: Vec<NodeId> = Vec::with_capacity(leaves.len());
                    for leaf in &leaves {
                        let leaf_te = TypeExpr::leaf(leaf.clone());
                        dep_ids.push(schedule_type_resolve(sched, scope, &leaf_te));
                    }
                    defer_val_structural_via_combine(scope, sched, name, te, dep_ids)
                }
                ResolveTypeExprOutcome::Unbound(msg) => {
                    err(KError::new(KErrorKind::ShapeError(format!("VAL type: {msg}"))))
                }
            }
        }
    }
}

/// Free type-name references inside `te`. Each leaf will sub-Dispatch independently
/// so the dispatcher installs one Notify edge per placeholder hit.
fn collect_leaf_names(te: &TypeExpr) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    fn walk(
        te: &TypeExpr,
        out: &mut Vec<String>,
        seen: &mut std::collections::HashSet<String>,
    ) {
        match &te.params {
            TypeParams::None => {
                if seen.insert(te.name.clone()) {
                    out.push(te.name.clone());
                }
            }
            TypeParams::List(items) => {
                // Outer constructor name is itself a leaf reference (e.g. `Wrap` in
                // `Wrap<Number>` must resolve against decl_scope).
                if seen.insert(te.name.clone()) {
                    out.push(te.name.clone());
                }
                for it in items {
                    walk(it, out, seen);
                }
            }
            TypeParams::Function { args, ret } => {
                // `Function<...>` is a builtin keyword, not a resolvable name.
                for a in args {
                    walk(a, out, seen);
                }
                walk(ret, out, seen);
            }
        }
    }
    walk(te, &mut out, &mut seen);
    out
}

/// Bind `name` to `KObject::KTypeValue(declared_kt)` under `scope.bindings.data`.
fn finalize_val<'a>(scope: &'a Scope<'a>, name: String, declared_kt: KType<'a>) -> BodyResult<'a> {
    let arena = scope.arena;
    let allocated: &'a KObject<'a> = arena.alloc(KObject::KTypeValue(declared_kt));
    if let Err(e) = scope.bind_value(name, allocated) {
        return err(e);
    }
    BodyResult::Value(allocated)
}

/// Combine over the sub-Dispatch; finalize once `results[0]` carries the resolved
/// `KTypeValue`. Errored deps short-circuit via `run_combine` before the closure runs.
fn defer_val_via_combine<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    name: String,
    te: TypeExpr,
    resolve_id: NodeId,
) -> BodyResult<'a> {
    let name_for_finish = name;
    let te_for_finish = te;
    let finish: CombineFinish<'a> = Box::new(move |scope, _sched, results| {
        debug_assert_eq!(results.len(), 1, "VAL Combine has exactly one dep");
        let resolved = results[0];
        let kt = match resolved {
            KObject::KTypeValue(kt) => kt.clone(),
            // A non-`KTypeValue` here would be a routing bug; surface a structured
            // error rather than panicking.
            other => {
                return BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
                    "VAL type `{}` sub-dispatch resolved to a non-type value of kind `{}`",
                    te_for_finish.render(),
                    other.ktype().name(),
                ))));
            }
        };
        finalize_val(scope, name_for_finish.clone(), kt)
    });
    let combine_id = sched.add_combine(vec![resolve_id], vec![], scope, finish);
    BodyResult::DeferTo(combine_id)
}

/// Combine over per-leaf sub-Dispatches; re-elaborates the structural `TypeExpr`
/// once every leaf has terminalized. Per-leaf parks use Notify edges so the SIG
/// outer Combine keeps exclusive cascade-free ownership of sibling LET producers.
fn defer_val_structural_via_combine<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    name: String,
    te: TypeExpr,
    dep_ids: Vec<NodeId>,
) -> BodyResult<'a> {
    let name_for_finish = name;
    let te_for_finish = te;
    let finish: CombineFinish<'a> = Box::new(move |scope, _sched, _results| {
        match scope.resolve_type_expr(&te_for_finish) {
            ResolveTypeExprOutcome::Done(kt) => finalize_val(scope, name_for_finish.clone(), kt.clone()),
            ResolveTypeExprOutcome::Park(_) => BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
                "VAL type `{}` elaboration parked again after all leaf sub-Dispatches \
                 terminalized — internal scheduling invariant violated",
                te_for_finish.render(),
            )))),
            ResolveTypeExprOutcome::Unbound(msg) => {
                BodyResult::Err(KError::new(KErrorKind::ShapeError(format!("VAL type: {msg}"))))
            }
        }
    });
    let combine_id = sched.add_combine(dep_ids, vec![], scope, finish);
    BodyResult::DeferTo(combine_id)
}

/// Dispatch-time placeholder extractor: `parts[1]` is the bound name's `Identifier`.
pub(crate) fn pre_run(expr: &KExpression<'_>) -> Option<String> {
    match &expr.parts.get(1)?.value {
        ExpressionPart::Identifier(s) => Some(s.clone()),
        _ => None,
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    // The Design-B sigil consumes `:`, so the signature has no explicit colon keyword.
    register_builtin_with_pre_run(
        scope,
        "VAL",
        sig(KType::Any, vec![
            kw("VAL"),
            arg("name", KType::Identifier),
            arg("ty", KType::TypeExprRef),
        ]),
        body,
        Some(pre_run),
    );
}

#[cfg(test)]
mod tests;
