//! `TYPE <name:ProperType>` / `TYPE (<Param> AS <Name>)` — SIG-body-only declarators for
//! *abstract* type members: a witness-less type slot a satisfying module must supply. See
//! [design/typing/modules.md § Structures and signatures](../../design/typing/modules.md#structures-and-signatures).
//!
//! Two overloads share the keyword `TYPE`:
//!
//! - the bare form `TYPE Elt` binds a first-order abstract member as
//!   [`KType::AbstractType`] `{ source: <decl_scope id>, name }` — no witness, open for a
//!   client to share via a `WITH` constraint;
//! - the higher-kinded form `TYPE (Type AS Wrap)` binds an abstract type *constructor*,
//!   mirroring the application surface `:(Number AS Wrap)` with the concrete argument replaced
//!   by the parameter name. It mints a sentinel [`KKind::TypeConstructor`] `SetRef` (the
//!   "declared abstract, awaiting per-call mint" brand); opaque ascription re-mints a fresh
//!   per-call singleton carrying the view module's scope id. Arity 1 only.
//!
//! Both bind through the same fused `register_user_type_delivered` + `resident_type_carrier`
//! path the `LET` type route uses, so a `TYPE`-declared member rides the same
//! `bindings.types` entry a manifest `LET` member does — value slots (`VAL`) live in the
//! decl scope's own slot collector, a separate storage channel `bindings.types` never sees.

use std::collections::HashMap;

use crate::machine::model::KType;
use crate::machine::model::{ExpressionPart, KExpression};
use crate::machine::model::{KKind, NominalSchema, RecursiveSet};
use crate::machine::StepCarried;
use crate::machine::{KError, KErrorKind, Scope, ScopeId};

use super::{arg, kw, sig};

/// Mint the type-constructor representation: a singleton [`RecursiveSet`] of one
/// [`KKind::TypeConstructor`] member at `scope_id`, filled with an empty variant schema
/// (identity ignores it) and the declared `param_names`. The `scope_id` is the sole
/// discriminant between the two shapes this mints: [`ScopeId::SENTINEL`] brands TYPE's abstract
/// slot (the "declared abstract, awaiting per-call mint" member a real constructor never
/// carries), while a real declaring scope's id brands a NEWTYPE-declared constructor family.
/// Opaque ascription re-mints a fresh per-call singleton off the sentinel shape.
pub(crate) fn mint_type_constructor<'a>(
    member_name: String,
    param_names: Vec<String>,
    scope_id: ScopeId,
) -> KType<'a> {
    let set = RecursiveSet::singleton(
        member_name,
        scope_id,
        NominalSchema::TypeConstructor {
            schema: HashMap::new(),
            param_names,
        },
    );
    KType::SetRef { set, index: 0 }
}

fn not_in_sig_body() -> KError {
    KError::new(KErrorKind::ShapeError(
        "TYPE declares an abstract type slot and is only valid inside a SIG body; \
         use LET to define a type alias"
            .to_string(),
    ))
}

/// Bind `kt` under `name` through the fused mint + alloc + register path, returning the bound
/// type's resident carrier. `kt` is region-pure (an `AbstractType`) or an owned
/// sentinel `SetRef`, so no delivered carrier folds in — the fused door picks the tier the
/// type's own shape needs.
fn bind_abstract_member<'a>(
    ctx: &crate::machine::BodyCtx<'a, '_>,
    name: String,
    kt: KType<'a>,
) -> crate::machine::Action<'a> {
    use crate::machine::Action;
    let bind_index = ctx.bind_index();
    let (kt_ref, stored) = match ctx
        .scope
        .register_user_type_delivered(name, kt, None, bind_index)
    {
        Ok(pair) => pair,
        Err(e) => return Action::Done(Err(e)),
    };
    let carrier = ctx.scope.resident_type_carrier(kt_ref, stored);
    Action::Done(Ok(StepCarried::born(carrier)))
}

