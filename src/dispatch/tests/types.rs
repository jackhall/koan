//! The type half of the one walk over loaded programs, read back through [`LoadFacts`]: each type
//! the load fixes, each lexical variable at the level its chain numbers it, each spelling it
//! leaves for the run, each refusal, and the unknowns each unit's frontier records.
//!
//! The walk's type facts are a unit's ([`crate::elaborate::declarations`] and its siblings), so a
//! frontier here is the frontier of the unit holding the fact.

use crate::memory::Bump;
use crate::program::{CellSubstrate, Program};
use crate::scope::{
    Address, BodyShape, CaptureSlot, Coordinate, FunctionGroupMap, LoadFacts, PathEntry, ShapeKind,
    Site, Slot, Source, Static, StaticType, Target, TypeExpression, UnitId, Unknown, Variable,
};
use crate::symbols::{BinderSymbol, TypeSymbol, ValueSymbol};
use crate::type_lattice::{DeclaredType, KType, Parametric, TypeNode, display_name};
use crate::values::SpecBounds;
use crate::values::tests::reading;

use super::statics::loaded;
use super::{Koan, output};

/// Why `source` does not load, rendered.
fn refused(source: &str) -> String {
    match CellSubstrate::load::<Koan>(source, "<test>", 8, SpecBounds::default(), output()) {
        Ok(_) => panic!("`{source}` is refused"),
        Err(error) => error.to_string(),
    }
}

/// `shape`'s type expressions written in `statement`, in site order.
fn written_in<'graph>(
    shape: &'graph BodyShape<'graph>,
    statement: u32,
) -> Vec<&'graph TypeExpression<'graph>> {
    (shape.type_expressions().iter())
        .filter(|expression| expression.statement == statement)
        .collect()
}

/// The one type expression `shape` records.
fn the_expression<'graph>(shape: &'graph BodyShape<'graph>) -> &'graph TypeExpression<'graph> {
    let [expression] = shape.type_expressions() else {
        panic!(
            "one type expression, not {}",
            shape.type_expressions().len()
        )
    };
    expression
}

/// What the load fixed for the one type expression `shape` records.
fn only<'graph>(facts: &LoadFacts<'graph>, shape: &BodyShape<'_>) -> StaticType<'graph> {
    let shape = facts.shapes().shape_at(shape.segment());
    facts.typed_expression(shape, the_expression(shape).site)
}

/// The one nested shape of `kind` in `shape`.
fn nested<'graph>(shape: &'graph BodyShape<'graph>, kind: ShapeKind) -> &'graph BodyShape<'graph> {
    let mut found = (shape.nested_shapes().iter()).filter(|(_, nested)| nested.kind() == kind);
    let (_, nested) = found.next().expect("a nested shape of the kind");
    assert!(found.next().is_none(), "one nested shape of the kind");
    nested
}

/// Each callable shape nested in `shape`, in order.
fn callables<'graph>(shape: &'graph BodyShape<'graph>) -> Vec<&'graph BodyShape<'graph>> {
    (shape.nested_shapes().iter())
        .filter(|(_, nested)| nested.kind() == ShapeKind::Callable)
        .map(|(_, nested)| *nested)
        .collect()
}

/// The callable body the top-level value binder `name` is born of.
fn birth<'graph>(program: &Program<'graph>, name: &str) -> &'graph BodyShape<'graph> {
    let shape = program.shape();
    let name = ValueSymbol::declared(name, program.symbols()).expect("a value name");
    let (slot, _) = shape
        .slot(BinderSymbol::Value(name))
        .expect("a top-level binder");
    shape.births(slot).expect("the binder is born of a body")
}

/// The callable body the program's first registration births.
fn registered<'graph>(program: &Program<'graph>) -> &'graph BodyShape<'graph> {
    let shape = program.shape();
    shape
        .births(shape.registrations()[0].slot)
        .expect("the registration births its body")
}

/// The rigid value `typed` holds, beside its variables' levels.
fn rigid(typed: StaticType<'_>) -> (Parametric, Vec<usize>) {
    match typed {
        Static::Rigid { value, variables } => (
            value,
            variables.iter().map(|variable| variable.level).collect(),
        ),
        _ => panic!("a rigid type, not {typed:?}"),
    }
}

