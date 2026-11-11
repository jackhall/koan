//! The trace at load ([the trace](../../build/README.md#the-trace)): per call site of a keyed body,
//! the bindings the load names, or why it names none. Each test loads a program and reads the
//! trace at a call by its text; some also run the enumeration over the loaded program's records.

use crate::program::{Program, TRACE_CAP as CAP};
use crate::scope::{CallTrace, Content, Located, Open, Untraced, each_part};

use super::statics::loaded;

/// `SIG Small`, which any module with a numeric `zero` fits, and `SIG Big`, which adds `one`.
const SIGS: &str = "SIG Small = #[(VAL zero :Number)]\n\
                    SIG Big = #[(VAL zero :Number) (VAL one :Number)]\n";

/// `count` modules fitting `Small`, `m0` on, each its own content.
fn modules(count: usize) -> String {
    (0..count)
        .map(|at| format!("MODULE m{at} = (LET zero = {at})\n"))
        .collect()
}

/// A keyworded registration `NAME m :Small` keyed on `m` through a view over it, whose body then
/// runs `then`.
fn keyed(name: &str, then: &str) -> String {
    format!("EXPR #({name} m :Small) -> Number = #((LET v = (m :| Small)) ({then}))\n")
}

/// The call the trace's site `site` of `shape` names: a keyworded use's node, or the call by name
/// whose head is the part there.
pub(super) fn call_of(
    shape: &crate::scope::BodyShape<'_>,
    site: crate::scope::Site,
) -> Option<crate::source::SourceRef> {
    match shape.located(site) {
        Located::Node(node) => Some(node.source),
        Located::Part(head) => {
            let mut found = None;
            for statement in shape.body() {
                let mut visit = |node: &crate::parse::KExpression<'_>| {
                    if node
                        .parts
                        .first()
                        .is_some_and(|first| std::ptr::eq(&first.value, head))
                    {
                        found = Some(node.source);
                    }
                };
                visit(statement);
                each_part(statement, |part| {
                    if let crate::parse::ExpressionPart::Expression(node) = part {
                        visit(node);
                    }
                });
            }
            found
        }
    }
}

/// `text` without one pair of enclosing parentheses, as a call is written alone or as a part.
fn bare(text: &str) -> &str {
    (text
        .strip_prefix('(')
        .and_then(|inner| inner.strip_suffix(')')))
    .unwrap_or(text)
}

/// The trace at the call written `text`, the one site of the program whose call reads so, with or
/// without its parentheses.
pub(super) fn trace_at<'a>(program: &Program<'a>, text: &str) -> CallTrace<'a> {
    let mut found = Vec::new();
    let mut every = Vec::new();
    for shape in program.shapes().bodies() {
        for (site, trace) in program.facts().traces(shape).iter() {
            let call = call_of(shape, *site).map(|call| call.text());
            if call.as_deref().map(bare) == Some(bare(text)) {
                found.push(*trace);
            }
            every.push(format!("{call:?}: {trace:?}"));
        }
    }
    match found[..] {
        [trace] => trace,
        _ => panic!(
            "`{text}` names {} traced sites among\n{}",
            found.len(),
            every.join("\n")
        ),
    }
}

/// The bindings the trace names at `text`.
fn named<'a>(program: &Program<'a>, text: &str) -> &'a [&'a [Content]] {
    match trace_at(program, text) {
        CallTrace::Named { bindings, .. } => bindings,
        untraced => panic!("`{text}` is {untraced:?}"),
    }
}

/// Why the trace names nothing at `text`.
fn untraced(program: &Program<'_>, text: &str) -> Untraced {
    match trace_at(program, text) {
        CallTrace::Untraced(reason) => reason,
        named => panic!("`{text}` is {named:?}"),
    }
}

/// The module content `content` is, as the type it carries into the call, its member's type and its
/// member's digest.
fn module(
    content: Content,
) -> (
    crate::type_lattice::KType,
    crate::type_lattice::KType,
    crate::values::ContentDigest,
) {
    match content {
        Content::Module {
            at, own, content, ..
        } => (at, own, content),
        Content::Type(_) => panic!("a type where a module was wanted"),
    }
}

/// [`Program::enumerated`] over `program`: per body with traced slots, how many bindings it names
/// or why they are open, beside how many times the enumeration read a site.
fn enumerated(program: &Program<'_>, limit: usize) -> (Vec<Result<usize, Open>>, usize) {
    let mut bodies = Vec::new();
    let reads = program.enumerated(limit, |body| bodies.push(body));
    (bodies, reads)
}

#[test]
fn a_top_level_call_on_a_bare_module_names_its_content() {
    let source = format!("{SIGS}{}{}LEAF m0\n", modules(1), keyed("LEAF", "1"));
    loaded(&source, |program| {
        let [[content]] = named(program, "LEAF m0") else {
            panic!("one binding of one key");
        };
        let (at, own, _) = module(*content);
        assert_eq!(at, own, "a bare module carries its own type");
    });
}

