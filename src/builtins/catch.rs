//! `CATCH <expr>` — lift a single interpreter fault into a `Result` value.
//! Shares the `add_catch` primitive with [`TRY-WITH`](super::try_with) but
//! lacks branches, an `it` binding, and the re-raise path: the finish closure
//! wraps the outcome in the prelude [`Result`](super::result) carrier as
//! either `Ok(v)` or `Error(KError::to_tagged())`.

use std::rc::Rc;

use crate::machine::core::kerror_ktype;
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
        require_kexpression, Action, CatchContinue, Dep, DepPlacement,
    };
    use crate::machine::model::Carried;
    let expr_inner = crate::try_action!(require_kexpression(ctx.args, "CATCH", "expr"));
    // Capture the prelude `Result` member identity at body time so the CATCH value shares the
    // nominal identity of a `Result (...)`-constructed one.
    let (result_set, result_index) = match ctx.scope.resolve_type("Result") {
        Some(KType::SetRef { set, index }) => (Rc::clone(set), *index),
        _ => panic!("Result must be registered before CATCH"),
    };
    let finish: CatchContinue<'a> = Box::new(move |fctx, result| {
        let (tag, payload): (&str, KObject<'a>) = match result {
            Ok(v) => ("Ok", v.deep_clone()),
            Err(e) => ("Error", e.to_tagged(fctx.scope.region)),
        };
        let tagged = KObject::Tagged {
            tag: tag.to_string(),
            value: Rc::new(payload),
            set: Rc::clone(&result_set),
            index: result_index,
            type_args: Rc::new(vec![]),
        };
        Action::Done(Ok(Carried::Object(fctx.scope.region.alloc_object(tagged))))
    });
    Action::Catch {
        watched: Dep::Dispatch {
            expr: expr_inner,
            placement: DepPlacement::OwnScope,
        },
        finish,
    }
}

#[cfg(test)]
mod tests;
