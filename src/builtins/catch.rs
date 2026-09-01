//! `CATCH <expr>` — lift a single interpreter fault into a `Result` value.
//! Shares the `add_catch` primitive with [`TRY-WITH`](super::try_with) but
//! lacks branches, an `it` binding, and the re-raise path: the finish closure
//! wraps the outcome in the prelude [`Result`](super::result) union as either
//! `Result.Ok v` or `Result.Error <lowered KError>`.

use crate::machine::WriteGate;
use std::rc::Rc;

use crate::machine::Scope;
use crate::machine::StepCarried;
use crate::machine::model::{KObject, KType};

use super::{arg, kw, sig};
use crate::machine::model::RunRegistries;

// This builtin's slot spellings, minted once and read back by symbol.
crate::slots! { SLOTS { expr } }

/// The `Result` union's two members, read off the registered prelude type. `result::register` and
/// `error_union::register` both run before CATCH, so both lookups land.
fn result_members(scope: &Scope<'_>, registries: &RunRegistries) -> (KType, KType) {
    let union = scope
        .resolve_type(crate::builtins::result::RESULT.symbol())
        .expect("Result must be registered before CATCH");
    let member = |name: crate::machine::model::TypeSymbol| {
        registries
            .types
            .union_member_named(union, name.symbol())
            .expect("Result declares both of its members")
    };
    (
        member(super::result::OK.symbol()),
        member(super::result::ERROR.symbol()),
    )
}

pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    // CATCH yields `Result {Ok = Any, Error = KError}` — `Any` covers the unpredictable `Ok`
    // payload, and the `Error` arm is the registered `KError` union. The application lowers per
    // member, so the declared return admits an `Ok` of anything and an `Error` of any lowered
    // kind — the shape the finish below actually builds.
    let result = scope
        .resolve_type(crate::builtins::result::RESULT.symbol())
        .expect("Result must be registered before CATCH");
    let kerror = scope
        .resolve_type(crate::machine::core::kerror::KERROR.symbol())
        .expect("KError must be registered before CATCH");
    let return_type = crate::builtins::union::apply_union_type_args(
        result,
        &[
            (
                registries.labels.record(&super::result::OK).symbol(),
                KType::ANY,
            ),
            (
                registries.labels.record(&super::result::ERROR).symbol(),
                kerror,
            ),
        ],
        registries,
    )
    .expect("`Ok` and `Error` are the members `Result` declares");
    let signature = sig(
        return_type,
        vec![
            kw(registries, "CATCH"),
            arg(registries, &SLOTS.expr, KType::KEXPRESSION),
        ],
    );
    crate::builtins::register_builtin(scope, signature, body, registries, gate);
}

/// Watches the captured `expr` and recovers into a `Result` carrier
/// (`Result.Ok v` / `Result.Error <lowered KError>`) via a `Catch` finish.
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::FoldingBrand;
    use crate::machine::RegionTypeFamily;
    use crate::machine::build_type_operand;
    use crate::machine::core::SubstrateDoor;
    use crate::machine::model::Carried;
    use crate::machine::model::CarriedFamily;
    use crate::machine::{Action, DepPlacement, DepRequest, require_kexpression};
    let expr_inner = crate::try_action!(require_kexpression(ctx.args, "CATCH", &SLOTS.expr));
    // Capture the prelude `Result` members at body time so the CATCH value carries the same
    // nominal identity a `Result.Ok` / `Result.Error` projection constructs under.
    let (ok_member, error_member) = result_members(ctx.scope, ctx.registries);
    // Both captures are `Copy` interned identities, so the finish erases onto the bumped tier: a
    // CATCH costs no heap allocation for its recovery.
    let finish =
        move |fctx: &crate::machine::FinishCtx<'a, '_>,
              result: Result<crate::machine::DeliveredCarried, crate::machine::KError>| {
            // Wrap `payload` under the selected `Result` member at the build brand `'x`,
            // allocating the payload substrate through the fold `door`. `wrapped_hold`, never
            // `wrapped_peel`: an `Error`'s payload is itself a `Wrapped` (the kind member over its
            // field record) and the two layers must nest, so a caught error renders as
            // `Error(UnboundName({…}))`. A free fn (no captured lifetime) so both branches'
            // `transfer_into` brand closures can call it.
            fn build_result<'x>(
                door: SubstrateDoor<'x, '_>,
                identity: KType,
                payload: &KObject<'x>,
            ) -> KObject<'x> {
                KObject::wrapped_hold(door, payload, identity)
            }
            // Build the `Result` `Wrapped` **inside the witness closure** so it names every
            // region the wrapped value reaches. The member handle crosses the build
            // brand as a
            // [`RegionTypeFamily`] operand, yoked into the dest region rather than paired
            // with an asserted singleton; the handle itself borrows no region.
            // Both arms fold a delivery envelope into `home` claiming the envelope's own pins
            // — the watched carrier for `Ok`, `to_wrapped`'s freshly-born envelope (its record
            // substrate can only be built through a fold door, so it is sealed as a delivered
            // carrier rather than routed through the born-door move-in) for `Err` — so the two
            // arms share one shape.
            let lowered_envelope;
            let carrier = match &result {
                Ok(carrier) => carrier,
                Err(e) => {
                    lowered_envelope = e.to_wrapped_delivered(fctx.scope, fctx.registries);
                    &lowered_envelope
                }
            };
            let member = if result.is_ok() {
                ok_member
            } else {
                error_member
            };
            let frame = fctx.ctx.frame();
            let home = build_type_operand(Rc::clone(&frame), member);
            // The payload rides into the `Wrapped` verbatim, so a payload substrate that stays
            // foreign keeps its own stored reach as the payload cell's run; the carrier's
            // coverage is the holder-rule proof for reading it, captured before the fold closure.
            let holder = carrier.coverage().clone();
            // The type operand is empty-reach; the transfer composes the result payload's reach and
            // homes the product in the operand's dest frame.
            let product = carrier.transfer_into::<RegionTypeFamily, CarriedFamily, _>(
                home,
                // The built `Ok`/`Error` wrap holds the payload's borrow verbatim, so the
                // predicate releases nothing: every region the payload reaches rides on.
                |_product, _region| true,
                |value, (_region, identity), placement| {
                    let region = FoldingBrand::in_fold_closure(placement);
                    let door = region.with_holder(&holder);
                    Carried::Object(region.alloc_object_folded(build_result(
                        door,
                        identity,
                        value.object(),
                    )))
                },
            );
            // Step-terminal seal: the transfer minted the product's description into the dest
            // frame's own region, so that region is the product's host and enters its members
            // whenever the payload borrows there — a fresh record does. The product's residence
            // *is* `frame`, which the step's own seal re-pins, so only its foreign coverage
            // rides on.
            Action::done(Ok(StepCarried::born_delivered(product)))
        };
    Action::catch(
        DepRequest::Dispatch {
            expr: crate::machine::model::WorkingExpression::from_ast(ctx.scope.brand(), expr_inner),
            placement: DepPlacement::OwnScope,
        },
        ctx.brand(),
        finish,
    )
}

#[cfg(test)]
mod tests;
