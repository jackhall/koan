//! Tests for `machine::core`, split by surface.

mod bindings_lookup;
mod dispatch;
mod operator_registry;
mod queue;
mod register;
mod types;

use super::Scope;
use crate::machine::model::types::{ExpressionSignature, KType, ReturnType, SignatureElement};
use crate::machine::model::values::KObject;

pub(super) fn unit_signature<'a>() -> ExpressionSignature<'a> {
    ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![SignatureElement::Keyword("FOO".into())],
    }
}

pub(super) fn body_no_op<'a, 's>(
    _scope: &'s Scope<'a>,
    _sched: &mut dyn crate::machine::core::kfunction::SchedulerHandle<'a, 's>,
    _bundle: crate::machine::core::kfunction::ArgumentBundle<'a>,
) -> crate::machine::core::kfunction::BodyResult<'a> {
    crate::machine::core::kfunction::BodyResult::value(_scope.arena.alloc_object(KObject::Null))
}
