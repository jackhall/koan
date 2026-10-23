//! The shape table's numbering: a body's nested bodies take the segments before its own, which a
//! frontier's cut reads ([`Segment`](crate::scope::Segment)).

use crate::scope::ShapeKind;

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