fn closed(typed: StaticType<'_>) -> KType {
    match typed {
        Static::Closed(value) => value,
        _ => panic!("a closed type, not {typed:?}"),
    }
}

fn unknown(typed: StaticType<'_>) -> bool {
    matches!(typed, Static::Unknown)
}

fn type_name(program: &Program<'_>, name: &str) -> TypeSymbol {
    TypeSymbol::declared(name, program.symbols()).expect("a Type token")
}

/// The lexical variable `name` at `level`, bounded by `Any`.
fn lexical(program: &Program<'_>, level: usize, name: &str) -> Parametric {
    super::statics::lexical(program, level, name, KType::ANY)
}

/// The slot of the type binder `name` in `shape`.
fn type_slot(program: &Program<'_>, shape: &BodyShape<'_>, name: &str) -> Slot {
    shape
        .slot(BinderSymbol::Type(type_name(program, name)))
        .expect("a declared type binder")
        .0
}

fn local(slot: Slot) -> Coordinate {
    Coordinate::Activation {
        hops: 0,
        target: Target::Local(slot),
    }
}

/// `typed`, rendered.
fn rendered(program: &Program<'_>, typed: impl Into<DeclaredType<Parametric>>) -> String {
    let bump = Bump::new();
    display_name(typed, &reading(program.tiers(), &bump), program.symbols()).to_string()
}

#[test]
fn a_closed_binder_is_typed_at_load() {
    let source = "NEWTYPE Dist = Number\nNEWTYPE Ay = :{b :Be}\nNEWTYPE Be = :{a :Ay}";
    loaded(source, |program| {
        let (facts, shape) = (program.facts(), program.shape());
        for name in ["Dist", "Ay", "Be"] {
            let declared = closed(facts.declared_type(shape, type_slot(program, shape, name)));
            assert_eq!(rendered(program, declared), name);
        }
    });
}

#[test]
fn a_value_position_type_is_closed() {
    loaded("LET t = :(LIST OF Number)", |program| {
        let bump = Bump::new();
        let types = reading(program.tiers(), &bump);
        let typed = closed(only(program.facts(), program.shape()));
        assert_eq!(typed, types.list(KType::NUMBER));
    });
}

#[test]
fn a_for_all_name_is_its_lexical_variable() {
    let source = "EXPR FOR ALL #[Held Unused] #(FUN x :(LIST OF Held)) -> Any = #(:(LIST OF Held))";
    loaded(source, |program| {
        let (facts, body) = (program.facts(), registered(program));
        let bump = Bump::new();
        let types = reading(program.tiers(), &bump);
        let held = lexical(program, 0, "Held");
        let (value, levels) = rigid(only(facts, body));
        assert_eq!(value, types.list(held));
        assert_eq!(levels, [0]);
        assert_eq!(
            facts.group_levels(body),
            [held, lexical(program, 1, "Unused")],
            "a name no position reads is still a variable of the group"
        );
        let Static::Closed(callable) = facts.callable_type(body) else {
            panic!("the callable's type is closed")
        };
        let (held, unused) = (type_name(program, "Held"), type_name(program, "Unused"));
        assert_eq!(
            callable.quantifier_map,
            FunctionGroupMap(&[(held, 0), (unused, 1)])
        );
        let Static::Rigid { variables, .. } = only(facts, body) else {
            unreachable!()
        };
        let slot = type_slot(program, body, "Held");
        assert_eq!(
            variables,
            [Variable {
                level: 0,
                source: Source::Type(local(slot))
            }]
        );
    });
}

#[test]
fn a_callable_records_where_its_for_all_names_live() {
    loaded(
        "EXPR FOR ALL #[Elt] #(ONLY x :Elt) -> Elt = #(x)",
        |program| {
            let (facts, body) = (program.facts(), registered(program));
            assert_eq!(facts.group_levels(body), [lexical(program, 0, "Elt")]);
            let slot = type_slot(program, body, "Elt");
            assert_eq!(facts.homes(body), [(0, Target::Local(slot))]);
        },
    );
}

#[test]
fn a_for_all_name_one_slot_reads_is_rigid_in_the_body() {
    loaded(
        "EXPR FOR ALL #[Elt] #(KIND x :Elt) -> Type = #(:(LIST OF Elt))",
        |program| {
            let (facts, body) = (program.facts(), registered(program));
            let bump = Bump::new();
            let elt = lexical(program, 0, "Elt");
            assert_eq!(facts.group_levels(body), [elt]);
            let (value, levels) = rigid(only(facts, body));
            assert_eq!(value, reading(program.tiers(), &bump).list(elt));
            assert_eq!(levels, [0]);
        },
    );
}

