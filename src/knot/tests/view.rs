//! Code views: what a read of code answers, laid flat by the materializer, its scopes and pins as
//! the scope README's rules give them, and its memos agreeing with what it lays.

use proptest::prelude::*;

use crate::memory::{Bump, program_storage};
use crate::parse::{ExpressionPart, KExpression, Mark, Scope};
use crate::type_lattice::display_name;
use crate::values::{Chain, Knotted as _, Value};

use super::super::view::{CodeTree, materialized};
use super::super::{Knotted, code_length, code_part, code_slice};
use super::{Fixture, bound, pin, with_fixture};

/// The quote bound under `name`.
fn code<'graph, 'cell>(
    fixture: &Fixture<'_, 'graph>,
    activation: &crate::knot::KActivation<'graph, 'cell>,
    name: &str,
) -> Knotted<'graph, 'cell> {
    bound(fixture, activation, name)
        .as_code()
        .unwrap_or_else(|| panic!("`{name}` is bound to code"))
}

fn rendered(
    fixture: &Fixture<'_, '_>,
    types: &Chain<'_, '_, '_, '_>,
    member: Knotted<'_, '_>,
) -> String {
    let mut out = String::new();
    Value::Knotted(member)
        .render(&mut out, types, fixture.symbols, fixture.scratch())
        .unwrap();
    out
}

/// The view `member` holds.
fn tree<'graph, 'cell>(member: Knotted<'graph, 'cell>) -> &'cell CodeTree<'graph, 'cell> {
    member
        .code()
        .and_then(|node| node.view())
        .expect("a code view")
}

/// Every mark `node` holds, in order, its nested nodes' included.
fn marks(node: &KExpression<'_>, out: &mut Vec<Mark>) {
    for part in node.parts {
        match &part.value {
            ExpressionPart::MarkedName(mark, _) => out.push(*mark),
            ExpressionPart::MarkedUse(mark, node) => {
                out.push(*mark);
                marks(node, out);
            }
            ExpressionPart::Expression(node) | ExpressionPart::QuotedExpression(node) => {
                marks(node, out)
            }
            _ => {}
        }
    }
}

#[test]
fn a_part_of_code_is_its_groups_node_or_a_one_part_node() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET k = 7\nLET q = #((LET x = 1) (PRINT $k) 7)");
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let activation = fixture.run(writer, types, &lines, &[]);
            let q = code(fixture, activation, "q");
            assert_eq!(code_length(q), 3);
            let part = |at| code_part(writer, q, at, types).expect("a part");
            let typed = |member: Knotted<'_, '_>| {
                display_name(member.ktype(), types, fixture.symbols).to_string()
            };
            assert_eq!(rendered(fixture, types, part(0)), "LET x = 1");
            assert_eq!(typed(part(0)), ":(UNBUILT Binder)");
            assert_eq!(rendered(fixture, types, part(1)), "PRINT $k");
            assert_eq!(typed(part(1)), ":(UNBUILT Expression)");
            assert_eq!(rendered(fixture, types, part(2)), "7");
            assert_eq!(typed(part(2)), ":(UNBUILT Literal)");
            assert!(code_part(writer, q, 3, types).is_none());
            // A part read again of a part reads the view, not the source.
            let inner = code_part(writer, part(1), 1, types).expect("a part of a part");
            assert_eq!(rendered(fixture, types, inner), "$k");
            assert_eq!(code_length(part(1)), 2);
        });
    });
}

#[test]
fn a_slice_of_code_is_the_node_of_exactly_its_parts_either_way() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET q = #((LET x = 1) (PRINT x) (PRINT 2))");
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let activation = fixture.run(writer, types, &lines, &[]);
            let q = code(fixture, activation, "q");
            let slice = |range, backward| code_slice(writer, q, range, backward, types);
            let forward = slice((1, 3), false);
            assert_eq!(rendered(fixture, types, forward), "(PRINT x) (PRINT 2)");
            assert_eq!(
                display_name(forward.ktype(), types, fixture.symbols).to_string(),
                ":(UNBUILT Block)"
            );
            let backward = slice((0, 3), true);
            assert_eq!(
                rendered(fixture, types, backward),
                "(PRINT 2) (PRINT x) (LET x = 1)"
            );
            let empty = slice((1, 1), false);
            assert_eq!(rendered(fixture, types, empty), "");
            assert_eq!(code_length(empty), 0);
            // A slice of a backward slice reads in its order.
            let again = code_slice(writer, backward, (1, 3), false, types);
            assert_eq!(rendered(fixture, types, again), "(PRINT x) (LET x = 1)");
        });
    });
}

