//! `FROM` — caller-side record projection. `(x y) FROM r` re-types the record
//! value `r` to carry only fields `x` and `y`, narrowing the carried per-field
//! type record while `Rc`-sharing the backing value record whole. The dropped
//! fields stay physically present but invisible through the narrowed type — the
//! same re-tag a typed `LET narrowed :{x,y} = r` ascription performs, except FROM
//! reads the kept fields' types off the record's own carrier, so the caller writes
//! field *names*, not field *types*.
//!
//! This closes record subtyping's projection direction: it can break an
//! `AmbiguousDispatch` tie between two width-incomparable record arms by
//! re-tagging the carrier so only one arm admits. See
//! [design/typing/ktype/parameterization-and-variance.md § Variance](../../design/typing/ktype/parameterization-and-variance.md#variance).

use crate::machine::model::TypeRegistry;
use std::rc::Rc;

use crate::machine::model::Carried;
use crate::machine::model::ExpressionPart;
use crate::machine::model::Record;
use crate::machine::model::{KObject, KType};
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, kw, sig};

/// `(x y) FROM <record:{}>` — re-tag the record's carried type to the named fields.
///
/// The `fields` operand arrives unevaluated through a `KExpression` slot: each part
/// must be a bare `Identifier` naming a field (never name-resolved). The `record`
/// operand is typed `:{}`, so dispatch shape-gates the slot to records and the body
/// reads a guaranteed `KObject::Record` carrier.
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::{arg_object, require_kexpression, Action};

    let fields_expr = crate::try_action!(require_kexpression(ctx.args, "FROM", "fields"));

    // A computed field list is out of scope: each part must be a bare identifier.
    let mut names: Vec<String> = Vec::with_capacity(fields_expr.parts.len());
    for part in &fields_expr.parts {
        match &part.value {
            ExpressionPart::Identifier(name) => {
                if names.iter().any(|n| n == name) {
                    return Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                        "FROM field list has duplicate field `{name}`",
                    )))));
                }
                names.push(name.clone());
            }
            other => {
                return Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                    "FROM field list must be bare field names, got `{}`",
                    other.summarize(),
                )))));
            }
        }
    }

    let record_obj = match arg_object(ctx.args, "record") {
        Some(obj @ KObject::Record(_, _)) => obj,
        // The `:{}` slot shape-gates to records, so a non-record argument is a
        // dispatch non-match that never reaches the body. Defensive arm only.
        Some(other) => {
            return Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                "FROM record operand must be a record, got `{}`",
                other.ktype().name(ctx.types),
            )))));
        }
        None => {
            return Action::Done(Err(KError::new(KErrorKind::MissingArg(
                "record".to_string(),
            ))));
        }
    };

    // Ambient probe: every named field must exist in the record's type map (the error arm stays
    // here). The at-brand rebuild below re-reads the same map from the operand view, so the two
    // cannot disagree on which fields the narrowed carrier keeps.
    let types = match record_obj {
        KObject::Record(_, types) => types,
        _ => unreachable!("record_obj is shape-gated to a Record above"),
    };
    for name in &names {
        if types.get(name).is_none() {
            return Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                "FROM: record has no field `{name}`",
            )))));
        }
    }

    // Cross the record as the projection's lhs operand. A carrier-less record literal (region-pure)
    // rebuilds into the read-site region and seals resident — coverage-equivalent to an empty-reach
    // seal.
    let resident;
    let lhs: &crate::machine::DeliveredCarried = match ctx.arg_carrier("record") {
        Some(c) => c,
        None => {
            resident = match ctx
                .scope
                .seal_fresh_object(record_obj.deep_clone(), ctx.types)
            {
                Ok(witnessed) => ctx.scope.seal_resident_delivered(witnessed),
                Err(e) => return Action::Done(Err(e)),
            };
            &resident
        }
    };
    // The projection `Rc`-shares the record's backing field values, so it reaches whatever the
    // `record` operand reaches. Built at the fold brand from the operand's own view — the narrowed
    // type map re-read from the view, the backing fields `Rc`-shared whole — so the result's witness
    // names the read-site home frame plus that reach by construction.
    Action::Done(Ok(ctx.ctx.alloc_carried_with(&[lhs], move |b, views| {
        let record = match views[0] {
            Carried::Object(o) => o,
            Carried::Type(_) | Carried::UnresolvedType(_) => {
                unreachable!("the `record` slot shape-gates to records")
            }
        };
        let (fields, types) = match record {
            KObject::Record(fields, types) => (fields, types),
            _ => unreachable!("the `record` slot shape-gates to records"),
        };
        let narrowed = Record::from_pairs(names.iter().map(|name| {
            (
                name.clone(),
                types
                    .get(name)
                    .expect("probed ambient: field exists in the record")
                    .clone(),
            )
        }));
        Carried::Object(
            b.alloc_object_folded(KObject::record_with_type(Rc::clone(fields), narrowed)),
        )
    })))
}

