//! Paths: a type member read through a module at load, as the load resolves it off the module's
//! content tree, against what the run binds, and the law that a load verdict over drawn functor
//! programs holds at every run.

use proptest::prelude::*;

use crate::scope::{Leaf, Resolved, Tree};

use super::super::statics::unnarrowed;
use super::run;
use super::statics::{body, interval, let_narrowing, loaded, narrowing};

/// `SIG Counter`, `MODULE ints`, which fits it, and `SAME`, which solves one variable from two
/// arguments.
const COUNTER: &str = "SIG Counter FOR ALL #[Carrier] = \
                       #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n\
                       MODULE ints = ((LET zero = 0) \
                       (LET succ = (FN :{x :Number} -> Number = #(x + 1))))\n\
                       EXPR FOR ALL #[Elt] #(SAME a :Elt AND b :Elt) -> Elt = #(a)\n";

/// `(SAME a AND b)` bound by `LET s` in `source`'s top level, after `COUNTER`: its narrowing.
fn same(source: &str) -> String {
    narrowing(&format!("{COUNTER}{source}"), "s")
}

/// Two member reads through one path share a type at load, so a call solving one variable from
/// both is *always*: through a view the load sees all of, and through a parameter, where the path
/// is the load's own variable, which the run binds by reading the module.
#[test]
fn member_reads_through_one_path_share_a_type() {
    let view = "LET v = (ints :| Counter)\nLET s = (SAME v.zero AND (v.succ {x = v.zero}))\n";
    assert_eq!(same(view), "selected");
    assert_eq!(run(&format!("{COUNTER}{view}PRINT s")), "Carrier(0)");
    let step = "LET step = (FN :{m :Counter} -> Any = \
                #(LET s = (SAME m.zero AND (m.succ {x = m.zero}))))\n";
    loaded(&format!("{COUNTER}{step}"), |program| {
        let step = body(program, program.shape(), "step");
        assert_eq!(let_narrowing(program, step, "s"), "selected");
    });
    assert_eq!(
        run(&format!(
            "{COUNTER}{step}PRINT (step {{m = (ints :| Counter)}})\nPRINT (step {{m = ints}})"
        )),
        "Carrier(0)\n0"
    );
}

/// The load reads two paths by their content trees: opaque views of bodies of different code are
/// *never* one type, two views of one module *always*, a member a body binds or `:!` solves is the
/// type it is, and a parameter's path against any other is *maybe*.
#[test]
fn two_paths_compare_by_their_content_trees() {
    assert_eq!(
        run(&format!(
            "{COUNTER}MODULE m2 = ((LET zero = 0) \
             (LET succ = (FN :{{x :Number}} -> Number = #(x + 2))))\n\
             LET meters = (ints :| Counter)\nLET feet = (m2 :| Counter)\n\
             PRINT (SAME meters.zero AND feet.zero)"
        )),
        "load: <test>:7:7: no overload of `SAME _ AND _` admits (Carrier, Carrier)"
    );
    let twice =
        "LET a = (ints :| Counter)\nLET b = (ints :| Counter)\nLET s = (SAME a.zero AND b.zero)\n";
    assert_eq!(same(twice), "selected");
    assert_eq!(run(&format!("{COUNTER}{twice}PRINT s")), "Carrier(0)");
    let never = "this value is Str, which can never satisfy its annotation Number";
    assert_eq!(
        run(&format!(
            "{COUNTER}MODULE b = ((LET Carrier = Number) (LET zero = 0))\nLET x :(b.Carrier) = \"s\""
        )),
        format!("load: <test>:5:1: {never}")
    );
    assert_eq!(
        run(&format!(
            "{COUNTER}LET x :((ints :! Counter).Carrier) = \"s\""
        )),
        format!("load: <test>:4:1: {never}")
    );
    assert_eq!(
        run(&format!(
            "{COUNTER}LET x :((ints :! Counter).Carrier) = 1\nPRINT x"
        )),
        "1"
    );
    loaded(
        &format!(
            "{COUNTER}LET v = (ints :| Counter)\n\
             LET f = (FN :{{p :Counter}} -> Any = #(LET s = (SAME p.zero AND v.zero)))"
        ),
        |program| {
            let f = body(program, program.shape(), "f");
            assert_eq!(let_narrowing(program, f, "s"), "full");
        },
    );
}

