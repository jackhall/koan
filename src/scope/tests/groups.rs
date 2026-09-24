//! Operator groups before any rewrite: which symbols the builtin groups cover, what a `GROUP`
//! body's member scan reads, and which declarations the position-blind pre-scan refuses.

use crate::scope::groups::{self, BuiltinGroup, Claim, declared_group, groups_equal, scan_members};
use crate::scope::{ShapeError, is_equality};
use crate::symbols::{KeywordSymbol, SymbolInterner};
use crate::type_lattice::{FoldDirection, ReductionMode};

use super::{Fixture, with_fixture};

fn keyword(text: &str, symbols: &SymbolInterner) -> KeywordSymbol {
    KeywordSymbol::declared(text, symbols).expect("a keyword token")
}

/// The pre-scan over `source`'s top-level statements, with no enclosing code.
fn claimed<R>(
    source: &str,
    check: impl for<'f, 'graph> FnOnce(
        &Fixture<'f, 'graph>,
        Result<&groups::Claims<'graph>, ShapeError>,
    ) -> R,
) -> R {
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        let claims = groups::claims(fixture.program, fixture.scratch(), lines.iter(), None);
        check(fixture, claims)
    })
}

#[test]
fn the_builtin_groups_cover_twelve_symbols_and_equality_belongs_to_none() {
    with_fixture(|fixture| {
        let symbols = fixture.symbols;
        let covered = [
            ("<", BuiltinGroup::Comparison),
            ("<=", BuiltinGroup::Comparison),
            (">", BuiltinGroup::Comparison),
            (">=", BuiltinGroup::Comparison),
            ("+", BuiltinGroup::Additive),
            ("-", BuiltinGroup::Additive),
            ("*", BuiltinGroup::Multiplicative),
            ("/", BuiltinGroup::Multiplicative),
            ("|", BuiltinGroup::Union),
            ("&", BuiltinGroup::Meet),
        ];
        for (text, group) in covered {
            assert_eq!(
                BuiltinGroup::of(keyword(text, symbols)),
                Some(group),
                "`{text}` is covered"
            );
        }
        assert_eq!(
            BuiltinGroup::Comparison.mode(),
            ReductionMode::Pairwise {
                combiner: keyword("AND", symbols),
                direction: FoldDirection::Left,
            }
        );
        assert_eq!(BuiltinGroup::Additive.mode(), ReductionMode::FoldLeft);
        assert_eq!(BuiltinGroup::Union.mode(), ReductionMode::Unary);
        assert_eq!(BuiltinGroup::Meet.mode(), ReductionMode::Unary);
        for text in ["==", "!="] {
            let symbol = keyword(text, symbols);
            assert!(is_equality(symbol), "`{text}` is an equality symbol");
            assert_eq!(BuiltinGroup::of(symbol), None, "`{text}` is in no group");
        }
    });
}

#[test]
fn a_group_body_declares_its_binary_operators_and_nothing_else() {
    with_fixture(|fixture| {
        let scratch = fixture.scratch();
        let fold = fixture.parse(
            "GROUP ring FOLD RIGHT = (\
             (OP #(@) OVER Ring = (left))\
             (OP #(%) OVER Ring = (right)))",
        );
        let group = declared_group(&fold[0], scratch)
            .expect("the body is a run of binary operators")
            .expect("the statement is a `GROUP`");
        assert_eq!(group.mode, ReductionMode::FoldRight);
        let mut expected = [keyword("@", fixture.symbols), keyword("%", fixture.symbols)];
        expected.sort_unstable();
        assert_eq!(group.members, expected);

        let pairwise = fixture
            .parse("GROUP cmp PAIRWISE FOLD #(AND) LEFT = ((OP #(~~) OVER Ring -> Bool = (left)))");
        let group = declared_group(&pairwise[0], scratch)
            .expect("the body is a run of binary operators")
            .expect("the statement is a `GROUP`");
        assert_eq!(
            group.mode,
            ReductionMode::Pairwise {
                combiner: keyword("AND", fixture.symbols),
                direction: FoldDirection::Left,
            }
        );

        // A `UNARY OP` is no member, and a body declaring no operator declares no group.
        let unary =
            fixture.parse("GROUP bad FOLD LEFT = ((UNARY OP #(@) OVER Ring -> Ring = (operands)))");
        assert!(declared_group(&unary[0], scratch).is_err());
        let empty = fixture.parse("GROUP bad FOLD LEFT = ((LET x = 1))");
        assert!(declared_group(&empty[0], scratch).is_err());

        // The scan reads a body directly, sorted and deduped.
        let twice = fixture.parse(
            "GROUP ring FOLD LEFT = (\
             (OP #(@) OVER Ring = (left))\
             (OP #(@) OVER Ring -> Ring = (right)))",
        );
        let group = declared_group(&twice[0], scratch).unwrap().unwrap();
        assert_eq!(group.members, [keyword("@", fixture.symbols)]);
        let body = fixture.parse("(OP #(@) OVER Ring = (left))");
        assert_eq!(
            &*scan_members(&body[0], scratch).expect("one member"),
            [keyword("@", fixture.symbols)]
        );
    });
}

