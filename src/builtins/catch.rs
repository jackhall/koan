//! `CATCH <expr>` — lift a single interpreter fault into a `Result` value.
//! Shares the `add_catch` primitive with [`TRY-WITH`](super::try_with) but
//! lacks branches, an `it` binding, and the re-raise path: the finish closure
//! wraps the outcome in the prelude [`Result`](super::result) carrier as
//! either `ok(v)` or `error(KError::to_tagged())`.

use std::rc::Rc;

use crate::machine::model::types::UserTypeKind;
use crate::machine::model::{KObject, KType};
use crate::machine::{
    ArgumentBundle, BodyResult, CatchFinish, KError, KErrorKind, SchedulerHandle, Scope,
};

use super::{arg, err, kw, register_builtin, sig};
use crate::machine::core::kfunction::argument_bundle::extract_kexpression;

pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let expr_inner = match extract_kexpression(&mut bundle, "expr") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "CATCH expr slot must be a parenthesized expression".to_string(),
            )));
        }
    };
    // Read the prelude `Result` identity's scope_id at body time (not in the finish
    // closure) so the CATCH-produced value matches the nominal identity of a
    // `Result (...)`-constructed one. Requires `result::register` to run first.
    let result_scope_id = match scope.resolve_type("Result") {
        Some(KType::UserType { scope_id, .. }) => *scope_id,
        _ => panic!("Result must be registered before CATCH"),
    };
    let sub_id = sched.add_dispatch(expr_inner, scope);
    let finish: CatchFinish<'a> = Box::new(move |scope, _sched, result| {
        let (tag, payload): (&str, KObject<'a>) = match result {
            Ok(v) => ("ok", v.deep_clone()),
            Err(e) => ("error", e.to_tagged()),
        };
        let tagged = KObject::Tagged {
            tag: tag.to_string(),
            value: Rc::new(payload),
            scope_id: result_scope_id,
            name: "Result".to_string(),
            // Erased: only the inhabited side's payload type is known here.
            // `matches_value(ConstructorApply, Tagged)` inspects that payload
            // directly, so leaving `type_args` empty still types correctly.
            type_args: Rc::new(vec![]),
        };
        BodyResult::Value(scope.arena.alloc(tagged))
    });
    let catch_id = sched.add_catch(sub_id, scope, finish);
    BodyResult::DeferTo(catch_id)
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "CATCH",
        sig(
            KType::AnyUserType {
                kind: UserTypeKind::tagged_sentinel(),
            },
            vec![kw("CATCH"), arg("expr", KType::KExpression)],
        ),
        body,
    );
}

#[cfg(test)]
mod tests;
