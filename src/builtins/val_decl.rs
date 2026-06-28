//! `VAL <name:Identifier> : <ty:ProperType>` — SIG-body-only declarator for value
//! slots whose declared type is recorded explicitly. See
//! [design/typing/modules.md § Structures and signatures](../../design/typing/modules.md#structures-and-signatures).
//!
//! A VAL slot records "value member whose declared type is `kt`" into the SIG decl_scope's
//! `bindings.types[name]` — the same type table the SIG-local `LET <TypeIdentifier> = …` abstract
//! members live in. A value-class slot name keeps it distinguishable from an abstract-type
//! member (Type-class name) when ascription enumerates the table.
//!
//! Type resolution dispatches on the `ty` carrier shape: a [`KType::Unresolved`] leaf or a
//! builtin leaf re-dispatch against decl_scope so a SIG-local `LET <name> = ...` shadow wins
//! over the builtin table; structural carriers (`KFunction`, `List`, ...) are taken directly.

use crate::machine::model::ast::{ExpressionPart, KExpression, TypeIdentifier};
use crate::machine::model::types::KKind;
use crate::machine::model::{Carried, KObject, KType};
use crate::machine::{BindingIndex, KError, KErrorKind, Scope};
use crate::source::Spanned;

use super::{arg, kw, sig};

fn typeexpr_from_carrier<'a>(kt: &KType<'a>) -> CarrierForm<'a> {
    match kt {
        KType::Unresolved(te) => CarrierForm::Raw(te.clone()),
        KType::Number
        | KType::Str
        | KType::Bool
        | KType::Null
        | KType::OfKind(KKind::AnyType)
        | KType::OfKind(KKind::Signature)
        | KType::OfKind(KKind::Module)
        | KType::Any
        | KType::Identifier
        | KType::KExpression
        | KType::OfKind(KKind::ProperType) => CarrierForm::Leaf(TypeIdentifier::leaf(kt.name())),
        _ => CarrierForm::Direct(kt.clone()),
    }
}

enum CarrierForm<'a> {
    /// Builtin leaf synthesized from `kt.name()`; re-elaborated against decl_scope
    /// so a SIG-local shadow wins over the builtin table.
    Leaf(TypeIdentifier),
    Raw(TypeIdentifier),
    /// Structural carrier accepted as-is; inner names are not re-bound.
    Direct(KType<'a>),
}

/// SIG-body-only value-slot declarator. Same SIG-body guard and carrier-shape split: reads its
/// args from `BodyCtx::args`, registers the value slot's declared type directly on a scope, and
/// returns `Action::Done` for a structural carrier or an `Action::AwaitDeps` (one `OwnScope` type
/// sub-dispatch) for a leaf that re-resolves against decl_scope.
pub fn body<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::{
        arg_object, arg_type, Action, AwaitContinue, Dep, DepPlacement,
    };

    let done_err = |e: KError| Action::Done(Err(e));

    if !ctx.scope.is_in_sig_body() {
        return done_err(KError::new(KErrorKind::ShapeError(
            "VAL is only valid inside a SIG body — use LET for value bindings in \
             modules and run-root scope"
                .to_string(),
        )));
    }

    let name = match arg_object(ctx.args, "name") {
        Some(KObject::KString(s)) => s.clone(),
        Some(other) => {
            return done_err(KError::new(KErrorKind::TypeMismatch {
                arg: "name".to_string(),
                expected: "Identifier".to_string(),
                got: other.ktype().name(),
            }));
        }
        None => return done_err(KError::new(KErrorKind::MissingArg("name".to_string()))),
    };

    // Defense-in-depth: abstract-type members must use `LET`, not `VAL`.
    if super::ascribe::is_abstract_type_name(&name) {
        return done_err(KError::new(KErrorKind::ShapeError(format!(
            "VAL slot name `{name}` classifies as a Type token; abstract-type members \
             must use `LET {name} = <Type>` instead of `VAL`",
        ))));
    }

    let carrier = match arg_type(ctx.args, "ty") {
        Some(kt) => typeexpr_from_carrier(kt),
        None => {
            return done_err(match arg_object(ctx.args, "ty") {
                Some(other) => KError::new(KErrorKind::TypeMismatch {
                    arg: "ty".to_string(),
                    expected: "ProperType".to_string(),
                    got: other.ktype().name(),
                }),
                None => KError::new(KErrorKind::MissingArg("ty".to_string())),
            });
        }
    };

    let bind_index = ctx.bind_index();

    let (te, _) = match carrier {
        CarrierForm::Direct(kt) => {
            return finalize_val(ctx.scope, name, kt, bind_index);
        }
        // Both leaf and raw carriers re-dispatch the leaf against decl_scope so a SIG-local
        // `LET <name> = ...` shadow wins over the builtin table. A `KType::Unresolved` carrier always
        // holds a bare-leaf `TypeIdentifier` (parameterized surface forms sub-Dispatch earlier).
        CarrierForm::Leaf(te) => (te, ()),
        CarrierForm::Raw(te) => (te, ()),
    };

    let expr = KExpression::new(vec![Spanned::bare(ExpressionPart::Type(te.clone()))]);
    let name_for_finish = name;
    let te_for_finish = te;
    let finish: AwaitContinue<'a> = Box::new(move |fctx, results| {
        debug_assert_eq!(results.len(), 1, "VAL dep-finish has exactly one dep");
        let kt = match &results[0] {
            Carried::Type(kt) => (*kt).clone(),
            // Routing bug — surface structured, don't panic.
            Carried::Object(other) => {
                return Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                    "VAL type `{}` sub-dispatch resolved to a non-type value of kind `{}`",
                    te_for_finish.render(),
                    other.ktype().name(),
                )))));
            }
        };
        finalize_val(fctx.scope, name_for_finish.clone(), kt, bind_index)
    });
    Action::AwaitDeps {
        deps: vec![Dep::Dispatch {
            expr,
            placement: DepPlacement::OwnScope,
        }],
        finish,
    }
}

/// Records the value slot's declared type in `bindings.types` and returns the slot's carrier as
/// `Action::Done`. A VAL is a *value* member whose *declared type* we keep; storing the `KType`
/// directly (not a boxed carrier) keeps the type table the single home for everything ascription
/// enumerates. Uses the same infallible `register_type` path as a SIG-local `LET <TypeIdentifier> = …`
/// abstract member.
fn finalize_val<'a>(
    scope: &Scope<'a>,
    name: String,
    declared_kt: KType<'a>,
    bind_index: BindingIndex,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::Action;
    let kt_ref: &'a KType<'a> = scope.region.alloc_ktype(declared_kt.clone());
    if let Err(e) = scope.register_user_type(name, declared_kt, bind_index) {
        return Action::Done(Err(e));
    }
    Action::DoneWitnessed(scope.seal_type(Carried::Type(kt_ref)))
}

pub(crate) fn binder_name(expr: &KExpression<'_>) -> Option<String> {
    match &expr.parts.get(1)?.value {
        ExpressionPart::Identifier(s) => Some(s.clone()),
        _ => None,
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    // Design-B sigil consumes `:`; no explicit colon keyword in the signature.
    let signature = sig(
        KType::Any,
        vec![
            kw("VAL"),
            arg("name", KType::Identifier),
            arg("ty", KType::OfKind(KKind::ProperType)),
        ],
    );
    crate::builtins::register_builtin_full(
        scope,
        "VAL",
        signature,
        body,
        Some(binder_name),
        None,
        false,
    );
}

#[cfg(test)]
mod tests;
