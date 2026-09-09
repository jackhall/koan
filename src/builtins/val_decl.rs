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
//! The `ty` slot is an ordinary kind expectation, so the dispatch lane elaborates it against the
//! SIG body's own scope before the body runs — which is what makes a SIG-local type member shadow
//! win over the builtin table, since the lane's scope walk reaches the shadow first.

use crate::machine::FinishCtx;
use crate::machine::StepCarried;
use crate::machine::WriteGate;
use crate::machine::model::{KKind, KType};
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, arg_labeled, kw, sig};
use crate::machine::model::Carried;
use crate::machine::model::RunRegistries;

// This builtin's slot spellings, minted once and read back by symbol.
crate::slots! { SLOTS { name, ty } }

/// SIG-body-only value-slot declarator: reads its args from `BodyCtx::args` and registers the value
/// slot's declared type on the decl scope.
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::Action;

    let done_err = |e: KError| Action::done(Err(e));

    if !ctx.scope.is_in_sig_body() {
        return done_err(KError::new(KErrorKind::ShapeError(
            "VAL is only valid inside a SIG body — use LET for value bindings in \
             modules and run-root scope"
                .to_string(),
        )));
    }

    let Some(slot_name) = ctx.args.identifier(&SLOTS.name) else {
        return done_err(KError::new(KErrorKind::MissingArg("name".to_string())));
    };

    let Some(declared_kt) = ctx.args.ktype(&SLOTS.ty) else {
        return done_err(match ctx.args.object(&SLOTS.ty) {
            Some(other) => KError::new(KErrorKind::TypeMismatch {
                arg: "ty".to_string(),
                expected: "ProperType".to_string(),
                got: other.ktype().name(ctx.registries),
            }),
            None => KError::new(KErrorKind::MissingArg("ty".to_string())),
        });
    };
    finalize_val(&ctx.finish_ctx(), slot_name, declared_kt)
}

/// Records the value slot's declared type into the SIG decl scope's slot collector and returns
/// the slot's carrier as `Action::Done`, uniform with `type_decl::bind_abstract_member` and the
/// `LET` type route.
///
/// A slot declares the type of a value, so the declared type must be a proper type: a bare
/// constructor (`VAL boxed :Wrapper` where `Wrapper` has kind `* -> *`) is a kind error here,
/// while a first-order abstract member (`TYPE Elem` → `VAL zero :Elem`) is proper and admits.
///
/// `declared_kt` arrives lane-resolved in the `ty` argument. The
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
        format_args!(
            "the type of SIG value slot `{}`",
            crate::machine::model::display_label(name.symbol(), fctx.registries)
        ),
        fctx.registries,
    ) {
        return Action::done(Err(KError::new(KErrorKind::ShapeError(message))));
    }
    Action::done(Ok(StepCarried::born(
        fctx.scope.resident(Carried::Type(declared_kt)),
    )))
    .with_effect(
        fctx.scratch,
        crate::machine::core::bindings::WriteOp::SigSlot {
            name,
            kt: declared_kt,
        },
    )
}

pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    // Design-B sigil consumes `:`; no explicit colon keyword in the signature.
    let signature = sig(
        KType::ANY,
        vec![
            kw(registries, "VAL"),
            arg(registries, &SLOTS.name, KType::IDENTIFIER),
            arg_labeled(
                registries,
                &SLOTS.ty,
                KType::of_kind(KKind::ProperType),
                "VAL slot type",
            ),
        ],
    );
    // VAL installs nothing: it records into the decl scope's slot collector, not into a binding map
    // any name lookup or forward-reference walk can see. Its `BINDER_SPECS` entry has empty
    // extractors to match — no name, no bucket. Its declaration slot is still
    // declaration-classified in dispatch, via the spec entry's `name_slot` cached on the
    // expression.
    crate::builtins::register_builtin(scope, signature, body, registries, gate);
}

#[cfg(test)]
mod tests;
