//! Content trees: what the load reads each `:|` ascription's operand as, read back from the
//! ascription's site once the load ends, and a closed tree's digest against the run's.

use crate::program::tests::digest_back;
use crate::program::{CellSubstrate, Outcome, Program};
use crate::scope::{BodyShape, Leaf, Site, Tree};
use crate::values::digest::{Operator, lone_member, view_content};

use super::statics::{body, loaded, slot};
use super::{Koan, output};

/// `SIG Counter`, and `MODULE ints`, which fits it.
const COUNTER: &str = "SIG Counter FOR ALL #[Carrier] = \
                       #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]\n\
                       MODULE ints = ((LET zero = 0) \
                       (LET succ = (FN :{x :Number} -> Number = #(x + 1))))\n";

/// The content tree of the operand of the `:|` ascription that is statement `index` of `shape`.
fn statement_tree<'graph>(shape: &BodyShape<'graph>, index: usize) -> Tree<'graph> {
    *shape
        .view_tree(Site::of_node(&shape.body()[index]))
        .expect("the statement is an opaque ascription")
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
    *shape
        .view_tree(Site::of_node(node.reference()))
        .expect("the right-hand side is an opaque ascription")
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
    let name = program
        .symbols()
        .display(binder.shape.slot_name(binder.slot).symbol())
        .to_string();
    (name, std::ptr::eq(binder.shape, shape))
}

#[test]
fn a_view_over_a_parameter_reads_back_a_leaf_on_the_parameter() {
    let source = format!("{COUNTER}LET seal = (FN :{{m :Counter}} -> Any = #(m :| Counter))");
    loaded(&source, |program| {
        let seal = body(program, program.shape(), "seal");
        let tree = statement_tree(seal, 0);
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
        let tree = statement_tree(inner, 0);
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
        let tree = statement_tree(f, 1);
        assert_eq!(leaf_binder(program, tree, f), ("local".to_string(), true));
    });
}

#[test]
fn a_view_over_an_expression_no_binder_names_reads_back_unnamed() {
    let source =
        format!("{COUNTER}LET f = (FN :{{g :(FN :{{}} -> Any)}} -> Any = #((g {{}}) :| Counter))");
    loaded(&source, |program| {
        let f = body(program, program.shape(), "f");
        let tree = statement_tree(f, 0);
        assert!(matches!(tree, Tree::Leaf(Leaf::Unnamed(_))), "{tree:?}");
        assert!(!tree.same_as(&tree), "an unnamed leaf is unlike itself");
    });
}

/// A `MODULE` body over literals is closed, and the load digests the view of it as the run does.
#[test]
fn a_view_over_a_module_of_literals_is_closed_at_the_run_s_digest() {
    let source = format!("{COUNTER}LET v = (ints :| Counter)\nLET w = (ints :| Counter)");
    let mut substrate = CellSubstrate::load::<Koan>(&source, "<test>", 8, output())
        .unwrap_or_else(|error| panic!("the program loads: {error}"));
    assert_eq!(
        substrate.with(|running| running.run()),
        Ok(Outcome::Completed)
    );
    let (closed, same, digest) = substrate.with(|running| {
        let program = running.program();
        let shape = program.shape();
        let (v, w) = (let_tree(program, shape, "v"), let_tree(program, shape, "w"));
        let application =
            match shape.typed_expression(Site::of(&let_node(program, shape, "v").parts[2].value)) {
                crate::scope::Static::Closed(application) => application,
                _ => panic!("`Counter` is closed"),
            };
        let view = lone_member(view_content(
            Operator::Opaque,
            application,
            v.digest().expect("closed"),
        ));
        (v.closed(), v.same_as(&w), format!("{view:?}"))
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