pub fn register<'a>(scope: &'a Scope<'a>, types: &TypeRegistry) {
    // Return type `:{}` is contract-only ("FROM returns a record"): a native
    // `Outcome::Done(Value)` flows straight to Done without being stamped against the
    // declared return, so the empty `:{}` does not coarsen the body's narrowed
    // `{x,y}` carrier. The `fields` slot is `KExpression` (captured unevaluated);
    // the `record` slot is `:{}`, which shape-gates the operand to records.
    let signature = sig(
        KType::record(Box::new(Record::new())),
        vec![
            arg("fields", KType::KExpression),
            kw("FROM"),
            arg("record", KType::record(Box::new(Record::new()))),
        ],
    );
    crate::builtins::register_builtin(scope, "FROM", signature, body, types);
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run, run_one, run_one_err, run_root_silent};
    use crate::machine::model::{KObject, KType};
    use crate::machine::run_root_storage;

    #[test]
    fn from_narrows_carried_type_keeping_all_fields_present() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let result = run_one(scope, parse_one("(x y) FROM {x = 1, y = 2, z = 3}"));
        match result {
            KObject::Record(fields, types) => {
                assert_eq!(fields.len(), 3);
                assert!(fields.get("z").is_some());
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
        let region = run_root_storage();
        let scope = run_root_silent(&region);
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

    #[test]
    fn from_empty_field_list_yields_empty_record() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let result = run_one(scope, parse_one("() FROM {x = 1}"));
        match result {
            KObject::Record(fields, types) => {
                assert_eq!(fields.len(), 1);
                assert_eq!(types.len(), 0);
            }
            other => panic!("expected empty Record, got {:?}", other.ktype()),
        }
    }

    #[test]
    fn from_unknown_field_errors() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let err = run_one_err(scope, parse_one("(x w) FROM {x = 1}"));
        let msg = format!("{err}");
        assert!(
            msg.contains("no field `w`"),
            "expected a 'no field w' shape error, got: {msg}",
        );
    }

    #[test]
    fn from_duplicate_field_errors() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let err = run_one_err(scope, parse_one("(x x) FROM {x = 1}"));
        let msg = format!("{err}");
        assert!(
            msg.contains("duplicate field `x`"),
            "expected a duplicate-field shape error, got: {msg}",
        );
    }

    /// A non-record operand matches no FROM overload — the `:{}` `record` slot rejects
    /// `5`, so dispatch fails cleanly with `DispatchFailed` rather than eagerly evaluating
    /// `(x y)` and leaking its `unbound name 'x'`: the relaxed admission pass keeps it a
    /// clean miss (see
    /// [scheduler.md § In-walk dispatch precedence](../../design/typing/scheduler.md#in-walk-dispatch-precedence)).
    #[test]
    fn from_non_record_operand_is_dispatch_non_match() {
        use crate::machine::KErrorKind;
        use crate::machine::KoanRuntime;

        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let mut runtime = KoanRuntime::new();
        let root = runtime.dispatch_in_scope(parse_one("(x y) FROM 5"), scope);
        runtime
            .execute()
            .expect("a dispatch failure is slot-terminal, not a fatal execute error");
        let err = runtime
            .result_error(root)
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
        use crate::machine::KErrorKind;
        use crate::machine::KoanRuntime;

        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(
            scope,
            "FN (PICK r :{x :Number, y :Str}) -> Str = (\"xy\")\n\
             FN (PICK r :{x :Number, z :Str}) -> Str = (\"xz\")\n\
             LET r = {x = 1, y = \"a\", z = \"b\"}",
        );

        // Bare call ties: the full `{x, y, z}` carrier fills both incomparable arms.
        let mut runtime = KoanRuntime::new();
        let root = runtime.dispatch_in_scope(parse_one("PICK r"), scope);
        runtime
            .execute()
            .expect("a dispatch failure is slot-terminal, not a fatal execute error");
        let error = runtime
            .result_error(root)
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
