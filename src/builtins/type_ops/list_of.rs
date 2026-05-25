use crate::machine::model::{KObject, KType};
use crate::machine::{ArgumentBundle, BodyResult, Scope, SchedulerHandle};

use crate::builtins::err;

/// `LIST_OF <elem:TypeExprRef>` → `TypeExprRef` carrying `List<elem>`.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let elem = match bundle.require_ktype("elem") {
        Ok(t) => t.clone(),
        Err(e) => return err(e),
    };
    BodyResult::Value(
        scope
            .arena
            .alloc(KObject::KTypeValue(KType::List(Box::new(elem)))),
    )
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run_one, run_root_silent};
    use crate::machine::model::{KObject, KType};
    use crate::machine::RuntimeArena;

    /// `(LIST_OF Number)` dispatches and produces a `KTypeValue` carrying the elaborated
    /// `KType::List(Number)` directly — no surface-form round-trip needed.
    #[test]
    fn list_of_number_lowers_to_list_number() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one("LIST_OF Number"));
        match result {
            KObject::KTypeValue(kt) => {
                assert_eq!(*kt, KType::List(Box::new(KType::Number)));
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// Nested dispatch: `(LIST_OF (LIST_OF Number))` schedules the inner LIST_OF as a
    /// sub-Dispatch and the outer Bind splices the result in. End-to-end exercises the
    /// scheduler-driven type-expression path.
    #[test]
    fn nested_list_of_dispatches_through_scheduler() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one("LIST_OF (LIST_OF Number)"));
        match result {
            KObject::KTypeValue(kt) => {
                assert_eq!(
                    *kt,
                    KType::List(Box::new(KType::List(Box::new(KType::Number))))
                );
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }
}
