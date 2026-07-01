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
//! [`Scope::bind_value`](crate::machine::core::Scope)'s borrowed-window arm.
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

use crate::machine::model::types::KKind;
use crate::machine::model::KType;
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, kw, sig};

/// Mint the transparent overlay and tail-replace into the body run inside it through the shared
/// [`block_tail`](super::block_tail::block_tail) constructor: `Inherit` (the overlay's binds persist
/// in the call-site cart), the overlay as its block, a module-carrier seed, and the body split into
/// leading statements + a tail. USING's result is that tail — the block's last statement's own
/// witnessed terminal, finalized through the ordinary `DoneWitnessed` path, not a forwarded dep. The
/// window's surfaced members resolve through [`Scope::binding_cutoff`]'s index-0 (no-cutoff) rule for
/// a borrowed window.
pub fn body<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use super::block_tail::{block_tail, BlockBody, BlockScope, BlockSeed};
    use crate::machine::core::kfunction::action::{
        arg_held, require_kexpression, Action, FramePlacement,
    };
    use crate::machine::model::values::Held;

    let module = match arg_held(ctx.args, "m") {
        Some(Held::Type(KType::Module { module: m })) => *m,
        Some(Held::Type(other)) => {
            return Action::Done(Err(KError::new(KErrorKind::TypeMismatch {
                arg: "m".to_string(),
                expected: "Module".to_string(),
                got: other.name(),
            })))
        }
        Some(Held::Object(other)) => {
            return Action::Done(Err(KError::new(KErrorKind::TypeMismatch {
                arg: "m".to_string(),
                expected: "Module".to_string(),
                got: other.ktype().name().to_string(),
            })))
        }
        None => return Action::Done(Err(KError::new(KErrorKind::MissingArg("m".to_string())))),
    };
    let body_expr = crate::try_action!(require_kexpression(ctx.args, "USING", "body"));
    // Transparent overlay in the call-site region so forwarded binds and block-defined functions
    // outlive the block. Under `Inherit` the USING node runs in it while keeping the call-site cart.
    let module_bindings = module.child_scope().bindings();
    let overlay: &'a Scope<'a> = ctx
        .scope
        .brand()
        .alloc_scope(Scope::child_transparent(ctx.scope, module_bindings));
    // Seed: root the opened module's reach on the overlay. Its child-scope region (a functor result's
    // per-call frame) is pinned only by the eager `m` dep across this step, but the overlay borrows
    // that region and outlives the step — and any closure escaping the block captures the overlay.
    // Folding the `m` carrier's reach onto the overlay (which lives in the same region) keeps the
    // region alive for the window's life. A top-level module reaches no per-call region, so
    // `fold_reach`'s home/ancestor omission folds nothing; a module with no delivered carrier (no
    // reach to root) needs no seed.
    let seed: Option<BlockSeed<'a>> = ctx.arg_carrier("m").map(|carrier| {
        let witness = carrier.witness().clone();
        let seed: BlockSeed<'a> = Box::new(move |overlay: &Scope| overlay.fold_reach(&witness));
        seed
    });
    block_tail(
        FramePlacement::Inherit,
        BlockScope::Overlay(overlay),
        seed,
        BlockBody::Block(body_expr),
        None,
    )
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    let signature = sig(
        KType::Any,
        vec![
            kw("USING"),
            arg("m", KType::OfKind(KKind::Module)),
            kw("SCOPE"),
            arg("body", KType::KExpression),
        ],
    );
    crate::builtins::register_builtin(scope, "USING", signature, body);
}

#[cfg(test)]
mod tests;
