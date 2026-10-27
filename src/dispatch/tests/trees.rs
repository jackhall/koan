//! Content trees: what the load reads each `:|` ascription's operand and view as, read back from
//! the ascription's site once the load ends, and a closed tree's digest against the run's.

use crate::program::tests::digest_back;
use crate::program::{CellSubstrate, Outcome, Program};
use crate::scope::{BodyShape, Leaf, Tree};

use super::statics::{body, loaded, slot};
use super::{Koan, assert_linear, load_counts, output};
use crate::values::BuildBudget;

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
            .view_tree(
                &program.tiers().resting(),
                shape,
                shape.site_of_node(&shape.body()[index]),
            )
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
            .view_tree(
                &program.tiers().resting(),
                shape,
                shape.site_of_node(node.reference()),
            )
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
        CellSubstrate::load::<Koan>(&source, "<test>", 8, BuildBudget::default(), output())
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
        CellSubstrate::load::<Koan>(source, "<test>", 8, BuildBudget::default(), output())
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
        .view_tree(
            &program.tiers().resting(),
            shape,
            shape.site_of_node(let_node(program, shape, name)),
        )
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

/// Signatures nested `n` deep: `Sig0` innermost, declaring a type member, and each other's `a` the
/// next one in.
pub(super) fn nested_signatures(n: usize) -> String {
    let mut source = String::from("SIG Sig0 FOR ALL #[Elem] = #[(VAL zero :Elem)]\n");
    for at in 1..=n {
        source += &format!("SIG Sig{at} = #[(VAL a :Sig{})]\n", at - 1);
    }
    source
}

/// A type path read through a parameter extends the parameter's leaf by prefixes of the path's own
/// chain, laying no label of its own.
#[test]
fn a_path_through_a_parameter_lays_its_chain_once() {
    let program = |n: usize| {
        let path = "a.".repeat(n);
        let signatures = nested_signatures(n);
        format!("{signatures}LET f = (FN :{{p :Sig{n}}} -> Any = #(LET x :(p.{path}Elem) = 1))\n")
    };
    assert_linear("chain labels", |n| load_counts(&program(n)).chain_labels);
}

/// A call unfolding a callee whose summary reads `n` steps through its parameter reads its
/// argument's leaf through them once, laying one chain joining the leaf's to them.
#[test]
fn a_substituted_leaf_lays_its_chain_once() {
    let callee = |n: usize| {
        let path = ".a".repeat(n);
        let signatures = nested_signatures(n);
        format!("{signatures}LET g = (FN :{{p :Sig{n}}} -> Any = #(p{path}))\n")
    };
    // The call's own labels, past the callee's: its written `r.b` and the chain joined to it.
    assert_linear("chain labels", |n| {
        let call = format!(
            "{}SIG Wrap = #[(VAL b :Sig{n})]\n\
             LET f = (FN :{{r :Wrap}} -> Any = #(g {{p = r.b}}))\n",
            callee(n)
        );
        load_counts(&call).chain_labels - load_counts(&callee(n)).chain_labels
    });
}

/// A module binder read from `n` sites has its tree built once: its body's captures are laid once,
/// however many sites read it.
#[test]
fn a_module_binder_read_from_many_sites_lays_its_captures_once() {
    let program = |n: usize| {
        let mut source = String::from(
            "SIG Plain = #[(VAL zero :Number)]\n\
             LET base = 1\n\
             MODULE m = ((LET zero = base) (LET one = 1))\n",
        );
        for at in 0..n {
            source += &format!("LET v{at} = (m :| Plain)\n");
        }
        source
    };
    let slices = |n: usize| load_counts(&program(n)).capture_slices;
    assert_eq!(slices(8), slices(16), "capture slices laid");
}

/// Two bodies of one content share an identity, and so the fit of a view over either: each view's
/// tree is still over its own body.
#[test]
fn views_of_two_bodies_alike_are_each_over_their_own() {
    let source = "SIG Plain = #[(VAL zero :Number)]\n\
                  MODULE a = (LET zero = 0)\n\
                  MODULE b = (LET zero = 0)\n\
                  LET va = (a :| Plain)\n\
                  LET vb = (b :| Plain)\n";
    loaded(source, |program| {
        let shape = program.shape();
        let (Tree::Body { body: a, .. }, Tree::Body { body: b, .. }) = (
            let_tree(program, shape, "va"),
            let_tree(program, shape, "vb"),
        ) else {
            panic!("each view is over a body");
        };
        let identity = |name| let_tree(program, shape, name).identity();
        assert_eq!(
            identity("va"),
            identity("vb"),
            "the two bodies are one content"
        );
        assert_ne!(a, b, "each view is over its own body");
    });
}

