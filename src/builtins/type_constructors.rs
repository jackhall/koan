//! Keyworded type-constructor builtins reached through the `:(...)` sigil.
//! See [type-language-via-dispatch](../../design/typing/type-language-via-dispatch.md).
//!
//! - `LIST OF :Type` → `KTypeValue(KType::List(_))`
//! - `MAP :Type -> :Type` → `KTypeValue(KType::Dict(_, _))`
//! - `FN <sig> -> :Type` → `KTypeValue(KType::KFunction { .. })`
//! - `FUNCTOR <sig> -> :Type` → `KTypeValue(KType::KFunctor { .. })`
//!
//! Fully-uppercase head keywords keep parameterized-type construction in
//! narrow candidate buckets so user-defined functors overloading short
//! connector words like `OF` don't pay a bucket-walk cost on every dispatched
//! parameterized type.

use crate::machine::execute::defer_field_list_via_combine;
use crate::machine::model::ast::KExpression;
use crate::machine::model::types::{
    parse_typed_field_list_via_elaborator, Elaborator, FieldListOutcome, FieldNameKind,
    NominalKind, ProjectedSchema, RecursiveSet,
};
use crate::machine::model::{KObject, KType, Record};
use crate::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, SchedulerHandle, Scope};

use super::{arg, err, kw, register_builtin, sig};

/// Which carrier the shared field-list path builds. All three ride the same parser and
/// Combine/defer machinery; they differ only in the `KType` they fold their fields into,
/// the diagnostic context string, and the field-name policy (both admit capitalized `Type`
/// param names like `Ty`). The record type `:{…}` is structural — a first-class
/// `ExpressionPart::RecordType` the dispatcher folds to `KType::Record` directly — so it is
/// not a carrier kind here.
#[derive(Clone, Copy)]
enum CarrierKind {
    Function,
    Functor,
}

impl CarrierKind {
    fn context(self) -> &'static str {
        match self {
            CarrierKind::Function => "FN parameters",
            CarrierKind::Functor => "FUNCTOR parameters",
        }
    }

    fn field_name_kind(self) -> FieldNameKind {
        match self {
            CarrierKind::Function | CarrierKind::Functor => FieldNameKind::IdentifierOrType,
        }
    }
}

fn body_list_of<'a>(
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
            .alloc_object(KObject::KTypeValue(KType::List(Box::new(elem)))),
    )
}

fn body_map<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let k = match bundle.require_ktype("k") {
        Ok(t) => t.clone(),
        Err(e) => return err(e),
    };
    let v = match bundle.require_ktype("v") {
        Ok(t) => t.clone(),
        Err(e) => return err(e),
    };
    BodyResult::Value(
        scope
            .arena
            .alloc_object(KObject::KTypeValue(KType::Dict(Box::new(k), Box::new(v)))),
    )
}

/// `:(<arg> AS <Ctor>)` → `KTypeValue(KType::ConstructorApply { ctor, args })`.
/// The constructor rides in as an ordinary `:Type` arg (the `AS` right-hand slot),
/// so a user-declared `TEMPLATE` head dispatches through the keyworded path like any
/// other parameterized type — no value-construction (`TypeCall`) lane involved.
/// Binary form, so arity-1 only; multi-parameter application is the
/// [modular implicits](../../roadmap/predicate_typing/modular-implicits.md) follow-up.
fn body_apply_as<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let applied = match bundle.require_ktype("applied") {
        Ok(t) => t.clone(),
        Err(e) => return err(e),
    };
    let ctor = match bundle.require_ktype("ctor") {
        Ok(t) => t.clone(),
        Err(e) => return err(e),
    };
    let param_count = match &ctor {
        KType::SetRef { set, index } if set.member(*index).kind == NominalKind::TypeConstructor => {
            match RecursiveSet::projected_schema(set, *index) {
                ProjectedSchema::TypeConstructor { param_names, .. } => param_names.len(),
                _ => unreachable!("TypeConstructor-kind member projects a TypeConstructor schema"),
            }
        }
        other => {
            return err(KError::new(KErrorKind::ShapeError(format!(
                "right-hand side of `AS` must be a type constructor, got `{}`",
                other.name(),
            ))));
        }
    };
    if param_count != 1 {
        return err(KError::new(KErrorKind::ShapeError(format!(
            "`{}` takes {param_count} type arguments; the `AS` form supplies one, so \
             multi-parameter application is not yet supported",
            ctor.name(),
        ))));
    }
    BodyResult::Value(
        scope
            .arena
            .alloc_object(KObject::KTypeValue(KType::ConstructorApply {
                ctor: Box::new(ctor),
                args: vec![applied],
            })),
    )
}

/// `sig` is `KExpression` (lazy) so the parser-emitted nested-parens
/// `(x :Number, y :Str)` arrives unevaluated. The parameter names round-trip into
/// `KType::KFunction`'s parameter record — see
/// [ktype.md § Record fields and KType hashing](../../design/typing/ktype.md#record-fields-and-ktype-hashing).
fn body_fn<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let sig_expr = match bundle.require_kexpression("sig") {
        Ok(e) => e.clone(),
        Err(e) => return err(e),
    };
    let ret = match bundle.require_ktype("ret") {
        Ok(t) => t.clone(),
        Err(e) => return err(e),
    };
    build_carrier(scope, sched, sig_expr, ret, CarrierKind::Function)
}

