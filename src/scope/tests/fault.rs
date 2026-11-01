//! A fault's wording and the types it names ([fault.rs](../fault.rs)): each kind renders its
//! refusal's message, and hands every type it names and every value it holds to a caching body's
//! exit.

use crate::parse::KeyElement;
use crate::scope::{
    CodeRefused, CoercionRefused, ComposeRefused, Elaboration, Fault, FaultKind, Named, Site,
    Untieable, UsingRefused,
};
use crate::symbols::{BinderSymbol, KeywordSymbol, TypeSymbol};
use crate::type_lattice::{DeclaredType, KType, Parametric};
use crate::values::{ConstructionRefused, KeyRejected, SealRefused, Value};

use super::{value_name, with_fixture};

#[test]
fn each_kind_renders_its_message() {
    with_fixture(|fixture| {
        let symbols = fixture.symbols;
        let keyword = KeywordSymbol::declared("FOO", symbols).expect("a keyword");
        let key = [KeyElement::Keyword(keyword), KeyElement::Slot];
        let name = value_name("x", symbols);
        let number = DeclaredType::from(KType::NUMBER);
        let (n, s) = (KType::NUMBER, KType::STR);
        let (one, text) = (Value::Number(1.0), Value::Str("a"));
        let cases: [(Fault<'_>, &str); 36] = [
            (
                Fault::NoOverload {
                    key: &key,
                    arguments: &[one],
                    types: &[n],
                },
                "no overload of `FOO _` admits (Number)",
            ),
            (
                Fault::AmbiguousOverloads {
                    key: &key,
                    arguments: &[one],
                    types: &[n],
                    count: 2,
                },
                "ambiguous call of `FOO _`: 2 overloads admit (Number) and none ranks first",
            ),
            (
                Fault::NoArm {
                    scrutinee: one,
                    union: None,
                },
                "no arm of `MATCH` admits Number",
            ),
            (
                Fault::AmbiguousArms {
                    scrutinee: one,
                    count: 2,
                },
                "ambiguous `MATCH`: 2 arms admit Number and none ranks first",
            ),
            (
                Fault::NoField {
                    of: one,
                    field: name.symbol(),
                },
                "Number has no field x",
            ),
            (
                Fault::NoMember {
                    of: one,
                    member: name.symbol(),
                },
                "Number has no member x",
            ),
            (
                Fault::Incomparable {
                    left: one,
                    right: text,
                },
                "Number and Str cannot be compared",
            ),
            (
                Fault::NotAModule { value: one },
                "Number is no module to ascribe",
            ),
            (
                Fault::Coercion {
                    name: name.symbol(),
                    refused: CoercionRefused::NotAFunction,
                },
                "member x cannot take its view's type: it is no function",
            ),
            (
                Fault::QuantifiedMember {
                    name: name.symbol(),
                },
                "member x is quantified, and no type was known to instantiate it at",
            ),
            (
                Fault::BarrierInstance {
                    name: name.symbol(),
                },
                "member x is quantified behind its view's barrier, and is read only at a call's \
                 head",
            ),
            (
                Fault::Unascribable {
                    value: one,
                    ascribed: s,
                },
                "Number does not satisfy its ascription Str",
            ),
            (
                Fault::NotCode { value: one },
                "Number is not code for `EVAL` to run",
            ),
            (
                Fault::Uncomposed(ComposeRefused::NotAList { value: n }),
                "`$..` splices a list of code, and Number is none",
            ),
            (
                Fault::Uncomposed(ComposeRefused::NotCode { element: n }),
                "a spliced element is no code: Number",
            ),
            (
                Fault::Uncomposed(ComposeRefused::TooDeep),
                "composed code nests deeper than the syntax depth limit of 1024",
            ),
            (
                Fault::Unfilled(UsingRefused::Ranking {
                    key: symbols.key("FOO _").expect("a key"),
                }),
                "FOO _ is ranked two ways",
            ),
            (
                Fault::NotAKey { key: Value::Null },
                "Null cannot be a dict key",
            ),
            (
                Fault::NotAKey {
                    key: Value::Number(f64::NAN),
                },
                "NaN cannot be a dict key",
            ),
            (
                Fault::NotAList { value: one },
                "`..` spreads a list, and Number is none",
            ),
            (
                Fault::Unreadable {
                    head: text,
                    argument: text,
                },
                "Str is read by a `Number` or by a slice record naming `from` or `to` as \
                 `Number`s, not by Str",
            ),
            (Fault::NotCallable { callee: one }, "Number is not callable"),
            (
                Fault::Misnamed {
                    callee: one,
                    signature: number,
                    arguments: text,
                },
                "arguments Str do not name the parameters of Number",
            ),
            (
                Fault::Unfit {
                    callee: one,
                    signature: number,
                    arguments: text,
                    refused: None,
                },
                "Number cannot be called with Str",
            ),
            (
                Fault::Unfit {
                    callee: one,
                    signature: number,
                    arguments: text,
                    refused: Some(&CoercionRefused::Seal(SealRefused::NotSealed {
                        carrier: n,
                    })),
                },
                "Number cannot be called with Str: it is not sealed under Number",
            ),
            (
                Fault::Unsolved {
                    callee: one,
                    signature: number,
                    arguments: text,
                },
                "Number cannot be solved against Str",
            ),
            (
                Fault::Returned {
                    callee: Some(number),
                    value: text,
                    returns: n,
                },
                "Number returned Str, which does not satisfy Number",
            ),
            (
                Fault::Returned {
                    callee: None,
                    value: text,
                    returns: n,
                },
                "`EVAL`'s code returned Str, which does not satisfy Number",
            ),
            (
                Fault::Annotated {
                    value: text,
                    annotation: n,
                },
                "Str does not satisfy its annotation Number",
            ),
            (
                Fault::CrossedOut(CoercionRefused::NoUnionMember),
                "a value returned through a view cannot take its type: no member of its union \
                 admits it",
            ),
            (
                Fault::Unrunnable(CodeRefused::Unbound(BinderSymbol::Value(name))),
                "unbound name 'x'",
            ),
            (
                Fault::Unrunnable(CodeRefused::QuantifiedRead(BinderSymbol::Value(name))),
                "`x` is quantified, so it is read only at the head of a call; wrap it in an \
                 unquantified `FN` to pass it",
            ),
            (
                Fault::Untied(Untieable::Opaque {
                    name: BinderSymbol::Value(name),
                }),
                "`x` is bound to nothing a knot holds",
            ),
            (
                Fault::Unconstructible(ConstructionRefused::NotConstructible(n)),
                "Number is not callable",
            ),
            (
                Fault::Unelaborated(Elaboration::NotAType {
                    name: TypeSymbol::declared("Foo", symbols).expect("a type name"),
                    site: Site(0),
                }),
                "Foo names no type",
            ),
            (
                Fault::Untied(Untieable::Key {
                    name: BinderSymbol::Value(name),
                    site: Site(0),
                    rejected: KeyRejected::NaN,
                }),
                "NaN cannot be a dict key",
            ),
        ];
        for (fault, message) in cases {
            assert_eq!(
                fault.display(symbols, fixture.types).to_string(),
                message,
                "{fault:?}"
            );
        }
    });
}

#[test]
fn a_fault_names_every_type_it_holds_beside_its_values() {
    let named = |fault: Fault<'_>| {
        let mut named: Vec<DeclaredType<Parametric>> = Vec::new();
        fault.each_named(&mut |kt| named.push(kt));
        let mut values = 0;
        fault.values(&mut |_| values += 1);
        (named, values)
    };
    let (n, s) = (KType::NUMBER, KType::STR);
    let (one, text) = (Value::Number(1.0), Value::Str("a"));
    assert_eq!(
        named(Fault::Returned {
            callee: Some(DeclaredType::from(KType::BOOL)),
            value: text,
            returns: n,
        }),
        (vec![s.into(), KType::BOOL.into(), n.into()], 1)
    );
    assert_eq!(
        named(Fault::NoOverload {
            key: &[],
            arguments: &[one, text],
            types: &[n, s],
        }),
        (vec![n.into(), s.into(), n.into(), s.into()], 2)
    );
    assert_eq!(
        named(Fault::Unfit {
            callee: one,
            signature: n.into(),
            arguments: text,
            refused: Some(&CoercionRefused::Seal(SealRefused::Misfit {
                carrier: KType::NULL,
                witness: KType::BOOL,
            })),
        }),
        (
            vec![
                n.into(),
                s.into(),
                n.into(),
                KType::NULL.into(),
                KType::BOOL.into()
            ],
            2
        )
    );
    assert_eq!(
        named(Fault::Unfilled(UsingRefused::Ranking {
            key: KeyElement::key([KeyElement::Slot])
        })),
        (vec![], 0)
    );
}

#[test]
fn a_trace_line_drops_only_the_parentheses_grouping_its_whole_call() {
    use crate::scope::fault::ungrouped;
    assert_eq!(ungrouped("(USE 1)"), "USE 1");
    assert_eq!(ungrouped("USE 1"), "USE 1");
    assert_eq!(
        ungrouped("(LIFT #(\"s\")) {x = 1}"),
        "(LIFT #(\"s\")) {x = 1}"
    );
    assert_eq!(ungrouped("(a) + (b)"), "(a) + (b)");
    assert_eq!(ungrouped("(PRINT \")\" x)"), "PRINT \")\" x");
    assert_eq!(ungrouped("(PRINT \"\\\")\")"), "PRINT \"\\\")\"");
}

#[test]
fn each_kind_names_its_variant() {
    let names = FaultKind::ALL.map(FaultKind::name);
    for (at, name) in names.iter().enumerate() {
        assert!(
            TypeSymbol::classify(name).is_some(),
            "{name} is a type token"
        );
        assert!(!names[..at].contains(name), "{name} names one kind");
    }
}
