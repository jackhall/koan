//! Content trees: what the load reads each `:|` ascription's operand and view as, read back from
//! the ascription's site once the load ends, and a closed tree's digest against the run's.

use crate::program::tests::digest_back;
use crate::program::{CellSubstrate, Outcome, Program};
use crate::scope::{BodyShape, Leaf, Site, Tree};

use super::statics::{body, loaded, slot};
use super::{Koan, output};
use crate::values::SpecBounds;

/// `SIG Counter`, and `MODULE ints`, which fits it.
const COUNTER: &str = "SIG Counter FOR ALL #[Carrier] = \
                       #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n\
                       MODULE ints = ((LET zero = 0) \
                       (LET succ = (FN :{x :Number} -> Number = #(x + 1))))\n";

/// The content tree of the operand of the `:|` ascription that is statement `index` of `shape`.
fn statement_tree<'graph>(
    program: &Program<'graph>,
    shape: &BodyShape<'_>,
    index: usize,
) -> Tree<'graph> {
    operand(
        (program.facts())
            .view_tree(shape, Site::of_node(&shape.body()[index]))
            .expect("the statement is an opaque ascription"),
    )
}

/// The operand of the view `tree`.
fn operand<'graph>(tree: &Tree<'graph>) -> Tree<'graph> {
    match tree {
        Tree::View { source, .. } => **source,
        _ => panic!("a view's tree: {tree:?}"),
    }
}

/// The content tree of the operand of the `:|` ascription `LET name = <ascription>` binds in
/// `shape`.
fn let_tree<'graph>(
    program: &Program<'graph>,
    shape: &BodyShape<'graph>,
    name: &str,
) -> Tree<'graph> {
    let Some(crate::parse::ExpressionPart::Expression(node)) =
        shape.rhs(slot(program, shape, name))
    else {
        panic!("`{name}` is bound to a node");
    };
    operand(
        (program.facts())
            .view_tree(shape, Site::of_node(node.reference()))
            .expect("the right-hand side is an opaque ascription"),
    )
}

/// The name of the binder a leaf is keyed on, and whether the body that binds it is `shape`.
fn leaf_binder<'graph>(
    program: &Program<'graph>,
    tree: Tree<'graph>,
    shape: &BodyShape<'graph>,
) -> (String, bool) {
    let Tree::Leaf(Leaf::Bound { binder, chain, .. }) = tree else {
        panic!("a leaf keyed on a binder: {tree:?}");
    };
    assert!(chain.is_empty());
    let declaring = program.shapes().shape_at(binder.body);
    let name = program
        .symbols()
        .display(declaring.slot_name(binder.slot).symbol())
        .to_string();
    (name, binder.body == shape.segment())
}

#[test]
fn a_view_over_a_parameter_reads_back_a_leaf_on_the_parameter() {
    let source = format!("{COUNTER}LET seal = (FN :{{m :Counter}} -> Any = #(m :| Counter))");
    loaded(&source, |program| {
        let seal = body(program, program.shape(), "seal");
        let tree = statement_tree(program, seal, 0);
        assert_eq!(leaf_binder(program, tree, seal), ("m".to_string(), true));
    });
}

#[test]
fn a_view_over_a_capture_reads_back_the_leaf_its_source_is() {
    let source = format!(
        "{COUNTER}LET outer = (FN :{{m :Counter}} -> Any = #(\
         (LET inner = (FN :{{}} -> Any = #(m :| Counter))) (inner {{}})))"
    );
    loaded(&source, |program| {
        let outer = body(program, program.shape(), "outer");
        let inner = body(program, outer, "inner");
        let tree = statement_tree(program, inner, 0);
        assert_eq!(leaf_binder(program, tree, outer), ("m".to_string(), true));
    });
}

#[test]
fn a_view_over_a_local_reads_back_a_leaf_on_its_let() {
    let source = format!(
        "{COUNTER}LET f = (FN :{{g :(FN :{{}} -> Any)}} -> Any = #(\
         (LET local = (g {{}})) (local :| Counter)))"
    );
    loaded(&source, |program| {
        let f = body(program, program.shape(), "f");
        let tree = statement_tree(program, f, 1);
        assert_eq!(leaf_binder(program, tree, f), ("local".to_string(), true));
    });
}

