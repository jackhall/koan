//! The type channel's load pass over shaped programs: each cell it fills read back and compared
//! with what elaborating through an activation gives, each variable numbered as its region
//! numbers it, each spelling it leaves for the run, and each refusal.

use crate::memory::BumpAllocator;
use crate::scope::{
    BodyShape, Canonical, Elaboration, ShapeError, ShapeKind, Slot, Static, TypeExpression,
    Variable,
};
use crate::symbols::{BinderSymbol, SymbolInterner};
use crate::type_lattice::{KType, TypeRegistry};

use super::super::type_channel;
use super::{Held, Program, with_program};

/// The scalars and `Type`, which a `:Type` parameter names.
fn with_type(
    _: &TypeRegistry<'_>,
    _: BumpAllocator<'_>,
    _: &SymbolInterner,
) -> Vec<(&'static str, KType)> {
    vec![("Type", KType::ANY_TYPE)]
}

/// Shape `source` with every slot empty, run the load pass over it, and hand its result to `check`.
fn loaded<R>(
    source: &str,
    check: impl for<'p, 'graph, 'cell> FnOnce(
        &Program<'p, 'graph, 'cell>,
        Result<(), ShapeError<'graph>>,
    ) -> R,
) -> R {
    with_program(
        source,
        with_type,
        |_, _, _| Held::Empty,
        |program| {
            let typed = type_channel(
                program.activation.shape(),
                program.activation.builtins(),
                program.types,
                program.storage,
                program.scratch,
            );
            check(&program, typed)
        },
    )
}

/// `source` loads, or the test fails with the refusal.
fn typed<R>(
    source: &str,
    check: impl for<'p, 'graph, 'cell> FnOnce(&Program<'p, 'graph, 'cell>) -> R,
) -> R {
    loaded(source, |program, typed| {
        if let Err(error) = typed {
            panic!(
                "`{source}` loads: {}",
                error.display(program.symbols, program.types)
            );
        }
        check(program)
    })
}

/// `shape`'s type expressions written in `statement`, in site order.
fn written_in<'graph>(
    shape: &'graph BodyShape<'graph>,
    statement: u32,
) -> Vec<&'graph TypeExpression<'graph>> {
    shape
        .type_expressions()
        .iter()
        .filter(|expression| expression.statement == statement)
        .collect()
}

/// The one type expression `shape` records.
fn only<'graph>(shape: &'graph BodyShape<'graph>) -> Static<'graph, KType> {
    let [expression] = shape.type_expressions() else {
        panic!(
            "one type expression, not {}",
            shape.type_expressions().len()
        )
    };
    expression.typed()
}

/// The one nested shape of `kind` in `shape`.
fn nested<'graph>(shape: &'graph BodyShape<'graph>, kind: ShapeKind) -> &'graph BodyShape<'graph> {
    let mut found = shape
        .nested_shapes()
        .iter()
        .filter(|(_, nested)| nested.kind() == kind);
    let (_, nested) = found.next().expect("a nested shape of the kind");
    assert!(found.next().is_none(), "one nested shape of the kind");
    nested
}

/// The rigid value `typed` holds, beside its variables' indices.
fn rigid(typed: Static<'_, KType>) -> (KType, Vec<usize>) {
    match typed {
        Static::Rigid { value, variables } => (
            value,
            variables.iter().map(|variable| variable.index).collect(),
        ),
        _ => panic!("a rigid type, not {typed:?}"),
    }
}

fn closed(typed: Static<'_, KType>) -> KType {
    match typed {
        Static::Closed(value) => value,
        _ => panic!("a closed type, not {typed:?}"),
    }
}

fn unknown(typed: Static<'_, KType>) -> bool {
    matches!(typed, Static::Unknown)
}

/// The slot of the type binder `name` in `shape`.
fn type_slot(program: &Program<'_, '_, '_>, shape: &BodyShape<'_>, name: &str) -> Slot {
    shape
        .slot(BinderSymbol::Type(program.type_name(name)))
        .expect("a declared type binder")
        .0
}

#[test]
fn a_closed_binder_is_typed_at_load() {
    let source = "NEWTYPE Dist = Number\nNEWTYPE Ay = :{b :Be}\nNEWTYPE Be = :{a :Ay}";
    typed(source, |program| {
        let shape = program.activation.shape();
        let names = ["Dist", "Ay", "Be"];
        let loaded = names.map(|name| closed(shape.declared_type(type_slot(program, shape, name))));
        program
            .declare()
            .expect("the binders declare through the activation");
        assert_eq!(loaded, names.map(|name| program.bound(name)));
    });
}

