//! `USING <module> SCOPE <block>` — block-scoped module opening. See
//! `design/typing/modules.md` § "Block-scoped opening".
//!
//! `m` is eager (a resolved module value), `body` is lazy
//! (a [`KType::KEXPRESSION`] type) so it evaluates in the opened scope.
//!
//! The block runs in an owned scope stacked inside a transparent window onto
//! the module's binding façade (`Scope::open_module_window`), both allocated in
//! the **call-site region** — not a per-call frame — so the tail's value,
//! including a closure carrying its captured block scope, stays live after the
//! block ends. Binds are block-local: they land in the block scope, which no
//! later call-site statement reaches on its ancestor walk, and a bind whose
//! name matches a surfaced member shadows the window from the next statement
//! on, on the value and type channels alike.
//!
//! Every table of the opened module's child scope is surfaced, `types`
//! included, so a module's type members name types by bare name inside the
//! block exactly as its value members name values. An opaque view's child
//! scope holds *only* the view's own type members — the per-call abstract
//! mints and the signature's manifest members, seeded at ascription — so an
//! abstract member surfaces as its `AbstractType` identity and the hidden
//! representation type is absent from the window by construction.
//!
//! Functor-result escape soundness: the opened module's child scope lives in a
//! per-call region pinned only by the eager `m` dep across the USING step. The
//! body runs in later steps, so the window door folds the `m` envelope's
//! coverage into the call-site region *before* building the window that borrows
//! into it, keeping the module's region alive for the window's life. Both
//! children are same-region with the call site, so they inherit that root. An
//! escaping closure captures the block scope, which anchors the call-site
//! frame, which pins the folded region. A module whose region the call site
//! already holds folds an empty member set — the library's self rule strips it.

use crate::machine::WriteGate;
use crate::machine::model::KType;
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, kw, sig};
use crate::machine::model::RunRegistries;

// This builtin's slot spellings, minted once and read back by symbol.
crate::slots! { SLOTS { body, m } }

/// USING's result is the body's tail — the block's last statement's own witnessed terminal via the
/// ordinary `DoneWitnessed` path, not a forwarded dep. The block runs in the owned scope
/// `open_module_window` returns; the window it is stacked in is named by no chain frame, so its
/// surfaced members take no cutoff and are visible throughout the block.
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::{Action, FramePlacement, require_kexpression};
    use crate::machine::{BlockBody, BlockScope, NoSeed, block_tail};

    let body_expr = crate::try_action!(require_kexpression(ctx.args, "USING", &SLOTS.body));
    // `m` is a value slot of a non-name-literal type, so its part is spliced before the call on
    // every shape that can carry a module — the absent arm is unreachable by construction and takes
    // a diagnostic rather than a panic.
    let Some(delivered) = ctx.args.carrier(&SLOTS.m) else {
        return Action::done(Err(KError::new(KErrorKind::ShapeError(
            "internal: USING's module argument reached the window door with no delivery envelope"
                .to_string(),
        ))));
    };
    // One door, one operand: the window's binding table is read off the module inside `delivered`
    // and the root is that same envelope's coverage (see the module-level soundness note), so the
    // block cannot run against a window whose backing region nothing holds. The door is also the
    // authority on whether the envelope carries a module at all; the arm below only renders what
    // the value channel holds for the diagnostic.
    let Some(block) = ctx.scope.open_module_window(delivered) else {
        return Action::done(Err(non_module_argument(ctx)));
    };
    block_tail(
        ctx.scope.brand(),
        FramePlacement::Inherit,
        BlockScope::Overlay(block),
        None::<NoSeed>,
        BlockBody::Block(body_expr),
        None,
        ctx.registries,
    )
}

/// Render the `m`-slot diagnostic for an argument the window door refused. Strict admission already
/// rejects most non-modules against the `:Signature` slot, so this surfaces the shapes that satisfy
/// an empty signature without being a module.
fn non_module_argument(ctx: &crate::machine::BodyCtx<'_, '_, '_>) -> KError {
    use crate::machine::model::Held;

    let got = match ctx.args.held(&SLOTS.m) {
        Some(Held::Type(other)) => other.name(ctx.registries),
        Some(Held::UnresolvedType(ti)) => {
            crate::machine::model::render_label(ti.symbol(), ctx.registries)
        }
        Some(Held::Object(other)) => other.ktype().name(ctx.registries).to_string(),
        // The `m` slot is `:Signature`, which admits no raw name part.
        Some(Held::Identifier(_)) => unreachable!("USING's `m` slot never captures an identifier"),
        None => return KError::new(KErrorKind::MissingArg("m".to_string())),
    };
    KError::new(KErrorKind::TypeMismatch {
        arg: "m".to_string(),
        expected: "Module".to_string(),
        got,
    })
}

pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    let signature = sig(
        KType::ANY,
        vec![
            kw(registries, "USING"),
            arg(registries, &SLOTS.m, KType::EMPTY_SIGNATURE),
            kw(registries, "SCOPE"),
            arg(registries, &SLOTS.body, KType::KEXPRESSION),
        ],
    );
    crate::builtins::register_builtin(scope, signature, body, registries, gate);
}

#[cfg(test)]
mod tests;
