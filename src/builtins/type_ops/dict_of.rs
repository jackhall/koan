use crate::machine::model::{KObject, KType};
use crate::machine::{ArgumentBundle, BodyResult, Scope, SchedulerHandle};

use crate::builtins::err;

/// `DICT_OF <key:TypeExprRef> <value:TypeExprRef>` → `TypeExprRef` carrying
/// `Dict<key, value>`.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let key = match bundle.require_ktype("key") {
        Ok(t) => t.clone(),
        Err(e) => return err(e),
    };
    let value = match bundle.require_ktype("value") {
        Ok(t) => t.clone(),
        Err(e) => return err(e),
    };
    BodyResult::Value(
        scope
            .arena
            .alloc(KObject::KTypeValue(KType::Dict(Box::new(key), Box::new(value)))),
    )
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run_one, run_root_silent};
    use crate::machine::model::{KObject, KType};
    use crate::machine::RuntimeArena;

    /// `(DICT_OF Str Number)` lowers to `Dict<Str, Number>`.
    #[test]
    fn dict_of_str_number_lowers_to_dict() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one("DICT_OF Str Number"));
        match result {
            KObject::KTypeValue(kt) => {
                assert_eq!(
                    *kt,
                    KType::Dict(Box::new(KType::Str), Box::new(KType::Number))
                );
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }
}
