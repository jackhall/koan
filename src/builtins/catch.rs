//! `CATCH <expr>` — lift a single interpreter fault into a `Result` value.
//!
//! Reuses the [`add_catch`](crate::machine::execute::Scheduler) primitive that drives
//! [`TRY-WITH`](super::try_with), minus the branches slot, the `it` binding, and the
//! re-raise path. `expr` is scheduled as a sub-dispatch; the finish closure wraps the
//! outcome in the prelude-registered [`Result`](super::result) carrier — `ok(v)` on
//! success, `error(e)` on failure where `e = KError::to_tagged()`. Per-`KErrorKind`
//! dispatch is reached by `MATCH`-ing the inner payload after destructuring the
//! `Result`.

use std::rc::Rc;

use crate::machine::model::types::UserTypeKind;
use crate::machine::model::{KObject, KType};
use crate::machine::{
    ArgumentBundle, BodyResult, CatchFinish, KError, KErrorKind, Scope, SchedulerHandle,
};

use crate::machine::core::kfunction::argument_bundle::extract_kexpression;
use super::{arg, err, kw, register_builtin, sig};

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
    // Capture the registered carrier's `scope_id` here in `body`, not in the finish
    // closure: `scope.id` is the call-site scope (root for a top-level CATCH, a per-call
    // scope inside a function body), whereas the `Result` carrier's id is fixed at
    // prelude registration. Matching it is what makes a CATCH-produced value and a
    // `Result (...)`-constructed value the same nominal type. (Requires `result::register`
    // to run before `catch::register`.)
    let result_scope_id = match scope.lookup("Result") {
        Some(KObject::TaggedUnionType { scope_id, .. }) => *scope_id,
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
            // Erased: CATCH knows only the inhabited side's payload type, not the absent
            // parameter. `matches_value(ConstructorApply, Tagged)` inspects the inhabited
            // tag's payload directly, so a `Result<_, MyErr>` slot still correctly rejects a
            // caught `error(KError)` without a stamped carrier here.
            type_args: Rc::new(vec![]),
        };
        BodyResult::Value(scope.arena.alloc_object(tagged))
    });
    let catch_id = sched.add_catch(sub_id, scope, finish);
    BodyResult::DeferTo(catch_id)
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "CATCH",
        sig(KType::AnyUserType { kind: UserTypeKind::Tagged }, vec![
            kw("CATCH"),
            arg("expr", KType::KExpression),
        ]),
        body,
    );
}

#[cfg(test)]
mod tests;