/// `SIG Ordered`, two modules of different content that fit it, two signatures over a head
/// parameter, `SAME`, and functors over an `Ordered` module.
const ORDERED: &str = "SIG Ordered = #[(VAL compare :Number)]\n\
                       MODULE asc = (LET compare = 1)\n\
                       MODULE desc = (LET compare = 2)\n\
                       SIG Collection FOR ALL #[Set] = #[(VAL empty :Set)]\n\
                       SIG Zeroed FOR ALL #[Carrier] = #[(VAL zero :Carrier)]\n\
                       EXPR FOR ALL #[Elt] #(SAME a :Elt AND b :Elt) -> Elt = #(a)\n\
                       EXPR #(MAKESET elem :Ordered) -> Module = #(\n  \
                       (MODULE built OVER #[elem] = (LET empty = [elem.compare]))\n  \
                       (built :| Collection))\n\
                       EXPR #(CONSTANT elem :Ordered) -> Module = #(\n  \
                       (MODULE built = (LET empty = [0]))\n  \
                       (built :| Collection))\n\
                       EXPR #(BUILD elem :Ordered) -> Module = #(\n  \
                       (LET start = elem.compare)\n  \
                       (MODULE built OVER #[start] = (LET zero = start))\n  \
                       (built :| Zeroed))\n\
                       EXPR #(PICK elem :Ordered) -> Module = #(\n  \
                       (MODULE built OVER #[elem] = (LET empty = elem))\n  \
                       (built :| Collection))\n";

/// A call unfolds: a path through it written at two sites is one type at load, the carrier the
/// run's value carries; calls over arguments of different content are *never* one type; and a
/// functor that ignores its argument gives one type over two arguments.
#[test]
fn a_path_through_a_call_is_the_carrier_the_run_computes() {
    // The run checks in debug builds that the value carries a type within its exact static type.
    assert_eq!(
        run(&format!(
            "{ORDERED}LET x :((MAKESET asc).Set) = (MAKESET asc).empty\n\
             LET y :((MAKESET asc).Set) = x\nPRINT y"
        )),
        "Set([1])"
    );
    assert_eq!(
        run(&format!(
            "{ORDERED}PRINT (SAME (MAKESET asc).empty AND (MAKESET desc).empty)"
        )),
        "load: <test>:20:7: no overload of `SAME _ AND _` admits (Set, Set)"
    );
    assert_eq!(
        narrowing(
            &format!("{ORDERED}LET s = (SAME (CONSTANT asc).empty AND (CONSTANT desc).empty)"),
            "s"
        ),
        "selected"
    );
}

/// A leaf a functor's body binds keys the path on the call's arguments: two calls of `BUILD` over
/// one argument are one type, and over two of different content *maybe*.
#[test]
fn a_leaf_a_call_binds_keys_its_path_on_the_arguments() {
    let calls = "LET one = (asc :| Ordered)\nLET two = (desc :| Ordered)\n\
                 LET a = (BUILD one)\nLET b = (BUILD one)\nLET c = (BUILD two)\n";
    let read = |pair: &str| narrowing(&format!("{ORDERED}{calls}LET s = (SAME {pair})"), "s");
    assert_eq!(read("a.zero AND b.zero"), "selected");
    assert_eq!(read("a.zero AND c.zero"), "full");
    assert_eq!(
        run(&format!("{ORDERED}{calls}PRINT (SAME a.zero AND b.zero)")),
        "Carrier(1)"
    );
}

/// A view of a module capturing two closures that call each other is closed, so the load knows
/// its signature exactly, carrier and all, as the run builds it.
#[test]
fn a_view_of_closures_that_call_each_other_has_its_carrier_at_load() {
    let source = "SIG Hh FOR ALL #[Carrier] = #[(VAL h :(FN :{n :Carrier} -> Carrier))]\n\
                  EXPR #(PAIRED base :Number) -> Module = #(\n  \
                  (LET f = (FN :{n :Number} -> Number = #(g {n = n})))\n  \
                  (LET g = (FN :{n :Number} -> Number = #(f {n = base})))\n  \
                  MODULE m OVER #[f g] = (LET h = f))\n\
                  LET p = (PAIRED 1)\nLET v = (p :| Hh)\nLET w = (p :| Hh)\n";
    loaded(source, |program| {
        assert!(interval(program, program.shape(), "v").is_exact());
    });
    assert_eq!(
        run(&format!(
            "{source}EXPR FOR ALL #[Elt] #(SAME a :Elt AND b :Elt) -> Elt = #(a)\n\
                      PRINT (SAME v.h AND w.h)"
        )),
        ":(FN :{n :Carrier} -> Carrier)"
    );
}