#[test]
fn a_view_is_its_own_member_and_a_retype_names_its_bare_module() {
    let source = format!(
        "{SIGS}{}{}LET viewed = (m0 :| Small)\nLET retyped = (m0 :! Small)\n\
         LEAF viewed\nLEAF retyped\nLEAF m0\n",
        modules(1),
        keyed("LEAF", "1")
    );
    loaded(&source, |program| {
        let [[viewed]] = named(program, "LEAF viewed") else {
            panic!("one binding at the view");
        };
        let (at, own, _) = module(*viewed);
        assert_eq!(at, own, "a view is its own member");
        let [[retyped]] = named(program, "LEAF retyped") else {
            panic!("one binding at the retype");
        };
        let [[bare]] = named(program, "LEAF m0") else {
            panic!("one binding at the bare module");
        };
        let (at, own, member) = module(*retyped);
        assert_ne!(at, own, "a retype is not its own member");
        assert_eq!(
            member,
            module(*bare).2,
            "a retype's member is its bare module"
        );
    });
}

#[test]
fn a_functor_s_result_passed_on_is_named() {
    let source = format!(
        "{SIGS}{}{}EXPR #(MAKE m :Small) -> Small = #(m)\nLET made = (MAKE m0)\nLEAF made\n",
        modules(1),
        keyed("LEAF", "1")
    );
    loaded(&source, |program| {
        let [[_]] = named(program, "LEAF made") else {
            panic!("one binding at a functor's result");
        };
    });
}

#[test]
fn a_call_by_name_of_a_quantified_fn_names_each_solution() {
    let source = "MODULE lib = (\n  LET pick = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #((PRINT Elt) (x)))\n  \
                  PRINT (pick {x = 1})\n  PRINT (pick {x = \"s\"})\n)\n";
    loaded(source, |program| {
        let types = [
            named(program, "(pick {x = 1})"),
            named(program, "(pick {x = \"s\"})"),
        ];
        let number = crate::type_lattice::KType::NUMBER;
        let str = crate::type_lattice::KType::STR;
        assert_eq!(
            types.map(|bindings| bindings.to_vec()),
            [
                vec![&[Content::Type(number)][..]],
                vec![&[Content::Type(str)][..]]
            ]
        );
    });
}

#[test]
fn a_parameter_passed_on_carries_each_caller_binding() {
    let once = format!(
        "{SIGS}{}{}{}OUTER m0\n",
        modules(2),
        keyed("LEAF", "1"),
        keyed("OUTER", "LEAF m")
    );
    loaded(&once, |program| {
        assert_eq!(named(program, "LEAF m").len(), 1)
    });
    let twice = format!("{once}OUTER m1\nOUTER m0\n");
    loaded(&twice, |program| {
        assert_eq!(named(program, "LEAF m").len(), 2)
    });
}

#[test]
fn a_for_all_name_passed_on_carries_each_caller_solution() {
    let source = "EXPR FOR ALL #[Item] #(INNER y :Item) -> Str = #(PRINT Item)\n\
                  EXPR FOR ALL #[Elt] #(OUTER x :Elt) -> Str = #(INNER x)\n\
                  OUTER 1\nOUTER \"s\"\nOUTER 2\n";
    loaded(source, |program| {
        let number = crate::type_lattice::KType::NUMBER;
        let str = crate::type_lattice::KType::STR;
        let mut solved: Vec<_> = (named(program, "INNER x").iter())
            .map(|binding| binding.to_vec())
            .collect();
        solved.sort();
        let mut expected = vec![vec![Content::Type(number)], vec![Content::Type(str)]];
        expected.sort();
        assert_eq!(solved, expected);
    });
}

#[test]
fn bindings_are_correlated_never_a_product_of_slots() {
    let source = format!(
        "{SIGS}{}EXPR #(COMBINE a :Small WITH b :Small) -> Number = \
         #((LET u = (a :| Small)) (LET v = (b :| Small)) (1))\n\
         EXPR #(PAIR a :Small WITH b :Small) -> Number = #(COMBINE a WITH b)\n\
         PAIR m0 WITH m0\nPAIR m1 WITH m1\n",
        modules(2)
    );
    loaded(&source, |program| {
        let bindings = named(program, "COMBINE a WITH b");
        assert_eq!(bindings.len(), 2, "two bindings, not the product's four");
        assert!(bindings.iter().all(|binding| binding[0] == binding[1]));
    });
}

