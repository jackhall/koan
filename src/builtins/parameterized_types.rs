//! Keyworded parameterized-type constructor builtins reached through the `:(...)` sigil.
//! See [type-language-via-dispatch](../../design/typing/type-language-via-dispatch.md).
//!
//! - `LIST OF :Type` → `Carried::Type(KType::List(_))`
//! - `MAP :Type -> :Type` → `Carried::Type(KType::Dict(_, _))`
//! - `FN <sig> -> :Type` → `Carried::Type(KType::KFunction { .. })`
//! - `FUNCTOR <sig> -> :Type` → `Carried::Type(KType::KFunctor { .. })`
//!
//! Fully-uppercase head keywords keep parameterized-type construction in
//! narrow candidate buckets so user-defined functors overloading short
//! connector words like `OF` don't pay a bucket-walk cost on every dispatched
//! parameterized type.

use crate::machine::model::types::KKind;
use crate::machine::model::types::{
    parse_typed_field_list_via_elaborator, Elaborator, FieldListOutcome, FieldNameKind,
};
use crate::machine::model::values::Carried;
use crate::machine::model::{KType, Record};
use crate::machine::{DeliveredCarried, KError, KErrorKind, Scope};

use super::{arg, kw, sig};
use crate::machine::execute::{
    defer_field_list_action_composed, fold_field_list_sync, BrandCompose,
};

/// Which carrier the shared field-list path builds. All three ride the same parser and
/// dep-finish/defer machinery; they differ only in the `KType` they fold their fields into,
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

/// Fold the elaborated `(name, type)` pairs into the parameter record and wrap the
/// `KFunction` / `KFunctor` identity in a `Carried::Type`. Shared by the synchronous and
/// dep-finish paths.
fn finalize_carrier<'a>(
    fields: Vec<(String, KType<'a>)>,
    ret: KType<'a>,
    kind: CarrierKind,
) -> KType<'a> {
    let record = Record::from_pairs(fields);
    match kind {
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
    }
}

/// `Action`-harness twins of the type-constructor bodies. LIST/MAP/AS fold resolved type args
/// directly (`Done`); FN/FUNCTOR route the parameter list through [`build_carrier`], which
/// either folds synchronously or defers via [`defer_field_list_action`].
mod action_bodies {
    use super::{build_carrier, CarrierKind};
    use crate::machine::core::kfunction::action::{require_ktype, Action, BodyCtx};
    use crate::machine::model::types::{KKind, ProjectedSchema, RecursiveSet};

    use crate::machine::model::KType;
    use crate::machine::{KError, KErrorKind};

