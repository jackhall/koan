//! `VAL <name:Identifier> : <ty:ProperType>` — SIG-body-only declarator for value
//! slots whose declared type is recorded explicitly. See
//! [design/typing/modules.md § Structures and signatures](../../design/typing/modules.md#structures-and-signatures).
//!
//! A VAL slot records "value member whose declared type is `kt`" into the SIG decl_scope's
//! own slot collector ([`Scope::sig_slot`]/[`Scope::sig_value_slots`]) — a schema-in-progress
//! separate from `bindings.types`, the table `TYPE <Name>` abstract members and
//! `LET <Name> = <Type>` manifest members live in. VAL never binds a value: the slot is a
//! specification (name → declared type) the module supplies a value for.
//!
//! Type resolution dispatches on the `ty` carrier shape: a [`KType::Unresolved`] leaf or a
//! builtin leaf re-dispatch against decl_scope so a SIG-local type member shadow wins over the
//! builtin table; structural carriers (`KFunction`, `List`, ...) are taken directly.

use crate::machine::model::{ExpressionPart, KExpression, TypeIdentifier};
use crate::machine::model::{KKind, SigSource};
use crate::machine::model::{KObject, KType};
use crate::machine::DeliveredCarried;
use crate::machine::FinishCtx;
use crate::machine::{KError, KErrorKind, Scope};
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
        // `:Module` lowers to the empty signature; its `name()` is "Module", so it re-resolves
        // against decl_scope through the same leaf path as the other builtin type names.
        | KType::Signature { sig: SigSource::Empty, .. }
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
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'a, '_>) -> crate::machine::Action<'a> {
    use crate::builtins::resolve_or_await::dispatch_type_then;
    use crate::machine::{arg_object, arg_type, Action};

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

    // Defense-in-depth: type members (Type-class names) are declared with `TYPE` (abstract)
    // or `LET` (manifest), not `VAL`.
    if crate::parse::is_type_name(&name) {
        return done_err(KError::new(KErrorKind::ShapeError(format!(
            "VAL slot name `{name}` classifies as a Type token; declare an abstract type \
             member with `TYPE {name}` or a manifest one with `LET {name} = <Type>`",
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

    let te = match carrier {
        CarrierForm::Direct(kt) => {
            // A bind-time `ty` argument: any caller-supplied carrier (a `:(...)` sub-dispatch
            // spliced in before this call), so `arg_carrier` names its own foreign reach if it
            // has one.
            return finalize_val(&ctx.finish_ctx(), name, kt, ctx.arg_carrier("ty"));
        }
        // Both leaf and raw carriers re-dispatch the leaf against decl_scope so a SIG-local
        // `LET <name> = ...` shadow wins over the builtin table. A `KType::Unresolved` carrier always
        // holds a bare-leaf `TypeIdentifier` (parameterized surface forms sub-Dispatch earlier).
        CarrierForm::Leaf(te) => te,
        CarrierForm::Raw(te) => te,
    };

    let expr = KExpression::new(vec![Spanned::bare(ExpressionPart::Type(te))]);
    dispatch_type_then(expr, "VAL type slot", move |fctx, kt, carrier| {
        finalize_val(fctx, name, kt, Some(carrier))
    })
}

/// Records the value slot's declared type into the SIG decl scope's slot collector and returns
/// the slot's carrier as `Action::Done`.
///
/// `declared_kt` can embed a borrow into `carrier`'s producer region (a declared `Signature`, a
/// nominal `SetRef`, ...) whether it arrived as a bind-time `ty` argument or a leaf re-dispatch's
/// dep terminal. [`Scope::register_sig_slot_delivered`] derives the slot's stored reach off
/// `carrier` (the empty token when `carrier` is `None`) and installs a region-resident copy of the
/// type into the collector; the returned resident `&KType` is not needed here, because the terminal
/// is sealed separately below from the carrier's own view.
fn finalize_val<'a>(
    fctx: &FinishCtx<'a>,
    name: String,
    declared_kt: KType<'a>,
    carrier: Option<&DeliveredCarried>,
) -> crate::machine::Action<'a> {
    use crate::machine::Action;
    if let Err(e) = fctx
        .scope
        .register_sig_slot_delivered(name, declared_kt.clone(), carrier)
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
        // region borrow (a declared `Signature`, a nominal `SetRef`) takes the runtime-checked seal.
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
        scope, "VAL", signature, body,
        // VAL installs no dispatch-time placeholder: it records into the decl scope's slot
        // collector, not into a binding map any name lookup or forward-reference walk can see.
        None, None,
    );
}

#[cfg(test)]
mod tests;