#[test]
fn a_value_position_type_is_closed() {
    typed("LET t = :(LIST OF Number)", |program| {
        let shape = program.activation.shape();
        assert_eq!(closed(only(shape)), program.types.list(KType::NUMBER));
    });
}

// Each `FOR ALL` name below occurs at least twice in its signature: canonical form drops one that
// occurs once, which a call binds to its bound, so the load reads it as that closed bound.

#[test]
fn a_for_all_name_is_its_canonical_variable() {
    let source = "LET f = (FN FOR ALL #[Held Unused] :{x :(LIST OF Held)} -> Held = \
                  #(:(LIST OF Held)))";
    typed(source, |program| {
        let body = program.birth("f");
        let types = program.types;
        let (value, indices) = rigid(only(body));
        assert_eq!(value, types.list(types.quantified(0, KType::ANY)));
        assert_eq!(indices, [0]);
        let Static::Closed(callable) = body.callable_type() else {
            panic!("the callable's type is closed")
        };
        let held = program.type_name("Held");
        let unused = program.type_name("Unused");
        assert_eq!(
            callable.quantifier_map,
            [
                (held, Canonical::At(0)),
                (unused, Canonical::Dropped { bound: KType::ANY })
            ]
        );
        let [Static::Rigid { variables, .. }] = [only(body)] else {
            unreachable!()
        };
        let slot = type_slot(program, body, "Held");
        assert_eq!(
            variables,
            [Variable {
                index: 0,
                at: super::local(slot)
            }]
        );
    });
}

#[test]
fn a_type_parameter_is_a_rigid_variable() {
    typed(
        "EXPR #(MAKESET Elt :Type) -> Any = #(:(LIST OF Elt))",
        |program| {
            let shape = program.activation.shape();
            let body = shape
                .births(shape.registrations()[0].slot)
                .expect("the registration births its body");
            let types = program.types;
            let (value, indices) = rigid(only(body));
            assert_eq!(value, types.list(types.quantified(0, KType::ANY)));
            assert_eq!(indices, [0]);
        },
    );
}

#[test]
fn an_outer_for_all_name_in_an_inner_group_takes_the_next_index() {
    let source = "LET f = (FN FOR ALL #[Ay] :{x :Ay} -> Ay = \
                  #(FN FOR ALL #[Be] :{y :Be} -> Be = #(:(MAP Ay -> Be))))";
    typed(source, |program| {
        let inner = nested(program.birth("f"), ShapeKind::Callable);
        let types = program.types;
        let (value, indices) = rigid(only(inner));
        let (ay, be) = (
            types.quantified(1, KType::ANY),
            types.quantified(0, KType::ANY),
        );
        assert_eq!(value, types.dict(ay, be));
        assert_eq!(indices, [1, 0]);
    });
}

#[test]
fn a_non_commuting_spelling_is_left_for_the_run() {
    let source = "LET f = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(\
                  (:(Elt & Number)) \
                  (:(Elt.x)) \
                  (:(FN FOR ALL #[Tee] :{y :Tee, z :Elt} -> Tee)) \
                  (:(Number AS Elt)) \
                  (:(Elt NEEDING #[y])) \
                  (:(Number & Str))))";
    typed(source, |program| {
        let body = program.birth("f");
        for statement in 0..5 {
            let [expression] = written_in(body, statement)[..] else {
                panic!("one expression in statement {statement}")
            };
            assert!(unknown(expression.typed()), "statement {statement}");
        }
        let [met] = written_in(body, 5)[..] else {
            panic!("one expression in the last statement")
        };
        closed(met.typed());
    });
}

#[test]
fn a_nominal_over_a_run_bound_type_is_declared_where_it_runs() {
    let source = "LET f = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(\
                  (NEWTYPE Boxed = :{v :Elt}) \
                  (:(LIST OF Boxed))))";
    typed(source, |program| {
        let body = program.birth("f");
        let types = program.types;
        assert!(unknown(
            body.declared_type(type_slot(program, body, "Boxed"))
        ));
        let (value, indices) = rigid(only(body));
        assert_eq!(value, types.list(types.quantified(1, KType::ANY)));
        assert_eq!(indices, [1]);
    });
}