#[test]
fn a_view_over_an_expression_no_binder_names_reads_back_unnamed() {
    let source =
        format!("{COUNTER}LET f = (FN :{{g :(FN :{{}} -> Any)}} -> Any = #((g {{}}) :| Counter))");
    loaded(&source, |program| {
        let f = body(program, program.shape(), "f");
        let tree = statement_tree(program, f, 0);
        assert!(matches!(tree, Tree::Leaf(Leaf::Unnamed(_))), "{tree:?}");
        assert!(!tree.same_as(&tree), "an unnamed leaf is unlike itself");
    });
}

/// A `MODULE` body over literals is closed, and the load digests the view of it as the run does.
#[test]
fn a_view_over_a_module_of_literals_is_closed_at_the_run_s_digest() {
    let source = format!("{COUNTER}LET v = (ints :| Counter)\nLET w = (ints :| Counter)");
    let mut substrate =
        CellSubstrate::load::<Koan>(&source, "<test>", 8, SpecBounds::default(), output())
            .unwrap_or_else(|error| panic!("the program loads: {error}"));
    assert_eq!(
        substrate.with(|running| running.run()),
        Ok(Outcome::Completed)
    );
    let (closed, same, digest) = substrate.with(|running| {
        let program = running.program();
        let shape = program.shape();
        let (v, w) = (let_tree(program, shape, "v"), let_tree(program, shape, "w"));
        (v.closed(), v.same_as(&w), view_digest(program, "v"))
    });
    assert!(closed, "a body over literals is closed");
    assert!(same, "two reads of one module are one tree");
    assert_eq!(digest, digest_back(&mut substrate, "v"));
}

/// The node `LET name = <node>` binds in `shape`.
fn let_node<'graph>(
    program: &Program<'graph>,
    shape: &BodyShape<'graph>,
    name: &str,
) -> &'graph crate::parse::KExpression<'graph> {
    let Some(crate::parse::ExpressionPart::Expression(node)) =
        shape.rhs(slot(program, shape, name))
    else {
        panic!("`{name}` is bound to a node");
    };
    node.reference()
}

/// `SIG Ordered`, two modules of different content that fit it, `SIG Sample`, and `SIG Zeroed`.
const ORDERED: &str = "SIG Ordered = #[(VAL compare :Number)]\n\
                       MODULE asc = (LET compare = 1)\n\
                       MODULE desc = (LET compare = 2)\n\
                       SIG Sample = #[(VAL sample :Number)]\n\
                       SIG Zeroed FOR ALL #[Carrier] = #[(VAL zero :Carrier)]\n";

/// Load and run `source`, then hand `inspect` the program and a reader of each top-level binding's
/// digest as the run computes it.
fn ran<R>(
    source: &str,
    inspect: impl for<'graph> FnOnce(&Program<'graph>) -> R,
) -> (R, CellSubstrate) {
    let mut substrate =
        CellSubstrate::load::<Koan>(source, "<test>", 8, SpecBounds::default(), output())
            .unwrap_or_else(|error| panic!("the program loads: {error}"));
    assert_eq!(
        substrate.with(|running| running.run()),
        Ok(Outcome::Completed)
    );
    let inspected = substrate.with(|running| inspect(running.program()));
    (inspected, substrate)
}

/// The digest the load computes for the opaque view `LET name = (<operand> :| <Sig>)` builds.
fn view_digest(program: &Program<'_>, name: &str) -> String {
    let shape = program.shape();
    let tree = (program.facts())
        .view_tree(shape, Site::of_node(let_node(program, shape, name)))
        .expect("the right-hand side is an opaque ascription");
    let digest = tree.digest().unwrap_or_else(|| panic!("closed: {tree:?}"));
    format!("{digest:?}")
}

#[test]
fn a_call_unfolds_into_the_tree_its_function_builds() {
    let source = format!(
        "{ORDERED}EXPR #(MAKESET elem :Ordered) -> Module = #(\n  \
         MODULE built OVER #[elem] = (LET sample = (elem.compare)))\n\
         LET a = ((MAKESET asc) :| Sample)\n\
         LET b = ((MAKESET asc) :| Sample)\n\
         LET c = ((MAKESET desc) :| Sample)"
    );
    let ((same, closed, apart, digest), mut substrate) = ran(&source, |program| {
        let shape = program.shape();
        let [a, b, c] = ["a", "b", "c"].map(|name| let_tree(program, shape, name));
        (
            a.same_as(&b),
            a.closed() && c.closed(),
            !a.same_as(&c),
            view_digest(program, "a"),
        )
    });
    assert!(same, "one call written at two sites is one tree");
    assert!(closed, "a call over closed arguments is closed");
    assert!(
        apart,
        "arguments of different content build different trees"
    );
    assert_eq!(digest, digest_back(&mut substrate, "a"));
}

