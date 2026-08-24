//! `CATCH <expr>` — lift a single interpreter fault into a `Result` value.
//! Shares the `add_catch` primitive with [`TRY-WITH`](super::try_with) but
//! lacks branches, an `it` binding, and the re-raise path: the finish closure
//! wraps the outcome in the prelude [`Result`](super::result) carrier as
//! either `Ok(v)` or `Error(KError::to_tagged())`.

use crate::machine::WriteGate;
use std::rc::Rc;

use crate::machine::Scope;
use crate::machine::StepCarried;
use crate::machine::kerror_ktype;
use crate::machine::model::{KObject, KType, Record, TypeSymbol};

use super::{arg, kw, sig};
use crate::machine::model::RunRegistries;

// This builtin's slot spellings, minted once and read back by symbol.
crate::slots! { SLOTS { expr } }

pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    let types = &registries.types;
    // CATCH yields `Result {Ok :Any, Error :KError}` — `Any` covers only the unpredictable
    // `Ok` payload, the `Error` arm is the `KError` carrier. `result::register` runs first, so
    // the `Result` member resolves here. This is a documentary contract: the catch finish
    // produces an `Outcome::Done(Value)` (never a `ReturnContract`), so the declared return is not
    // validated against the runtime value, and the throwaway `kerror_ktype()` identity is fine.
    let result_ctor = match scope.resolve_type(crate::builtins::result::RESULT.symbol()) {
        Some(member) => member,
        None => panic!("Result must be registered before CATCH"),
    };
    let return_type = types.constructor_apply(
        result_ctor,
        Record::from_pairs([
            (
                registries.labels.record(&super::result::OK).symbol(),
                KType::ANY,
            ),
            (
                registries.labels.record(&super::result::ERROR).symbol(),
                kerror_ktype(registries),
            ),
        ]),
    );
    let signature = sig(
        return_type,
        vec![
            kw(registries, "CATCH"),
            arg(registries, &SLOTS.expr, KType::KEXPRESSION),
        ],
    );
    crate::builtins::register_builtin(scope, "CATCH", signature, body, registries, gate);
}

/// Watches the captured `expr` and recovers into a `Result` carrier
/// (`Ok(v)` / `Error(KError::to_tagged())`) via a `Catch` finish.
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::FoldingBrand;
    use crate::machine::RegionTypeFamily;
    use crate::machine::build_type_operand;
    use crate::machine::core::SubstrateDoor;
    use crate::machine::model::Carried;
    use crate::machine::model::CarriedFamily;
    use crate::machine::{Action, CatchContinue, DepPlacement, DepRequest, require_kexpression};
    let expr_inner = crate::try_action!(require_kexpression(ctx.args, "CATCH", &SLOTS.expr));
    // Capture the prelude `Result` member identity at body time so the CATCH value shares the
    // nominal identity of a `Result (...)`-constructed one.
    let result_member: KType = match ctx
        .scope
        .resolve_type(crate::builtins::result::RESULT.symbol())
    {
        Some(member) => member,
        None => panic!("Result must be registered before CATCH"),
    };
    let finish: CatchContinue<'a> = Box::new(move |fctx, result| {
        // Wrap `payload` as a `Result` `Tagged` at the build brand `'x`, allocating the payload
        // substrate through the fold `door`. A free fn (no captured lifetime) so both branches'
        // `transfer_into` brand closures can call it.
        fn build_result<'x>(
            door: SubstrateDoor<'x, '_>,
            tag: TypeSymbol,
            identity: KType,
            payload: &KObject<'x>,
        ) -> KObject<'x> {
            KObject::tagged(door, tag, payload, identity)
        }
        // Build the `Result` `Tagged` **inside the witness closure** so it names every region the
        // wrapped value reaches. The `Result` member handle crosses the build brand as a
        // [`RegionTypeFamily`] operand, yoked into the dest region rather than paired with an
        // asserted singleton; the handle itself borrows no region.
        let frame = fctx.ctx.frame();
        let home = build_type_operand(Rc::clone(&frame), result_member);
        // Both arms fold a delivery envelope into `home` claiming the envelope's own pins — the watched
        // carrier for `Ok`, `to_tagged`'s freshly-born envelope (its record substrate can only be
        // built through a fold door, so it is sealed as a delivered carrier rather than routed
        // through the born-door move-in) for `Err` — so the two arms share one shape.
        let tagged_envelope;
        let carrier = match &result {
            Ok(carrier) => carrier,
            Err(e) => {
                tagged_envelope = e.to_tagged_delivered(fctx.scope, fctx.registries);
                &tagged_envelope
            }
        };
        // The tags are read off `result`'s own statics, whose spellings the prelude registration
        // already recorded in this run's interner, so a rendered tag resolves back.
        let tag = if result.is_ok() {
            super::result::OK.symbol()
        } else {
            super::result::ERROR.symbol()
        };
        // The payload rides into the `Tagged` verbatim, so a payload substrate that stays foreign
        // keeps its own stored reach as the payload cell's run; the carrier's coverage is the
        // holder-rule proof for reading it, captured before the fold closure.
        let holder = carrier.coverage().clone();
        // The type operand is empty-reach; the transfer composes the result payload's reach and
        // homes the product in the operand's dest frame.
        let product = carrier.transfer_into::<RegionTypeFamily, CarriedFamily, _>(
            home,
            // The built `Ok`/`Error` record holds the payload's borrow verbatim, so the
            // predicate releases nothing: every region the payload reaches rides on.
            |_product, _region| true,
            |value, (_region, identity), placement| {
                let region = FoldingBrand::in_fold_closure(placement);
                let door = region.with_holder(&holder);
                Carried::Object(region.alloc_object_folded(build_result(
                    door,
                    tag,
                    identity,
                    value.object(),
                )))
            },
        );
        // Step-terminal seal: the transfer minted the product's description into the dest frame's
        // own region, so that region is the product's host and enters its members whenever the
        // payload borrows there — a fresh record does. The product's residence *is* `frame`, which
        // the step's own seal re-pins, so only its foreign coverage rides on.
        Action::done(Ok(StepCarried::born_delivered(product)))
    });
    Action::catch(
        DepRequest::Dispatch {
            expr: crate::machine::model::WorkingExpression::from_ast(ctx.scope.brand(), expr_inner),
            placement: DepPlacement::OwnScope,
        },
        finish,
    )
}

#[cfg(test)]
mod tests;
