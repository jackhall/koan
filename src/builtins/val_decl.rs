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

use crate::machine::core::kfunction::action::FinishCtx;
use crate::machine::model::ast::{ExpressionPart, KExpression, TypeIdentifier};
use crate::machine::model::types::KKind;
use crate::machine::model::{KObject, KType};
use crate::machine::DeliveredCarried;
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
    use crate::builtins::resolve_or_await::dispatch_type_then;
    use crate::machine::core::kfunction::action::{arg_object, arg_type, Action};

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

    // Defense-in-depth: abstract-type members (Type-class names) must use `LET`, not `VAL`.
    if crate::parse::is_type_name(&name) {
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

    let te = match carrier {
        CarrierForm::Direct(kt) => {
            // A bind-time `ty` argument: any caller-supplied carrier (a `:(...)` sub-dispatch
            // spliced in before this call), so `arg_carrier` names its own foreign reach if it
            // has one.
            return finalize_val(
                &ctx.finish_ctx(),
                name,
                kt,
                bind_index,
                ctx.arg_carrier("ty"),
            );
        }
        // Both leaf and raw carriers re-dispatch the leaf against decl_scope so a SIG-local
        // `LET <name> = ...` shadow wins over the builtin table. A `KType::Unresolved` carrier always
        // holds a bare-leaf `TypeIdentifier` (parameterized surface forms sub-Dispatch earlier).
        CarrierForm::Leaf(te) => te,
        CarrierForm::Raw(te) => te,
    };

    let expr = KExpression::new(vec![Spanned::bare(ExpressionPart::Type(te))]);
    dispatch_type_then(expr, "VAL type slot", move |fctx, kt, carrier| {
        finalize_val(fctx, name, kt, bind_index, Some(carrier))
    })
}

/// Records the value slot's declared type in `bindings.types` and returns the slot's carrier as
/// `Action::Done`. A VAL is a *value* member whose *declared type* we keep; storing the `KType`
/// directly (not a boxed carrier) keeps the type table the single home for everything ascription
/// enumerates. Uses the same infallible `register_type` path as a SIG-local `LET <TypeIdentifier> = …`
/// abstract member.
///
/// `declared_kt` can embed a borrow into `carrier`'s producer region (a bound `KFunctor`, a
/// nominal `SetRef`, ...) whether it arrived as a bind-time `ty` argument or a leaf re-dispatch's
/// dep terminal. When `carrier` is `Some`, the stored binding's reach and the sealed result's
/// witness fold it in. When `carrier` is `None`, the seal picks the tier `declared_kt`'s own shape
/// needs — the compile-enforced `'static` tier ([`StepAllocator::alloc_type`]) for an owned leaf,
/// the runtime-audited seal ([`StepAllocator::alloc_type_checked`]) otherwise — so neither
/// under-witnesses the declared type's actual reach.
fn finalize_val<'a>(
    fctx: &FinishCtx<'a>,
    name: String,
    declared_kt: KType<'a>,
    bind_index: BindingIndex,
    carrier: Option<&DeliveredCarried>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::Action;
    let stored = carrier
        .map(|c| fctx.scope.host_reach_of(c))
        .unwrap_or_default();
    if let Err(e) = fctx
        .scope
        .register_user_type(name, declared_kt.clone(), bind_index, stored)
    {
        return Action::Done(Err(e));
    }
    let sealed = match carrier {
        // Seal the carrier's own type terminal. `alloc_type_of` rebuilds the type from the dep's
        // view at the fold brand — the built value equals `declared_kt` because both callers source
        // `kt` from this carrier's own terminal (`expect_type_terminal` clones `Carried::Type(kt)`;
        // the `Direct` arm's `ty` argument is the spliced sub-dispatch this carrier delivers), so
        // the view and the ambient `declared_kt` are the same delivered type.
        Some(c) => fctx.ctx.alloc_type_of(c),
        // A region-free declared type takes the compile-enforced `'static` tier; one embedding a
        // region borrow (a bound `KFunctor`, a nominal `SetRef`) takes the runtime-checked seal.
        None => {
            let sealed = match declared_kt.to_static() {
                Some(owned) => Ok(fctx.ctx.alloc_type(owned)),
                None => fctx.ctx.alloc_type_checked(declared_kt),
            };
            match sealed {
                Ok(sealed) => sealed,
                Err(e) => return Action::Done(Err(e)),
            }
        }
    };
    Action::Done(Ok(sealed))
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
        // VAL records a value-slot's declared type into the SIG decl-scope's `types` map
        // (a type-language write), so its forward-reference placeholder is `Type`-kind.
        Some((binder_name, crate::machine::BindKind::Type)),
        None,
        false,
    );
}

#[cfg(test)]
mod tests;
