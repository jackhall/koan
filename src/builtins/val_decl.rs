//! `VAL <name:Identifier> : <ty:ProperType>` — SIG-body-only declarator for value
//! slots whose declared type is recorded explicitly. See
//! [design/typing/modules.md § Structures and signatures](../../design/typing/modules.md#structures-and-signatures).
//!
//! A VAL slot records "value member whose declared type is `kt`" into the SIG decl_scope's
//! own slot collector ([`Scope::sig_value_slots`]) — a schema-in-progress separate from
//! `bindings.types`, the table `TYPE <Name>` abstract members and `LET <Name> = <Type>`
//! manifest members live in. VAL never binds a value: the slot is a specification (name →
//! declared type) the module supplies a value for.
//!
//! Type resolution dispatches on the `ty` carrier shape: a [`Held::UnresolvedType`] name carrier
//! or a builtin leaf re-dispatch against decl_scope so a SIG-local type member shadow wins over the
//! builtin table; structural carriers (`KFunction`, `List`, ...) are taken directly.
//!
//! [`Held::UnresolvedType`]: crate::machine::model::Held::UnresolvedType

use crate::machine::model::{ExpressionPart, KExpression, TypeIdentifier};
use crate::machine::model::{KKind, KObject, KType, TypeRegistry};
use crate::machine::FinishCtx;
use crate::machine::StepCarried;
use crate::machine::{KError, KErrorKind, Scope, ScopeId};
use crate::source::Spanned;

use super::{arg, kw, sig};

fn typeexpr_from_carrier(kt: &KType, types: &TypeRegistry) -> CarrierForm {
    match kt {
        KType::Number
        | KType::Str
        | KType::Bool
        | KType::Null
        | KType::OfKind(KKind::AnyType)
        | KType::OfKind(KKind::Signature)
        | KType::Any
        | KType::Identifier
        | KType::KExpression
        | KType::OfKind(KKind::ProperType) => {
            CarrierForm::Leaf(TypeIdentifier::leaf(kt.name(types)))
        }
        // `:Module` lowers to the empty signature (no declaring scope); its `name()` is
        // "Module", so it re-resolves against decl_scope through the same leaf path as the
        // other builtin type names. A user-declared signature (a real `sig_id`) stays `Direct`:
        // re-resolution is by name, and an aliased user SIG reached through a `LET` could miss
        // or hit a shadow.
        KType::Signature { content, .. } if content.sig_id == ScopeId::SENTINEL => {
            CarrierForm::Leaf(TypeIdentifier::leaf(kt.name(types)))
        }
        _ => CarrierForm::Direct(kt.clone()),
    }
}

enum CarrierForm {
    /// Builtin leaf synthesized from `kt.name()`; re-elaborated against decl_scope
    /// so a SIG-local shadow wins over the builtin table.
    Leaf(TypeIdentifier),
    Raw(TypeIdentifier),
    /// Structural carrier accepted as-is; inner names are not re-bound.
    Direct(KType),
}

/// SIG-body-only value-slot declarator. Same SIG-body guard and carrier-shape split: reads its
/// args from `BodyCtx::args`, registers the value slot's declared type directly on a scope, and
/// returns `Action::Done` for a structural carrier or an `Action::AwaitDeps` (one `OwnScope` type
/// sub-dispatch) for a leaf that re-resolves against decl_scope.
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'a, '_>) -> crate::machine::Action<'a> {
    use crate::builtins::resolve_or_await::dispatch_type_then;
    use crate::machine::{arg_object, arg_type, arg_unresolved_type, Action};

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
                got: other.ktype().name(ctx.types),
            }));
        }
        None => return done_err(KError::new(KErrorKind::MissingArg("name".to_string()))),
    };

    // Defense-in-depth: type members (Type-class names) are declared with `TYPE` (abstract)
    // or `LET` (manifest), not `VAL`.
    if crate::parse::is_type_name(&name) {
        return done_err(KError::new(KErrorKind::ShapeError(format!(
            "VAL slot name `{name}` classifies as a Type token; declare an abstract type \
             member with `TYPE {name}` or a manifest one with `LET {name} = <Type>`",
        ))));
    }

    let carrier = match arg_unresolved_type(ctx.args, "ty") {
        Some(te) => CarrierForm::Raw(te.clone()),
        None => match arg_type(ctx.args, "ty") {
            Some(kt) => typeexpr_from_carrier(kt, ctx.types),
            None => {
                return done_err(match arg_object(ctx.args, "ty") {
                    Some(other) => KError::new(KErrorKind::TypeMismatch {
                        arg: "ty".to_string(),
                        expected: "ProperType".to_string(),
                        got: other.ktype().name(ctx.types),
                    }),
                    None => KError::new(KErrorKind::MissingArg("ty".to_string())),
                });
            }
        },
    };

    let te = match carrier {
        CarrierForm::Direct(kt) => return finalize_val(&ctx.finish_ctx(), name, kt),
        // Both leaf and raw carriers re-dispatch the leaf against decl_scope so a SIG-local
        // `LET <name> = ...` shadow wins over the builtin table. A `Raw` carrier always holds a
        // bare-leaf `TypeIdentifier` (parameterized surface forms sub-Dispatch earlier).
        CarrierForm::Leaf(te) => te,
        CarrierForm::Raw(te) => te,
    };

    let expr = KExpression::new(vec![Spanned::bare(ExpressionPart::Type(te))]);
    dispatch_type_then(expr, "VAL type slot", move |fctx, kt| {
        finalize_val(fctx, name, kt)
    })
}

/// Records the value slot's declared type into the SIG decl scope's slot collector and returns
/// the slot's carrier as `Action::Done`, uniform with `type_decl::bind_abstract_member` and the
/// `LET` type route.
///
/// A slot declares the type of a value, so the declared type must be a proper type: a bare
/// constructor (`VAL boxed :Wrapper` where `Wrapper` has kind `* -> *`) is a kind error here,
/// while a first-order abstract member (`TYPE Elem` → `VAL zero :Elem`) is proper and admits.
///
/// `declared_kt` arrives as owned data — a bind-time `ty` argument or a leaf re-dispatch's dep
/// terminal. [`Scope::register_sig_slot_delivered`] allocates it into the SIG decl scope's own
/// region through the single storage door and installs it in the collector, handing back that
/// resident `&KType` — which [`Scope::resident_type_carrier`] seals into the terminal, born
/// co-located with the stored slot rather than rebuilt from a second allocation.
fn finalize_val<'a>(
    fctx: &FinishCtx<'a, '_>,
    name: String,
    declared_kt: KType,
) -> crate::machine::Action<'a> {
    use crate::machine::Action;
    if let Some(message) = crate::machine::model::unsaturated_constructor_message(
        &declared_kt,
        &format!("the type of SIG value slot `{name}`"),
        fctx.types,
    ) {
        return Action::Done(Err(KError::new(KErrorKind::ShapeError(message))));
    }
    let kt_ref = match fctx.scope.register_sig_slot_delivered(name, declared_kt) {
        Ok(kt_ref) => kt_ref,
        Err(e) => return Action::Done(Err(e)),
    };
    Action::Done(Ok(StepCarried::born(
        fctx.scope.resident_type_carrier(kt_ref),
    )))
}

pub fn register<'a>(scope: &'a Scope<'a>, types: &TypeRegistry) {
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
        scope, "VAL", signature, body,
        // VAL installs no dispatch-time placeholder: it records into the decl scope's slot
        // collector, not into a binding map any name lookup or forward-reference walk can see.
        None, None, types,
    );
}

#[cfg(test)]
mod tests;