    /// LIST / MAP / AS build their composite `KType` at the fold brand from a total,
    /// embedding-ordered operand list: one operand per embedded arg (`elem` / `k` / `v` /
    /// `applied` / `ctor`), each produced by [`BodyCtx::type_operand`]. An arg that resolved to
    /// a carrier-bearing value crosses the fold as that carrier's view, folding its reach into
    /// the result's witness; a region-free arg (a scalar-literal type token) rebuilds at the
    /// brand with no reach contribution. The `compose` function receives one `&KType` per
    /// operand, positionally, and assembles the composite type from those parts alone.
    pub(super) fn body_list_of<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
        let elem = crate::try_action!(require_ktype(ctx.args, "elem"));
        let operands = vec![crate::try_action!(ctx.type_operand("elem", &elem))];
        Action::Done(Ok(ctx
            .ctx
            .alloc_type_composed(operands, |_brand, parts| {
                KType::List(Box::new(parts[0].clone()))
            })))
    }

    pub(super) fn body_map<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
        let k = crate::try_action!(require_ktype(ctx.args, "k"));
        let v = crate::try_action!(require_ktype(ctx.args, "v"));
        let operands = vec![
            crate::try_action!(ctx.type_operand("k", &k)),
            crate::try_action!(ctx.type_operand("v", &v)),
        ];
        Action::Done(Ok(ctx
            .ctx
            .alloc_type_composed(operands, |_brand, parts| {
                KType::Dict(Box::new(parts[0].clone()), Box::new(parts[1].clone()))
            })))
    }

    pub(super) fn body_apply_as<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
        let applied = crate::try_action!(require_ktype(ctx.args, "applied"));
        let ctor = crate::try_action!(require_ktype(ctx.args, "ctor"));
        let param_count = match &ctor {
            KType::SetRef { set, index } if set.member(*index).kind == KKind::TypeConstructor => {
                match RecursiveSet::projected_schema(set, *index) {
                    ProjectedSchema::TypeConstructor { param_names, .. } => param_names.len(),
                    _ => unreachable!(
                        "TypeConstructor-kind member projects a TypeConstructor schema"
                    ),
                }
            }
            other => {
                return Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                    "right-hand side of `AS` must be a type constructor, got `{}`",
                    other.name(),
                )))))
            }
        };
        if param_count != 1 {
            return Action::Done(Err(KError::new(KErrorKind::ShapeError(format!(
                "`{}` takes {param_count} type arguments; the `AS` form supplies one, so \
                 multi-parameter application is not yet supported",
                ctor.name(),
            )))));
        }
        let operands = vec![
            crate::try_action!(ctx.type_operand("applied", &applied)),
            crate::try_action!(ctx.type_operand("ctor", &ctor)),
        ];
        Action::Done(Ok(ctx.ctx.alloc_type_composed(
            operands,
            |_brand, parts| KType::ConstructorApply {
                ctor: Box::new(parts[1].clone()),
                args: vec![parts[0].clone()],
            },
        )))
    }

    pub(super) fn body_fn<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
        build_carrier(ctx, "sig", "ret", CarrierKind::Function)
    }

    pub(super) fn body_functor<'a>(ctx: &BodyCtx<'a, '_>) -> Action<'a> {
        build_carrier(ctx, "sig", "ret", CarrierKind::Functor)
    }
}

/// The return-type operand crossing shared by [`build_carrier`]'s sync and deferred arms: the ret
/// crosses the fold as a carrier view when it has one, rebuilds at the brand from its `'static`
/// value when region-free, and errors loudly when it reaches a region carrier-less. The composed
/// carrier folds its params and the crossed return type into a `KFunction` / `KFunctor` at the brand.
fn ret_extras_and_compose<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
    ret_slot: &str,
    ret: KType<'a>,
    kind: CarrierKind,
) -> Result<(Vec<DeliveredCarried>, BrandCompose<'a>), KError> {
    // A ret that arrived as a resolved value carries a reach carrier (duplicated so the producer
    // keeps its terminal); a region-free token like `Bool` has none and rebuilds from its `'static`
    // value at the brand. A ret that reaches a region but arrived carrier-less cannot be crossed as
    // an operand, so it errors loudly.
    let ret_carrier: Option<DeliveredCarried> = ctx.arg_carrier(ret_slot).map(|d| d.duplicate());
    let ret_static: Option<KType<'static>> = ret.to_static();
    if ret_carrier.is_none() && ret_static.is_none() {
        return Err(KError::new(KErrorKind::ShapeError(
            "FN/FUNCTOR return type reaches a region but arrived without a carrier".into(),
        )));
    }
    let extras: Vec<DeliveredCarried> = ret_carrier.into_iter().collect();
    let compose: BrandCompose<'a> = Box::new(move |brand, fields, extra_views| {
        let ret = match extra_views.first().copied() {
            // The ret crossed as a dep view: its type is already at the brand, and its reach/host
            // fold into the result's witness.
            Some(Carried::Type(kt)) => kt.clone(),
            Some(other @ Carried::Object(_)) => {
                return Err(KError::new(KErrorKind::ShapeError(format!(
                    "FN/FUNCTOR return slot resolved to non-type value `{}`",
                    other.summarize(),
                ))))
            }
            // Region-free ret with no carrier (a builtin type token like `Bool`): rebuild it at the
            // brand through the region's own `'static` alloc door and clone it back out at the fold
            // lifetime.
            None => brand.alloc_ktype(ret_static.expect("gated above")).clone(),
        };
        Ok(finalize_carrier(fields, ret, kind))
    });
    Ok((extras, compose))
}