#[test]
fn a_callable_and_its_registration_are_typed_at_load() {
    let source = "LET near = FN EXPR #(NEAR x :Number) -> Number = #(x)\n\
                  LET f = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(FN :{y :Elt} -> Elt = #(y)))";
    typed(source, |program| {
        let shape = program.activation.shape();
        let body = program.birth("near");
        let elaborated = program
            .callable("near", true)
            .expect("the callable elaborates through the activation");
        let Static::Closed(callable) = body.callable_type() else {
            panic!("the callable's type is closed")
        };
        assert_eq!(callable.ktype, elaborated.ktype);
        let registration = program.registration(body);
        let Static::Closed(registered) = shape.registered_type(registration.slot) else {
            panic!("the registration's shape is closed")
        };
        assert_eq!(Some(registered), elaborated.registered);

        let inner = nested(program.birth("f"), ShapeKind::Callable);
        let Static::Rigid { value, variables } = inner.callable_type() else {
            panic!("a callable over a run-bound name is rigid")
        };
        let types = program.types;
        let elt = types.quantified(0, KType::ANY);
        let name = BinderSymbol::Value(
            crate::symbols::ValueSymbol::declared("y", program.symbols).expect("a name"),
        );
        assert_eq!(
            value.ktype,
            types
                .function_type(program.scratch, &[], &[(name, elt)], elt)
                .handle
        );
        assert_eq!(variables.len(), 1);
    });
}

#[test]
fn a_quote_s_holes_are_rigid_variables() {
    let source = "LET Alias = Number\n\
                  LET q = #(PRINT :(LIST OF Tee))\n\
                  LET r = #(PRINT :(LIST OF $Alias))";
    typed(source, |program| {
        let shape = program.activation.shape();
        let types = program.types;
        let codes: Vec<_> = shape
            .nested_shapes()
            .iter()
            .filter(|(_, nested)| nested.kind() == ShapeKind::Code)
            .map(|(_, nested)| *nested)
            .collect();
        let [first, second] = codes[..] else {
            panic!("two quotes")
        };
        let (hole, alias) = if first.captures()[0].mark.is_none() {
            (first, second)
        } else {
            (second, first)
        };
        let (value, indices) = rigid(only(hole));
        assert_eq!(value, types.list(types.quantified(0, KType::ANY)));
        assert_eq!(indices, [0]);
        assert_eq!(closed(only(alias)), types.list(KType::NUMBER));
    });
}

/// The source text `error` is located at.
fn located(error: &ShapeError<'_>) -> String {
    error.at().text()
}

#[test]
fn a_closed_type_that_does_not_elaborate_refuses_the_load() {
    for (source, at) in [
        ("NEWTYPE Bad = :(Number.z)", ":(Number.z)"),
        ("LET t = :(LIST OF Number.z)", "Number.z"),
        ("LET f = (FN :{} -> Any = #(:(Number.z)))", ":(Number.z)"),
    ] {
        loaded(source, |_, typed| {
            let error = typed.expect_err(source);
            assert!(
                matches!(
                    error,
                    ShapeError::Type {
                        error: Elaboration::NoSuchMember { .. },
                        ..
                    }
                ),
                "`{source}`: {error:?}"
            );
            assert_eq!(located(&error), at, "`{source}`");
        });
    }
}

#[test]
fn a_refusal_in_a_quote_s_code_is_kept_for_its_eval() {
    loaded("LET q = #(PRINT :(Number.z))", |program, typed| {
        typed.expect("the program loads");
        let code = nested(program.activation.shape(), ShapeKind::Code);
        assert!(matches!(code.refusal(), Some(ShapeError::Type { .. })));
    });
}

#[test]
fn a_guard_written_twice_refuses_the_arm_set() {
    for source in [
        "MATCH 1 -> :Number WITH #{Number: (1), :(Number): (2)}",
        "LET Num = Number\nMATCH 1 -> :Number WITH #{Num: (1), Number: (2)}",
        "MATCH 1 -> :Number WITH #{:(Number | Str): (1), :(Str | Number): (2)}",
        "LET f = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = \
         #(MATCH x -> :Number WITH #{Elt: (1), :(Elt): (2)}))",
    ] {
        loaded(source, |_, typed| {
            let error = typed.expect_err(source);
            assert!(
                matches!(error, ShapeError::RepeatedGuard { .. }),
                "`{source}`: {error:?}"
            );
        });
    }
    typed(
        "MATCH 1 -> :Number WITH #{Number: (1), Str: (2)}",
        |program| {
            let guards = program
                .activation
                .shape()
                .type_expressions()
                .iter()
                .filter(|expression| expression.guard.is_some())
                .count();
            assert_eq!(guards, 2);
        },
    );
}
