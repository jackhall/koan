//! `VAL <name:Identifier> : <ty:TypeExprRef>` — SIG-body-only declarator for value
//! slots whose declared type is recorded explicitly. See
//! [design/typing/modules.md § Structures and signatures](../../design/typing/modules.md#structures-and-signatures).
//!
//! Inside a SIG decl_scope `bindings.data[name] = KObject::KTypeValue(declared_kt)`
//! means "value slot whose declared type is `kt`" rather than "name bound to a type
//! value"; the disambiguation is by scope context.
//!
//! Type resolution dispatches on the `ty` carrier shape: leaf carriers (builtin
//! `KTypeValue` or unresolved-leaf `TypeNameRef`) re-dispatch against decl_scope so a
//! SIG-local `LET <name> = ...` shadow wins over the builtin table; structural
//! carriers (`KFunction`, `List`, ...) are lifted directly.

use crate::machine::core::source::Spanned;
use crate::machine::model::ast::{ExpressionPart, KExpression, TypeName};
use crate::machine::model::types::KKind;
use crate::machine::model::{KObject, KType};
use crate::machine::{
    ArgumentBundle, BindingIndex, BodyResult, CombineFinish, KError, KErrorKind, NodeId,
    SchedulerHandle, Scope,
};

use super::{arg, err, kw, register_builtin_with_binder, sig};

fn schedule_type_resolve<'a>(
    sched: &mut dyn SchedulerHandle<'a>,
    decl_scope: &'a Scope<'a>,
    te: &TypeName,
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
            | KType::OfKind(KKind::Any)
            | KType::OfKind(KKind::Signature)
            | KType::OfKind(KKind::Module)
            | KType::Any
            | KType::Identifier
            | KType::KExpression
            | KType::OfKind(KKind::Proper) => Ok(CarrierForm::Leaf(TypeName::leaf(kt.name()))),
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
    Leaf(TypeName),
    Raw(TypeName),
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

    // Defense-in-depth: abstract-type members must use `LET`, not `VAL`.
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

    // Value-style: strict lexical cutoff against the SIG body's chain index.
    let bind_index = sched
        .current_lexical_chain()
        .map(|chain| BindingIndex::value(chain.index))
        .unwrap_or(BindingIndex::BUILTIN);

    match carrier {
        CarrierForm::Direct(kt) => finalize_val(scope, name, kt, bind_index),
        CarrierForm::Leaf(te) => {
            let resolve_id = schedule_type_resolve(sched, scope, &te);
            defer_val_via_combine(scope, sched, name, te, resolve_id, bind_index)
        }
        // A `TypeNameRef` carrier always holds a bare-leaf `TypeName` now —
        // parameterized surface forms sub-Dispatch and never reach this slot — so the
        // leaf is the only shape and always re-dispatches against decl_scope.
        CarrierForm::Raw(te) => {
            let resolve_id = schedule_type_resolve(sched, scope, &te);
            defer_val_via_combine(scope, sched, name, te, resolve_id, bind_index)
        }
    }
}

fn finalize_val<'a>(
    scope: &'a Scope<'a>,
    name: String,
    declared_kt: KType<'a>,
    bind_index: BindingIndex,
) -> BodyResult<'a> {
    let arena = scope.arena;
    let allocated: &'a KObject<'a> = arena.alloc_object(KObject::KTypeValue(declared_kt));
    if let Err(e) = scope.bind_value(name, allocated, bind_index) {
        return err(e);
    }
    BodyResult::value(allocated)
}

/// Errored deps short-circuit via `run_combine` before the closure runs.
fn defer_val_via_combine<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    name: String,
    te: TypeName,
    resolve_id: NodeId,
    bind_index: BindingIndex,
) -> BodyResult<'a> {
    let name_for_finish = name;
    let te_for_finish = te;
    let finish: CombineFinish<'a> = Box::new(move |scope, _sched, results| {
        debug_assert_eq!(results.len(), 1, "VAL Combine has exactly one dep");
        let resolved = results[0];
        let kt = match resolved {
            KObject::KTypeValue(kt) => kt.clone(),
            // Routing bug — surface structured, don't panic.
            other => {
                return BodyResult::Err(KError::new(KErrorKind::ShapeError(format!(
                    "VAL type `{}` sub-dispatch resolved to a non-type value of kind `{}`",
                    te_for_finish.render(),
                    other.ktype().name(),
                ))));
            }
        };
        finalize_val(scope, name_for_finish.clone(), kt, bind_index)
    });
    let combine_id = sched.add_combine(vec![resolve_id], vec![], scope, finish);
    BodyResult::DeferTo(combine_id)
}

pub(crate) fn binder_name(expr: &KExpression<'_>) -> Option<String> {
    match &expr.parts.get(1)?.value {
        ExpressionPart::Identifier(s) => Some(s.clone()),
        _ => None,
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    // Design-B sigil consumes `:`; no explicit colon keyword in the signature.
    register_builtin_with_binder(
        scope,
        "VAL",
        sig(
            KType::Any,
            vec![
                kw("VAL"),
                arg("name", KType::Identifier),
                arg("ty", KType::OfKind(KKind::Proper)),
            ],
        ),
        body,
        Some(binder_name),
    );
}

#[cfg(test)]
mod tests;
