//! `CATCH <expr>` — lift a single interpreter fault into a `Result` value.
//! Shares the `add_catch` primitive with [`TRY-WITH`](super::try_with) but
//! lacks branches, an `it` binding, and the re-raise path: the finish closure
//! wraps the outcome in the prelude [`Result`](super::result) carrier as
//! either `Ok(v)` or `Error(KError::to_tagged())`.

use std::rc::Rc;

use crate::machine::core::{kerror_ktype, KoanRegionExt};
use crate::machine::execute::StepCarried;
use crate::machine::model::{KObject, KType};
use crate::machine::Scope;

use super::{arg, kw, sig};

pub fn register<'a>(scope: &'a Scope<'a>) {
    // CATCH yields `Result {Ok :Any, Error :KError}` — `Any` covers only the unpredictable
    // `Ok` payload, the `Error` arm is the `KError` carrier. `result::register` runs first, so
    // the `Result` `SetRef` resolves here. This is a documentary contract: the catch finish
    // produces an `Outcome::Done(Value)` (never a `ReturnContract`), so the declared return is not
    // validated against the runtime value, and the throwaway `kerror_ktype()` identity is fine.
    let result_ctor = match scope.resolve_type("Result") {
        Some(kt @ KType::SetRef { .. }) => kt.clone(),
        _ => panic!("Result must be registered before CATCH"),
    };
    let return_type = KType::ConstructorApply {
        ctor: Box::new(result_ctor),
        args: vec![KType::Any, kerror_ktype()],
    };
    let signature = sig(
        return_type,
        vec![kw("CATCH"), arg("expr", KType::KExpression)],
    );
    crate::builtins::register_builtin(scope, "CATCH", signature, body);
}

/// Watches the captured `expr` and recovers into a `Result` carrier
/// (`Ok(v)` / `Error(KError::to_tagged())`) via a `Catch` finish.
pub fn body<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::{
        require_kexpression, Action, CatchContinue, DepPlacement, DepRequest,
    };
    use crate::machine::core::FoldingBrand;
    use crate::machine::execute::build_type_operand;
    use crate::machine::model::values::CarriedFamily;
    use crate::machine::model::Carried;
    use crate::machine::{KoanRegion, RegionTypeFamily};
    use crate::witnessed::Residence;
    let expr_inner = crate::try_action!(require_kexpression(ctx.args, "CATCH", "expr"));
    // Capture the prelude `Result` member identity at body time so the CATCH value shares the
    // nominal identity of a `Result (...)`-constructed one.
    let (result_set, result_index) = match ctx.scope.resolve_type("Result") {
        Some(KType::SetRef { set, index }) => (Rc::clone(set), *index),
        _ => panic!("Result must be registered before CATCH"),
    };
    // The `Result` identity's stored per-binding type token, captured at body time so the
    // construction operand's witness names its own region. Empty while `RecursiveSet` is heap-`Rc`'d.
    let reach = ctx
        .scope
        .resolve_type_stored("Result", None)
        .unwrap_or_default();
    let finish: CatchContinue<'a> = Box::new(move |fctx, result| {
        // Wrap `payload` as a `Result` `Tagged` at the build brand `'x`. A free fn (no captured
        // lifetime) so both the `Ok` `transfer_into` and the `Err` `merge` brand closures can call it.
        fn build_result<'x>(tag: &str, identity: &KType<'x>, payload: KObject<'x>) -> KObject<'x> {
            let (set, index) = match identity {
                KType::SetRef { set, index } => (Rc::clone(set), *index),
                _ => unreachable!("the prelude Result identity is always a SetRef"),
            };
            KObject::Tagged {
                tag: tag.to_string(),
                value: Rc::new(payload),
                set,
                index,
                type_args: Rc::new(vec![]),
            }
        }
        // Build the `Result` `Tagged` **inside the witness closure** so it names every region the
        // wrapped value reaches. The `Result` `SetRef` identity — freshly minted in the scope region —
        // crosses the build brand as a [`RegionTypeFamily`] operand, `merge`d in under the scope's yoke
        // plus its stored reach rather than paired with an asserted singleton.
        let region = fctx.scope.brand();
        let frame = fctx.ctx.frame();
        let identity: &KType<'a> = crate::try_action!(region.alloc_ktype_checked(KType::SetRef {
            set: Rc::clone(&result_set),
            index: result_index,
        }));
        let home = build_type_operand(fctx.scope, Rc::clone(&frame), identity, reach);
        let witnessed = match result {
            // The watched carrier folds onto the result: `transfer_into` relocates the value into the
            // consumer region and unions its reach onto the `Ok` carrier.
            Ok(carrier) => carrier.transfer_into::<RegionTypeFamily, CarriedFamily, _>(
                home,
                Residence::Copied,
                |value, (region, identity), token| {
                    let region = FoldingBrand::in_fold_closure(region, token);
                    Carried::Object(region.alloc_object_folded(build_result(
                        "Ok",
                        identity,
                        value.object().deep_clone(),
                    )))
                },
            ),
            // The error payload is built region-pure into the scope region (it reaches no foreign
            // region); `yoke` it, then `merge` the identity operand to wrap it as `Result::Error`.
            Err(e) => {
                let payload = KoanRegion::alloc_witnessed(Rc::clone(&frame), |region| {
                    Carried::Object(
                        region
                            .alloc_object_checked(e.to_tagged(region))
                            .expect("a freshly-built KError payload is always resident-in-self"),
                    )
                });
                // The pinned merge: `frame` covers the freshly-built payload (it lives in that
                // frame's own region); the identity operand's backing is the live scope.
                payload.merge_pinned::<RegionTypeFamily, CarriedFamily, _>(
                    home,
                    &frame,
                    |payload, (region, identity), token| {
                        let region = FoldingBrand::in_fold_closure(region, token);
                        Carried::Object(region.alloc_object_folded(build_result(
                            "Error",
                            identity,
                            payload.object().deep_clone(),
                        )))
                    },
                )
            }
        };
        Action::Done(Ok(StepCarried::born(witnessed)))
    });
    Action::Catch {
        watched: DepRequest::Dispatch {
            expr: expr_inner,
            placement: DepPlacement::OwnScope,
        },
        finish,
    }
}

#[cfg(test)]
mod tests;
