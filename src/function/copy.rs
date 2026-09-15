//! A callable's copy: its whole knot re-tied at the destination.
//!
//! Every node of the source knot is rebuilt in index order, so an edge means the same node in the
//! copy and is carried verbatim; each closure binding's value is deep-copied through the copy the
//! crossing handed down, and the type, body shape and knot weight ride over. The copied callable is
//! the member at the source's own index.

use crate::memory::{KnotPlan, Writer};
use crate::values::{self, DeepCopy};

use super::{Callable, CallableFamily, Function, Node};

impl<'graph> values::CallableFamily<'graph> for CallableFamily {
    type Closed<'cell>
        = Callable<'graph, 'cell>
    where
        'graph: 'cell;

    fn copy_into<'from, 'to>(
        writer: Writer<'to>,
        callable: &Callable<'graph, 'from>,
        copy: &mut DeepCopy<'_, 'graph, 'from, 'to, Callable<'graph, 'from>, Callable<'graph, 'to>>,
    ) -> Callable<'graph, 'to>
    where
        'graph: 'from,
        'graph: 'to,
    {
        let source = callable.member().knot();
        let knot = KnotPlan::new(source.len()).tie(writer, |edge| {
            let Node::Function(function) = source.member(edge).payload();
            Node::Function(Function {
                ktype: function.ktype,
                shape: function.shape,
                closure: function.closure.copied(writer, &mut *copy),
                knot_weight: function.knot_weight,
            })
        });
        Callable(knot.member(callable.member().index()))
    }
}