/// A `$` name a builtin's body owned in its quote is pinned where a read lays it inside a quote
/// value; a quote value's own mark keeps its scope and brings no binding.
#[test]
fn a_read_pins_a_root_owned_mark_it_moves_into_a_quote_value() {
    with_fixture(|fixture| {
        let lines = fixture.parse(
            "LET x = 3\nLET y = 4\nLET q = #(FN :{a :Number} -> Any = #(PRINT $x))\n\
             LET p = #(LET r = #(PRINT $y))",
        );
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let activation = fixture.run(writer, types, &lines, &[]);
            let (storage, scratch) = (program_storage(), Bump::new());
            let laid = |member| {
                let flat = materialized(storage.brand().writer(), tree(member), &scratch);
                let mut found = Vec::new();
                marks(flat.body, &mut found);
                (
                    found,
                    flat.bindings
                        .iter()
                        .map(|binding| binding.key())
                        .collect::<Vec<_>>(),
                )
            };
            let body = code_slice(writer, code(fixture, activation, "q"), (5, 6), false, types);
            assert_eq!(rendered(fixture, types, body), "#(PRINT $x)");
            let pinned = Scope {
                number: 0,
                pinned: true,
            };
            let (found, bindings) = laid(body);
            assert_eq!(found, [Mark::Written(pinned)]);
            assert_eq!(bindings, [(Scope::WRITTEN, fixture.name("x"))]);
            // The inner quote's `$y` belongs to the quote value it lies in, before and after.
            let value = code_slice(writer, code(fixture, activation, "p"), (3, 4), false, types);
            let (found, bindings) = laid(value);
            assert_eq!(found, [Mark::DOLLAR]);
            assert!(bindings.is_empty());
        });
    });
}

/// The quote every law case reads: forty-odd parts of every kind, its `$k` marks owned by its
/// root, by a builtin's body, and by a quote value.
const LONG: &str = "LET k = 7\nLET q = #($k PLUS (LET a = $k) MINUS #(PRINT $k) PLUS [1, $k] \
     MINUS \"s\" PLUS (b) MINUS (FN :{c :Number} -> Number = #($k)) PLUS 3 MINUS $k PLUS (d e) \
     MINUS 4 PLUS (PRINT $k) MINUS 5 PLUS #(6) MINUS (7) PLUS $k MINUS 8 PLUS (9) MINUS 10)";

proptest! {
    #![proptest_config(ProptestConfig {
        cases: crate::tests::case_share(1, 4),
        ..ProptestConfig::default()
    })]

    /// A chain of slices of a quote holds the tree's laws, and lays flat as the parts it names, as
    /// deep as its memo says, of the kind it is typed by, and within its weight.
    #[test]
    fn a_view_lays_flat_as_its_memos_say(
        cuts in prop::collection::vec((0.0..=1.0f64, 0.0..=1.0f64, any::<bool>()), 1..5),
    ) {
        with_fixture(|fixture| {
            let lines = fixture.parse(LONG);
            fixture.in_cell(pin, |context| {
                let types = &fixture.tiers.of(context);
                let writer = context.writer();
                let activation = fixture.run(writer, types, &lines, &[]);
                let q = code(fixture, activation, "q");
                let body = q.code().and_then(|node| node.body()).expect("flat");
                let mut model: Vec<String> =
                    body.parts.iter().map(|part| part.value.summarize(fixture.symbols)).collect();
                let mut current = q;
                for (from, to, backward) in &cuts {
                    let len = model.len();
                    let (a, b) = ((from * len as f64) as usize, (to * len as f64) as usize);
                    let (from, to) = (a.min(b), a.max(b));
                    current = code_slice(writer, current, (from, to), *backward, types);
                    model = model[from..to].to_vec();
                    if *backward {
                        model.reverse();
                    }
                }
                let view = tree(current);
                assert!(view.root().holds_its_laws());
                assert_eq!(view.len(), model.len());
                assert_eq!(rendered(fixture, types, current), model.join(" "));
                let (storage, scratch) = (program_storage(), Bump::new());
                let flat = materialized(storage.brand().writer(), view, &scratch);
                assert_eq!(view.depth() as usize, flat.body.depth());
                assert_eq!(view.kind(), flat.body.code_kind());
                let laid = flat.body.weight() as usize
                    + flat.bindings.len() * size_of::<crate::values::Binding<'_, Knotted<'_, '_>>>();
                assert!(view.weight().bytes() >= laid, "{} < {laid}", view.weight().bytes());
            });
        });
    }
}