fn body_functor<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let sig_expr = match bundle.require_kexpression("sig") {
        Ok(e) => e.clone(),
        Err(e) => return err(e),
    };
    let ret = match bundle.require_ktype("ret") {
        Ok(t) => t.clone(),
        Err(e) => return err(e),
    };
    build_carrier(scope, sched, sig_expr, ret, CarrierKind::Functor)
}

/// Walk the parameter list through the shared field-list parser (the same one STRUCT /
/// UNION use), so nested parameterized param types like `xs :(LIST OF Number)` sub-Dispatch
/// and capitalized FUNCTOR param names like `Ty` are accepted. An anonymous function type
/// has no self-reference binder, so the elaborator carries no nominal-binder bookkeeping.
fn build_carrier<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    sig_expr: KExpression<'a>,
    ret: KType<'a>,
    kind: CarrierKind,
) -> BodyResult<'a> {
    let mut elaborator = Elaborator::new(scope);
    match parse_typed_field_list_via_elaborator(
        &sig_expr,
        kind.context(),
        kind.field_name_kind(),
        &mut elaborator,
        None,
    ) {
        FieldListOutcome::Done(fields) => {
            BodyResult::Value(finalize_carrier(scope, fields, ret, kind))
        }
        FieldListOutcome::Err(msg) => err(KError::new(KErrorKind::ShapeError(msg))),
        // An anonymous function/functor type has no self-reference binder, so the
        // deferral threads no name and carries no pending-binder guard.
        FieldListOutcome::Pending {
            park_producers,
            sub_dispatches,
        } => defer_field_list_via_combine(
            scope,
            sched,
            sig_expr,
            park_producers,
            sub_dispatches,
            kind.context(),
            kind.field_name_kind(),
            Vec::new(),
            None,
            None,
            None,
            Box::new(move |scope, fields| {
                BodyResult::Value(finalize_carrier(scope, fields, ret, kind))
            }),
        ),
    }
}