#[test]
fn a_type_parameter_is_a_rigid_variable() {
    loaded(
        "EXPR #(MAKESET Elt :Type) -> Any = #(:(LIST OF Elt))",
        |program| {
            let (facts, body) = (program.facts(), registered(program));
            let bump = Bump::new();
            let (value, levels) = rigid(only(facts, body));
            let elt = lexical(program, 0, "Elt");
            assert_eq!(value, reading(program.tiers(), &bump).list(elt));
            assert_eq!(levels, [0]);
        },
    );
}

#[test]
fn an_outer_for_all_name_keeps_its_level_in_an_inner_body() {
    let source = "EXPR FOR ALL #[Ay] #(FUN x :Ay) -> Ay = \
                  #(\n  EXPR FOR ALL #[Be] #(GEN y :Be) -> Any = #(:(MAP Ay -> Be))\n  x\n)";
    loaded(source, |program| {
        let inner = nested(registered(program), ShapeKind::Callable);
        let bump = Bump::new();
        let (value, levels) = rigid(only(program.facts(), inner));
        let (ay, be) = (lexical(program, 0, "Ay"), lexical(program, 1, "Be"));
        assert_eq!(value, reading(program.tiers(), &bump).dict(ay, be));
        assert_eq!(levels, [0, 1]);
    });
}

#[test]
fn sibling_quantified_bodies_number_their_own_names_alike() {
    let source = "EXPR FOR ALL #[Elt] #(FUN x :Elt) -> Elt = #(\n  \
                  EXPR FOR ALL #[Ay] #(GEN a :Ay) -> Ay = #(a)\n  \
                  EXPR FOR ALL #[Be] #(HOLD b :Be) -> Be = #(b)\n  \
                  x\n)";
    loaded(source, |program| {
        // Nested shapes sit in site order, which is address order, so the two may come either way.
        let levels: Vec<_> = (callables(registered(program)).iter())
            .map(|body| program.facts().group_levels(body).to_vec())
            .collect();
        let (ay, be) = (lexical(program, 1, "Ay"), lexical(program, 1, "Be"));
        assert_eq!(levels.len(), 2);
        assert!(levels.contains(&vec![ay]) && levels.contains(&vec![be]));
    });
}

#[test]
fn a_nested_quantified_callable_is_rigid_over_its_enclosing_names() {
    let source = "EXPR FOR ALL #[Elt] #(FUN x :Elt) -> Elt = \
                  #(\n  EXPR FOR ALL #[Tee] #(GEN t :Tee AND u :Elt) -> Tee = #(t)\n  x\n)";
    loaded(source, |program| {
        let facts = program.facts();
        let inner = nested(registered(program), ShapeKind::Callable);
        let (value, levels) = match facts.callable_type(inner) {
            Static::Rigid { value, variables } => (
                value,
                (variables.iter())
                    .map(|variable| variable.level)
                    .collect::<Vec<_>>(),
            ),
            typed => panic!("a rigid callable, not {typed:?}"),
        };
        assert_eq!(levels, [0]);
        let bump = Bump::new();
        let types = reading(program.tiers(), &bump);
        let scheme = value.ktype.as_scheme().expect("`g` is quantified");
        let TypeNode::KFunction { params, .. } =
            DeclaredType::<Parametric>::from(scheme).view(&types).body
        else {
            panic!("a function type")
        };
        let elt = lexical(program, 0, "Elt");
        assert!(params.iter().any(|(_, param)| param == elt));
        assert_eq!(facts.group_levels(inner), [lexical(program, 1, "Tee")]);
    });
}

#[test]
fn a_bound_naming_a_run_bound_name_is_left_for_the_run() {
    let source = "EXPR FOR ALL #[Elt] #(FUN x :Elt) -> Elt = \
                  #(\n  EXPR FOR ALL #{Tee: Elt} #(GEN t :Tee) -> Tee = #(t)\n  x\n)";
    loaded(source, |program| {
        let facts = program.facts();
        let inner = nested(registered(program), ShapeKind::Callable);
        assert!(matches!(facts.callable_type(inner), Static::Unknown));
        assert!(facts.group_levels(inner).is_empty());
    });
}