/// A part read out of a quote equals the quote written of it alone: a name its binder filled is a
/// hole again, and a `$` name keeps the binding its quote gave it.
#[test]
fn a_fragment_equals_the_quote_written_of_its_parts() {
    with_fixture(|fixture| {
        let lines = fixture.parse(
            "LET k = 1\nLET q = #((LET x = 4) (PRINT x))\nLET p = #((LET x = 4) (PRINT $k))\n\
             LET w = #(PRINT x)\nLET v = #(PRINT $k)",
        );
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let activation = fixture.run(writer, types, &lines, &[]);
            let scratch = fixture.scratch();
            let part = |name, at| {
                Value::Knotted(
                    code_part(writer, code(fixture, activation, name), at, types).unwrap(),
                )
            };
            let written = |name| Value::Knotted(code(fixture, activation, name));
            assert_eq!(part("q", 1).equals(&written("w"), types, scratch), Ok(true));
            assert_eq!(part("p", 1).equals(&written("v"), types, scratch), Ok(true));
            assert_eq!(
                part("p", 1).equals(&written("w"), types, scratch),
                Ok(false)
            );
            let (storage, laying) = (program_storage(), Bump::new());
            let flat = |name| {
                let member = code_part(writer, code(fixture, activation, name), 1, types).unwrap();
                materialized(storage.brand().writer(), tree(member), &laying).bindings
            };
            assert!(flat("q").is_empty(), "`x` is a hole again");
            assert_eq!(flat("p").len(), 1);
        });
    });
}

/// A part read out of a knot's quote holds the fellow member it names as that member's value, not
/// as an edge: no code value outside a knot holds one.
#[test]
fn a_fragment_of_a_knot_quote_holds_its_member_as_a_value() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET echo = #(PRINT $echo)");
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let activation = fixture.run(writer, types, &lines, &[]);
            let echo = code(fixture, activation, "echo");
            let part = code_part(writer, echo, 1, types).expect("a part");
            assert!(tree(part).weight().bytes() >= echo.weight().bytes());
            let (storage, scratch) = (program_storage(), Bump::new());
            let flat = materialized(storage.brand().writer(), tree(part), &scratch);
            let [binding] = flat.bindings[..] else {
                panic!("one binding")
            };
            match binding.link {
                crate::values::Link::Value(Value::Knotted(member)) => assert!(member == echo),
                _ => panic!("`$echo` is held as its member's value"),
            }
        });
    });
}

/// A splice's operand is read only to compose its quote, so a chain of quotes each splicing the
/// last weighs what its syntax and bindings do, linear in its length: a view holding each operand
/// beside its elements would weigh twice the last at each link.
#[test]
fn a_chain_of_composed_quotes_weighs_linearly_in_its_length() {
    let weight = |n: usize| {
        let mut source = String::from("LET k = 1\nLET a0 = #(PRINT $k)\nLET l0 = [a0]\n");
        for i in 1..=n {
            source += &format!("LET a{i} = #(g $..l{})\nLET l{i} = [a{i}]\n", i - 1);
        }
        with_fixture(|fixture| {
            let lines = fixture.parse(&source);
            fixture.in_cell(pin, |context| {
                let types = &fixture.tiers.of(context);
                let activation = fixture.run(context.writer(), types, &lines, &[]);
                tree(code(fixture, activation, &format!("a{n}")))
                    .weight()
                    .bytes()
            })
        })
    };
    let [at8, at16, at24] = [8, 16, 24].map(weight);
    let (first, second) = (at16 - at8, at24 - at16);
    assert!(
        second <= first + first / 8,
        "{at8} at 8, {at16} at 16, {at24} at 24"
    );
}

