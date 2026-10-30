//! A body's sites ([`Sites`](crate::scope::Sites)): every part and node of its statements has one,
//! numbered in program order, and resolves back to it.

use crate::parse::ExpressionPart;
use crate::scope::{BodyShape, Located, Site, each_part};

use super::with_shapes;

/// A program nesting callables, a block, a quote, an arm set and every container literal.
const NESTED: &str = "LET f = (FN :{x :Number} -> Any = #(\n  \
                      LET g = (FN :{y :Number} -> Number = #(1 < (y) <= 3))\n  \
                      LET q = #(x + 1)\n  \
                      LET r = {a = [x 2], b = {\"k\": (g 1)}}\n  \
                      MATCH r -> :Any WITH #{_: #(it)}\n))\n\
                      LET z = 2\n1 < (z) <= 3";

/// Each node and part `shape`'s statements hold, in written order: a node before its parts.
fn written<'g>(shape: &BodyShape<'g>) -> Vec<Result<&'g ExpressionPart<'g>, usize>> {
    let mut out = Vec::new();
    for statement in shape.body() {
        out.push(Err(statement.parts.as_ptr().addr()));
        each_part(statement, |part| {
            out.push(Ok(part));
            if let ExpressionPart::Expression(node)
            | ExpressionPart::SigiledTypeExpr(node)
            | ExpressionPart::RecordType(node)
            | ExpressionPart::QuotedExpression(node)
            | ExpressionPart::MarkedUse(_, node) = part
            {
                out.push(Err(node.parts.as_ptr().addr()));
            }
        });
    }
    out
}

#[test]
fn every_part_and_node_of_a_body_has_one_site_and_resolves_back_to_it() {
    with_shapes(NESTED, |shapes, _, _, _| {
        for shape in shapes.bodies() {
            let sites = shape.sites();
            let mut seen: Vec<(usize, Site)> = Vec::new();
            for met in written(shape) {
                let (address, site) = match met {
                    Ok(part) => {
                        let site = shape.site(part);
                        match sites.located(site) {
                            Located::Part(found) => assert!(std::ptr::eq(found, part)),
                            Located::Node(_) => panic!("a part's site names a part"),
                        }
                        (std::ptr::from_ref(part).addr(), site)
                    }
                    Err(address) => {
                        let site = (sites.find_at_address(address))
                            .expect("every node of the statements has a site");
                        let named = match sites.located(site) {
                            Located::Node(node) => node.parts.as_ptr().addr(),
                            // A node and its first part share an address, and the part is named.
                            Located::Part(part) => std::ptr::from_ref(part).addr(),
                        };
                        assert_eq!(named, address);
                        (address, site)
                    }
                };
                assert!(site.index() < sites.count());
                for (other, at) in &seen {
                    assert_eq!(*other == address, *at == site, "one site per address");
                }
                seen.push((address, site));
            }
        }
    });
}

#[test]
fn two_builds_of_one_text_give_every_part_the_same_site() {
    let numbered = || {
        with_shapes(NESTED, |shapes, _, _, _| {
            (shapes.bodies().iter())
                .map(|shape| {
                    (written(shape).into_iter())
                        .filter_map(|met| met.ok().map(|part| shape.site(part)))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        })
    };
    let (first, second) = (numbered(), numbered());
    assert!(first.iter().any(|sites| sites.len() > 10));
    assert_eq!(first, second);
    for sites in &first {
        let mut sorted = sites.clone();
        sorted.dedup();
        assert!(sorted.is_sorted(), "sites follow written order");
    }
}

/// A site indexes its own shape's table alone: read against a body with fewer sites, it names
/// nothing there.
#[test]
#[should_panic]
fn a_site_of_one_shape_is_never_read_against_another() {
    with_shapes(NESTED, |shapes, _, _, _| {
        let bodies = shapes.bodies();
        let most = (bodies.iter())
            .max_by_key(|shape| shape.sites().count())
            .expect("the program has bodies");
        let fewest = (bodies.iter())
            .min_by_key(|shape| shape.sites().count())
            .expect("the program has bodies");
        assert!(fewest.sites().count() < most.sites().count());
        let last = written(most).into_iter().rev().find_map(|met| met.ok());
        let site = most.site(last.expect("the body holds a part"));
        fewest.sites().located(site);
    });
}