#[test]
fn a_dollar_name_crossing_into_code_is_a_name_of_the_code_s_own() {
    let source = "EXPR FOR ALL #[Elt] #(FUN x :Elt) -> Elt = #(\n  \
                  LET q = #(PRINT :(LIST OF $Elt))\n  \
                  x\n)";
    loaded(source, |program| {
        let code = nested(registered(program), ShapeKind::Code);
        let Static::Rigid { value, variables } = only(program.facts(), code) else {
            panic!("a rigid type")
        };
        let bump = Bump::new();
        let elt = lexical(program, 0, "Elt");
        assert_eq!(value, reading(program.tiers(), &bump).list(elt));
        let elt = BinderSymbol::Type(type_name(program, "Elt"));
        let capture = (code.captures().iter())
            .position(|capture| capture.name == elt)
            .expect("the code captures `$Elt`");
        assert_eq!(
            variables,
            [Variable {
                level: 0,
                source: Source::Type(Coordinate::Activation {
                    hops: 0,
                    target: Target::Capture(CaptureSlot(capture as u32)),
                }),
            }]
        );
    });
}

#[test]
fn a_non_commuting_spelling_is_left_for_the_run() {
    let source = "EXPR FOR ALL #[Elt] #(FUN x :Elt) -> Any = #(\
                  (:(Elt & Number)) \
                  (:(Elt.x)) \
                  (:(FN :{y :Number, z :Elt} -> Number)) \
                  (:(Number AS Elt)) \
                  (:(Elt NEEDING #[y])) \
                  (:(Number & Str)))";
    loaded(source, |program| {
        let (facts, body) = (program.facts(), registered(program));
        let typed = |expression: &TypeExpression<'_>| facts.typed_expression(body, expression.site);
        for statement in [0, 1, 3, 4] {
            let [expression] = written_in(body, statement)[..] else {
                panic!("one expression in statement {statement}")
            };
            assert!(unknown(typed(expression)), "statement {statement}");
        }
        let [function] = written_in(body, 2)[..] else {
            panic!("one expression in statement 2")
        };
        let (value, levels) = rigid(typed(function));
        let field = |name| {
            BinderSymbol::Value(ValueSymbol::declared(name, program.symbols()).expect("a name"))
        };
        let bump = Bump::new();
        let expected = reading(program.tiers(), &bump).function_type(
            &bump,
            &[
                (field("y"), KType::NUMBER.into()),
                (field("z"), lexical(program, 0, "Elt")),
            ],
            KType::NUMBER.into(),
        );
        assert_eq!(value, expected);
        assert_eq!(levels, [0]);
        let [met] = written_in(body, 5)[..] else {
            panic!("one expression in the last statement")
        };
        closed(typed(met));
    });
}

#[test]
fn a_nominal_over_a_run_bound_type_is_declared_where_it_runs() {
    let source = "EXPR FOR ALL #[Elt] #(FUN x :Elt) -> Any = #(\
                  (NEWTYPE Boxed = :{v :Elt}) \
                  (:(LIST OF Boxed)))";
    loaded(source, |program| {
        let (facts, body) = (program.facts(), registered(program));
        let slot = type_slot(program, body, "Boxed");
        assert!(unknown(facts.declared_type(body, slot)));
        let bump = Bump::new();
        let (value, levels) = rigid(only(facts, body));
        let boxed = lexical(program, 1, "Boxed");
        assert_eq!(value, reading(program.tiers(), &bump).list(boxed));
        assert_eq!(levels, [1]);
    });
}