/// A type expression reading a path through a call written in place whose tree holds a leaf
/// refuses the load: the run builds no module there.
#[test]
fn a_type_through_an_inline_call_over_a_leaf_refuses_the_load() {
    assert_eq!(
        run(&format!(
            "{ORDERED}EXPR #(USE p :Ordered) -> Any = #(LET x :((PICK p).Set) = 1)"
        )),
        "load: <test>:20:43: this type reads a member of a module built by a call written in \
         place, which the run never makes; bind the call with `LET` to name its type"
    );
}

/// Once the load ends, each path over a tree with a leaf reads back that tree: through a parameter,
/// a leaf on it; through a call that unfolds, the view it builds over the leaf it binds.
#[test]
fn each_open_path_reads_back_its_tree() {
    let source = format!(
        "{ORDERED}EXPR #(USE p :Ordered) -> Any = #(LET x :(p.Carrier) = 1)\n\
         LET one = (asc :| Ordered)\nLET a = (BUILD one)\nLET z = a.zero"
    );
    loaded(&source, |program| {
        let mut trees = program.shape().paths().iter().filter_map(|entry| {
            let resolution = entry.resolution().expect("the load resolves every path");
            match resolution.resolved {
                Resolved::Class(_) => Some((entry.label, resolution.tree)),
                Resolved::Closed(_) => None,
            }
        });
        let label = |entry| program.symbols().display(entry).to_string();
        let (carrier, tree) = trees.next().expect("`p.Carrier` is open");
        assert_eq!(label(carrier.symbol()), "Carrier");
        let Tree::Leaf(Leaf::Bound { binder, chain, .. }) = tree else {
            panic!("a leaf on `p`: {tree:?}");
        };
        assert!(chain.is_empty());
        assert_eq!(label(binder.shape.slot_name(binder.slot).symbol()), "p");
        let (carrier, tree) = trees.next().expect("`a`'s `Carrier` is open");
        assert_eq!(label(carrier.symbol()), "Carrier");
        let Tree::View { source, .. } = tree else {
            panic!("the view `BUILD` builds: {tree:?}");
        };
        assert!(!source.closed());
        assert!(trees.next().is_none());
    });
}

/// `SIG Counter`, `SIG Zeroed`, `SAME`, and two functors over a `Counter`: `WRAP` views a body
/// capturing its parameter, `BUILD` one capturing a local `LET` of a member read.
const FUNCTORS: &str = "SIG Counter FOR ALL #[Carrier] = \
                        #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n\
                        SIG Zeroed FOR ALL #[Carrier] = #[(VAL zero :Carrier)]\n\
                        EXPR FOR ALL #[Elt] #(SAME a :Elt AND b :Elt) -> Elt = #(a)\n\
                        EXPR #(WRAP p :Counter) -> Module = #(\n  \
                        (MODULE built OVER #[p] = ((LET zero = p.zero) (LET succ = p.succ)))\n  \
                        (built :| Counter))\n\
                        EXPR #(BUILD p :Counter) -> Module = #(\n  \
                        (LET start = p.zero)\n  \
                        (MODULE built OVER #[start] = (LET zero = start))\n  \
                        (built :| Zeroed))\n";

/// A functor's argument: a `MODULE`, or an earlier binding that is a `Counter`, each picked by
/// index modulo what there is.
#[derive(Clone, Copy, Debug)]
enum Argument {
    Module(usize),
    Earlier(usize),
}

/// A top-level binding of a drawn functor program, each `MODULE` picked by index modulo their
/// count.
#[derive(Clone, Copy, Debug)]
enum Binding {
    /// The module itself.
    Module(usize),
    /// `(m :| Counter)`.
    Opaque(usize),
    /// `(m :! Counter)`.
    Transparent(usize),
    /// `(WRAP <argument>)`: a `Counter`.
    Wrap(Argument),
    /// `(BUILD <argument>)`: a `Zeroed`, so no functor's argument.
    Build(Argument),
}

