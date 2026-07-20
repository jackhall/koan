//! `USING <module> SCOPE <block>` — block-scoped module opening. See
//! `design/typing/modules.md` § "Block-scoped opening".
//!
//! `m` is eager (a resolved module value), `body` is lazy
//! ([`KType::KExpression`]) so it evaluates in the opened scope.
//!
//! The block runs in a transparent scope ([`Scope::child_transparent`])
//! allocated in the **call-site region** — not a per-call frame — so forwarded
//! binds and functions defined in the block stay live after the block ends.
//! A bind colliding with a surfaced member is rejected in
//! [`Scope::bind_value`](crate::machine::Scope)'s borrowed-window arm.
//!
//! Only `data` and `functions` are surfaced; `Module::type_members` is not in
//! `Bindings`, so abstract ascriptions stay opaque inside the block.
//!
//! Functor-result escape soundness: the opened module's child scope lives in a
//! per-call region pinned only by the eager `m` dep across the USING step. The
//! body runs in later steps, so the seed folds the `m` carrier's reach onto the
//! overlay (which lives in the call-site region), keeping that region alive for
//! the window's life. An escaping closure captures the transparent overlay,
//! which anchors the call-site frame, which pins the folded region. A top-level
//! module reaches no per-call region and needs no fold.

use crate::machine::model::KType;
use crate::machine::model::TypeRegistry;
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, kw, sig};

/// USING's result is the body's tail — the block's last statement's own witnessed terminal via the
/// ordinary `DoneWitnessed` path, not a forwarded dep. Surfaced members resolve through
/// [`Scope::binding_cutoff`]'s index-0 (no-cutoff) rule for a borrowed window.
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'a, '_>) -> crate::machine::Action<'a> {
    use super::block_tail::{block_tail, BlockBody, BlockScope, BlockSeed};
    use crate::machine::model::{Held, KObject};
    use crate::machine::{arg_held, require_kexpression, Action, FramePlacement};

    let module = match arg_held(ctx.args, "m") {
        // A module reaches USING on the value channel's Object arm.
        Some(Held::Object(KObject::Module(m))) => *m,
        Some(Held::Type(other)) => {
            return Action::Done(Err(KError::new(KErrorKind::TypeMismatch {
                arg: "m".to_string(),
                expected: "Module".to_string(),
                got: other.name(ctx.types),
            })))
        }
        Some(Held::UnresolvedType(ti)) => {
            return Action::Done(Err(KError::new(KErrorKind::TypeMismatch {
                arg: "m".to_string(),
                expected: "Module".to_string(),
                got: ti.render(),
            })))
        }
        Some(Held::Object(other)) => {
            return Action::Done(Err(KError::new(KErrorKind::TypeMismatch {
                arg: "m".to_string(),
                expected: "Module".to_string(),
                got: other.ktype().name(ctx.types).to_string(),
            })))
        }
        None => return Action::Done(Err(KError::new(KErrorKind::MissingArg("m".to_string())))),
    };
    let body_expr = crate::try_action!(require_kexpression(ctx.args, "USING", "body"));
    let module_bindings = module.child_scope().bindings();
    let overlay: &'a Scope<'a> = ctx
        .scope
        .brand()
        .alloc_scope(Scope::child_transparent(ctx.scope, module_bindings));
    // Fold the eager `m` carrier's reach onto the overlay so the opened module's per-call region stays
    // alive for the window's life (see the module-level soundness note). A top-level module reaches no
    // per-call region and a carrier-less module has no reach to root, so both fold nothing.
    let seed: Option<BlockSeed<'a>> = ctx.arg_carrier("m").map(|carrier| {
        let carrier = carrier.duplicate();
        let seed: BlockSeed<'a> = Box::new(move |overlay: &Scope, _types: &TypeRegistry| {
            let _ = overlay.host_reach_of(&carrier);
        });
        seed
    });
    block_tail(
        FramePlacement::Inherit,
        BlockScope::Overlay(overlay),
        seed,
        BlockBody::Block(body_expr),
        None,
        ctx.types,
    )
}

pub fn register<'a>(scope: &'a Scope<'a>, types: &TypeRegistry) {
    let signature = sig(
        KType::Any,
        vec![
            kw("USING"),
            arg("m", KType::empty_signature()),
            kw("SCOPE"),
            arg("body", KType::KExpression),
        ],
    );
    crate::builtins::register_builtin(scope, "USING", signature, body, types);
}

#[cfg(test)]
mod tests;
