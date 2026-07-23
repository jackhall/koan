//! `CATCH <expr>` — lift a single interpreter fault into a `Result` value.
//! Shares the `add_catch` primitive with [`TRY-WITH`](super::try_with) but
//! lacks branches, an `it` binding, and the re-raise path: the finish closure
//! wraps the outcome in the prelude [`Result`](super::result) carrier as
//! either `Ok(v)` or `Error(KError::to_tagged())`.

use crate::machine::model::TypeRegistry;
use std::rc::Rc;

use crate::machine::model::{KObject, KType, Record};
use crate::machine::Scope;
use crate::machine::StepCarried;
use crate::machine::{force_substrate_borrows_host, kerror_ktype};

use super::{arg, kw, sig};

pub fn register<'a>(scope: &'a Scope<'a>, types: &TypeRegistry) {
    // CATCH yields `Result {Ok :Any, Error :KError}` — `Any` covers only the unpredictable
    // `Ok` payload, the `Error` arm is the `KError` carrier. `result::register` runs first, so
    // the `Result` member resolves here. This is a documentary contract: the catch finish
    // produces an `Outcome::Done(Value)` (never a `ReturnContract`), so the declared return is not
    // validated against the runtime value, and the throwaway `kerror_ktype()` identity is fine.
    let result_ctor = match scope.resolve_type("Result") {
        Some(member) => member,
        None => panic!("Result must be registered before CATCH"),
    };
    let return_type = types.constructor_apply(
        result_ctor,
        Record::from_pairs([
            ("Ok".to_string(), KType::ANY),
            ("Error".to_string(), kerror_ktype(types)),
        ]),
    );
    let signature = sig(
        return_type,
        vec![kw("CATCH"), arg("expr", KType::KEXPRESSION)],
    );
    crate::builtins::register_builtin(scope, "CATCH", signature, body, types);
}

/// Watches the captured `expr` and recovers into a `Result` carrier
/// (`Ok(v)` / `Error(KError::to_tagged())`) via a `Catch` finish.
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::build_type_operand;
    use crate::machine::model::Carried;
    use crate::machine::model::CarriedFamily;
    use crate::machine::FoldingBrand;
    use crate::machine::RegionTypeFamily;
    use crate::machine::{require_kexpression, Action, CatchContinue, DepPlacement, DepRequest};
    use crate::witnessed::Residence;
    let expr_inner = crate::try_action!(require_kexpression(ctx.args, "CATCH", "expr"));
    // Capture the prelude `Result` member identity at body time so the CATCH value shares the
    // nominal identity of a `Result (...)`-constructed one.
    let result_member: KType = match ctx.scope.resolve_type("Result") {
        Some(member) => member,
        None => panic!("Result must be registered before CATCH"),
    };
    let finish: CatchContinue<'a> = Box::new(move |fctx, result| {
        // Wrap `payload` as a `Result` `Tagged` at the build brand `'x`, allocating the payload
        // substrate through the fold `door`. A free fn (no captured lifetime) so both branches'
        // `transfer_into_placing` brand closures can call it.
        fn build_result<'x>(
            door: FoldingBrand<'x>,
            tag: &str,
            identity: KType,
            payload: &KObject<'x>,
        ) -> KObject<'x> {
            KObject::tagged(door, tag.to_string(), payload, identity)
        }
        // Build the `Result` `Tagged` **inside the witness closure** so it names every region the
        // wrapped value reaches. The `Result` member handle crosses the build brand as a
        // [`RegionTypeFamily`] operand, `merge`d in under the scope's yoke rather than paired with
        // an asserted singleton; the handle itself borrows no region.
        let frame = fctx.ctx.frame();
        let home = build_type_operand(fctx.scope, Rc::clone(&frame), result_member);
        // Both arms fold a delivery envelope into `home` at `Residence::Copied` — the watched
        // carrier for `Ok`, `to_tagged`'s freshly-born envelope (its record substrate can only be
        // built through a fold door, so it is sealed as a delivered carrier rather than routed
        // through the checked/audited move-in tier) for `Err` — so the two arms share one shape.
        let tagged_envelope;
        let carrier = match &result {
            Ok(carrier) => carrier,
            Err(e) => {
                tagged_envelope = e.to_tagged_delivered(fctx.scope, fctx.types);
                &tagged_envelope
            }
        };
        let tag = if result.is_ok() { "Ok" } else { "Error" };
        let witnessed = carrier.transfer_into_placing::<RegionTypeFamily, CarriedFamily, _>(
            home,
            Residence::Copied,
            |value, (_region, identity), placement| {
                let region = FoldingBrand::in_fold_closure(placement);
                Carried::Object(region.alloc_object_folded(build_result(
                    region,
                    tag,
                    identity,
                    value.object(),
                )))
            },
        );
        // Step-terminal seal: either arm's payload may be (or embed) a fresh record — force the
        // bit rather than trust the fold's operand-only compose.
        let witnessed = force_substrate_borrows_host(witnessed, &frame);
        Action::Done(Ok(StepCarried::born(witnessed)))
    });
    Action::Catch {
        watched: DepRequest::Dispatch {
            expr: expr_inner,
            placement: DepPlacement::OwnScope,
            binder_covered: false,
        },
        finish,
    }
}

#[cfg(test)]
mod tests;