/// Composed views lay flat within their weight, their new nodes' binder plans included: a chain,
/// and an `FN` whose signature a splice lays beside a nested view.
#[test]
fn a_composed_view_lays_flat_within_its_weight() {
    with_fixture(|fixture| {
        let lines = fixture.parse(
            "LET k = 1\nLET a0 = #(PRINT $k)\nLET l0 = [a0]\nLET a1 = #((g $..l0) (h $..l0))\n\
             LET l1 = [a1]\nLET a2 = #(f $..l1 $k)\nLET fn = #(FN)\nLET sig = #(:{a :Number, b :Str})\n\
             LET arrow = #(->)\nLET any = #(Any)\nLET eq = #(=)\nLET body = #(#(PRINT $k))\n\
             LET head = [fn, sig, arrow, any, eq, body]\nLET f = #($..head)",
        );
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let activation = fixture.run(context.writer(), types, &lines, &[]);
            for name in ["a1", "a2", "f"] {
                let view = tree(code(fixture, activation, name));
                let (storage, scratch) = (program_storage(), Bump::new());
                let flat = materialized(storage.brand().writer(), view, &scratch);
                let laid = flat.body.weight() as usize
                    + flat.bindings.len()
                        * size_of::<crate::values::Binding<'_, Knotted<'_, '_>>>();
                assert!(
                    view.weight().bytes() >= laid,
                    "`{name}`: {} < {laid}",
                    view.weight().bytes()
                );
            }
        });
    });
}

/// A fill is a binding the filled mark reads, so a slice of filled code holds both.
#[test]
fn a_filled_hole_keeps_its_fill_when_sliced() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET q = #((PRINT x) (PRINT 2))\nLET r = {x = 1}");
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let activation = fixture.run(writer, types, &lines, &[]);
            let filled = crate::knot::using(
                writer,
                &fixture.code(),
                &fixture.facts(),
                code(fixture, activation, "q"),
                bound(fixture, activation, "r"),
                types,
                fixture.scratch(),
            )
            .expect("the fill agrees");
            let slice = code_slice(writer, filled, (0, 1), false, types);
            assert_eq!(rendered(fixture, types, slice), "(PRINT x)");
            let (storage, scratch) = (program_storage(), Bump::new());
            let flat = materialized(storage.brand().writer(), tree(slice), &scratch);
            let mut found = Vec::new();
            marks(flat.body, &mut found);
            let [Mark::Filled(scope)] = found[..] else {
                panic!("`x` is a filled mark: {found:?}")
            };
            let [binding] = flat.bindings[..] else {
                panic!("one binding")
            };
            assert_eq!(binding.key(), (scope, fixture.name("x")));
            assert!(matches!(
                binding.link,
                crate::values::Link::Value(Value::Number(1.0))
            ));
        });
    });
}

/// A view digests as its flat copy does, though it lists every binding its environments hold and
/// lays itself flat once: each flat binding takes the digest of its own value.
#[test]
fn a_view_digests_as_its_flat_copy() {
    with_fixture(|fixture| {
        let lines =
            fixture.parse("LET k = 1\nLET j = \"two\"\nLET q = #((PRINT $k) (PRINT $j) (PRINT 3))");
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let activation = fixture.run(writer, types, &lines, &[]);
            let scratch = fixture.scratch();
            let q = code(fixture, activation, "q");
            for (from, to, backward) in [(1, 3, false), (0, 2, true), (0, 3, false)] {
                let view = code_slice(writer, q, (from, to), backward, types);
                let laying = Bump::new();
                let flat = materialized(writer, tree(view), &laying);
                let copy = crate::knot::composed_code(
                    writer,
                    flat.body,
                    Value::Knotted(view).concrete_ktype(),
                    &flat.bindings,
                    types,
                );
                assert_eq!(
                    Value::Knotted(view).digest(types, scratch),
                    Value::Knotted(copy).digest(types, scratch),
                    "{from}..{to}"
                );
            }
            // A slice holding only `$j` still digests apart from one holding only `$k`.
            let k = code_slice(writer, q, (0, 1), false, types);
            let j = code_slice(writer, q, (1, 2), false, types);
            assert_ne!(
                Value::Knotted(k).digest(types, scratch),
                Value::Knotted(j).digest(types, scratch)
            );
        });
    });
}
