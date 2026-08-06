//! `USING <module> SCOPE <block>` — block-scoped module opening. See
//! `design/typing/modules.md` § "Block-scoped opening".
//!
//! `m` is eager (a resolved module value), `body` is lazy
//! (a [`KType::KEXPRESSION`] type) so it evaluates in the opened scope.
//!
//! The block runs in a transparent scope (`Scope::open_module_window`)
//! allocated in the **call-site region** — not a per-call frame — so forwarded
//! binds and functions defined in the block stay live after the block ends.
//! A bind colliding with a surfaced member is rejected in the write op's
//! transparent-window arm, on the value and type channels alike.
//!
//! All four tables of the opened module's child scope are surfaced, `types`
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
//! into it, keeping the module's region alive for the window's life. A
//! transparent child is same-region with its parent, so the window inherits that
//! root. An escaping closure captures the window, which anchors the call-site
//! frame, which pins the folded region. A module whose region the call site
//! already holds folds an empty member set — the library's self rule strips it.

use crate::machine::model::KType;
use crate::machine::model::TypeRegistry;
use crate::machine::WriteGate;
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, kw, sig};

/// USING's result is the body's tail — the block's last statement's own witnessed terminal via the
/// ordinary `DoneWitnessed` path, not a forwarded dep. Surfaced members resolve through
/// [`Scope::binding_cutoff`]'s index-0 (no-cutoff) rule for a borrowed window.
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::{block_tail, BlockBody, BlockScope};
    use crate::machine::{require_kexpression, Action, FramePlacement};

    let body_expr = crate::try_action!(require_kexpression(ctx.args, "USING", "body"));
    // `m` is a value slot of a non-name-literal type, so its part is spliced before the call on
    // every shape that can carry a module — the absent arm is unreachable by construction and takes
    // a diagnostic rather than a panic.
    let Some(delivered) = ctx.arg_carrier("m") else {
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
    let Some(overlay) = ctx.scope.open_module_window(delivered) else {
        return Action::done(Err(non_module_argument(ctx)));
    };
    block_tail(
        ctx.scope.brand(),
        FramePlacement::Inherit,
        BlockScope::Overlay(overlay),
        None,
        BlockBody::Block(body_expr),
        None,
        ctx.types,
    )
}

/// Render the `m`-slot diagnostic for an argument the window door refused. Strict admission already
/// rejects most non-modules against the `:Signature` slot, so this surfaces the shapes that satisfy
/// an empty signature without being a module.
fn non_module_argument(ctx: &crate::machine::BodyCtx<'_, '_>) -> KError {
    use crate::machine::arg_held;
    use crate::machine::model::Held;

    let got = match arg_held(ctx.args, "m") {
        Some(Held::Type(other)) => other.name(ctx.types),
        Some(Held::UnresolvedType(ti)) => ti.render(),
        Some(Held::Object(other)) => other.ktype().name(ctx.types).to_string(),
        None => return KError::new(KErrorKind::MissingArg("m".to_string())),
    };
    KError::new(KErrorKind::TypeMismatch {
        arg: "m".to_string(),
        expected: "Module".to_string(),
        got,
    })
}

pub fn register<'a>(scope: &'a Scope<'a>, types: &TypeRegistry, gate: &mut WriteGate) {
    let signature = sig(
        KType::ANY,
        vec![
            kw("USING"),
            arg("m", KType::EMPTY_SIGNATURE),
            kw("SCOPE"),
            arg("body", KType::KEXPRESSION),
        ],
    );
    crate::builtins::register_builtin(scope, "USING", signature, body, types, gate);
}

#[cfg(test)]
mod tests;
