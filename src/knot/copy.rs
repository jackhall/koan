//! A knot member's copy: its whole knot re-tied at the destination.
//!
//! Every node of the source knot is rebuilt in index order, so an edge means the same node in the
//! copy and is carried verbatim; each closure binding's value, each data node's value cell and each
//! module member is deep-copied through the copy the crossing handed down, and the types, body
//! shapes and knot weight ride over. The copied member is the one at the source's own index.
//!
//! A module's members are rebuilt through that one copy, so a member that is itself a knot member
//! brings its whole knot with it. Two members of one foreign knot therefore arrive as two copies of
//! that knot — the price of "a member brings its knot", which a data node holding two such words
//! already pays. A barrier's underlying function goes the same way, as the value word it would be
//! if a member held it.

use crate::memory::{KnotPlan, Writer, resident};
use crate::values::{self, DeepCopy, Value};

use super::{Knotted, KnottedFamily, Node};

impl<'graph> values::KnottedFamily<'graph> for KnottedFamily {
    type Closed<'cell>
        = Knotted<'graph, 'cell>
    where
        'graph: 'cell;

    fn copy_into<'from, 'to>(
        writer: Writer<'to>,
        member: &Knotted<'graph, 'from>,
        copy: &mut DeepCopy<'_, 'graph, 'from, 'to, Knotted<'graph, 'from>, Knotted<'graph, 'to>>,
    ) -> Knotted<'graph, 'to>
    where
        'graph: 'from,
        'graph: 'to,
    {
        let source = member.member().knot();
        let knot =
            KnotPlan::new(source.len()).tie(writer, |edge| match source.member(edge).payload() {
                Node::Function(function) => Node::Function(
                    function.rebuilt(writer, function.closure().copied(writer, &mut *copy)),
                ),
                Node::Data {
                    circular,
                    knot_weight,
                } => Node::Data {
                    circular: circular.copied(writer, copy),
                    knot_weight: *knot_weight,
                },
                Node::Module(module) => {
                    let source = module.members();
                    let members = writer.fill(source.len(), |at| copy(&source[at]));
                    Node::Module(module.rebuilt(members))
                }
                Node::Coerced(coerced) => {
                    let underlying = match copy(&Value::Knotted(coerced.underlying())) {
                        Value::Knotted(member) => member,
                        _ => unreachable!("the copy of a knot member is a knot member"),
                    };
                    Node::Coerced(resident(writer, coerced.rebuilt(underlying)))
                }
            });
        Knotted(knot.member(member.member().index()))
    }
}
