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

use crate::machine::FinishCtx;
use crate::machine::StepCarried;
use crate::machine::WriteGate;
use crate::machine::model::labels::TypeSymbol;
use crate::machine::model::{ExpressionPart, KExpression};
use crate::machine::model::{KKind, KObject, KType, TypeNode};
use crate::machine::{KError, KErrorKind, Scope};
use crate::source::Spanned;

use super::{arg, kw, sig};
use crate::machine::model::Carried;
use crate::machine::model::RunRegistries;

// This builtin's slot spellings, minted once and read back by symbol.
crate::slots! { SLOTS { name, ty } }

fn typeexpr_from_carrier(kt: KType, registries: &RunRegistries) -> CarrierForm {
    let types = &registries.types;
    // The builtin leaf type names re-resolve against decl_scope through the same name path so a
    // SIG-local shadow wins over the builtin table. `:Module` lowers to the empty signature —
    // its `name()` is "Module" — and joins that leaf path. A user-declared signature (a non-empty
    // interface) stays `Direct`: re-resolution is by name, and an aliased user SIG reached
    // through a `LET` could miss or hit a shadow.
    let is_leaf_builtin = matches!(
        types.node(kt),
        TypeNode::Number
            | TypeNode::Str
            | TypeNode::Bool
            | TypeNode::Null
            | TypeNode::Any
            | TypeNode::Identifier
            | TypeNode::KExpression
            | TypeNode::OfKind(KKind::AnyType | KKind::Signature | KKind::ProperType)
    );
    if is_leaf_builtin || kt == KType::EMPTY_SIGNATURE {
        // A leaf handle reaches its name only as rendered text, so this re-declares it — the one
        // seam where a builtin's name is minted from a string rather than a token.
        CarrierForm::Leaf(
            TypeSymbol::declared(&kt.name(registries), &registries.labels)
                .expect("a builtin leaf name is a Type token"),
        )
    } else {
        CarrierForm::Direct(kt)
    }
}

enum CarrierForm {
    /// Builtin leaf synthesized from `kt.name()`; re-elaborated against decl_scope
    /// so a SIG-local shadow wins over the builtin table.
    Leaf(TypeSymbol),
    Raw(TypeSymbol),
    /// Structural carrier accepted as-is; inner names are not re-bound.
    Direct(KType),
}

/// SIG-body-only value-slot declarator. Same SIG-body guard and carrier-shape split: reads its
/// args from `BodyCtx::args`, registers the value slot's declared type directly on a scope, and
/// returns `Action::Done` for a structural carrier or an `Action::AwaitDeps` (one `OwnScope` type
/// sub-dispatch) for a leaf that re-resolves against decl_scope.
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> crate::machine::Action<'a> {
    use crate::builtins::resolve_or_await::dispatch_type_then;
    use crate::machine::Action;

    let done_err = |e: KError| Action::done(Err(e));

    if !ctx.scope.is_in_sig_body() {
        return done_err(KError::new(KErrorKind::ShapeError(
            "VAL is only valid inside a SIG body — use LET for value bindings in \
             modules and run-root scope"
                .to_string(),
        )));
    }

    let name = match ctx.args.object(&SLOTS.name) {
        Some(KObject::KString(s)) => (*s).to_string(),
        Some(other) => {
            return done_err(KError::new(KErrorKind::TypeMismatch {
                arg: "name".to_string(),
                expected: "Identifier".to_string(),
                got: other.ktype().name(ctx.registries),
            }));
        }
        None => return done_err(KError::new(KErrorKind::MissingArg("name".to_string()))),
    };

    // A slot binds a value name. Type members (Type-class names) are declared with `TYPE`
    // (abstract) or `LET` (manifest), not `VAL` — so a Type token here gets the spelling
    // correction rather than the generic partition message.
    let Some(slot_name) =
        crate::machine::model::ValueSymbol::declared(&name, &ctx.registries.labels)
    else {
        return done_err(KError::new(KErrorKind::ShapeError(format!(
            "VAL slot name `{name}` classifies as a Type token; declare an abstract type \
             member with `TYPE {name}` or a manifest one with `LET {name} = <Type>`",
        ))));
    };

    let carrier = match ctx.args.unresolved_type(&SLOTS.ty) {
        Some(te) => CarrierForm::Raw(te),
        None => match ctx.args.ktype(&SLOTS.ty) {
            Some(kt) => typeexpr_from_carrier(kt, ctx.registries),
            None => {
                return done_err(match ctx.args.object(&SLOTS.ty) {
                    Some(other) => KError::new(KErrorKind::TypeMismatch {
                        arg: "ty".to_string(),
                        expected: "ProperType".to_string(),
                        got: other.ktype().name(ctx.registries),
                    }),
                    None => KError::new(KErrorKind::MissingArg("ty".to_string())),
                });
            }
        },
    };

    let te = match carrier {
        CarrierForm::Direct(kt) => return finalize_val(&ctx.finish_ctx(), slot_name, kt),
        // Both leaf and raw carriers re-dispatch the leaf against decl_scope so a SIG-local
        // `LET <name> = ...` shadow wins over the builtin table. A `Raw` carrier always holds a
        // bare-leaf name (parameterized surface forms sub-Dispatch earlier).
        CarrierForm::Leaf(te) => te,
        CarrierForm::Raw(te) => te,
    };

    let brand = ctx.scope.brand();
    let expr = KExpression::new(brand, &[Spanned::bare(ExpressionPart::Type(te))]);
    dispatch_type_then(brand, expr, "VAL type slot", move |fctx, kt| {
        finalize_val(fctx, slot_name, kt)
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
/// `declared_kt` arrives from a bind-time `ty` argument or a leaf re-dispatch's dep terminal. The
/// [`WriteOp::SigSlot`](crate::machine::core::bindings::WriteOp) the outcome carries installs it in
/// the SIG decl scope's slot collector; [`Scope::resident`] seals the same handle into
/// the terminal.
fn finalize_val<'a>(
    fctx: &FinishCtx<'a, '_>,
    name: crate::machine::model::ValueSymbol,
    declared_kt: KType,
) -> crate::machine::Action<'a> {
    use crate::machine::Action;
    if let Some(message) = crate::machine::model::unsaturated_constructor_message(
        declared_kt,
        &format!(
            "the type of SIG value slot `{}`",
            crate::machine::model::render_label(name.symbol(), fctx.registries)
        ),
        fctx.registries,
    ) {
        return Action::done(Err(KError::new(KErrorKind::ShapeError(message))));
    }
    Action::done(Ok(StepCarried::born(
        fctx.scope.resident(Carried::Type(declared_kt)),
    )))
    .with_effect(crate::machine::core::bindings::WriteOp::SigSlot {
        name,
        kt: declared_kt,
    })
}

pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    // Design-B sigil consumes `:`; no explicit colon keyword in the signature.
    let signature = sig(
        KType::ANY,
        vec![
            kw("VAL"),
            arg(registries, &SLOTS.name, KType::IDENTIFIER),
            arg(registries, &SLOTS.ty, KType::of_kind(KKind::ProperType)),
        ],
    );
    // VAL installs nothing: it records into the decl scope's slot collector, not into a binding map
    // any name lookup or forward-reference walk can see. Its `BINDER_SPECS` entry has empty
    // extractors to match — no name, no bucket. Its declaration slot is still
    // declaration-classified in dispatch, via the spec entry's `name_slot` cached on the
    // expression.
    crate::builtins::register_builtin(scope, "VAL", signature, body, registries, gate);
}

#[cfg(test)]
mod tests;
