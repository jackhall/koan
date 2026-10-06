//! A knot member's copy: its whole knot re-tied at the destination.
//!
//! Every node of the source knot is rebuilt in index order, so an edge means the same node in the
//! copy and is carried verbatim; each closure binding's value, each data node's value cell and each
//! module member is the finished copy the crossing hands back, and the types and body shapes ride
//! over, beside one copy of the knot's facts, so a copy keeps its weight and its digest. A builtin's node points into program storage, so it is carried as-is. The
//! copied member is the one at the source's own index.
//!
//! [`held`](values::KnottedFamily::held) lists the values a rebuild asks for, in the order it asks,
//! so each arm below sits beside its rebuild's twin. The crossing copies those values over its own
//! worklist and keys every knot it has rebuilt, so two members of one foreign knot a module holds
//! arrive as members of one copy of that knot. A barrier's underlying function goes the same way,
//! as the value word it would be if a member held it.

use crate::memory::{KnotPlan, Writer, resident};
use crate::values::{self, DeepCopy, Value};

use super::{Knotted, KnottedFamily, Node};

impl<'graph> values::KnottedFamily<'graph> for KnottedFamily {
    type Closed<'cell>
        = Knotted<'graph, 'cell>
    where
        'graph: 'cell;

    fn held<'from>(
        member: &Knotted<'graph, 'from>,
        out: &mut dyn FnMut(Value<'from, Knotted<'graph, 'from>>),
    ) where
        'graph: 'from,
    {
        for node in member.member().knot().members() {
            match node.payload() {
                Node::Function(function) => function.closure().held(out),
                Node::Builtin(_) => {}
                Node::Data { circular, .. } => circular.held(out),
                Node::Module(module) => module.members().iter().for_each(|value| out(*value)),
                Node::Coerced(coerced) => out(Value::Knotted(coerced.underlying())),
                Node::Code(code) => code.held(out),
            }
        }
    }

    fn copy_into<'from, 'to>(
        writer: Writer<'to>,
        member: &Knotted<'graph, 'from>,
        copy: &mut DeepCopy<'_, 'from, 'to, Knotted<'graph, 'from>, Knotted<'graph, 'to>>,
    ) -> Knotted<'graph, 'to>
    where
        'graph: 'from,
        'graph: 'to,
    {
        let source = member.member().knot();
        // The knot's facts are laid down once and shared by every copied node, as the source's are.
        let facts = member.facts().map(|facts| facts.copied(writer));
        let shared = || facts.expect("a knot of more than a builtin has facts");
        let knot =
            KnotPlan::new(source.len()).tie(writer, |edge| match source.member(edge).payload() {
                Node::Function(function) => Node::Function(function.rebuilt(
                    writer,
                    function.closure().copied(writer, &mut *copy),
                    shared(),
                )),
                // The record lives in program storage, which outlives the destination.
                Node::Builtin(builtin) => Node::Builtin(builtin),
                Node::Data { circular, .. } => Node::Data {
                    circular: circular.copied(writer, copy),
                    facts: shared(),
                },
                Node::Module(module) => {
                    let source = module.members();
                    let members = writer.fill(source.len(), |at| copy(&source[at]));
                    Node::Module(module.rebuilt(members, shared()))
                }
                Node::Coerced(coerced) => {
                    let underlying = match copy(&Value::Knotted(coerced.underlying())) {
                        Value::Knotted(member) => member,
                        _ => unreachable!("the copy of a knot member is a knot member"),
                    };
                    Node::Coerced(resident(writer, coerced.rebuilt(underlying, shared())))
                }
                Node::Code(code) => {
                    Node::Code(resident(writer, code.rebuilt(writer, &mut *copy, shared())))
                }
            });
        Knotted(knot.member(member.member().index()))
    }
}
