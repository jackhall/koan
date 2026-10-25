//! The shape table's numbering: a body's nested bodies take the segments before its own, which a
//! frontier's cut reads ([`Segment`](crate::scope::Segment)). And the bit a body seals saying
//! whether it can birth a callable, which gates the level a call seeking a settlement lays.

use crate::scope::{BodyShape, ShapeKind};

use super::with_shapes;

/// Along every chain segments fall from the root inward, and `Shapes::encloses` agrees with a walk
/// up the parents for every pair.
#[test]
fn a_body_s_segment_exceeds_every_body_it_encloses() {
    let source = "LET f = (FN :{x :Number} -> Any = #(\n  \
                  LET g = (FN :{y :Number} -> Number = #(1 < (y) <= 3))\n  \
                  LET q = #(x + 1)\n  g\n))\n\
                  LET z = 2\n1 < (z) <= 3";
    with_shapes(source, |shapes, _, _, _| {
        let bodies = shapes.bodies();
        for kind in [ShapeKind::Callable, ShapeKind::Block, ShapeKind::Code] {
            assert!(
                bodies.iter().any(|body| body.kind() == kind),
                "the program shapes a {kind:?}"
            );
        }
        let walked = |outer, mut inner| loop {
            if inner == outer {
                return true;
            }
            match shapes.parent(inner) {
                Some((parent, _)) => inner = parent,
                None => return false,
            }
        };
        for body in bodies {
            if let Some((parent, _)) = shapes.parent(body.segment()) {
                assert!(
                    parent > body.segment(),
                    "{parent:?} encloses {:?}",
                    body.segment()
                );
            }
            for other in bodies {
                let (outer, inner) = (body.segment(), other.segment());
                assert_eq!(shapes.encloses(outer, inner), walked(outer, inner));
            }
        }
    });
}

/// The first callable body `body` nests directly.
fn callable<'graph>(body: &BodyShape<'graph>) -> &'graph BodyShape<'graph> {
    body.nested_shapes()
        .iter()
        .map(|(_, nested)| *nested)
        .find(|nested| nested.kind() == ShapeKind::Callable)
        .expect("the body nests a callable")
}

/// A body births a callable through a nested callable, a block or a module body, but a callable
/// written in a quote is the `EVAL`'s to birth.
#[test]
fn a_body_births_a_callable_through_its_blocks_and_module_bodies_but_not_its_quotes() {
    let births = |source: &str, expected: bool| {
        with_shapes(source, |shapes, _, _, _| {
            let f = callable(shapes.root());
            assert_eq!(f.births_callable(), expected, "`f` in `{source}`");
            if expected
                && f.nested_shapes()
                    .iter()
                    .any(|(_, n)| n.kind() == ShapeKind::Callable)
            {
                assert!(!callable(f).births_callable(), "`g` in `{source}`");
            }
        })
    };
    births(
        "LET f = (FN :{x :Number} -> Any = #(\n  \
         LET g = (FN :{y :Number} -> Number = #(y))\n  g\n))",
        true,
    );
    births(
        "LET f = (FN :{x :Number} -> Any = #(\n  \
         LET q = #(FN :{y :Number} -> Number = #(y))\n  q\n))",
        false,
    );
    births(
        "MODULE empty = (LET unused = 0)\n\
         LET f = (FN :{x :Number} -> Any = #(USING empty SCOPE (FN :{y :Number} -> Number = #(y))))",
        true,
    );
    births(
        "LET f = (FN :{x :Number} -> Any = #(\n  \
         MODULE inner = (LET g = (FN :{y :Number} -> Number = #(y)))\n  inner\n))",
        true,
    );
}
