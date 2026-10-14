//! A knot member's copy: its whole knot re-tied at the destination.
//!
//! Every node of the source knot is rebuilt in index order, so an edge means the same node in the
//! copy and is carried verbatim; each closure binding's value, each data node's value cell and each
//! module member is the finished copy the crossing hands back, and the types, body shapes, knot
//! weight and a module's content ride over. A builtin's node points into program storage, so it is
//! carried as-is. The copied member is the one at the source's own index.
//!
//! [`carried_types`](values::KnottedFamily::carried_types) lists every type a node carries beside
//! them, which the crossing makes resolve in the destination before the rebuild runs.
//!
//! [`held`](values::KnottedFamily::held) lists the values a rebuild asks for, in the order it asks,
//! so each arm below sits beside its rebuild's twin. The crossing copies those values over its own
//! worklist and keys every knot it has rebuilt, so two members of one foreign knot a module holds
//! arrive as members of one copy of that knot. A barrier's underlying function goes the same way,
//! as the value word it would be if a member held it.

use crate::memory::{KnotPlan, Writer, resident};
use crate::type_lattice::{DeclaredType, Parametric};
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

    fn carried_types<'from>(
        member: &Knotted<'graph, 'from>,
        out: &mut dyn FnMut(DeclaredType<Parametric>),
    ) where
        'graph: 'from,
    {
        for node in member.member().knot().members() {
            match node.payload() {
                Node::Function(function) => {
                    out(function.ktype().into());
                    if function.declared() != function.ktype() {
                        out(function.declared().into());
                    }
                    if let Some(registered) = function.registered() {
                        out(registered.shape.into());
                    }
                    for solved in function.instance().unwrap_or(&[]) {
                        out((*solved).into());
                    }
                }
                Node::Builtin(builtin) => out(builtin.ktype().into()),
                Node::Data { circular, .. } => out(circular.ktype().into()),
                Node::Module(module) => out(module.ktype().into()),
                Node::Coerced(coerced) => {
                    out(coerced.ktype().into());
                    out(coerced.declared());
                    out(coerced.from().into());
                    out(coerced.to().into());
                }
                Node::Code(code) => out(code.ktype().into()),
            }
        }
    }

    fn copy_into<'from, 'to>(
        writer: Writer<'to, '_>,
        member: &Knotted<'graph, 'from>,
        copy: &mut DeepCopy<'_, 'from, 'to, Knotted<'graph, 'from>, Knotted<'graph, 'to>>,
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
                // The record lives in program storage, which outlives the destination.
                Node::Builtin(builtin) => Node::Builtin(builtin),
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
                    Node::Module(module.rebuilt(writer, members))
                }
                Node::Coerced(coerced) => {
                    let underlying = match copy(&Value::Knotted(coerced.underlying())) {
                        Value::Knotted(member) => member,
                        _ => unreachable!("the copy of a knot member is a knot member"),
                    };
                    Node::Coerced(resident(writer, coerced.rebuilt(underlying)))
                }
                Node::Code(code) => Node::Code(resident(writer, code.rebuilt(writer, &mut *copy))),
            });
        Knotted(knot.member(member.member().index()))
    }
}