/// `TYPE <name:ProperType>` — first-order abstract member. Binds `AbstractType { decl scope id, name }`.
pub fn body_bare<'a>(ctx: &crate::machine::BodyCtx<'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::{require_bare_type_name, Action};

    if !ctx.scope.is_in_sig_body() {
        return Action::Done(Err(not_in_sig_body()));
    }
    let name = match require_bare_type_name(ctx.args, "name", "TYPE") {
        Ok(name) => name,
        Err(e) => return Action::Done(Err(e)),
    };
    let kt = KType::AbstractType {
        source: ctx.scope.id,
        name: name.clone(),
    };
    bind_abstract_member(ctx, name, kt)
}

/// `TYPE (<Param> AS <Name>)` — higher-kinded abstract member (declaration-by-example). Reads the
/// raw `(Param AS Name)` expression, mints the sentinel `TypeConstructor` `SetRef` under `Name`
/// with parameter `Param`. Arity 1 only.
pub fn body_hk<'a>(ctx: &crate::machine::BodyCtx<'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::{require_kexpression, Action};

    if !ctx.scope.is_in_sig_body() {
        return Action::Done(Err(not_in_sig_body()));
    }
    let decl = match require_kexpression(ctx.args, "TYPE", "decl") {
        Ok(decl) => decl,
        Err(e) => return Action::Done(Err(e)),
    };
    let (param_name, member_name) = match parse_hk_decl(&decl) {
        Ok(pair) => pair,
        Err(e) => return Action::Done(Err(e)),
    };
    let kt = mint_type_constructor(member_name.clone(), vec![param_name], ScopeId::SENTINEL);
    bind_abstract_member(ctx, member_name, kt)
}

/// Parse `(<Param> AS <Name>)` into `(param_name, member_name)`, both bare Type-class
/// identifiers. More than one part before `AS` is the arity-above-1 error; any other shape is a
/// form error naming the expected surface.
pub(crate) fn parse_hk_decl(decl: &KExpression<'_>) -> Result<(String, String), KError> {
    let shape_error = || {
        KError::new(KErrorKind::ShapeError(
            "TYPE constructor declaration must read `TYPE (<Param> AS <Name>)`".to_string(),
        ))
    };
    let as_pos = decl
        .parts
        .iter()
        .position(|p| matches!(&p.value, ExpressionPart::Keyword(k) if k == "AS"))
        .ok_or_else(shape_error)?;
    let param_parts = &decl.parts[..as_pos];
    let name_parts = &decl.parts[as_pos + 1..];
    if param_parts.len() > 1 {
        return Err(KError::new(KErrorKind::ShapeError(
            "TYPE constructor arity above 1 is not yet supported".to_string(),
        )));
    }
    if param_parts.len() != 1 || name_parts.len() != 1 {
        return Err(shape_error());
    }
    let bare_type = |part: &ExpressionPart<'_>| match part {
        ExpressionPart::Type(t) => Some(t.render()),
        _ => None,
    };
    let param_name = bare_type(&param_parts[0].value).ok_or_else(shape_error)?;
    let member_name = bare_type(&name_parts[0].value).ok_or_else(shape_error)?;
    Ok((param_name, member_name))
}

/// Dispatch-time placeholder extractor covering both overloads: the bare form's name is the
/// `Type` part at `parts[1]`; the higher-kinded form's name is the *last* inner part of the
/// parenthesized `(Param AS Name)` expression.
pub(crate) fn binder_name(expr: &KExpression<'_>) -> Option<String> {
    match &expr.parts.get(1)?.value {
        ExpressionPart::Type(t) => Some(t.render()),
        ExpressionPart::Expression(inner) => match &inner.parts.last()?.value {
            ExpressionPart::Type(t) => Some(t.render()),
            _ => None,
        },
        _ => None,
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    let bare_signature = sig(
        KType::Any,
        vec![kw("TYPE"), arg("name", KType::OfKind(KKind::ProperType))],
    );
    crate::builtins::register_builtin_full(
        scope,
        "TYPE",
        bare_signature,
        body_bare,
        Some((binder_name, crate::machine::BindKind::Type)),
        None,
    );
    let hk_signature = sig(
        KType::Any,
        vec![kw("TYPE"), arg("decl", KType::KExpression)],
    );
    crate::builtins::register_builtin_full(
        scope,
        "TYPE",
        hk_signature,
        body_hk,
        Some((binder_name, crate::machine::BindKind::Type)),
        None,
    );
}

#[cfg(test)]
mod tests;
