//! `FROM` — caller-side record projection. `(x y) FROM r` re-types the record
//! value `r` to carry only fields `x` and `y`, narrowing the carried per-field
//! type record while sharing the backing substrate borrow whole. The dropped
//! fields stay physically present but invisible through the narrowed type — the
//! same re-tag a typed `LET narrowed :{x,y} = r` ascription performs, except FROM
//! reads the kept fields' types off the record's own carrier, so the caller writes
//! field *names*, not field *types*.
//!
//! This closes record subtyping's projection direction: it can break an
//! `AmbiguousDispatch` tie between two width-incomparable record arms by
//! re-tagging the carrier so only one arm admits. See
//! [design/typing/ktype/parameterization-and-variance.md § Variance](../../design/typing/ktype/parameterization-and-variance.md#variance).

use crate::machine::WriteGate;

use crate::machine::model::Carried;
use crate::machine::model::ExpressionPart;
use crate::machine::model::Record;
use crate::machine::model::{KObject, KType};
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, kw, sig};
use crate::machine::model::RunRegistries;
use crate::machine::model::Symbol;
use crate::witnessed::BumpVec;

// This builtin's slot spellings, minted once and read back by symbol.
crate::slots! { SLOTS { fields, record } }

/// `(x y) FROM <record:{}>` — re-tag the record's carried type to the named fields.
///
/// The `fields` operand arrives unevaluated through a `KExpression` slot: each part
/// must be a bare `Identifier` naming a field (never name-resolved). The `record`
/// operand is typed `:{}`, so dispatch shape-gates the slot to records and the body
/// reads a guaranteed `KObject::Record` carrier.
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::{Action, require_kexpression};

    let fields_expr = crate::try_action!(require_kexpression(ctx.args, "FROM", &SLOTS.fields));

    // A computed field list is out of scope: each part must be a bare identifier, which arrives
    // as the symbol the parse minted for it — the same currency the narrowed record type keys by,
    // and the interner the diagnostics below render through already holds its spelling.
    //
    // The list is read by the projection below and never leaves the step, so it stages on the step
    // scratch. One part contributes at most one name — the loop either pushes or returns — so the
    // part count is an exact upper bound and the capacity is taken up front rather than grown.
    let mut names: BumpVec<'a, Symbol> =
        BumpVec::with_capacity_in(fields_expr.parts.len(), ctx.scratch);
    for part in fields_expr.parts {
        match part.value {
            ExpressionPart::Identifier(v) => {
                if names.contains(&v.symbol()) {
                    return Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
                        "FROM field list has duplicate field `{}`",
                        ctx.registries.labels.render(v.symbol()),
                    )))));
                }
                names.push(v.symbol());
            }
            other => {
                return Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
                    "FROM field list must be bare field names, got `{}`",
                    other.summary(&ctx.registries.labels),
                )))));
            }
        }
    }

    let record_obj = match ctx.args.object(&SLOTS.record) {
        Some(obj @ KObject::Record(_, _)) => obj,
        // The `:{}` slot shape-gates to records, so a non-record argument is a
        // dispatch non-match that never reaches the body. Defensive arm only.
        Some(other) => {
            return Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
                "FROM record operand must be a record, got `{}`",
                other.ktype().name(ctx.registries),
            )))));
        }
        None => {
            return Action::done(Err(KError::new(KErrorKind::MissingArg(
                "record".to_string(),
            ))));
        }
    };

    // Ambient probe: every named field must exist in the record's type map (the error arm stays
    // here). The at-brand rebuild below re-reads the same map from the operand view, so the two
    // cannot disagree on which fields the narrowed carrier keeps.
    let record_type = match record_obj {
        KObject::Record(_, record_type) => *record_type,
        _ => unreachable!("record_obj is shape-gated to a Record above"),
    };
    // The projected field record is assembled under the type-table borrow: the probe and the
    // narrowed record read the same fields by reference, and the intern below runs once the read
    // has closed.
    let projected = ctx.types().with_node(record_type, |node| {
        let crate::machine::model::TypeNode::Record { fields } = node else {
            unreachable!("a Record value's type is always a Record node")
        };
        names
            .iter()
            .map(|symbol| {
                fields
                    .get_key_value(*symbol)
                    .map(|(name, ktype)| (name, *ktype))
                    .ok_or(*symbol)
            })
            .collect::<Result<Record<KType>, _>>()
    });
    let projected = match projected {
        Ok(projected) => projected,
        Err(missing) => {
            return Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
                "FROM: record has no field `{}`",
                ctx.registries.labels.display(missing),
            )))));
        }
    };
    // The narrowed record type — interned once here where the registry is in scope, then copied
    // into the at-brand rebuild below.
    let narrowed_type = ctx.types().record(projected);

    // Cross the record as the projection's lhs operand. A carrier-less lhs is region-pure by the
    // argument view's carrier contract, so it is placed into the read-site region through the
    // shape-split pure door and enveloped there — coverage-equivalent to an empty-reach seal. No region-pure
    // shape is a `Record`, so that arm's diagnostic is what a construction bug would surface here.
    let resident;
    let lhs: &crate::machine::DeliveredCarried = match ctx.args.carrier(&SLOTS.record) {
        Some(c) => c,
        None => {
            resident = match ctx.scope.deliver_pure_value(record_obj) {
                Ok(resident) => resident,
                Err(e) => return Action::done(Err(e)),
            };
            &resident
        }
    };
    // The projection shares the record's backing substrate borrow, so it reaches whatever the
    // `record` operand reaches. Built at the fold brand from the operand's own view — the narrowed
    // type map re-read from the view, the backing substrate shared whole — so the result's witness
    // names the read-site home frame plus that reach by construction.
    Action::done(Ok(ctx.ctx.alloc_carried_with(&[lhs], move |b, views| {
        let record = match views[0] {
            Carried::Object(o) => o,
            Carried::Type(_) | Carried::UnresolvedType(_) => {
                unreachable!("the `record` slot shape-gates to records")
            }
        };
        let substrate = match record {
            KObject::Record(substrate, _) => *substrate,
            _ => unreachable!("the `record` slot shape-gates to records"),
        };
        Carried::Object(b.alloc_object_folded(KObject::record_with_type(substrate, narrowed_type)))
    })))
}

pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    let types = &registries.types;
    // Return type `:{}` is contract-only ("FROM returns a record"): a native
    // `Outcome::Done(Value)` flows straight to Done without being stamped against the
    // declared return, so the empty `:{}` does not coarsen the body's narrowed
    // `{x,y}` carrier. The `fields` slot is `KExpression` (captured unevaluated);
    // the `record` slot is `:{}`, which shape-gates the operand to records.
    let signature = sig(
        types.record(Record::new()),
        vec![
            arg(registries, &SLOTS.fields, KType::KEXPRESSION),
            kw(registries, "FROM"),
            arg(registries, &SLOTS.record, types.record(Record::new())),
        ],
    );
    crate::builtins::register_builtin(scope, signature, body, registries, gate);
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::TestRun;
    use crate::machine::model::{KObject, KType, TypeNode};
    use crate::machine::program_storage;
    use crate::machine::run_root_storage;

    #[test]
    fn from_narrows_carried_type_keeping_all_fields_present() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let result = test_run.run_one(test_run.parse_one("(x y) FROM {x = 1, y = 2, z = 3}"));
        match result {
            KObject::Record(substrate, record_type) => {
                assert_eq!(substrate.len(), 3);
                assert!(
                    substrate
                        .field(crate::machine::model::Symbol::of("z"))
                        .is_some()
                );
                let field_types = match test_run.types().node(*record_type) {
                    TypeNode::Record { fields } => fields,
                    _ => panic!("record value's type must be a Record node, got {record_type:?}"),
                };
                assert_eq!(field_types.len(), 2);
                assert_eq!(
                    field_types
                        .get(crate::machine::model::Symbol::of("x"))
                        .copied(),
                    Some(KType::NUMBER)
                );
                assert_eq!(
                    field_types
                        .get(crate::machine::model::Symbol::of("y"))
                        .copied(),
                    Some(KType::NUMBER)
                );
                assert!(
                    field_types
                        .get(crate::machine::model::Symbol::of("z"))
                        .is_none()
                );
            }
            other => panic!("expected Record, got {:?}", other.ktype()),
        }
    }

    /// Single-field projection: `(x)` parses as `Expression([Identifier("x")])` —
    /// it does *not* unwrap to a bare `Identifier` — so the same `KExpression` slot
    /// admits it and no second overload is needed.
    #[test]
    fn from_single_field_projection() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let result = test_run.run_one(test_run.parse_one("(x) FROM {x = 1, y = 2}"));
        match result {
            KObject::Record(substrate, record_type) => {
                assert_eq!(substrate.len(), 2);
                let field_types = match test_run.types().node(*record_type) {
                    TypeNode::Record { fields } => fields,
                    _ => panic!("record value's type must be a Record node, got {record_type:?}"),
                };
                assert_eq!(field_types.len(), 1);
                assert_eq!(
                    field_types
                        .get(crate::machine::model::Symbol::of("x"))
                        .copied(),
                    Some(KType::NUMBER)
                );
                assert!(
                    field_types
                        .get(crate::machine::model::Symbol::of("y"))
                        .is_none()
                );
            }
            other => panic!("expected Record, got {:?}", other.ktype()),
        }
    }

    #[test]
    fn from_empty_field_list_yields_empty_record() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let result = test_run.run_one(test_run.parse_one("() FROM {x = 1}"));
        match result {
            KObject::Record(substrate, record_type) => {
                assert_eq!(substrate.len(), 1);
                let field_types = match test_run.types().node(*record_type) {
                    TypeNode::Record { fields } => fields,
                    _ => panic!("record value's type must be a Record node, got {record_type:?}"),
                };
                assert_eq!(field_types.len(), 0);
            }
            other => panic!("expected empty Record, got {:?}", other.ktype()),
        }
    }

    #[test]
    fn from_unknown_field_errors() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let err = test_run.run_one_err(test_run.parse_one("(x w) FROM {x = 1}"));
        let msg = format!("{err}");
        assert!(
            msg.contains("no field `w`"),
            "expected a 'no field w' shape error, got: {msg}",
        );
    }

    #[test]
    fn from_duplicate_field_errors() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let err = test_run.run_one_err(test_run.parse_one("(x x) FROM {x = 1}"));
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

        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let scope = test_run.scope;
        let root = test_run.dispatch_in_scope(
            crate::machine::model::WorkingExpression::from_ast(
                scope.brand(),
                test_run.parse_one("(x y) FROM 5"),
            ),
            scope,
        );
        let edge = test_run.runtime.install_edge_for_test(root, scope);
        test_run
            .runtime
            .execute()
            .expect("a dispatch failure is slot-terminal, not a fatal execute error");
        let err = test_run
            .runtime
            .edge_result_error(edge)
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

        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let scope = test_run.scope;
        test_run.run(
            "FN (PICK r :{x :Number, y :Str}) -> Str = (\"xy\")\n\
             FN (PICK r :{x :Number, z :Str}) -> Str = (\"xz\")\n\
             LET r = {x = 1, y = \"a\", z = \"b\"}",
        );

        // Bare call ties: the full `{x, y, z}` carrier fills both incomparable arms.
        let root = test_run.dispatch_in_scope(
            crate::machine::model::WorkingExpression::from_ast(
                scope.brand(),
                test_run.parse_one("PICK r"),
            ),
            scope,
        );
        let edge = test_run.runtime.install_edge_for_test(root, scope);
        test_run
            .runtime
            .execute()
            .expect("a dispatch failure is slot-terminal, not a fatal execute error");
        let error = test_run
            .runtime
            .edge_result_error(edge)
            .expect_err("the bare call must tie across both incomparable arms");
        assert!(
            matches!(error.kind, KErrorKind::AmbiguousDispatch { .. }),
            "expected AmbiguousDispatch on the bare call, got {error:?}",
        );

        // `(x y) FROM r` re-tags the carrier to `{x, y}`; only `:{x,y}` admits.
        let picked = test_run.run_one(test_run.parse_one("PICK ((x y) FROM r)"));
        match picked {
            KObject::KString(s) => assert_eq!(*s, "xy"),
            other => panic!("expected \"xy\", got {:?}", other.ktype()),
        }
    }
}
