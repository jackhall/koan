//! Keyworded parameterized-type constructor builtins reached through the `:(...)` sigil.
//! See [type-language-via-dispatch](../../design/typing/type-language-via-dispatch.md).
//!
//! - `LIST OF :Type` → `Carried::Type` of an interned `List` handle
//! - `MAP :Type -> :Type` → `Carried::Type` of an interned `Dict` handle
//! - `FN <sig> -> :Type` → `Carried::Type` of an interned function-type handle
//!
//! Fully-uppercase head keywords keep parameterized-type construction in
//! narrow candidate buckets so user-defined overloads of short connector words
//! like `OF` don't pay a bucket-walk cost on every dispatched parameterized type.

use crate::machine::WriteGate;
use crate::machine::model::KKind;
use crate::machine::model::KType;
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, kw, sig};
use crate::machine::model::RunRegistries;

// This builtin's slot spellings, minted once and read back by symbol.
crate::slots! { SLOTS { applied, ctor, elem, k, ret, sig, v } }

/// Reject a bare type constructor in a type-language argument position that demands kind `*`:
/// a list's element, a dict's key and value, a function type's return. Each names the type of a
/// value, so each must be a proper type. An `:(FN …)` parameter list is a record type, whose own
/// elaboration checks its fields.
fn require_proper_type(
    kt: KType,
    position: &str,
    registries: &crate::machine::model::RunRegistries,
) -> Result<(), KError> {
    match crate::machine::model::unsaturated_constructor_message(kt, position, registries) {
        Some(message) => Err(KError::new(KErrorKind::ShapeError(message))),
        None => Ok(()),
    }
}

/// `Action`-harness twins of the type-constructor bodies. Each composes from resolved type args
/// directly (`Done`) — including FN, whose parameter list arrives as an already-resolved record
/// type.
mod action_bodies {
    use super::SLOTS;
    use super::require_proper_type;
    use crate::machine::model::BinderSymbol;
    use crate::machine::model::TypeNode;
    use crate::machine::model::constructor_param_names;
    use crate::machine::{Action, BodyCtx, require_ktype};

    use crate::machine::model::Record;
    use crate::machine::{KError, KErrorKind};

    /// LIST / MAP / AS read each embedded arg (`elem` / `k` / `v` / `applied` / `ctor`) as an
    /// owned `KType` and assemble the composite from those values, then allocate it into the
    /// step's own region through the single type door.
    pub(super) fn body_list_of<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
        let elem = crate::try_action!(require_ktype(ctx.args, &SLOTS.elem, ctx.registries));
        crate::try_action!(require_proper_type(
            elem,
            "the element type of `LIST OF`",
            ctx.registries
        ));
        let list = ctx.types().list(elem);
        Action::done(Ok(ctx.ctx.type_carried(list)))
    }

    pub(super) fn body_map<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
        let k = crate::try_action!(require_ktype(ctx.args, &SLOTS.k, ctx.registries));
        let v = crate::try_action!(require_ktype(ctx.args, &SLOTS.v, ctx.registries));
        crate::try_action!(require_proper_type(
            k,
            "the key type of `MAP`",
            ctx.registries
        ));
        crate::try_action!(require_proper_type(
            v,
            "the value type of `MAP`",
            ctx.registries
        ));
        let dict = ctx.types().dict(k, v);
        Action::done(Ok(ctx.ctx.type_carried(dict)))
    }

    pub(super) fn body_apply_as<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
        let applied = crate::try_action!(require_ktype(ctx.args, &SLOTS.applied, ctx.registries));
        let ctor = crate::try_action!(require_ktype(ctx.args, &SLOTS.ctor, ctx.registries));
        // A declared family and a SLOTS.sig's abstract constructor slot both name their parameters.
        let Some(param_names) = constructor_param_names(ctor, ctx.types()) else {
            return Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
                "right-hand side of `AS` must be a type constructor, got `{}`",
                ctor.display_name(ctx.registries),
            )))));
        };
        let [param_name] = &param_names[..] else {
            return Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
                "`{}` takes {} type arguments; the `AS` form supplies one, so \
                 multi-parameter application is not yet supported",
                ctor.display_name(ctx.registries),
                param_names.len(),
            )))));
        };
        // `AS` is arity-1 sugar: the applied type fills the constructor's sole parameter, so
        // `:(Number AS Wrap)` elaborates exactly as `:(Wrap {Elem = Number})` does. The parameter
        // name is the type symbol its declaration minted, so it keys the record directly.
        let args = Record::from_pairs([(BinderSymbol::Type(*param_name), applied)]);
        let apply = ctx.types().constructor_apply(ctor, args);
        Action::done(Ok(ctx.ctx.type_carried(apply)))
    }

    /// The parameter list is a record type that resolved before `FN` dispatched — the `:{…}`
    /// operand sub-dispatches through the parameter-list slot, so a nested sigil param type or a
    /// reaching field resolves inside the record's own elaboration and nothing defers here. The
    /// kind lattice has no structural kinds, so record-ness is this body's check.
    pub(super) fn body_fn<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
        let params_kt = crate::try_action!(require_ktype(ctx.args, &SLOTS.sig, ctx.registries));
        let ret = crate::try_action!(require_ktype(ctx.args, &SLOTS.ret, ctx.registries));
        crate::try_action!(require_proper_type(
            ret,
            "the return type of an `:(FN …)` type",
            ctx.registries
        ));
        let params = ctx.types().with_node(params_kt, |node| match node {
            TypeNode::Record { fields } => Some(fields.clone()),
            _ => None,
        });
        let Some(params) = params else {
            return Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
                "the parameter list of an `:(FN …)` type must be a record type `:{{…}}`, got `{}`",
                params_kt.display_name(ctx.registries),
            )))));
        };
        let carrier = ctx.types().function_type(params, ret);
        Action::done(Ok(ctx.ctx.type_carried(carrier)))
    }
}

pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    use crate::builtins::register_builtin;
    register_builtin(
        scope,
        sig(
            KType::of_kind(KKind::AnyType),
            vec![
                kw(registries, "LIST"),
                kw(registries, "OF"),
                arg(registries, &SLOTS.elem, KType::of_kind(KKind::AnyType)),
            ],
        ),
        action_bodies::body_list_of,
        registries,
        gate,
    );
    register_builtin(
        scope,
        sig(
            KType::of_kind(KKind::AnyType),
            vec![
                kw(registries, "MAP"),
                arg(registries, &SLOTS.k, KType::of_kind(KKind::AnyType)),
                kw(registries, "->"),
                arg(registries, &SLOTS.v, KType::of_kind(KKind::AnyType)),
            ],
        ),
        action_bodies::body_map,
        registries,
        gate,
    );
    register_builtin(
        scope,
        sig(
            KType::of_kind(KKind::AnyType),
            vec![
                arg(registries, &SLOTS.applied, KType::of_kind(KKind::AnyType)),
                kw(registries, "AS"),
                arg(registries, &SLOTS.ctor, KType::of_kind(KKind::AnyType)),
            ],
        ),
        action_bodies::body_apply_as,
        registries,
        gate,
    );
    register_builtin(
        scope,
        sig(
            KType::of_kind(KKind::AnyType),
            vec![
                kw(registries, "FN"),
                // The parameter list is a record type, so it sub-dispatches to a resolved `KType`
                // before this body fires. `OfKind(AnyType)` is the widest carrier — it matches
                // every sibling slot here and, unlike `OfKind(ProperType)`, does not withhold the
                // bare-name auto-wrap a `LET`-bound alias operand needs
                // ([`classify_for_pick`](../machine/core/kfunction/pick.rs)). Everything it admits
                // beyond a record — a nominal handle, a signature kind — the body's record check
                // rejects with a pointed error.
                arg(registries, &SLOTS.sig, KType::of_kind(KKind::AnyType)),
                kw(registries, "->"),
                // `OfKind(AnyType)` admits every type value — a `-> Ordered` signature return
                // and `-> Module` (which lowers to the empty signature) included.
                arg(registries, &SLOTS.ret, KType::of_kind(KKind::AnyType)),
            ],
        ),
        action_bodies::body_fn,
        registries,
        gate,
    );
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{TestRun, type_name};
    use crate::machine::KErrorKind;
    use crate::machine::model::{KKind, KType, Record, TypeNode};
    use crate::machine::program_storage;
    use crate::machine::run_root_storage;

    #[test]
    fn list_of_number_lowers_to_list_number() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let result = test_run.run_one_type(test_run.parse_one(":(LIST OF Number)"));
        let types = test_run.types();
        assert_eq!(result, types.list(KType::NUMBER));
    }

    // A root-scope-bound `Wrap` TypeConstructor applied with `:(Number AS Wrap)`
    // lowers to `ConstructorApply(Wrap, {Type = Number})` — `AS` fills the sole parameter.
    #[test]
    fn apply_as_lowers_to_constructor_apply() {
        use crate::machine::model::{RecursiveGroupWindow, RelativeSchema};
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let scope = test_run.scope;
        // Seal a singleton `Wrap` constructor member through the real declaration window, then
        // bind its absolute handle as a builtin type.
        let window = RecursiveGroupWindow::new(vec![(
            type_name("Wrap", test_run.registries()),
            KKind::TypeConstructor,
        )]);
        let sealed = window
            .fill_member(
                0,
                RelativeSchema::TypeConstructor {
                    schema: crate::machine::model::TypeMemberMap::default(),
                    param_names: vec![type_name("Type", test_run.registries())],
                },
                test_run.types(),
            )
            .expect("a singleton window seals on its sole fill");
        scope.register_builtin_type(
            type_name("Wrap", test_run.registries()),
            sealed.members[0],
            test_run.registries(),
            &mut crate::machine::WriteGate::for_test(),
        );
        let result = test_run.run_one_type(test_run.parse_one(":(Number AS Wrap)"));
        let types = test_run.types();
        match types.node(result) {
            TypeNode::ConstructorApply {
                constructor,
                arguments,
            } => {
                match types.node(constructor) {
                    TypeNode::SetMember { kind, .. } => {
                        assert_eq!(kind, KKind::TypeConstructor);
                    }
                    _ => panic!("expected SetMember ctor, got {constructor:?}"),
                }
                assert_eq!(
                    arguments,
                    Record::from_pairs([(
                        crate::builtins::test_support::binder_name("Type", test_run.registries()),
                        KType::NUMBER
                    )]),
                );
            }
            _ => panic!("expected ConstructorApply, got {result:?}"),
        }
    }

    #[test]
    fn map_str_number_lowers_to_dict() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let result = test_run.run_one_type(test_run.parse_one(":(MAP Str -> Number)"));
        let types = test_run.types();
        assert_eq!(result, types.dict(KType::STR, KType::NUMBER));
    }

    #[test]
    fn fn_lowers_to_kfunction() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let result =
            test_run.run_one_type(test_run.parse_one(":(FN :{x :Number, y :Str} -> Bool)"));
        let types = test_run.types();
        assert_eq!(
            result,
            types.function_type(
                Record::from_pairs(vec![
                    (
                        crate::builtins::test_support::binder_name("x", test_run.registries()),
                        KType::NUMBER
                    ),
                    (
                        crate::builtins::test_support::binder_name("y", test_run.registries()),
                        KType::STR
                    )
                ]),
                KType::BOOL,
            )
        );
    }

    #[test]
    fn fn_nullary_lowers_to_kfunction() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let result = test_run.run_one_type(test_run.parse_one(":(FN :{} -> Number)"));
        let types = test_run.types();
        assert_eq!(result, types.function_type(Record::new(), KType::NUMBER));
    }

    /// A functor — a module-returning function — types as an ordinary `KFunction`.
    // Param name `Ty` uses two letters because koan rejects single-uppercase-letter tokens.
    #[test]
    fn fn_with_type_param_and_module_return_lowers_to_kfunction() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let result = test_run.run_one_type(test_run.parse_one(":(FN :{Ty :Signature} -> Module)"));
        let types = test_run.types();
        assert_eq!(
            result,
            types.function_type(
                Record::from_pairs(vec![(
                    crate::builtins::test_support::binder_name("Ty", test_run.registries()),
                    KType::of_kind(KKind::Signature)
                )]),
                KType::EMPTY_SIGNATURE,
            )
        );
    }

    /// The parameter-list slot takes any record-valued type expression, so a `LET`-bound alias
    /// names the parameter list as directly as a literal `:{…}` does.
    #[test]
    fn fn_with_alias_parameter_list_lowers_to_kfunction() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("LET Params = :{x :Number}");
        let result = test_run.run_one_type(test_run.parse_one(":(FN Params -> Bool)"));
        let types = test_run.types();
        assert_eq!(
            result,
            types.function_type(
                Record::from_pairs(vec![(
                    crate::builtins::test_support::binder_name("x", test_run.registries()),
                    KType::NUMBER
                )]),
                KType::BOOL,
            )
        );
    }

    /// The kind lattice has no structural kinds, so a non-record operand passes the slot and the
    /// body rejects it with a pointed shape error.
    #[test]
    fn fn_with_non_record_parameter_list_errors() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let err = test_run.run_one_err(test_run.parse_one(":(FN Number -> Str)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("must be a record type")),
            "expected a pointed non-record ShapeError, got {err}",
        );
    }

    /// A `NEWTYPE` over a record is a `SetMember`, not structurally a record, so it is not a
    /// parameter list — the nominal identity is the point of declaring one.
    #[test]
    fn fn_with_nominal_parameter_list_errors() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("NEWTYPE Wrapped = :{a :Number}");
        let err = test_run.run_one_err(test_run.parse_one(":(FN Wrapped -> Str)"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("must be a record type")),
            "expected a pointed non-record ShapeError, got {err}",
        );
    }

    /// The parenthesized parameter list is gone: `(x :Number)` is a parenthesized group, not a
    /// type expression, so no `FN` overload takes it.
    #[test]
    fn fn_with_parenthesized_parameter_list_no_longer_elaborates() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let err = test_run.run_one_err(test_run.parse_one(":(FN (x :Number) -> Bool)"));
        assert!(
            !matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("must be a record type")),
            "the parenthesized form must not reach the FN body at all, got {err}",
        );
    }

    /// A nested parameterized param type (`:(LIST OF Number)`) sub-Dispatches through the
    /// shared field-list parser and lands in the parameter record.
    #[test]
    fn fn_with_nested_list_param_lowers_to_kfunction() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let result =
            test_run.run_one_type(test_run.parse_one(":(FN :{xs :(LIST OF Number)} -> Bool)"));
        let types = test_run.types();
        assert_eq!(
            result,
            types.function_type(
                Record::from_pairs(vec![(
                    crate::builtins::test_support::binder_name("xs", test_run.registries()),
                    types.list(KType::NUMBER)
                )]),
                KType::BOOL,
            )
        );
    }

    /// A `:{…}` record type that mixes a scope-alias field (`:Wrapped`, resolved from the crossed
    /// scope during the deferred re-walk) with a sigil field (`:(LIST OF Number)`, which forces
    /// deferral) composes its `Record` handle at the fold brand with both field types resolved. The
    /// scope-alias field reads through the brand-delivered scope envelope; the sigil field pops its
    /// sub-Dispatch carrier from the fed views.
    #[test]
    fn record_sigil_defers_and_mixes_scope_read_with_sub_dispatch() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("LET Wrapped = :{a :Number}");
        let result =
            test_run.run_one_type(test_run.parse_one(":{x :Wrapped, y :(LIST OF Number)}"));
        let types = test_run.types();
        let inner = types.record(Record::from_pairs(vec![(
            crate::builtins::test_support::binder_name("a", test_run.registries()),
            KType::NUMBER,
        )]));
        assert_eq!(
            result,
            types.record(Record::from_pairs(vec![
                (
                    crate::builtins::test_support::binder_name("x", test_run.registries()),
                    inner
                ),
                (
                    crate::builtins::test_support::binder_name("y", test_run.registries()),
                    types.list(KType::NUMBER)
                ),
            ])),
        );
    }

    /// An FN whose parameter list defers (the `:(LIST OF Number)` field parks inside the record's
    /// own elaboration) and whose return type names a `NEWTYPE` alias — a `SetMember` that is not
    /// region-free, so it cannot be rebuilt from a `'static` value. Both operands reach the body
    /// through the carrier view, so a successful compose proves the reaching return type survived
    /// the park the parameter list forced.
    #[test]
    fn fn_deferred_with_reaching_ret_composes_from_carrier_view() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("NEWTYPE Wrapped = :{a :Number}");
        let result =
            test_run.run_one_type(test_run.parse_one(":(FN :{xs :(LIST OF Number)} -> Wrapped)"));
        let types = test_run.types();
        match types.node(result) {
            TypeNode::KFunction { params, ret } => {
                assert_eq!(
                    params.get(crate::machine::model::Symbol::of("xs")).copied(),
                    Some(types.list(KType::NUMBER)),
                    "the sigil param must lower to LIST OF Number",
                );
                assert_eq!(
                    ret.name(test_run.registries()),
                    "Wrapped",
                    "the reaching return type must survive the carrier-view crossing",
                );
            }
            _ => panic!("expected a KFunction carrier, got {result:?}"),
        }
    }

    /// A deferred record field whose sigil sub-Dispatch resolves to a non-type value (`:(1)` → the
    /// number `1`) surfaces the walker's shape error directly: `compose_field_list` propagates the
    /// rewalk's `Err` before any allocation runs.
    #[test]
    fn record_field_sub_dispatch_to_non_type_value_errors() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let err = test_run.run_one_err(test_run.parse_one(":{x :(1)}"));
        assert!(
            matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("resolved to non-type value")),
            "expected a non-type-value ShapeError through the deferred side-channel, got {err}",
        );
    }

    /// `t.name()` round-trips: rendering `expected` and re-running its surface form yields
    /// a type carrier equal to `expected`. The expected value is built at each call site so
    /// it shares the scope's lifetime, keeping the comparison off `'static`.
    fn assert_round_trips(test_run: &mut TestRun<'_>, expected: KType) {
        let rendered = expected.name(test_run.registries());
        let expr = test_run.parse_one(&rendered);
        let result = test_run.run_one_type(expr);
        assert_eq!(
            result, expected,
            "round-trip of `{rendered}` did not reproduce the original KType",
        );
    }

    #[test]
    fn fn_multi_param_round_trips() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let expected = test_run.types().function_type(
            Record::from_pairs(vec![
                (
                    crate::builtins::test_support::binder_name("x", test_run.registries()),
                    KType::NUMBER,
                ),
                (
                    crate::builtins::test_support::binder_name("y", test_run.registries()),
                    KType::STR,
                ),
            ]),
            KType::BOOL,
        );
        assert_round_trips(&mut test_run, expected);
    }

    #[test]
    fn fn_nullary_round_trips() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let expected = test_run.types().function_type(Record::new(), KType::ANY);
        assert_round_trips(&mut test_run, expected);
    }

    #[test]
    fn fn_nested_param_round_trips() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let types = test_run.types();
        let expected = types.function_type(
            Record::from_pairs(vec![(
                crate::builtins::test_support::binder_name("xs", test_run.registries()),
                types.list(KType::NUMBER),
            )]),
            KType::BOOL,
        );
        assert_round_trips(&mut test_run, expected);
    }

    #[test]
    fn fn_capitalized_param_round_trips_and_preserves_name() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let types = test_run.types();
        let expected = types.function_type(
            Record::from_pairs(vec![(
                crate::builtins::test_support::binder_name("Ty", test_run.registries()),
                KType::of_kind(KKind::Signature),
            )]),
            KType::EMPTY_SIGNATURE,
        );
        // Param name `Ty` (capitalized, a `Type` token) must survive the round-trip.
        assert!(
            matches!(types.node(expected), TypeNode::KFunction { params, .. } if params.get(crate::machine::model::Symbol::of("Ty")).is_some()),
        );
        assert_round_trips(&mut test_run, expected);
    }

    /// `:(MAP Str -> Wrapped)` correlates a scalar-literal key with no carrier (`k`) and a
    /// reaching value type (`v`) by total operand position, not by carrier presence: the scalar
    /// lands in `k` and the reaching type survives the carrier-view crossing into `v`.
    #[test]
    fn map_scalar_key_reaching_value_correlates() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("NEWTYPE Wrapped = :{a :Number}");
        let result = test_run.run_one_type(test_run.parse_one(":(MAP Str -> Wrapped)"));
        let types = test_run.types();
        match types.node(result) {
            TypeNode::Dict { key, value } => {
                assert_eq!(key, KType::STR, "scalar key must lower to Str");
                assert_eq!(
                    value.name(test_run.registries()),
                    "Wrapped",
                    "reaching value type must survive the carrier-view crossing",
                );
            }
            _ => panic!("expected a Dict carrier, got {result:?}"),
        }
    }

    /// Mirror of `map_scalar_key_reaching_value_correlates`: the reaching type lands in `k` and
    /// the scalar lands in `v`, proving the correlation is positional, not carrier-presence-based.
    #[test]
    fn map_reaching_key_scalar_value_correlates() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("NEWTYPE Wrapped = :{a :Number}");
        let result = test_run.run_one_type(test_run.parse_one(":(MAP Wrapped -> Str)"));
        let types = test_run.types();
        match types.node(result) {
            TypeNode::Dict { key, value } => {
                assert_eq!(
                    key.name(test_run.registries()),
                    "Wrapped",
                    "reaching key type must survive the carrier-view crossing",
                );
                assert_eq!(value, KType::STR, "scalar value must lower to Str");
            }
            _ => panic!("expected a Dict carrier, got {result:?}"),
        }
    }

    /// A sync record type whose field names a `NEWTYPE` alias (`:Wrapped`, a `SetMember`) resolves
    /// in one ambient walk — no sigil field forces deferral — so its `Record` handle composes
    /// directly from the elaborated pairs, where the `SetMember` field survives as owned data.
    #[test]
    fn record_sync_reaching_field_folds_at_brand() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("NEWTYPE Wrapped = :{a :Number}");
        let result = test_run.run_one_type(test_run.parse_one(":{x :Wrapped}"));
        let types = test_run.types();
        match types.node(result) {
            TypeNode::Record { fields: record } => {
                let field = record
                    .get(crate::machine::model::Symbol::of("x"))
                    .expect("record must have field x");
                assert_eq!(
                    field.name(test_run.registries()),
                    "Wrapped",
                    "the reaching field must survive the sync brand re-fold",
                );
            }
            _ => panic!("expected a Record, got {result:?}"),
        }
    }

    /// A record type admits a capitalized field name: `Ty` lexes as a `Type` token and keys the
    /// record by `BinderSymbol::Type`, the class a functor parameter list needs.
    #[test]
    fn record_admits_a_capitalized_field_name() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        let result = test_run.run_one_type(test_run.parse_one(":{Ty :Signature}"));
        let types = test_run.types();
        assert_eq!(
            result,
            types.record(Record::from_pairs(vec![(
                crate::builtins::test_support::binder_name("Ty", test_run.registries()),
                KType::of_kind(KKind::Signature)
            )])),
        );
        match types.node(result) {
            TypeNode::Record { fields } => assert!(
                fields
                    .iter()
                    .all(|(key, _)| matches!(key, crate::machine::model::BinderSymbol::Type(_))),
                "a capitalized field name must key the record as a Type symbol",
            ),
            _ => panic!("expected a Record, got {result:?}"),
        }
    }

    /// A sync FN whose parameter type names a `NEWTYPE` alias (`x :Wrapped`, a `SetMember`) resolves
    /// in one ambient walk, so its function type composes directly from the elaborated pairs:
    /// the `SetMember` param survives as owned data alongside the plain `Bool` return type.
    #[test]
    fn fn_sync_reaching_param_folds_at_brand() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("NEWTYPE Wrapped = :{a :Number}");
        let result = test_run.run_one_type(test_run.parse_one(":(FN :{x :Wrapped} -> Bool)"));
        let types = test_run.types();
        match types.node(result) {
            TypeNode::KFunction { params, ret } => {
                assert_eq!(
                    params
                        .get(crate::machine::model::Symbol::of("x"))
                        .map(|kt| kt.name(test_run.registries())),
                    Some("Wrapped".to_string()),
                    "the SetMember param must survive the sync compose",
                );
                assert_eq!(ret, KType::BOOL, "the region-free return type must be Bool");
            }
            _ => panic!("expected a KFunction, got {result:?}"),
        }
    }

    /// A sync FN whose return type names a `NEWTYPE` alias (`-> Wrapped`) resolves in one ambient
    /// walk, so its `KType::KFunction` composes directly from the elaborated pairs: the `ret`
    /// argument the caller closed over crosses into the composed carrier as owned data.
    #[test]
    fn fn_sync_reaching_ret_folds_at_brand() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("NEWTYPE Wrapped = :{a :Number}");
        let result = test_run.run_one_type(test_run.parse_one(":(FN :{x :Number} -> Wrapped)"));
        let types = test_run.types();
        match types.node(result) {
            TypeNode::KFunction { params, ret } => {
                assert_eq!(
                    params.get(crate::machine::model::Symbol::of("x")).copied(),
                    Some(KType::NUMBER),
                    "the region-free param must be Number",
                );
                assert_eq!(
                    ret.name(test_run.registries()),
                    "Wrapped",
                    "the SetMember return type must survive the carrier-view crossing",
                );
            }
            _ => panic!("expected a KFunction, got {result:?}"),
        }
    }

    /// `:(LIST OF Wrapped)` lowers with the reaching elem type surviving the carrier-view
    /// crossing (the single-operand analog of the MAP correlation tests above).
    #[test]
    fn list_of_reaching_elem_lowers() {
        let program = program_storage();
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run("NEWTYPE Wrapped = :{a :Number}");
        let result = test_run.run_one_type(test_run.parse_one(":(LIST OF Wrapped)"));
        let types = test_run.types();
        match types.node(result) {
            TypeNode::List { element } => {
                assert_eq!(
                    element.name(test_run.registries()),
                    "Wrapped",
                    "reaching elem type must survive the carrier-view crossing",
                );
            }
            _ => panic!("expected a List carrier, got {result:?}"),
        }
    }
}
