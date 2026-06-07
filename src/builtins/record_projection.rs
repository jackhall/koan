//! `FROM` — caller-side record projection. `(x y) FROM r` re-types the record
//! value `r` to carry only fields `x` and `y`, narrowing the carried per-field
//! type record while `Rc`-sharing the backing value record whole. The dropped
//! fields stay physically present but become invisible through the narrowed
//! type — the same re-tag a typed `LET narrowed :{x,y} = r` ascription performs,
//! except FROM reads the kept fields' types off the record's own carrier, so the
//! caller writes field *names*, not field *types*, inline.
//!
//! This closes record subtyping's projection direction: it can break an
//! `AmbiguousDispatch` tie between two width-incomparable record arms by
//! re-tagging the carrier so only one arm admits. See
//! [design/typing/ktype.md § Variance](../../design/typing/ktype.md#variance).

use std::rc::Rc;

use crate::machine::model::ast::ExpressionPart;
use crate::machine::model::types::Record;
use crate::machine::model::{KObject, KType};
use crate::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, SchedulerHandle, Scope};

use super::{arg, err, kw, register_builtin, sig};

/// `(x y) FROM <record:{}>` — re-tag the record's carried type to the named fields.
///
/// The `fields` operand arrives unevaluated through a `KExpression` slot: each part
/// must be a bare `Identifier` naming a field (never name-resolved). The `record`
/// operand is typed `:{}`, so dispatch shape-gates the slot to records and the body
/// reads a guaranteed `KObject::Record` carrier.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let fields_expr = match bundle.require_kexpression("fields") {
        Ok(e) => e,
        Err(e) => return err(e),
    };

    // Extract field names from the captured expression. Each part must be a bare
    // identifier; a computed field list is out of scope by design.
    let mut names: Vec<String> = Vec::with_capacity(fields_expr.parts.len());
    for part in &fields_expr.parts {
        match &part.value {
            ExpressionPart::Identifier(name) => {
                if names.iter().any(|n| n == name) {
                    return err(KError::new(KErrorKind::ShapeError(format!(
                        "FROM field list has duplicate field `{name}`",
                    ))));
                }
                names.push(name.clone());
            }
            other => {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "FROM field list must be bare field names, got `{}`",
                    other.summarize(),
                ))));
            }
        }
    }

    let (fields, types) = match bundle.get("record") {
        Some(KObject::Record(fields, types)) => (fields, types),
        // The `:{}` slot shape-gates to records, so a non-record argument is a
        // dispatch non-match that never reaches the body. Defensive arm only.
        Some(other) => {
            return err(KError::new(KErrorKind::ShapeError(format!(
                "FROM record operand must be a record, got `{}`",
                other.ktype().name(),
            ))));
        }
        None => return err(KError::new(KErrorKind::MissingArg("record".to_string()))),
    };

    // Each named field must exist in the record; narrow the carried type to exactly
    // the named fields, reading each kept field's type off the record's own carrier.
    let mut narrowed_pairs: Vec<(String, KType<'a>)> = Vec::with_capacity(names.len());
    for name in &names {
        match types.get(name) {
            Some(kt) => narrowed_pairs.push((name.clone(), kt.clone())),
            None => {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "FROM: record has no field `{name}`",
                ))));
            }
        }
    }

    let narrowed = Record::from_pairs(narrowed_pairs);
    let result = KObject::record_with_type(Rc::clone(fields), narrowed);
    BodyResult::value(scope.arena.alloc_object(result))
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    // Return type `:{}` is contract-only ("FROM returns a record"): a native
    // `BodyResult::Value` flows straight to Done without being stamped against the
    // declared return, so the empty `:{}` does not coarsen the body's narrowed
    // `{x,y}` carrier. The `fields` slot is `KExpression` (captured unevaluated);
    // the `record` slot is `:{}`, which shape-gates the operand to records.
    register_builtin(
        scope,
        "FROM",
        sig(
            KType::Record(Box::new(Record::new())),
            vec![
                arg("fields", KType::KExpression),
                kw("FROM"),
                arg("record", KType::Record(Box::new(Record::new()))),
            ],
        ),
        body,
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
    use crate::machine::model::{KObject, KType};
    use crate::machine::RuntimeArena;

    /// `(x y) FROM r` re-tags the carried type to `{x, y}` while every field of `r`
    /// stays physically present on the `Rc`-shared backing record.
    #[test]
    fn from_narrows_carried_type_keeping_all_fields_present() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one("(x y) FROM {x = 1, y = 2, z = 3}"));
        match result {
            KObject::Record(fields, types) => {
                // All three fields stay physically present on the shared value record.
                assert_eq!(fields.len(), 3);
                assert!(fields.get("z").is_some());
                // The carried type is narrowed to exactly x, y.
                assert_eq!(types.len(), 2);
                assert_eq!(types.get("x"), Some(&KType::Number));
                assert_eq!(types.get("y"), Some(&KType::Number));
                assert!(types.get("z").is_none());
            }
            other => panic!("expected Record, got {:?}", other.ktype()),
        }
    }

    /// Single-field projection: `(x)` parses as `Expression([Identifier("x")])` —
    /// it does *not* unwrap to a bare `Identifier` — so the same `KExpression` slot
    /// admits it and no second overload is needed.
    #[test]
    fn from_single_field_projection() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one("(x) FROM {x = 1, y = 2}"));
        match result {
            KObject::Record(fields, types) => {
                assert_eq!(fields.len(), 2);
                assert_eq!(types.len(), 1);
                assert_eq!(types.get("x"), Some(&KType::Number));
                assert!(types.get("y").is_none());
            }
            other => panic!("expected Record, got {:?}", other.ktype()),
        }
    }

    /// `() FROM r` projects to zero fields → the empty record `:{}`, not an error.
    #[test]
    fn from_empty_field_list_yields_empty_record() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one("() FROM {x = 1}"));
        match result {
            KObject::Record(fields, types) => {
                // The backing value record is shared whole; only the type narrows.
                assert_eq!(fields.len(), 1);
                assert_eq!(types.len(), 0);
            }
            other => panic!("expected empty Record, got {:?}", other.ktype()),
        }
    }

    /// Naming a field absent from the record is a `ShapeError`.
    #[test]
    fn from_unknown_field_errors() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let err = run_one_err(scope, parse_one("(x w) FROM {x = 1}"));
        let msg = format!("{err}");
        assert!(
            msg.contains("no field `w`"),
            "expected a 'no field w' shape error, got: {msg}",
        );
    }

    /// A duplicate name in the field list is a `ShapeError`.
    #[test]
    fn from_duplicate_field_errors() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let err = run_one_err(scope, parse_one("(x x) FROM {x = 1}"));
        let msg = format!("{err}");
        assert!(
            msg.contains("duplicate field `x`"),
            "expected a duplicate-field shape error, got: {msg}",
        );
    }

    /// A non-record operand matches no FROM overload — the `:{}` `record` slot rejects
    /// `5`, and no evaluation of the `(x y)` operand can change that, so dispatch fails
    /// cleanly with `DispatchFailed` ("no matching function") at the call rather than
    /// eagerly evaluating `(x y)` and leaking its `unbound name 'x'` — the relaxed
    /// admission pass keeps it a clean miss (see
    /// [scheduler.md § In-walk dispatch precedence](../../design/typing/scheduler.md#in-walk-dispatch-precedence)).
    /// The root miss surfaces through `execute()` like an `AmbiguousDispatch`.
    #[test]
    fn from_non_record_operand_is_dispatch_non_match() {
        use crate::machine::core::KErrorKind;
        use crate::machine::execute::Scheduler;

        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let mut sched = Scheduler::new();
        sched.add_dispatch(parse_one("(x y) FROM 5"), scope);
        let err = sched
            .execute()
            .expect_err("a non-record operand must fail dispatch");
        assert!(
            matches!(&err.kind, KErrorKind::DispatchFailed { .. }),
            "expected a clean DispatchFailed (not a leaked unbound-name), got: {err}",
        );
    }

    /// The disambiguation win: a value carrying `{x, y, z}` ties two width-incomparable
    /// record FN arms (`:{x,y}` and `:{x,z}`); `(x y) FROM r` re-tags the carrier so
    /// only the `:{x,y}` arm admits.
    #[test]
    fn from_breaks_ambiguous_record_dispatch_tie() {
        use crate::machine::core::KErrorKind;
        use crate::machine::execute::Scheduler;

        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "FN (PICK r :{x :Number, y :Str}) -> Str = (\"xy\")\n\
             FN (PICK r :{x :Number, z :Str}) -> Str = (\"xz\")\n\
             LET r = {x = 1, y = \"a\", z = \"b\"}",
        );

        // Bare call ties: the full `{x, y, z}` carrier fills both incomparable arms.
        let mut sched = Scheduler::new();
        sched.add_dispatch(parse_one("PICK r"), scope);
        let error = sched
            .execute()
            .expect_err("the bare call must tie across both incomparable arms");
        assert!(
            matches!(error.kind, KErrorKind::AmbiguousDispatch { .. }),
            "expected AmbiguousDispatch on the bare call, got {error:?}",
        );

        // `(x y) FROM r` re-tags the carrier to `{x, y}`; only `:{x,y}` admits.
        let picked = run_one(scope, parse_one("PICK ((x y) FROM r)"));
        match picked {
            KObject::KString(s) => assert_eq!(s, "xy"),
            other => panic!("expected \"xy\", got {:?}", other.ktype()),
        }
    }
}