#[test]
fn a_caller_that_keys_on_nothing_still_passes_its_binding_on() {
    let source = format!(
        "{SIGS}{}{}EXPR #(OUTER m :Small) -> Number = #(LEAF m)\nOUTER m0\n",
        modules(1),
        keyed("LEAF", "1")
    );
    loaded(&source, |program| {
        assert_eq!(named(program, "LEAF m").len(), 1)
    });
}

#[test]
fn a_slot_no_callee_traces_opens_nothing() {
    // `MID`'s `n` is neither keyed nor passed on, so `TOP` passing a number the run reads there
    // leaves `MID`'s bindings named.
    let source = format!(
        "{SIGS}{}{}EXPR #(MID m :Small WITH n :Number) -> Number = #(LEAF m)\n\
         EXPR #(TOP k :Small) -> Number = #(MID m0 WITH (k.zero))\nTOP m0\n",
        modules(1),
        keyed("LEAF", "1")
    );
    loaded(&source, |program| {
        assert_eq!(named(program, "LEAF m").len(), 1)
    });
}

#[test]
fn a_cycle_ends_and_reads_each_edge_once_per_binding() {
    let source = format!(
        "{SIGS}{}{}{}PING m0\n",
        modules(1),
        keyed("PING", "PONG m"),
        keyed("PONG", "PING m")
    );
    loaded(&source, |program| {
        assert_eq!(named(program, "PONG m").len(), 1);
        assert_eq!(named(program, "PING m").len(), 1);
        let (bodies, reads) = enumerated(program, CAP);
        assert_eq!(bodies, [Ok(1), Ok(1)]);
        assert_eq!(reads, 2, "two edges, one binding");
    });
}

#[test]
fn an_escaping_caller_leaves_what_it_passes_on_untraced() {
    let source = format!(
        "{SIGS}{}{}LET outer = (FN :{{m :Small}} -> Number = #(LEAF m))\nLET alias = outer\n\
         PRINT (outer {{m = m0}})\n",
        modules(1),
        keyed("LEAF", "1")
    );
    loaded(&source, |program| {
        assert_eq!(untraced(program, "LEAF m"), Untraced::Caller(Open::Escapes));
    });
}

#[test]
fn a_parameter_at_another_declared_type_passes_nothing() {
    let source = format!(
        "{SIGS}MODULE both = ((LET zero = 0) (LET one = 1))\n{}\
         EXPR #(WIDE m :Big) -> Number = #(LEAF m)\nWIDE both\n",
        keyed("LEAF", "1")
    );
    loaded(&source, |program| {
        assert_eq!(untraced(program, "LEAF m"), Untraced::Unfollowed);
    });
}

#[test]
fn past_the_cap_a_body_names_nothing_and_opens_what_it_passes_on_to() {
    let calls: String = (0..=CAP).map(|at| format!("ENTRY m{at}\n")).collect();
    let source = format!(
        "{SIGS}{}{}{}{}{calls}",
        modules(CAP + 1),
        keyed("LEAF", "1"),
        keyed("MIDDLE", "LEAF m"),
        keyed("ENTRY", "MIDDLE m")
    );
    loaded(&source, |program| {
        for at in 0..=CAP {
            assert_eq!(named(program, &format!("ENTRY m{at}")).len(), 1);
        }
        assert_eq!(
            untraced(program, "MIDDLE m"),
            Untraced::Caller(Open::Capped)
        );
        assert_eq!(untraced(program, "LEAF m"), Untraced::Caller(Open::Fed));
        let (bodies, _) = enumerated(program, usize::MAX);
        assert_eq!(
            bodies,
            [Ok(CAP + 1); 3],
            "with no limit, every binding is named"
        );
    });
}

#[test]
fn a_call_in_a_quote_s_code_is_composed_and_opens_its_callee() {
    let source = format!(
        "{SIGS}{}{}{}LET q = #(PRINT $(MIDDLE m0))\nMIDDLE m1\n",
        modules(2),
        keyed("LEAF", "1"),
        keyed("MIDDLE", "LEAF m")
    );
    loaded(&source, |program| {
        let code = (program.shapes().bodies().iter())
            .find(|shape| shape.kind() == crate::scope::ShapeKind::Code)
            .expect("the quote's code");
        let traces: Vec<_> = (program.facts().traces(code).iter())
            .map(|(_, trace)| match trace {
                CallTrace::Untraced(reason) => Some(*reason),
                CallTrace::Named { .. } => None,
            })
            .collect();
        assert_eq!(traces, [Some(Untraced::Composed)]);
        assert_eq!(named(program, "MIDDLE m1").len(), 1);
        assert_eq!(untraced(program, "LEAF m"), Untraced::Caller(Open::Fed));
    });
}