/// Walk the parameter list through the shared field-list parser (the same one UNION / NEWTYPE use),
/// so nested parameterized param types like `xs :(LIST OF Number)` sub-Dispatch and capitalized
/// FUNCTOR param names like `Ty` are accepted. Folds synchronously or defers via
/// [`defer_field_list_action`] (no self-reference binder, no pending guard).
fn build_carrier<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
    sig_slot: &str,
    ret_slot: &str,
    kind: CarrierKind,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::{require_kexpression, require_ktype, Action};
    let sig_expr = crate::try_action!(require_kexpression(ctx.args, "FN", sig_slot));
    let ret = crate::try_action!(require_ktype(ctx.args, ret_slot));
    let mut elaborator = Elaborator::new(ctx.scope);
    match parse_typed_field_list_via_elaborator(
        &sig_expr,
        kind.context(),
        kind.field_name_kind(),
        &mut elaborator,
        None,
    ) {
        FieldListOutcome::Done(fields) => {
            let kt = finalize_carrier(fields, ret.clone(), kind);
            match kt.to_static() {
                // Region-free carrier: the compile-enforced `'static` tier.
                Some(owned) => Action::Done(Ok(ctx.ctx.alloc_type(owned))),
                // A param or return type that cannot rebuild at `'static`: discard the ambient walk
                // and re-fold at the brand, where the scope reads are declared operands and the
                // return type crosses as an extras operand.
                None => {
                    let (extras, compose) =
                        crate::try_action!(ret_extras_and_compose(ctx, ret_slot, ret, kind));
                    let extra_refs: Vec<&DeliveredCarried> = extras.iter().collect();
                    Action::Done(fold_field_list_sync(
                        &ctx.ctx,
                        ctx.scope,
                        sig_expr,
                        kind.context(),
                        kind.field_name_kind(),
                        Vec::new(),
                        None,
                        None,
                        &extra_refs,
                        compose,
                    ))
                }
            }
        }
        FieldListOutcome::Err(msg) => Action::Done(Err(KError::new(KErrorKind::ShapeError(msg)))),
        FieldListOutcome::Pending {
            park_producers,
            sub_dispatches,
        } => {
            let (extras, compose) =
                crate::try_action!(ret_extras_and_compose(ctx, ret_slot, ret, kind));
            defer_field_list_action_composed(
                sig_expr,
                park_producers,
                sub_dispatches,
                kind.context(),
                kind.field_name_kind(),
                Vec::new(),
                None,
                None,
                None,
                extras,
                compose,
            )
        }
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    use crate::builtins::register_builtin;
    register_builtin(
        scope,
        "LIST",
        sig(
            KType::OfKind(KKind::AnyType),
            vec![
                kw("LIST"),
                kw("OF"),
                arg("elem", KType::OfKind(KKind::AnyType)),
            ],
        ),
        action_bodies::body_list_of,
    );
    register_builtin(
        scope,
        "MAP",
        sig(
            KType::OfKind(KKind::AnyType),
            vec![
                kw("MAP"),
                arg("k", KType::OfKind(KKind::AnyType)),
                kw("->"),
                arg("v", KType::OfKind(KKind::AnyType)),
            ],
        ),
        action_bodies::body_map,
    );
    register_builtin(
        scope,
        "AS",
        sig(
            KType::OfKind(KKind::AnyType),
            vec![
                arg("applied", KType::OfKind(KKind::AnyType)),
                kw("AS"),
                arg("ctor", KType::OfKind(KKind::AnyType)),
            ],
        ),
        action_bodies::body_apply_as,
    );
    register_builtin(
        scope,
        "FN",
        sig(
            KType::OfKind(KKind::AnyType),
            vec![
                kw("FN"),
                arg("sig", KType::KExpression),
                kw("->"),
                arg("ret", KType::OfKind(KKind::AnyType)),
            ],
        ),
        action_bodies::body_fn,
    );
    register_builtin(
        scope,
        "FUNCTOR",
        sig(
            KType::OfKind(KKind::AnyType),
            vec![
                kw("FUNCTOR"),
                arg("sig", KType::KExpression),
                kw("->"),
                arg("ret", KType::OfKind(KKind::AnyType)),
            ],
        ),
        action_bodies::body_functor,
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{
        parse_one, run, run_one_err, run_one_type, run_root_silent,
    };
    use crate::machine::core::run_root_storage;
    use crate::machine::core::StoredReach;
    use crate::machine::model::{KKind, KType, Record};
    use crate::machine::{KErrorKind, Scope};

    #[test]
    fn list_of_number_lowers_to_list_number() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let result = run_one_type(scope, parse_one(":(LIST OF Number)"));
        assert_eq!(*result, KType::List(Box::new(KType::Number)));
    }

    // A root-scope-bound `Wrap` TypeConstructor applied with `:(Number AS Wrap)`
    // lowers to `ConstructorApply(Wrap, [Number])`.
    #[test]
    fn apply_as_lowers_to_constructor_apply() {
        use crate::machine::model::types::{KKind, NominalSchema, RecursiveSet};
        use crate::machine::{BindingIndex, ScopeId};
        let region = run_root_storage();
        let scope = run_root_silent(&region);
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
            StoredReach::empty(),
        );
        let result = run_one_type(scope, parse_one(":(Number AS Wrap)"));
        match result {
            KType::ConstructorApply { ctor, args } => {
                match ctor.as_ref() {
                    KType::SetRef { set, index } => {
                        assert_eq!(set.member(*index).kind, KKind::TypeConstructor);
                    }
                    other => panic!("expected SetRef ctor, got {other:?}"),
                }
                assert_eq!(*args, vec![KType::Number]);
            }
            other => panic!("expected ConstructorApply, got {other:?}"),
        }
    }

    #[test]
    fn map_str_number_lowers_to_dict() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let result = run_one_type(scope, parse_one(":(MAP Str -> Number)"));
        assert_eq!(
            *result,
            KType::Dict(Box::new(KType::Str), Box::new(KType::Number))
        );
    }

    #[test]
    fn fn_lowers_to_kfunction() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let result = run_one_type(scope, parse_one(":(FN (x :Number, y :Str) -> Bool)"));
        assert_eq!(
            *result,
            KType::KFunction {
                params: Record::from_pairs(vec![
                    ("x".into(), KType::Number),
                    ("y".into(), KType::Str),
                ]),
                ret: Box::new(KType::Bool),
            }
        );
    }

    #[test]
    fn fn_nullary_lowers_to_kfunction() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let result = run_one_type(scope, parse_one(":(FN () -> Number)"));
        assert_eq!(
            *result,
            KType::KFunction {
                params: Record::new(),
                ret: Box::new(KType::Number),
            }
        );
    }

    // Param name `Ty` uses two letters because koan rejects single-uppercase-letter tokens.
    #[test]
    fn functor_lowers_to_kfunctor() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let result = run_one_type(scope, parse_one(":(FUNCTOR (Ty :Signature) -> Module)"));
        assert_eq!(
            *result,
            KType::KFunctor {
                params: Record::from_pairs(vec![("Ty".into(), KType::OfKind(KKind::Signature))]),
                ret: Box::new(KType::OfKind(KKind::Module)),
                body: None,
            }
        );
    }

    /// A nested parameterized param type (`:(LIST OF Number)`) sub-Dispatches through the
    /// shared field-list parser and lands in the parameter record.
    #[test]
    fn fn_with_nested_list_param_lowers_to_kfunction() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let result = run_one_type(scope, parse_one(":(FN (xs :(LIST OF Number)) -> Bool)"));
        assert_eq!(
            *result,
            KType::KFunction {
                params: Record::from_pairs(vec![(
                    "xs".into(),
                    KType::List(Box::new(KType::Number)),
                )]),
                ret: Box::new(KType::Bool),
            }
        );
    }

    /// A `:{…}` record type that mixes a scope-alias field (`:Wrapped`, resolved from the crossed
    /// scope during the deferred re-walk) with a sigil field (`:(LIST OF Number)`, which forces
    /// deferral) composes its `KType::Record` at the fold brand with both field types resolved. The
    /// scope-alias field reads through the brand-delivered scope envelope; the sigil field pops its
    /// sub-Dispatch carrier from the fed views.
    #[test]
    fn record_sigil_defers_and_mixes_scope_read_with_sub_dispatch() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "LET Wrapped = :{a :Number}");
        let result = run_one_type(scope, parse_one(":{x :Wrapped, y :(LIST OF Number)}"));
        assert_eq!(
            *result,
            KType::Record(Box::new(Record::from_pairs(vec![
                (
                    "x".into(),
                    KType::Record(Box::new(Record::from_pairs(vec![(
                        "a".into(),
                        KType::Number,
                    )]))),
                ),
                ("y".into(), KType::List(Box::new(KType::Number))),
            ]))),
        );
    }

    /// A deferred FN (the `:(LIST OF Number)` param forces deferral) whose return type names a
    /// `NEWTYPE` alias — a `SetRef` that is not region-free, so it cannot be rebuilt from a `'static`
    /// value — composes its `KType::KFunction` by cloning the return type out of its own carrier
    /// view (the `Some(Carried::Type(_))` compose arm). Were the return type to arrive without a
    /// carrier, `build_carrier`'s guard would error instead of producing a function type, so a
    /// successful compose proves the carrier-view path ran.
    #[test]
    fn fn_deferred_with_reaching_ret_composes_from_carrier_view() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "NEWTYPE Wrapped = :{a :Number}");
        let result = run_one_type(scope, parse_one(":(FN (xs :(LIST OF Number)) -> Wrapped)"));
        match result {
            KType::KFunction { params, ret } => {
                assert_eq!(
                    params.get("xs"),
                    Some(&KType::List(Box::new(KType::Number))),
                    "the sigil param must lower to LIST OF Number",
                );
                assert_eq!(
                    ret.name(),
                    "Wrapped",
                    "the reaching return type must survive the carrier-view crossing",
                );
            }
            other => panic!("expected a KFunction carrier, got {other:?}"),
        }
    }

    /// A deferred record field whose sigil sub-Dispatch resolves to a non-type value (`:(1)` → the
    /// number `1`) surfaces the walker's shape error through `fold_fields_at_brand`'s side-channel:
    /// the fold closure stores a placeholder type and re-raises the stashed error after the alloc.
    #[test]
    fn record_field_sub_dispatch_to_non_type_value_errors() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let err = run_one_err(scope, parse_one(":{x :(1)}"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("resolved to non-type value")),
            "expected a non-type-value ShapeError through the deferred side-channel, got {err}",
        );
    }

    /// `t.name()` round-trips: rendering `expected` and re-running its surface form yields
    /// a type carrier equal to `expected`. The expected value is built at each call site so
    /// it shares the scope's lifetime, keeping the comparison off `'static`.
    fn assert_round_trips<'a>(scope: &'a Scope<'a>, expected: KType<'a>) {
        let rendered = expected.name();
        let result = run_one_type(scope, parse_one(&rendered));
        assert_eq!(
            *result, expected,
            "round-trip of `{rendered}` did not reproduce the original KType",
        );
    }

    #[test]
    fn fn_multi_param_round_trips() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
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
        let region = run_root_storage();
        let scope = run_root_silent(&region);
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
        let region = run_root_storage();
        let scope = run_root_silent(&region);
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
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        let expected = KType::KFunctor {
            params: Record::from_pairs(vec![("Ty".into(), KType::OfKind(KKind::Signature))]),
            ret: Box::new(KType::OfKind(KKind::Module)),
            body: None,
        };
        // Param name `Ty` (capitalized, a `Type` token) must survive the round-trip.
        assert!(matches!(&expected, KType::KFunctor { params, .. } if params.get("Ty").is_some()),);
        assert_round_trips(scope, expected);
    }

    /// `:(MAP Str -> Wrapped)` correlates a scalar-literal key with no carrier (`k`) and a
    /// reaching value type (`v`) by total operand position, not by carrier presence: the scalar
    /// lands in `k` and the reaching type survives the carrier-view crossing into `v`.
    #[test]
    fn map_scalar_key_reaching_value_correlates() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "NEWTYPE Wrapped = :{a :Number}");
        let result = run_one_type(scope, parse_one(":(MAP Str -> Wrapped)"));
        match result {
            KType::Dict(k, v) => {
                assert_eq!(**k, KType::Str, "scalar key must lower to Str");
                assert_eq!(
                    v.name(),
                    "Wrapped",
                    "reaching value type must survive the carrier-view crossing",
                );
            }
            other => panic!("expected a Dict carrier, got {other:?}"),
        }
    }

    /// Mirror of `map_scalar_key_reaching_value_correlates`: the reaching type lands in `k` and
    /// the scalar lands in `v`, proving the correlation is positional, not carrier-presence-based.
    #[test]
    fn map_reaching_key_scalar_value_correlates() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "NEWTYPE Wrapped = :{a :Number}");
        let result = run_one_type(scope, parse_one(":(MAP Wrapped -> Str)"));
        match result {
            KType::Dict(k, v) => {
                assert_eq!(
                    k.name(),
                    "Wrapped",
                    "reaching key type must survive the carrier-view crossing",
                );
                assert_eq!(**v, KType::Str, "scalar value must lower to Str");
            }
            other => panic!("expected a Dict carrier, got {other:?}"),
        }
    }

    /// A sync record type whose field names a `NEWTYPE` alias (`:Wrapped`, a `SetRef` that never
    /// rebuilds at `'static`) resolves in one ambient walk — no sigil field forces deferral — so its
    /// `KType::Record` composes through the sync `to_static`-declines path: the ambient pairs are
    /// discarded and the record is re-folded at the brand, where the reaching field survives.
    #[test]
    fn record_sync_reaching_field_folds_at_brand() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "NEWTYPE Wrapped = :{a :Number}");
        let result = run_one_type(scope, parse_one(":{x :Wrapped}"));
        match result {
            KType::Record(record) => {
                let field = record.get("x").expect("record must have field x");
                assert_eq!(
                    field.name(),
                    "Wrapped",
                    "the reaching field must survive the sync brand re-fold",
                );
            }
            other => panic!("expected a Record, got {other:?}"),
        }
    }

    /// A sync FN whose parameter type reaches a region (`x :Wrapped`, a `NEWTYPE` `SetRef`) resolves
    /// in one ambient walk, so its `KType::KFunction` composes through the sync brand re-fold: the
    /// reaching param survives and the region-free `Bool` return type rebuilds at the brand.
    #[test]
    fn fn_sync_reaching_param_folds_at_brand() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "NEWTYPE Wrapped = :{a :Number}");
        let result = run_one_type(scope, parse_one(":(FN (x :Wrapped) -> Bool)"));
        match result {
            KType::KFunction { params, ret } => {
                assert_eq!(
                    params.get("x").map(|kt| kt.name()),
                    Some("Wrapped".to_string()),
                    "the reaching param must survive the sync brand re-fold",
                );
                assert_eq!(
                    **ret,
                    KType::Bool,
                    "the region-free return type must be Bool"
                );
            }
            other => panic!("expected a KFunction, got {other:?}"),
        }
    }

    /// A sync FN whose return type reaches a region (`-> Wrapped`) resolves in one ambient walk, so
    /// its `KType::KFunction` composes through the sync brand re-fold: the reaching return type
    /// crosses as an extras operand (its carrier view) and survives.
    #[test]
    fn fn_sync_reaching_ret_folds_at_brand() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "NEWTYPE Wrapped = :{a :Number}");
        let result = run_one_type(scope, parse_one(":(FN (x :Number) -> Wrapped)"));
        match result {
            KType::KFunction { params, ret } => {
                assert_eq!(
                    params.get("x"),
                    Some(&KType::Number),
                    "the region-free param must be Number",
                );
                assert_eq!(
                    ret.name(),
                    "Wrapped",
                    "the reaching return type must survive the extras-operand crossing",
                );
            }
            other => panic!("expected a KFunction, got {other:?}"),
        }
    }

    /// `:(LIST OF Wrapped)` lowers with the reaching elem type surviving the carrier-view
    /// crossing (the single-operand analog of the MAP correlation tests above).
    #[test]
    fn list_of_reaching_elem_lowers() {
        let region = run_root_storage();
        let scope = run_root_silent(&region);
        run(scope, "NEWTYPE Wrapped = :{a :Number}");
        let result = run_one_type(scope, parse_one(":(LIST OF Wrapped)"));
        match result {
            KType::List(elem) => {
                assert_eq!(
                    elem.name(),
                    "Wrapped",
                    "reaching elem type must survive the carrier-view crossing",
                );
            }
            other => panic!("expected a List carrier, got {other:?}"),
        }
    }
}