#[test]
fn a_group_equal_to_a_builtin_one_is_that_group_and_a_partial_one_is_refused() {
    claimed(
        "GROUP additive FOLD LEFT = (\
         (OP #(+) OVER Number = (left))\
         (OP #(-) OVER Number = (right)))",
        |fixture, claims| {
            let claims = claims.expect("an equal group declares nothing new");
            assert!(claims.get(keyword("+", fixture.symbols)).is_none());
        },
    );
    claimed(
        "GROUP partial FOLD LEFT = ((OP #(+) OVER Number = (left)))",
        |fixture, claims| {
            assert_eq!(
                claims.err(),
                Some(ShapeError::RedeclaresGroup {
                    symbol: keyword("+", fixture.symbols),
                    at: crate::scope::Position::PARAMETER,
                })
            );
        },
    );
}

#[test]
fn two_equal_groups_are_one_record_and_two_unequal_ones_are_refused() {
    let body = |name: &str, mode: &str| {
        format!(
            "MODULE {name} = (GROUP g {mode} = (\
             (OP #(@) OVER Ring = (left))\
             (OP #(%) OVER Ring = (right))))"
        )
    };
    let equal = format!(
        "{}\n{}",
        body("first", "FOLD LEFT"),
        body("second", "FOLD LEFT")
    );
    claimed(&equal, |fixture, claims| {
        let claims = claims.expect("two equal groups agree");
        let at = keyword("@", fixture.symbols);
        let percent = keyword("%", fixture.symbols);
        let (Some(Claim::Group(left)), Some(Claim::Group(right))) =
            (claims.get(at), claims.get(percent))
        else {
            panic!("both members are claimed by a group");
        };
        assert!(std::ptr::eq(left, right), "one record for one group");
        assert!(groups_equal(left, right));
    });

    let unequal = format!(
        "{}\n{}",
        body("first", "FOLD LEFT"),
        body("second", "FOLD RIGHT")
    );
    claimed(&unequal, |_, claims| {
        assert!(matches!(
            claims.err(),
            Some(ShapeError::RedeclaresGroup { .. })
        ));
    });

    let narrower = format!(
        "{}\nMODULE second = (GROUP g FOLD LEFT = ((OP #(@) OVER Ring = (left))))",
        body("first", "FOLD LEFT")
    );
    claimed(&narrower, |_, claims| {
        assert!(matches!(
            claims.err(),
            Some(ShapeError::RedeclaresGroup { .. })
        ));
    });
}

#[test]
fn a_unary_mark_and_a_group_never_claim_one_symbol_in_either_order() {
    let unary = "UNARY OP #(~) OVER Ring -> Ring = (operands)";
    let group = "GROUP g FOLD LEFT = ((OP #(~) OVER Ring = (left)))";
    for source in [format!("{unary}\n{group}"), format!("{group}\n{unary}")] {
        claimed(&source, |fixture, claims| {
            assert_eq!(
                claims.err(),
                Some(ShapeError::RedeclaresGroup {
                    symbol: keyword("~", fixture.symbols),
                    at: crate::scope::Position::PARAMETER,
                })
            );
        });
    }
    // The same mark twice is one fact about the symbol, so two siblings agree.
    let twice = format!("MODULE first = ({unary})\nMODULE second = ({unary})");
    claimed(&twice, |fixture, claims| {
        let claims = claims.expect("two unary marks agree");
        assert!(matches!(
            claims.get(keyword("~", fixture.symbols)),
            Some(Claim::Unary)
        ));
    });
}

#[test]
fn a_declaration_naming_the_derived_symbol_is_refused_and_a_quoted_group_is_data() {
    claimed("OP #(!=) OVER Ring -> Bool = (left)", |fixture, claims| {
        assert_eq!(
            claims.err(),
            Some(ShapeError::Derived {
                symbol: keyword("!=", fixture.symbols),
                at: crate::scope::Position::PARAMETER,
            })
        );
    });
    claimed(
        "LET quoted = #(GROUP g FOLD LEFT = ((OP #(+) OVER Number = (left))))",
        |fixture, claims| {
            let claims = claims.expect("a quote is data");
            assert!(claims.get(keyword("+", fixture.symbols)).is_none());
        },
    );
}