/// A drawn functor program: `MODULE`s over drawn literals, top-level bindings over them, and the
/// two bindings whose `zero` members `SAME` compares.
#[derive(Clone, Debug)]
struct Functors {
    /// Per `MODULE`, its `zero` and its `succ`'s step: few enough that two often share content.
    modules: Vec<(u8, u8)>,
    bindings: Vec<Binding>,
    pair: (usize, usize),
}

fn functors() -> impl Strategy<Value = Functors> {
    let argument = prop_oneof![
        (0..3usize).prop_map(Argument::Module),
        (0..4usize).prop_map(Argument::Earlier),
    ];
    let binding = prop_oneof![
        (0..3usize).prop_map(Binding::Module),
        (0..3usize).prop_map(Binding::Opaque),
        (0..3usize).prop_map(Binding::Transparent),
        argument.clone().prop_map(Binding::Wrap),
        argument.prop_map(Binding::Build),
    ];
    (
        prop::collection::vec((0..2u8, 1..3u8), 2..=3),
        prop::collection::vec(binding, 2..=4),
        (0..4usize, 0..4usize),
    )
        .prop_map(|(modules, bindings, pair)| Functors {
            modules,
            bindings,
            pair,
        })
}

impl Functors {
    /// The program as source ending in `LET s = (SAME ...)` and `PRINT s`, beside the line of the
    /// `SAME`.
    fn render(&self) -> (String, usize) {
        let mut source = FUNCTORS.to_string();
        for (at, (zero, step)) in self.modules.iter().enumerate() {
            source.push_str(&format!(
                "MODULE m{at} = ((LET zero = {zero}) \
                 (LET succ = (FN :{{x :Number}} -> Number = #(x + {step}))))\n"
            ));
        }
        let module = |at: usize| format!("m{}", at % self.modules.len());
        for (at, binding) in self.bindings.iter().enumerate() {
            let argument = |argument: Argument| match argument {
                Argument::Module(of) => module(of),
                Argument::Earlier(of) => {
                    let counters: Vec<usize> = (0..at)
                        .filter(|earlier| !matches!(self.bindings[*earlier], Binding::Build(_)))
                        .collect();
                    match counters.len() {
                        0 => module(of),
                        count => format!("b{}", counters[of % count]),
                    }
                }
            };
            let value = match *binding {
                Binding::Module(of) => module(of),
                Binding::Opaque(of) => format!("({} :| Counter)", module(of)),
                Binding::Transparent(of) => format!("({} :! Counter)", module(of)),
                Binding::Wrap(of) => format!("(WRAP {})", argument(of)),
                Binding::Build(of) => format!("(BUILD {})", argument(of)),
            };
            source.push_str(&format!("LET b{at} = {value}\n"));
        }
        let count = self.bindings.len();
        let (first, second) = (self.pair.0 % count, self.pair.1 % count);
        let line = source.lines().count() + 1;
        source.push_str(&format!(
            "LET s = (SAME b{first}.zero AND b{second}.zero)\nPRINT s"
        ));
        (source, line)
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: crate::tests::case_share(1, 4),
        ..ProptestConfig::default()
    })]

    /// The load's verdict on two paths through drawn modules, views and functor calls holds at
    /// every run: where it selects `SAME` over two `zero` members the run prints one, where it
    /// refuses the use the run faults there, and otherwise the run is the unnarrowed one.
    #[test]
    fn a_load_verdict_on_two_paths_holds_at_the_run(program in functors()) {
        let (source, line) = program.render();
        let narrowed = run(&source);
        let whole = unnarrowed(|| run(&source));
        if narrowed.starts_with("load: ") {
            prop_assert!(
                narrowed.starts_with(&format!("load: <test>:{line}:")),
                "refused away from the use: {}\nin\n{}",
                narrowed,
                source
            );
            prop_assert!(
                whole.starts_with("error: ") && !whole.contains('\n'),
                "{} unnarrowed ran {:?}\nin\n{}",
                narrowed,
                whole,
                source
            );
        } else {
            prop_assert_eq!(&narrowed, &whole, "in\n{}", source);
            if narrowing(&source, "s") == "selected" {
                prop_assert!(!narrowed.starts_with("error: "), "{}\nin\n{}", narrowed, source);
            }
        }
    }
}