#[test]
fn a_callable_and_its_registration_are_typed_at_load() {
    let source = "LET near = FN EXPR #(NEAR x :Number) -> Number = #(x)\n\
                  EXPR FOR ALL #[Elt] #(FUN x :Elt) -> Any = #(FN :{y :Elt} -> Elt = #(y))";
    loaded(source, |program| {
        let (facts, shape) = (program.facts(), program.shape());
        let bump = Bump::new();
        let types = reading(program.tiers(), &bump);
        let field = |name| {
            BinderSymbol::Value(ValueSymbol::declared(name, program.symbols()).expect("a name"))
        };
        let body = birth(program, "near");
        let near = body;
        let Static::Closed(callable) = facts.callable_type(body) else {
            panic!("the callable's type is closed")
        };
        let expected = types.function_type(&bump, &[(field("x"), KType::NUMBER)], KType::NUMBER);
        assert_eq!(callable.ktype, DeclaredType::<KType>::from(expected));
        let registration = (shape.registrations().iter())
            .find(|registration| {
                shape.births(registration.slot).map(BodyShape::segment) == Some(body.segment())
            })
            .expect("`near`'s registration");
        let Static::Closed(registered) = facts.registered_type(shape, registration.slot) else {
            panic!("the registration's shape is closed")
        };
        assert!(registered.quantifier_map.0.is_empty());

        let f = (shape.registrations().iter())
            .filter_map(|registration| shape.births(registration.slot))
            .find(|body| body.segment() != near.segment())
            .expect("`FUN` births its body");
        let inner = nested(f, ShapeKind::Callable);
        let Static::Rigid { value, variables } = facts.callable_type(inner) else {
            panic!("a callable over a run-bound name is rigid")
        };
        let elt = lexical(program, 0, "Elt");
        assert_eq!(
            value.ktype,
            types.function_type(&bump, &[(field("y"), elt)], elt).into()
        );
        assert_eq!(variables.len(), 1);
    });
}

#[test]
fn a_quote_s_holes_are_rigid_variables() {
    let source = "LET Alias = Number\n\
                  LET q = #(PRINT :(LIST OF Tee))\n\
                  LET r = #(PRINT :(LIST OF $Alias))";
    loaded(source, |program| {
        let facts = program.facts();
        let codes: Vec<_> = (program.shape().nested_shapes().iter())
            .filter(|(_, nested)| nested.kind() == ShapeKind::Code)
            .map(|(_, nested)| *nested)
            .collect();
        let [first, second] = codes[..] else {
            panic!("two quotes")
        };
        // Nested shapes are ordered by address, so the two quotes are told apart by their
        // captures: both open with `PRINT`'s key, and only the alias quote's `$` name is marked.
        let unmarked = |code: &BodyShape<'_>| code.captures().iter().all(|c| c.mark.is_none());
        let (hole, alias) = match unmarked(first) {
            true => (first, second),
            false => (second, first),
        };
        let bump = Bump::new();
        let types = reading(program.tiers(), &bump);
        let (value, levels) = rigid(only(facts, hole));
        assert_eq!(value, types.list(lexical(program, 0, "Tee")));
        assert_eq!(levels, [0]);
        assert_eq!(closed(only(facts, alias)), types.list(KType::NUMBER));
    });
}

#[test]
fn a_closed_type_that_does_not_elaborate_refuses_the_load() {
    for (source, at) in [
        ("NEWTYPE Bad = :(Number.z)", ":(Number.z)"),
        ("LET t = :(LIST OF Number.z)", "Number.z"),
        ("LET f = (FN :{} -> Any = #(:(Number.z)))", ":(Number.z)"),
    ] {
        let error = refused(source);
        let column = source.find(at).expect("the spelling is in the source") + 1;
        assert_eq!(
            error,
            format!("<test>:1:{column}: Number has no member z"),
            "`{source}`"
        );
    }
}

#[test]
fn a_refusal_in_a_quote_s_code_is_kept_for_its_eval() {
    loaded("LET q = #(PRINT :(Number.z))", |program| {
        let code = nested(program.shape(), ShapeKind::Code);
        let refusal = program.facts().typing_refusal(code);
        assert!(matches!(
            refusal,
            Some(crate::scope::ShapeError::Type { .. })
        ));
    });
}

#[test]
fn a_guard_written_twice_refuses_the_arm_set() {
    for source in [
        "MATCH 1 -> :Number WITH #{Number: (1), :(Number): (2)}",
        "LET Num = Number\nMATCH 1 -> :Number WITH #{Num: (1), Number: (2)}",
        "MATCH 1 -> :Number WITH #{:(Number | Str): (1), :(Str | Number): (2)}",
        "EXPR FOR ALL #[Elt] #(FUN x :Elt) -> Any = \
         #(MATCH x -> :Number WITH #{Elt: (1), :(Elt): (2)})",
    ] {
        let error = refused(source);
        assert!(error.contains("guards two arms"), "`{source}`: {error}");
    }
    loaded(
        "MATCH 1 -> :Number WITH #{Number: (1), Str: (2)}",
        |program| {
            let guards = (program.shape().type_expressions().iter())
                .filter(|expression| expression.guard.is_some())
                .count();
            assert_eq!(guards, 2);
        },
    );
}