#[test]
fn a_functor_that_ignores_its_argument_builds_one_tree() {
    let source = format!(
        "{ORDERED}EXPR #(CONSTANT elem :Ordered) -> Module = #(\n  \
         MODULE built = (LET sample = 1))\n\
         LET a = ((CONSTANT asc) :| Sample)\n\
         LET b = ((CONSTANT desc) :| Sample)"
    );
    loaded(&source, |program| {
        let shape = program.shape();
        let (a, b) = (let_tree(program, shape, "a"), let_tree(program, shape, "b"));
        assert!(a.closed() && a.same_as(&b));
    });
}

/// A leaf inside the function's body is keyed on the call: `BUILD` over one argument twice is one
/// tree, over two arguments two.
#[test]
fn a_leaf_a_call_binds_is_keyed_on_its_arguments() {
    let source = format!(
        "{ORDERED}EXPR #(BUILD elem :Ordered) -> Module = #(\n  \
         (LET start = elem.compare)\n  \
         (MODULE built OVER #[start] = (LET zero = start))\n  \
         (built :| Zeroed))\n\
         LET one = (asc :| Ordered)\n\
         LET two = (desc :| Ordered)\n\
         LET a = ((BUILD one) :| Zeroed)\n\
         LET b = ((BUILD one) :| Zeroed)\n\
         LET c = ((BUILD two) :| Zeroed)"
    );
    loaded(&source, |program| {
        let shape = program.shape();
        let [a, b, c] = ["a", "b", "c"].map(|name| let_tree(program, shape, name));
        assert!(!a.closed(), "`start` reads a member through a view: {a:?}");
        assert!(a.same_as(&b), "one argument");
        assert!(!a.same_as(&c), "two arguments");
    });
}

#[test]
fn a_recursive_functor_never_unfolds_whichever_is_called_first() {
    let looping = format!(
        "{ORDERED}EXPR #(LOOP elem :Ordered) -> Module = #(LOOP elem)\n\
         LET r = (LOOP asc)\nLET v = (r :| Sample)"
    );
    loaded(&looping, |program| {
        let shape = program.shape();
        let tree = let_tree(program, shape, "v");
        assert_eq!(leaf_binder(program, tree, shape), ("r".to_string(), true));
    });
    for first in ["PING", "PONG"] {
        let mutual = format!(
            "{ORDERED}EXPR #(PING elem :Ordered) -> Module = #(PONG elem)\n\
             EXPR #(PONG elem :Ordered) -> Module = #(PING elem)\n\
             LET r = ({first} asc)\nLET v = (r :| Sample)"
        );
        loaded(&mutual, |program| {
            let shape = program.shape();
            let tree = let_tree(program, shape, "v");
            assert_eq!(leaf_binder(program, tree, shape), ("r".to_string(), true));
        });
    }
}

/// The tutorial's `SCALED` captures the local registration `SCALE`, a closure over `factor`: the
/// call substitutes `factor`, so the module is closed, at the run's digest.
#[test]
fn a_module_capturing_a_local_registration_is_closed_at_the_call() {
    let source = "SIG Ten = #[(VAL ten :Number)]\n\
                  EXPR #(SCALED factor :Number) -> Module = #(\n  \
                  (EXPR #(SCALE x :Number) -> Number = #(x * factor))\n  \
                  MODULE scale OVER #[(SCALE _)] = (LET ten = (SCALE 10)))\n\
                  LET triple = (SCALED 3)\nLET v = (triple :| Ten)";
    let (digest, mut substrate) = ran(source, |program| view_digest(program, "v"));
    assert_eq!(digest, digest_back(&mut substrate, "v"));
}

/// Two closures that call each other are one knot, which the load digests as the run does.
#[test]
fn a_module_capturing_a_knot_of_closures_is_closed_at_the_run_s_digest() {
    let source = "SIG Hh = #[(VAL h :(FN :{n :Number} -> Number))]\n\
                  EXPR #(PAIRED base :Number) -> Module = #(\n  \
                  (LET f = (FN :{n :Number} -> Number = #(g {n = n})))\n  \
                  (LET g = (FN :{n :Number} -> Number = #(f {n = base})))\n  \
                  MODULE m OVER #[f g] = (LET h = f))\n\
                  LET p = (PAIRED 1)\nLET v = (p :| Hh)";
    let (digest, mut substrate) = ran(source, |program| view_digest(program, "v"));
    assert_eq!(digest, digest_back(&mut substrate, "v"));
}

