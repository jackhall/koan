//! Tests for `machine::core`, split by surface.

mod bindings_lookup;
mod derived_reach;
mod dispatch;
mod operator_registry;
mod register;
mod types;

use crate::builtins::test_support::probe_symbol;
use crate::machine::model::{KType, ReturnType, Scalar, SignatureDraft, SignatureElement};

pub(super) fn unit_signature<'a>() -> SignatureDraft<'a> {
    SignatureDraft {
        return_type: ReturnType::Resolved(KType::ANY),
        elements: vec![SignatureElement::Keyword(probe_symbol("FOO"))],
    }
}

pub(super) fn body_no_op<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'_, 'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    crate::machine::core::kfunction::action::Action::done_resident(
        ctx.scope,
        crate::machine::model::Carried::Object(ctx.scope.brand().alloc_scalar(Scalar::Null)),
    )
}
