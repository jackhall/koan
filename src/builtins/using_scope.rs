//! `USING <module> SCOPE <block>` — block-scoped module opening.
//!
//! Evaluates `<block>` with `<module>`'s members (`data` + `functions`) in scope as bare
//! names, and returns the value of the last expression. `<module>` is any module-valued
//! expression — a named module, a chained `Outer.Inner`, or a functor result.
//!
//! Surface shape mirrors [`try_with`](super::try_with): `m` is eager (arrives as a
//! resolved [`KObject::KModule`]), `body` is lazy ([`KType::KExpression`]) so it runs in
//! the opened scope rather than at the call site.
//!
//! The block runs in a single *transparent* scope ([`Scope::child_transparent`]) that is
//! allocated in the **call-site arena** (not a throwaway per-call frame). Its `outer` is
//! the call site and its bindings are a read-only window onto the module's child-scope
//! façade. Reads walk the window first, then the call-site chain — so module names win in
//! the block — which needs no change to the resolver. Binds made inside the block forward
//! to the call site and persist after the block; a bind colliding with a surfaced member
//! is rejected in [`Scope::bind_value`](crate::machine::core::Scope)'s borrowed-window
//! arm.
//!
//! Allocating the transparent scope in the call-site arena (rather than a per-call
//! `CallArena` that drops at block end) is what makes forwarding *sound*: a forwarded
//! bind — or a function defined in the block and forward-registered into the call site —
//! references values and a captured scope that all live in the call-site arena, so
//! nothing dangles when the block ends. The block is dispatched as a sub-node and the
//! USING node lifts its result (`BodyResult::DeferTo`), same as MODULE/SIG.
//!
//! Only `data` and `functions` are surfaced (the whole `Bindings` façade is borrowed). A
//! module's abstract type ascriptions live in `Module::type_members`, *not* in
//! `Bindings`, so they are naturally not exposed — opacity is preserved inside the block.
//!
//! Escape soundness for a functor-result module: the opened module's child scope lives in
//! a per-call `CallArena` kept alive by an `Rc` carried on the eager `m` value. That `Rc`
//! would drop when the `m` arg drops at body return, so we root a copy of the module
//! value in the call-site arena — the arena then owns the `Rc` for its lifetime. An
//! escaping closure that reads the window captures the transparent scope (call-site
//! arena), so on escape it anchors the call-site frame, which keeps the call-site arena —
//! and thus the rooted module `Rc` — alive. Top-level modules carry no `Rc` and need no
//! rooting.

use crate::machine::model::{KObject, KType};
use crate::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, Scope, SchedulerHandle};

use crate::machine::core::kfunction::argument_bundle::extract_kexpression;
use super::{arg, err, kw, register_builtin, sig};

pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    // `m` is eager — a resolved module value, carrying a per-call frame anchor when it is
    // a functor result living in a `CallArena` (top-level modules carry `None`).
    // Post-collapse: module values ride `KTypeValue(KType::Module { module, frame })`.
    let (module, module_frame) = match bundle.get("m") {
        Some(KObject::KTypeValue(KType::Module { module: m, frame: anchor })) => {
            (*m, anchor.clone())
        }
        Some(other) => {
            return err(KError::new(KErrorKind::TypeMismatch {
                arg: "m".to_string(),
                expected: "Module".to_string(),
                got: other.ktype().name().to_string(),
            }));
        }
        None => return err(KError::new(KErrorKind::MissingArg("m".to_string()))),
    };
    let body_expr = match extract_kexpression(&mut bundle, "body") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "USING body slot must be a parenthesized expression".to_string(),
            )));
        }
    };

    // Root a functor-result module's frame `Rc` in the call-site arena so the borrowed
    // window outlives the eager `m` arg and any escaping closure. No-op for top-level
    // modules (`module_frame` is `None`). Post-collapse the carrier is
    // `KTypeValue(KType::Module { .. })`.
    if module_frame.is_some() {
        scope.arena.alloc(KObject::KTypeValue(KType::Module {
            module,
            frame: module_frame,
        }));
    }

    // Transparent window onto the module's whole façade, allocated in the call-site arena
    // so block-body allocations and forwarded binds all live as long as the call site.
    let module_bindings = module.child_scope().bindings();
    let child: &'a Scope<'a> =
        scope.arena.alloc_scope(Scope::child_transparent(scope, module_bindings));
    let sub_id = sched.add_dispatch(body_expr, child);
    BodyResult::DeferTo(sub_id)
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "USING",
        sig(KType::Any, vec![
            kw("USING"),
            arg("m", KType::AnyModule),
            kw("SCOPE"),
            arg("body", KType::KExpression),
        ]),
        body,
    );
}

#[cfg(test)]
mod tests;