#[test]
fn an_enclosing_for_all_name_keeps_its_level_along_the_chain() {
    let source = "EXPR FOR ALL #[Elt] #(FUN x :Elt) -> Any = \
                  #(FN :{y :Elt} -> Elt = #(:(LIST OF Elt)))";
    loaded(source, |program| {
        let facts = program.facts();
        let inner = nested(registered(program), ShapeKind::Callable);
        let bump = Bump::new();
        let types = reading(program.tiers(), &bump);
        let elt = lexical(program, 0, "Elt");
        let (value, levels) = rigid(only(facts, inner));
        assert_eq!(
            value,
            types.list(elt),
            "read through the inner body's capture"
        );
        assert_eq!(levels, [0]);
        let Static::Rigid { value, .. } = facts.callable_type(inner) else {
            panic!("the inner callable's type is rigid")
        };
        let ktype = (value.ktype.as_type()).expect("the inner callable is unquantified");
        let TypeNode::KFunction { ret, .. } = types.node(ktype) else {
            panic!("a function type")
        };
        assert_eq!(ret, elt, "its declared return is the same variable");
    });
}

/// The one path variable among `variables`, which a shape on the chain `facts` holds reads: its
/// entry.
fn path_entry<'graph>(facts: &LoadFacts<'graph>, variables: &[Variable]) -> PathEntry<'graph> {
    let [
        Variable {
            source: Source::Path { root, level, .. },
            ..
        },
    ] = variables
    else {
        panic!("one path variable, not {variables:?}")
    };
    let root = facts.shapes().shape_at(*root);
    (facts.path_entry(root, *level)).expect("a path variable reads a minted entry")
}

/// The path variable `typed` is rigid over: its value, a lexical variable, and its entry.
fn path<'graph>(
    facts: &LoadFacts<'graph>,
    typed: StaticType<'graph>,
) -> (Parametric, PathEntry<'graph>) {
    let Static::Rigid { value, variables } = typed else {
        panic!("a rigid type, not {typed:?}")
    };
    (value, path_entry(facts, variables))
}

/// Each path `shape`'s spellings read: its chain root and level.
fn path_reads(facts: &LoadFacts<'_>, shape: &BodyShape<'_>) -> Vec<(crate::scope::Segment, usize)> {
    (shape.spelled().iter())
        .filter_map(|spelled| facts.path_read(shape, Site::of(spelled.part)))
        .collect()
}

/// Paths through a module binder past every name level: two spellings of one path read one entry,
/// closed through its resolution (ruling 32), and a label the module lacks is a path variable of
/// its own.
#[test]
fn a_path_through_a_module_name_is_one_variable_past_every_name_level() {
    let source = "MODULE ints = (LET Carrier = Number)\n\
                  LET x :(ints.Carrier) = 1\n\
                  LET y :(ints.Carrier) = 2\n\
                  LET z :(ints.Other) = 3";
    loaded(source, |program| {
        let (facts, shape) = (program.facts(), program.shape());
        for statement in [1, 2] {
            let [expression] = written_in(shape, statement)[..] else {
                panic!("one type expression in statement {statement}")
            };
            let typed = facts.typed_expression(shape, expression.site);
            assert_eq!(closed(typed), KType::NUMBER, "statement {statement}");
        }
        // `ints`'s own `Carrier` takes level 0; the paths start past it.
        let [x, y, z] = path_reads(facts, shape)[..] else {
            panic!("three path reads")
        };
        assert_eq!((x.1, y.1), (1, 1), "two spellings of one path share it");
        assert_eq!(z.1, 2, "another label is another path");
        let [expression] = written_in(shape, 3)[..] else {
            panic!("one type expression in statement 3")
        };
        let (value, entry) = path(facts, facts.typed_expression(shape, expression.site));
        assert_eq!(value, lexical(program, 2, "Other"));
        assert!(entry.through.is_none(), "a binder with no declared type");
    });
}