/// A module binder read through its enclosing module's tree from `n` sites has its tree built
/// once: its body's captures are laid once, however many sites read it.
#[test]
fn a_module_read_through_a_module_from_many_sites_lays_its_captures_once() {
    let program = |n: usize| {
        let mut source = String::from(
            "SIG Plain = #[(VAL zero :Number)]\n\
             LET base = 1\n\
             MODULE m = ((LET zero = 0) (MODULE inner = ((LET zero = base) (LET one = 1))))\n",
        );
        for at in 0..n {
            source += &format!("LET v{at} = (m.inner :| Plain)\n");
        }
        source
    };
    let slices = |n: usize| load_counts(&program(n)).capture_slices;
    assert_eq!(slices(8), slices(16), "capture slices laid");
}

/// Each module binder named in `names` whose load tree is closed: its tree's identity is the
/// digest its value computes at the run, and the load finds the tree by that identity.
fn closed_identities_are_run_digests(source: &str, names: &[&str]) {
    let (identities, mut substrate) = ran(source, |program| {
        let shape = program.shape();
        (names.iter())
            .map(|name| {
                let tree = (program.facts())
                    .content(
                        &program.tiers().resting(),
                        shape,
                        slot(program, shape, name),
                    )
                    .unwrap_or_else(|| panic!("the load reads `{name}`'s content"));
                let digest = tree
                    .digest()
                    .unwrap_or_else(|| panic!("`{name}` is closed"));
                let found = (program.facts().tree_of(digest))
                    .unwrap_or_else(|| panic!("the load indexes `{name}`'s tree"));
                assert!(found.same_as(&tree));
                format!("{digest:?}")
            })
            .collect::<Vec<_>>()
    });
    for (name, identity) in names.iter().zip(identities) {
        assert_eq!(identity, digest_back(&mut substrate, name), "`{name}`");
    }
}

#[test]
fn a_closed_module_trees_identity_is_the_digest_its_value_computes() {
    closed_identities_are_run_digests(COUNTER, &["ints"]);
    closed_identities_are_run_digests(
        &format!(
            "{ORDERED}EXPR #(MAKESET elem :Ordered) -> Module = #(\n  \
             MODULE built OVER #[elem] = (LET sample = (elem.compare)))\n\
             LET made = (MAKESET asc)"
        ),
        &["asc", "made"],
    );
    closed_identities_are_run_digests(
        "MODULE outer = (MODULE inner = (LET one = 1))\nLET nested = outer.inner",
        &["outer", "nested"],
    );
}

/// Every closed subtree of a tree the load computed is indexed: the arguments a call unfolds over
/// among them.
#[test]
fn the_load_finds_every_closed_module_tree_by_its_identity() {
    let source = format!(
        "{ORDERED}EXPR #(MAKESET elem :Ordered) -> Module = #(\n  \
         MODULE built OVER #[elem] = (LET sample = (elem.compare)))\n\
         LET a = ((MAKESET asc) :| Sample)\n\
         LET c = ((MAKESET desc) :| Sample)"
    );
    loaded(&source, |program| {
        let shape = program.shape();
        let facts = program.facts();
        for name in ["asc", "desc"] {
            let tree = (facts.content(
                &program.tiers().resting(),
                shape,
                slot(program, shape, name),
            ))
            .unwrap_or_else(|| panic!("the load reads `{name}`'s content"));
            let digest = tree.digest().expect("a module of literals is closed");
            assert!(
                facts
                    .tree_of(digest)
                    .is_some_and(|found| found.same_as(&tree))
            );
        }
        for name in ["a", "c"] {
            let view = let_tree(program, shape, name);
            let digest = view
                .digest()
                .expect("a call over closed arguments is closed");
            assert!(
                facts
                    .tree_of(digest)
                    .is_some_and(|found| found.same_as(&view))
            );
            let Tree::Body { captures, .. } = view else {
                panic!("a call unfolds into the body its function builds: {view:?}");
            };
            for (_, capture) in captures {
                if let Some(digest) = capture.digest() {
                    assert!(
                        facts.tree_of(digest).is_some(),
                        "a closed subtree is indexed"
                    );
                }
            }
        }
    });
}