/// Fold the elaborated `(name, type)` pairs into the parameter record and wrap the
/// `KFunction` / `KFunctor` identity in a `KTypeValue`. Shared by the synchronous and
/// Combine-finish paths.
fn finalize_carrier<'a>(
    scope: &'a Scope<'a>,
    fields: Vec<(String, KType<'a>)>,
    ret: KType<'a>,
    kind: CarrierKind,
) -> &'a KObject<'a> {
    let record = Record::from_pairs(fields);
    let kt = match kind {
        // A `:(FUNCTOR …)` type-position annotation is a shape, not a bound
        // callable, so it carries no body.
        CarrierKind::Functor => KType::KFunctor {
            params: record,
            ret: Box::new(ret),
            body: None,
        },
        CarrierKind::Function => KType::KFunction {
            params: record,
            ret: Box::new(ret),
        },
    };
    scope.arena.alloc_object(KObject::KTypeValue(kt))
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "LIST",
        sig(
            KType::Type,
            vec![kw("LIST"), kw("OF"), arg("elem", KType::Type)],
        ),
        body_list_of,
    );
    register_builtin(
        scope,
        "MAP",
        sig(
            KType::Type,
            vec![
                kw("MAP"),
                arg("k", KType::Type),
                kw("->"),
                arg("v", KType::Type),
            ],
        ),
        body_map,
    );
    register_builtin(
        scope,
        "AS",
        sig(
            KType::Type,
            vec![
                arg("applied", KType::Type),
                kw("AS"),
                arg("ctor", KType::Type),
            ],
        ),
        body_apply_as,
    );
    register_builtin(
        scope,
        "FN",
        sig(
            KType::Type,
            vec![
                kw("FN"),
                arg("sig", KType::KExpression),
                kw("->"),
                arg("ret", KType::Type),
            ],
        ),
        body_fn,
    );
    register_builtin(
        scope,
        "FUNCTOR",
        sig(
            KType::Type,
            vec![
                kw("FUNCTOR"),
                arg("sig", KType::KExpression),
                kw("->"),
                arg("ret", KType::Type),
            ],
        ),
        body_functor,
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run_one, run_root_silent};
    use crate::machine::model::{KObject, KType, Record};
    use crate::machine::{RuntimeArena, Scope};

    #[test]
    fn list_of_number_lowers_to_list_number() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one(":(LIST OF Number)"));
        match result {
            KObject::KTypeValue(kt) => {
                assert_eq!(*kt, KType::List(Box::new(KType::Number)));
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    // A root-scope-bound `Wrap` TypeConstructor applied with `:(Number AS Wrap)`
    // lowers to `ConstructorApply(Wrap, [Number])`.
    #[test]
    fn apply_as_lowers_to_constructor_apply() {
        use crate::machine::model::types::{NominalKind, NominalSchema, RecursiveSet};
        use crate::machine::{BindingIndex, ScopeId};
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let wrap_set = RecursiveSet::singleton(
            "Wrap".into(),
            ScopeId::from_raw(0, 0xC0DE),
            NominalSchema::TypeConstructor {
                schema: std::collections::HashMap::new(),
                param_names: vec!["Type".into()],
            },
        );
        scope.register_type(
            "Wrap".into(),
            KType::SetRef {
                set: wrap_set,
                index: 0,
            },
            BindingIndex::BUILTIN,
        );
        let result = run_one(scope, parse_one(":(Number AS Wrap)"));
        match result {
            KObject::KTypeValue(KType::ConstructorApply { ctor, args }) => {
                match ctor.as_ref() {
                    KType::SetRef { set, index } => {
                        assert_eq!(set.member(*index).kind, NominalKind::TypeConstructor);
                    }
                    other => panic!("expected SetRef ctor, got {other:?}"),
                }
                assert_eq!(*args, vec![KType::Number]);
            }
            other => panic!(
                "expected ConstructorApply KTypeValue, got {:?}",
                other.ktype()
            ),
        }
    }

    #[test]
    fn map_str_number_lowers_to_dict() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one(":(MAP Str -> Number)"));
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

    #[test]
    fn fn_lowers_to_kfunction() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one(":(FN (x :Number, y :Str) -> Bool)"));
        match result {
            KObject::KTypeValue(kt) => {
                assert_eq!(
                    *kt,
                    KType::KFunction {
                        params: Record::from_pairs(vec![
                            ("x".into(), KType::Number),
                            ("y".into(), KType::Str),
                        ]),
                        ret: Box::new(KType::Bool),
                    }
                );
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    #[test]
    fn fn_nullary_lowers_to_kfunction() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one(":(FN () -> Number)"));
        match result {
            KObject::KTypeValue(kt) => {
                assert_eq!(
                    *kt,
                    KType::KFunction {
                        params: Record::new(),
                        ret: Box::new(KType::Number),
                    }
                );
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    // Param name `Ty` uses two letters because koan rejects single-uppercase-letter tokens.
    #[test]
    fn functor_lowers_to_kfunctor() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one(":(FUNCTOR (Ty :Signature) -> Module)"));
        match result {
            KObject::KTypeValue(kt) => {
                assert_eq!(
                    *kt,
                    KType::KFunctor {
                        params: Record::from_pairs(vec![("Ty".into(), KType::AnySignature)]),
                        ret: Box::new(KType::AnyModule),
                        body: None,
                    }
                );
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// A nested parameterized param type (`:(LIST OF Number)`) sub-Dispatches through the
    /// shared field-list parser and lands in the parameter record.
    #[test]
    fn fn_with_nested_list_param_lowers_to_kfunction() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one(":(FN (xs :(LIST OF Number)) -> Bool)"));
        match result {
            KObject::KTypeValue(kt) => {
                assert_eq!(
                    *kt,
                    KType::KFunction {
                        params: Record::from_pairs(vec![(
                            "xs".into(),
                            KType::List(Box::new(KType::Number)),
                        )]),
                        ret: Box::new(KType::Bool),
                    }
                );
            }
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// `t.name()` round-trips: rendering `expected` and re-running its surface form yields
    /// a `KTypeValue` equal to `expected`. The expected value is built at each call site so
    /// it shares the scope's lifetime, keeping the comparison off `'static`.
    fn assert_round_trips<'a>(scope: &'a Scope<'a>, expected: KType<'a>) {
        let rendered = expected.name();
        let result = run_one(scope, parse_one(&rendered));
        match result {
            KObject::KTypeValue(kt) => assert_eq!(
                *kt, expected,
                "round-trip of `{rendered}` did not reproduce the original KType",
            ),
            other => panic!(
                "expected KTypeValue from `{rendered}`, got {:?}",
                other.ktype()
            ),
        }
    }

    #[test]
    fn fn_multi_param_round_trips() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        assert_round_trips(
            scope,
            KType::KFunction {
                params: Record::from_pairs(vec![
                    ("x".into(), KType::Number),
                    ("y".into(), KType::Str),
                ]),
                ret: Box::new(KType::Bool),
            },
        );
    }

    #[test]
    fn fn_nullary_round_trips() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        assert_round_trips(
            scope,
            KType::KFunction {
                params: Record::new(),
                ret: Box::new(KType::Any),
            },
        );
    }

    #[test]
    fn fn_nested_param_round_trips() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        assert_round_trips(
            scope,
            KType::KFunction {
                params: Record::from_pairs(vec![(
                    "xs".into(),
                    KType::List(Box::new(KType::Number)),
                )]),
                ret: Box::new(KType::Bool),
            },
        );
    }

    #[test]
    fn functor_capitalized_param_round_trips_and_preserves_name() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let expected = KType::KFunctor {
            params: Record::from_pairs(vec![("Ty".into(), KType::AnySignature)]),
            ret: Box::new(KType::AnyModule),
            body: None,
        };
        // Param name `Ty` (capitalized, a `Type` token) must survive the round-trip.
        assert!(matches!(&expected, KType::KFunctor { params, .. } if params.get("Ty").is_some()),);
        assert_round_trips(scope, expected);
    }
}