#[test]
fn a_path_through_a_parameter_reads_through_its_declared_application() {
    let source = "SIG Mapper FOR ALL #[Carrier] = #[(VAL seed :Carrier)]\n\
                  LET f = (FN :{p :Mapper} -> Any = \
                  #(LET g = (FN :{x :(p.Carrier)} -> Any = #(x))))";
    loaded(source, |program| {
        let facts = program.facts();
        let body = birth(program, "f");
        let inner = nested(body, ShapeKind::Callable);
        let Static::Rigid { variables, .. } = facts.callable_type(inner) else {
            panic!("`g`'s type is rigid over the path")
        };
        let entry = path_entry(facts, variables);
        assert!(
            entry.through.is_some(),
            "`Mapper` leaves `Carrier` unpinned, so the path reads through it"
        );
        assert_eq!(
            path_reads(facts, body).len(),
            1,
            "the parameter type is spelled where `g` is born"
        );
    });
}

/// A type binder spelling a path through an annotated binder of its own body, whose annotation
/// reads another of the body's type binders: the path is read once the binders are typed, through
/// the annotation's application.
#[test]
fn a_path_through_an_annotated_binder_reads_after_the_binders() {
    let source = "SIG Counter FOR ALL #[Carrier] = \
                  #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n\
                  EXPR FOR ALL #[Elt] #(WRAP x :Elt AND mk :(FN :{} -> Counter)) -> Any = #(\n  \
                  LET Items = :(LIST OF Elt)\n  \
                  LET n :(Counter WITH {Carrier = Items}) = (mk {})\n  \
                  LET Holder = :{b :(n.Carrier)}\n  x\n)";
    loaded(source, |program| {
        let facts = program.facts();
        let body = nested(program.shape(), ShapeKind::Callable);
        let [(root, level)] = path_reads(facts, body)[..] else {
            panic!("one path read")
        };
        let root = facts.shapes().shape_at(root);
        let entry = (facts.path_entry(root, level)).expect("a minted entry");
        assert!(
            entry.through.is_some(),
            "the annotation's application names `Carrier` a head parameter"
        );
    });
}

/// A type binder over a path its walk resolves closed declares closed (ruling 32).
#[test]
fn a_type_binder_over_a_closed_path_declares_closed() {
    let source = "MODULE ints = (LET Carrier = Number)\nLET Elt = :(ints.Carrier)";
    loaded(source, |program| {
        let (facts, shape) = (program.facts(), program.shape());
        let slot = type_slot(program, shape, "Elt");
        assert_eq!(closed(facts.declared_type(shape, slot)), KType::NUMBER);
        assert_eq!(path_reads(facts, shape).len(), 1);
    });
}

/// The frontier of the unit of `shape` that holds `statement`.
fn unit_frontier<'graph>(
    facts: &LoadFacts<'graph>,
    shape: &BodyShape<'_>,
    statement: u32,
) -> &'graph [Unknown] {
    let unit = UnitId {
        segment: shape.segment(),
        unit: shape.unit_of_statement(statement as usize),
    };
    let (_, frontier) = facts.unit(unit).expect("the walk ran the unit");
    frontier.unknowns()
}

/// The frontier of the unit holding the one type expression `shape` records.
fn only_frontier<'graph>(facts: &LoadFacts<'graph>, shape: &BodyShape<'_>) -> &'graph [Unknown] {
    let shape = facts.shapes().shape_at(shape.segment());
    unit_frontier(facts, shape, the_expression(shape).statement)
}

/// The frontier of the unit `body` is born in, which fixes its callable type.
fn birth_frontier<'graph>(facts: &LoadFacts<'graph>, body: &BodyShape<'_>) -> &'graph [Unknown] {
    let shapes = facts.shapes();
    let (parent, statement) = shapes.parent(body.segment()).expect("a nested body");
    unit_frontier(facts, shapes.shape_at(parent), statement)
}

/// The unknown the type slot `name` of `shape` is.
fn unknown_at(program: &Program<'_>, shape: &BodyShape<'_>, name: &str) -> Unknown {
    Unknown {
        segment: shape.segment(),
        at: Address::Slot(type_slot(program, shape, name)),
    }
}

#[test]
fn a_cell_over_a_for_all_name_waits_on_its_callable_s_slot() {
    let source = "EXPR FOR ALL #[Elt] #(FUN x :Elt) -> Any = #(:(LIST OF Elt))";
    loaded(source, |program| {
        let body = registered(program);
        assert_eq!(
            only_frontier(program.facts(), body),
            [unknown_at(program, body, "Elt")]
        );
    });
}