#[test]
fn a_use_the_load_resolves_to_no_one_callee_is_unresolved() {
    let source = format!(
        "{SIGS}{}{}{}SIG Named = #[(VAL name :Str)]\n\
         EXPR #(PICK m :Named) -> Number = #((LET v = (m :| Named)) (1))\n\
         LET f = (FN :{{x :Any}} -> Number = #(PICK x))\n",
        modules(1),
        keyed("LEAF", "1"),
        keyed("PICK", "LEAF m")
    );
    loaded(&source, |program| {
        assert_eq!(untraced(program, "PICK x"), Untraced::Unresolved);
        assert_eq!(untraced(program, "LEAF m"), Untraced::Caller(Open::Fed));
    });
}

#[test]
fn the_fold_is_linear_in_the_sites_it_reads() {
    const NAMES: [&str; 6] = ["ONE", "TWO", "THREE", "FOUR", "FIVE", "SIX"];
    let chain: String = (0..6)
        .map(|at| keyed(NAMES[at], &format!("{} m", NAMES[(at + 1) % 6])))
        .collect();
    let one = format!("{SIGS}{}{chain}ONE m0\n", modules(2));
    loaded(&one, |program| {
        let (bodies, reads) = enumerated(program, CAP);
        assert_eq!(bodies, [Ok(1); 6]);
        assert_eq!(reads, 6, "six edges, one binding");
    });
    let two = format!("{one}ONE m1\n");
    loaded(&two, |program| {
        let (bodies, reads) = enumerated(program, CAP);
        assert_eq!(bodies, [Ok(2); 6]);
        assert_eq!(reads, 12, "six edges, two bindings");
    });
}

#[test]
fn a_body_offered_to_code_names_its_own_calls_but_opens_what_it_passes_on() {
    let source = format!(
        "{SIGS}{}{}{}LET run = (FN :{{body :(Expression NEEDING #[(MIDDLE _)])}} -> Any = \
         #(EVAL body -> Any))\nMIDDLE m0\n",
        modules(1),
        keyed("LEAF", "1"),
        keyed("MIDDLE", "LEAF m")
    );
    loaded(&source, |program| {
        assert_eq!(named(program, "MIDDLE m0").len(), 1);
        assert_eq!(untraced(program, "LEAF m"), Untraced::Caller(Open::Fed));
        let traced: usize = (program.shapes().bodies().iter())
            .map(|shape| program.facts().traces(shape).len())
            .sum();
        assert_eq!(traced, 2, "the `EVAL`'s operand has no trace");
    });
}

#[test]
fn each_covered_site_keeps_its_slot_map_on_its_owner() {
    use crate::scope::{Fixed, ShapeKind};
    let source = super::caching::program(
        0,
        "EXPR #(CARRIER m :Counter) -> Number = #((LET v = (m :| Counter)) (7))\n\
         EXPR #(OUTER m :Counter) -> Number = #((LET w = (m :| Counter)) \
         (USING empty SCOPE ((CARRIER m))))\n\
         LET q = #(PRINT $(CARRIER ints))\nPRINT (CARRIER ints)\nPRINT (OUTER ints)\n",
    );
    loaded(&source, |program| {
        let shapes = program.shapes();
        let mut maps = Vec::new();
        for shape in shapes.bodies() {
            for map in program.facts().slot_maps(shape.segment()) {
                maps.push((shape.segment(), *map));
            }
        }
        let at = |text: &str| {
            let found: Vec<_> = (maps.iter())
                .filter(|(_, map)| bare(&map.call.text()) == bare(text))
                .collect();
            match found[..] {
                [found] => *found,
                _ => panic!("`{text}` has {} slot maps", found.len()),
            }
        };
        // The quote's site is recorded, composed, and keeps no slot map.
        let code = (shapes.bodies().iter())
            .find(|shape| shape.kind() == ShapeKind::Code)
            .expect("the quote's code");
        assert!(matches!(
            program.facts().traces(code).entries(),
            [(_, CallTrace::Untraced(Untraced::Composed))]
        ));
        assert!(maps.iter().all(|(_, map)| !shapes.in_code(map.at)));
        let (owner, top) = at("(CARRIER ints)");
        assert_eq!(owner, shapes.root().segment());
        assert_eq!(top.at, owner);
        assert!(matches!(top.passed, [(_, Fixed::Content(_))]));
        assert!(!top.passes_on());
        assert_eq!(call_of(shapes.shape_at(top.at), top.site), Some(top.call));

        let (owner, block) = at("(CARRIER m)");
        assert_eq!(shapes.shape_at(owner).kind(), ShapeKind::Callable);
        assert_eq!(shapes.shape_at(block.at).kind(), ShapeKind::Block);
        assert_eq!(shapes.owner(block.at), owner);
        assert!(matches!(block.passed, [(_, Fixed::Passed(_))]));
        assert!(block.passes_on());
        assert_eq!(
            call_of(shapes.shape_at(block.at), block.site),
            Some(block.call)
        );
    });
}
