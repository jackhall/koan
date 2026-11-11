//! The shape table's numbering: a body's nested bodies take the segments before its own, which a
//! frontier's cut reads ([`Segment`](crate::scope::Segment)). And the bit a body seals saying
//! whether it can birth a callable, which gates the level a call seeking a settlement lays. And the
//! call sites the text lists per callable, and which bodies lie in a quote's code.

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

/// Every call site of a callable a binder births, by callee: a keyworded use, a tail call, a call
/// by name, a `$` use in a quote's code, and an `EVAL`'s offer of a name and of a key. Only the
/// quote's code, and the callable nested in it, are in code.
#[test]
fn the_text_lists_every_call_site_of_a_callable_and_marks_a_quote_s_code() {
    let source = "EXPR #(STEP x :Number) -> Number = #(x)\n\
                  LET f = (FN :{x :Number} -> Number = #(STEP x))\n\
                  LET g = (FN :{y :Number} -> Number = #((LET a = (f {x = y})) (a)))\n\
                  LET q = #(PRINT $(STEP 1) (FN :{z :Number} -> Number = #(z)))\n\
                  LET run = (FN :{body :(Expression NEEDING #[f (STEP _)])} -> Any = \
                  #(EVAL body -> Any))\n\
                  STEP 2";
    with_shapes(source, |shapes, _, _, _| {
        let bodies = shapes.bodies();
        // Each callee's sites, as the kind of the shape holding each beside whether it offers.
        let mut called: Vec<Vec<(ShapeKind, bool)>> = bodies
            .iter()
            .map(|body| {
                let mut sites: Vec<_> = (shapes.calls_into(body.segment()).iter())
                    .map(|call| (shapes.shape_at(call.at).kind(), call.offered))
                    .collect();
                sites.sort_by_key(|(kind, offered)| (format!("{kind:?}"), *offered));
                sites
            })
            .filter(|sites| !sites.is_empty())
            .collect();
        called.sort_by_key(Vec::len);
        assert_eq!(
            called,
            [
                // `f`: called by name in `g`, offered by `run`'s `EVAL`.
                vec![(ShapeKind::Callable, false), (ShapeKind::Callable, true)],
                // `STEP`: a tail call in `f`, offered by `run`, a `$` use in `q`'s code, at the top.
                vec![
                    (ShapeKind::Callable, false),
                    (ShapeKind::Callable, true),
                    (ShapeKind::Code, false),
                    (ShapeKind::Program, false),
                ],
            ]
        );
        for body in bodies {
            let nested_in_code = shapes
                .parent(body.segment())
                .is_some_and(|(parent, _)| shapes.shape_at(parent).kind() == ShapeKind::Code);
            assert_eq!(
                shapes.in_code(body.segment()),
                body.kind() == ShapeKind::Code || nested_in_code,
                "{:?} {:?}",
                body.kind(),
                body.segment()
            );
        }
        assert!(
            bodies
                .iter()
                .any(|body| shapes.in_code(body.segment()) && body.kind() == ShapeKind::Callable)
        );
    });
}