#[test]
fn a_cell_that_reads_no_unknown_is_fixed() {
    loaded("LET t = :(LIST OF Number)", |program| {
        assert!(only_frontier(program.facts(), program.shape()).is_empty());
    });
    let source = "EXPR FOR ALL #[Elt] #(FUN x :Elt) -> Any = #(:(LIST OF Number))";
    loaded(source, |program| {
        assert!(only_frontier(program.facts(), registered(program)).is_empty());
    });
}

#[test]
fn an_inner_cell_over_an_outer_name_waits_on_the_outer_slot() {
    let source = "EXPR FOR ALL #[Ay] #(FUN x :Ay) -> Ay = \
                  #(\n  EXPR FOR ALL #[Be] #(GEN y :Be) -> Any = #(:(MAP Ay -> Be))\n  x\n)";
    loaded(source, |program| {
        let facts = program.facts();
        let outer = registered(program);
        let inner = nested(outer, ShapeKind::Callable);
        let mut expected = [
            unknown_at(program, outer, "Ay"),
            unknown_at(program, inner, "Be"),
        ];
        expected.sort();
        assert_eq!(only_frontier(facts, inner), expected);
        assert!(
            birth_frontier(facts, inner).is_empty(),
            "`g`'s type reads no outer name"
        );
    });
}

#[test]
fn sibling_bodies_numbering_their_names_alike_wait_on_distinct_unknowns() {
    let source = "EXPR FOR ALL #[Elt] #(FUN x :Elt) -> Elt = #(\n  \
                  EXPR FOR ALL #[Ay] #(GEN a :Ay) -> Any = #(:(LIST OF Ay))\n  \
                  EXPR FOR ALL #[Be] #(HOLD b :Be) -> Any = #(:(LIST OF Be))\n  \
                  x\n)";
    loaded(source, |program| {
        let facts = program.facts();
        let [g, h] = callables(registered(program))[..] else {
            panic!("two inner callables")
        };
        assert_eq!(
            (facts.group_levels(g), facts.group_levels(h)),
            (
                &[lexical(program, 1, "Ay")][..],
                &[lexical(program, 1, "Be")][..]
            ),
            "the two number their names alike"
        );
        let (one, other) = (only_frontier(facts, g), only_frontier(facts, h));
        assert_eq!((one.len(), other.len()), (1, 1));
        assert_ne!(one, other);
        assert_eq!(one[0].segment, g.segment());
        assert_eq!(other[0].segment, h.segment());
    });
}

/// A type binder over a run-bound name: the unit declaring it waits on that name and is the
/// binder's own, and its reader waits on both.
#[test]
fn a_type_binder_over_a_run_bound_name_waits_on_it_and_its_reader_on_both() {
    let source = "EXPR FOR ALL #[Elt] #(FUN x :Elt) -> Any = #(\
                  (NEWTYPE Boxed = :{v :Elt}) \
                  (:(LIST OF Boxed)))";
    loaded(source, |program| {
        let facts = program.facts();
        let body = registered(program);
        let (elt, boxed) = (
            unknown_at(program, body, "Elt"),
            unknown_at(program, body, "Boxed"),
        );
        let mut both = [elt, boxed];
        both.sort();
        assert_eq!(unit_frontier(facts, body, 0), both);
        assert_eq!(only_frontier(facts, body), both);
    });
}

#[test]
fn a_meet_over_a_for_all_name_waits_on_it() {
    let source = "EXPR FOR ALL #[Elt] #(FUN x :Elt) -> Any = #(:(Elt & Number))";
    loaded(source, |program| {
        let body = registered(program);
        assert_eq!(
            only_frontier(program.facts(), body),
            [unknown_at(program, body, "Elt")]
        );
    });
}

#[test]
fn an_inner_callable_over_an_outer_name_waits_on_the_outer_slot() {
    let source = "EXPR FOR ALL #[Elt] #(FUN x :Elt) -> Elt = \
                  #(\n  EXPR FOR ALL #[Tee] #(GEN t :Tee AND u :Elt) -> Tee = #(t)\n  x\n)";
    loaded(source, |program| {
        let outer = registered(program);
        let inner = nested(outer, ShapeKind::Callable);
        assert!(
            birth_frontier(program.facts(), inner).contains(&unknown_at(program, outer, "Elt"))
        );
    });
}
