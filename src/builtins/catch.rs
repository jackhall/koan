//! `CATCH <expr>` — lift a single interpreter fault into a `Result` value.
//! Shares the `add_catch` primitive with [`TRY-WITH`](super::try_with) but
//! lacks branches, an `it` binding, and the re-raise path: the finish closure
//! wraps the outcome in the prelude [`Result`](super::result) carrier as
//! either `Ok(v)` or `Error(KError::to_tagged())`.

use std::rc::Rc;

use crate::machine::core::kerror_ktype;
use crate::machine::model::{KObject, KType};
use crate::machine::{ArgumentBundle, BodyResult, CatchFinish, SchedulerHandle, Scope};

use super::{arg, err, kw, register_builtin, sig};

pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let expr_inner = match bundle.extract_kexpression_or_shape_error("CATCH", "expr") {
        Ok(e) => e,
        Err(e) => return err(e),
    };
    // Capture the prelude `Result` member identity at body time (not in the finish
    // closure) so the CATCH-produced value shares the nominal identity of a
    // `Result (...)`-constructed one. Requires `result::register` to run first.
    let (result_set, result_index) = match scope.resolve_type("Result") {
        Some(KType::SetRef { set, index }) => (Rc::clone(set), *index),
        _ => panic!("Result must be registered before CATCH"),
    };
    let sub_id = sched.add_dispatch(expr_inner, scope);
    let finish: CatchFinish<'a> = Box::new(move |scope, _sched, result| {
        let (tag, payload): (&str, KObject<'a>) = match result {
            Ok(v) => ("Ok", v.deep_clone()),
            Err(e) => ("Error", e.to_tagged(scope.arena)),
        };
        let tagged = KObject::Tagged {
            tag: tag.to_string(),
            value: Rc::new(payload),
            set: Rc::clone(&result_set),
            index: result_index,
            // Erased: only the inhabited side's payload type is known here.
            // `matches_value(ConstructorApply, Tagged)` inspects that payload
            // directly, so leaving `type_args` empty still types correctly.
            type_args: Rc::new(vec![]),
        };
        BodyResult::value(scope.arena.alloc_object(tagged))
    });
    let catch_id = sched.add_catch(sub_id, scope, finish);
    BodyResult::DeferTo(catch_id)
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    // CATCH yields `Result {Ok :Any, Error :KError}` — `Any` covers only the unpredictable
    // `Ok` payload, the `Error` arm is the `KError` carrier. `result::register` runs first, so
    // the `Result` `SetRef` resolves here. This is a documentary contract: the catch finish
    // produces a `BodyResult::Value` (never a `ReturnContract`), so the declared return is not
    // validated against the runtime value, and the throwaway `kerror_ktype()` identity is fine.
    let result_ctor = match scope.resolve_type("Result") {
        Some(kt @ KType::SetRef { .. }) => kt.clone(),
        _ => panic!("Result must be registered before CATCH"),
    };
    let return_type = KType::ConstructorApply {
        ctor: Box::new(result_ctor),
        args: vec![KType::Any, kerror_ktype()],
    };
    register_builtin(
        scope,
        "CATCH",
        sig(
            return_type,
            vec![kw("CATCH"), arg("expr", KType::KExpression)],
        ),
        body,
    );
}

#[cfg(test)]
mod tests;
