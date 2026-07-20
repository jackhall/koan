//! Tests for `machine::core`, split by surface.

mod bindings_lookup;
mod dispatch;
mod operator_registry;
mod queue;
mod register;
mod types;

use crate::machine::model::KObject;
use crate::machine::model::{ExpressionSignature, KType, ReturnType, SignatureElement};

pub(super) fn unit_signature<'a>() -> ExpressionSignature<'a> {
    ExpressionSignature {
        return_type: ReturnType::Resolved(KType::ANY),
        elements: vec![SignatureElement::Keyword("FOO".into())],
    }
}

pub(super) fn body_no_op<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    crate::machine::core::kfunction::action::Action::done_resident(
        crate::machine::model::Carried::Object(ctx.scope.brand().alloc_object(KObject::Null)),
    )
}