#[test]
fn a_call_by_name_unfolds_at_the_run_s_digest() {
    let source = format!(
        "{ORDERED}LET make = (FN :{{elem :Ordered}} -> Module = #(\n  \
         MODULE built OVER #[elem] = (LET sample = (elem.compare))))\n\
         LET a = ((make {{elem = asc}}) :| Sample)\n\
         LET b = ((make {{elem = asc}}) :| Sample)"
    );
    let ((same, digest), mut substrate) = ran(&source, |program| {
        let shape = program.shape();
        let (a, b) = (let_tree(program, shape, "a"), let_tree(program, shape, "b"));
        (a.same_as(&b), view_digest(program, "a"))
    });
    assert!(same);
    assert_eq!(digest, digest_back(&mut substrate, "a"));
}

/// A `FOR ALL` variable a module lists is substituted by the point the call solves it to.
#[test]
fn a_quantified_functor_unfolds_at_its_solution() {
    let source = format!(
        "{ORDERED}EXPR FOR ALL #[Elt] #(BOXED x :Elt) -> Module = #(\n  \
         MODULE built OVER #[x Elt] = (LET sample = 1))\n\
         LET a = ((BOXED 1) :| Sample)\n\
         LET b = ((BOXED \"s\") :| Sample)"
    );
    let ((apart, digest), mut substrate) = ran(&source, |program| {
        let shape = program.shape();
        let (a, b) = (let_tree(program, shape, "a"), let_tree(program, shape, "b"));
        (!a.same_as(&b), view_digest(program, "a"))
    });
    assert!(apart);
    assert_eq!(digest, digest_back(&mut substrate, "a"));
}

/// A binder of a module body holds one value per run of the body, so its leaf is keyed on the
/// body's captures: read through one functor's result over one argument twice it is one tree, over
/// two arguments two.
#[test]
fn a_module_body_s_leaf_is_keyed_on_its_captures() {
    let source = format!(
        "{ORDERED}SIG Picky = #[(VAL pick :(FN :{{}} -> Module))]\n\
         MODULE first = (LET pick = (FN :{{}} -> Module = #(asc)))\n\
         MODULE second = (LET pick = (FN :{{}} -> Module = #(desc)))\n\
         EXPR #(MK elem :Picky) -> Module = #(\n  \
         MODULE built OVER #[elem] = (LET v = (elem.pick {{}})))\n\
         LET a = ((MK first).v :| Ordered)\n\
         LET b = ((MK first).v :| Ordered)\n\
         LET c = ((MK second).v :| Ordered)"
    );
    loaded(&source, |program| {
        let shape = program.shape();
        let [a, b, c] = ["a", "b", "c"].map(|name| let_tree(program, shape, name));
        assert!(!a.closed(), "`v` is a call the load cannot see into: {a:?}");
        assert!(a.same_as(&b), "one run of the body");
        assert!(!a.same_as(&c), "two runs of the body");
    });
}

/// A closure an unfolded call binds at a function-typed parameter is retyped there, as the run
/// lays it under a header: the module capturing it is closed at the run's digest.
#[test]
fn a_closure_retyped_at_a_parameter_is_closed_at_the_run_s_digest() {
    let source = "SIG Hh = #[(VAL h :(FN :{} -> Any))]\n\
                  LET wide = (FN :{} -> :{x :Number, y :Str} = #({x = 1, y = \"s\"}))\n\
                  EXPR #(HOLD g :(FN :{} -> :{x :Number})) -> Module = #(\n  \
                  MODULE m OVER #[g] = (LET h = g))\n\
                  LET p = (HOLD wide)\nLET v = (p :| Hh)";
    let ((closed, digest), mut substrate) = ran(source, |program| {
        let tree = let_tree(program, program.shape(), "v");
        (tree.closed(), view_digest(program, "v"))
    });
    assert!(
        closed,
        "the closure's own type is known, so its retype is exact"
    );
    assert_eq!(digest, digest_back(&mut substrate, "v"));
}
